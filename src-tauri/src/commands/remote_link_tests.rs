//! [#576 D22] Every survey mutation's IPC answer carries the open project's
//! SpeleoDB link bits as the backend holds them after the mutation.
//!
//! The frontend used to re-derive "un-pushed" / "unwound" at each editing call
//! site, and got it wrong where a path marked the copy dirty that the backend did
//! not (a provable no-op reverse of a shot-less section left the copy clean and
//! unwound, while the mirror marked it dirty and dropped the Unwind state). Now
//! `pool::run_mutation` attaches `AppState::remote_link_bits` to the answer, Ok or
//! Err, and the frontend applies exactly that. These tests drive the registered
//! commands on a LINKED synthetic project and compare the answer with the link row
//! read back from the file through a second handle.
//!
//! All data is synthetic: a project built in code, saved into a temp dir.

use std::path::{Path, PathBuf};

use tauri::test::{mock_app, MockRuntime};
use tauri::Manager;
use tempfile::TempDir;

use super::command_tests::{h, made_for, run, shot_data, with_config_lock};
use super::{history, section};
use crate::core::{AppState, Command, CommandErrorCode, RemoteLinkBits};
use crate::models::{Project, SectionData, SectionId, SectionUpdates, SpeleoLink};
use crate::persistence::Database;

/// The revision an Unwind leaves the copy at, in the fixture.
const UNWOUND_TO: &str = "abc123";

fn link(dirty: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_owned(),
        project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
        project_name: "CaveR".to_owned(),
        base_commit_sha: Some(UNWOUND_TO.to_owned()),
        has_unpushed_edits: dirty,
        last_edit_at: dirty.then(|| "2026-11-02T10:00:00Z".to_owned()),
        last_verify_at: None,
        undo_mode: None,
    }
}

/// A linked project in an edit session: one section with shots, one with none.
struct Linked {
    app: tauri::App<MockRuntime>,
    path: PathBuf,
    with_shots: SectionId,
    shotless: SectionId,
    _dir: TempDir,
}

/// Save the project with a CLEAN link row (and the Unwind pointer set when
/// `unwound`), open it in a fresh state, and open the edit gate as Start editing
/// does.
fn linked(unwound: bool) -> Linked {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("CaveR.stalx");
    let mut project = Project::new("CaveR".to_owned(), None);
    project.add_section(SectionData {
        name: Some("WithShots".to_owned()),
        color: "#123456".to_owned(),
        shots: vec![shot_data(5.0), shot_data(7.0)],
        ..Default::default()
    });
    project.add_section(SectionData {
        name: Some("Shotless".to_owned()),
        color: "#654321".to_owned(),
        shots: Vec::new(),
        ..Default::default()
    });
    let mut db = Database::open(&path).expect("create the file");
    db.save_project(&project).expect("save the project");
    db.save_speleo_link(Some(&link(false)))
        .expect("stamp the link");
    if unwound {
        db.save_speleo_undo_chain(Some(UNWOUND_TO))
            .expect("set the Unwind pointer");
    }
    drop(db);

    let state = AppState::new().expect("app state");
    let opened = state.execute(Command::OpenProject { path: path.clone() });
    assert!(opened.success, "open: {:?}", opened.error);
    state.set_edit_enabled_for_tests(true);
    let live = state.get_project_state().expect("the project is open");
    let with_shots = live.sections[0].id;
    let shotless = live.sections[1].id;
    assert!(
        live.sections[1].shots.is_empty(),
        "fixture: a shot-less section"
    );
    let app = mock_app();
    app.manage(state);
    let fixture = Linked {
        app,
        path,
        with_shots,
        shotless,
        _dir: dir,
    };
    assert_eq!(
        link_row(&fixture.path),
        RemoteLinkBits {
            has_unpushed_edits: false,
            unwound,
        },
        "fixture: the link row as stamped"
    );
    fixture
}

/// The link row as the file holds it, read through a second handle.
fn link_row(path: &Path) -> RemoteLinkBits {
    let db = Database::open_existing(path).expect("a second handle on the file");
    let stored = db
        .load_speleo_link()
        .expect("the link reads")
        .expect("the project is linked");
    RemoteLinkBits {
        has_unpushed_edits: stored.has_unpushed_edits,
        unwound: db
            .load_speleo_undo_chain()
            .expect("the Unwind pointer reads")
            .is_some(),
    }
}

/// Rewrite the link row behind the session's back (a push landing, an Unwind).
fn restamp(path: &Path, dirty: bool, unwound: bool) {
    let db = Database::open_existing(path).expect("a second handle on the file");
    db.save_speleo_link(Some(&link(dirty)))
        .expect("restamp the link");
    db.save_speleo_undo_chain(unwound.then_some(UNWOUND_TO))
        .expect("restamp the Unwind pointer");
}

fn create(app: &tauri::App<MockRuntime>, name: &str) -> SectionId {
    run(section::create_section(
        h(app),
        made_for(app),
        Some(name.to_owned()),
        None,
        None,
        "#abcdef".to_owned(),
        None,
        vec![shot_data(3.0)],
    ))
    .expect("the section is created")
    .section_id
}

#[test]
fn an_edit_answers_the_link_row_dirty_and_no_longer_unwound() {
    with_config_lock(|| {
        let fx = linked(true);
        let outcome = run(section::update_section(
            h(&fx.app),
            made_for(&fx.app),
            fx.with_shots,
            SectionUpdates {
                name: Some(Some("Renamed".to_owned())),
                ..Default::default()
            },
        ))
        .expect("the edit lands");
        let row = link_row(&fx.path);
        println!(
            "edit -> remote_link {:?}; link row {row:?}",
            outcome.remote_link
        );
        assert_eq!(outcome.remote_link, Some(row), "the answer is the link row");
        assert_eq!(
            row,
            RemoteLinkBits {
                has_unpushed_edits: true,
                unwound: false,
            },
            "a forward edit dirties the copy and drops the Unwind position"
        );
    });
}

/// The drift the frontend mirror had: a reverse of a shot-less section is a
/// provable no-op the backend skips (#24) - no dirty flip, no Unwind reset - and
/// the answer says exactly that.
#[test]
fn a_provable_no_op_reverse_answers_clean_and_still_unwound() {
    with_config_lock(|| {
        let fx = linked(true);
        let outcome = run(section::reverse_section(
            h(&fx.app),
            made_for(&fx.app),
            fx.shotless,
            false,
        ))
        .expect("the no-op reverse succeeds");
        let row = link_row(&fx.path);
        println!(
            "no-op reverse -> remote_link {:?}; link row {row:?}",
            outcome.remote_link
        );
        assert!(outcome.reversed_sections.is_empty(), "nothing was reversed");
        assert_eq!(outcome.remote_link, Some(row), "the answer is the link row");
        assert_eq!(
            row,
            RemoteLinkBits {
                has_unpushed_edits: false,
                unwound: true,
            },
            "the copy stays clean, and an unwound copy stays unwound"
        );
    });
}

#[test]
fn an_undo_answers_the_link_row() {
    with_config_lock(|| {
        let fx = linked(false);
        create(&fx.app, "A");
        // As a push and an Unwind leave it: clean, and off the server tip.
        restamp(&fx.path, false, true);
        let outcome = run(history::undo(h(&fx.app), made_for(&fx.app))).expect("the undo lands");
        let row = link_row(&fx.path);
        println!(
            "undo -> remote_link {:?}; link row {row:?}",
            outcome.remote_link
        );
        assert_eq!(outcome.remote_link, Some(row), "the answer is the link row");
        assert_eq!(
            row,
            RemoteLinkBits {
                has_unpushed_edits: true,
                unwound: true,
            },
            "an undo dirties the copy and keeps the Unwind position"
        );
    });
}

/// A jump that fails part-way has still changed the project (step 1 landed), and
/// its REJECTION carries the bits that say so.
#[test]
fn a_failed_undo_to_that_mutated_rejects_with_the_dirty_link_row() {
    with_config_lock(|| {
        let fx = linked(false);
        let first = create(&fx.app, "A");
        create(&fx.app, "B");
        // Take section A away behind the history's back, so the jump's second step
        // (undoing A's creation) has nothing to delete, while its first (B) lands.
        let state = fx.app.state::<AppState>();
        let mut live = state.get_project_state().expect("the project is open");
        assert!(live.remove_section(first).is_some(), "fixture: A removed");
        state.set_project_for_tests(live);
        restamp(&fx.path, false, false);

        let error =
            run(history::undo_to(h(&fx.app), made_for(&fx.app), 1)).expect_err("step 2 of 2 fails");
        let row = link_row(&fx.path);
        println!(
            "failed undo_to -> code {} message {:?} history_step {:?}; remote_link {:?}; \
             link row {row:?}",
            error.code, error.message, error.payload.history_step, error.payload.remote_link
        );
        assert_eq!(error.code, CommandErrorCode::HistoryStepFailed);
        assert_eq!(
            error.payload.history_step,
            Some(crate::core::history_step_failure(2, 2))
        );
        assert_eq!(
            error.payload.remote_link,
            Some(row),
            "the rejection carries the link row"
        );
        assert!(
            row.has_unpushed_edits,
            "the step that landed dirtied the copy: {row:?}"
        );
    });
}

#[test]
fn a_local_project_answers_no_link_bits() {
    with_config_lock(|| {
        let app = mock_app();
        app.manage(AppState::new().expect("app state"));
        let mut project = Project::new("CaveL".to_owned(), None);
        let section_id = project
            .add_section(SectionData {
                color: "#123456".to_owned(),
                shots: vec![shot_data(5.0)],
                ..Default::default()
            })
            .id;
        app.state::<AppState>().set_project_for_tests(project);
        let outcome = run(section::update_section(
            h(&app),
            made_for(&app),
            section_id,
            SectionUpdates {
                name: Some(Some("Renamed".to_owned())),
                ..Default::default()
            },
        ))
        .expect("the edit lands");
        println!("local edit -> remote_link {:?}", outcome.remote_link);
        assert_eq!(outcome.remote_link, None, "a local project has no link");
        let error = run(section::delete_section(h(&app), made_for(&app), 999))
            .expect_err("no such section");
        assert_eq!(
            error.payload.remote_link, None,
            "and its failures carry none either"
        );
    });
}

/// [#576 RV7a-2] A mutation whose body crashes AFTER it marked the copy - a single
/// undo marks the link before its geometry reconcile - rejects with
/// `INTERNAL_ERROR` carrying the link row as it now stands, so the status button
/// and the quit guard show the un-pushed copy at once instead of at the next
/// status poll. The crash is real, on the pool, inside `run_mutation`.
#[test]
fn a_mutation_that_crashes_after_marking_the_copy_rejects_with_the_dirty_link_row() {
    with_config_lock(|| {
        let fx = linked(false);
        let section_id = fx.with_shots;
        let error = run(super::pool::run_mutation(
            h(&fx.app),
            made_for(&fx.app),
            move |state| -> Result<crate::core::EditOutcome, crate::core::CommandError> {
                let edited = state.execute(Command::UpdateSectionMetadata {
                    section_id,
                    updates: SectionUpdates {
                        name: Some(Some("Renamed".to_owned())),
                        ..Default::default()
                    },
                });
                assert!(
                    edited.success,
                    "the edit marks the copy: {:?}",
                    edited.error
                );
                panic!("synthetic crash after the edit marked the copy (#576 RV7a-2 test)")
            },
        ))
        .expect_err("a crashed body answers a failure");
        let row = link_row(&fx.path);
        println!(
            "crash after marking -> code {} remote_link {:?}; link row {row:?}",
            error.code, error.payload.remote_link
        );
        assert_eq!(error.code, CommandErrorCode::InternalError);
        assert_eq!(
            row,
            RemoteLinkBits {
                has_unpushed_edits: true,
                unwound: false,
            },
            "fixture: the edit marked the copy before the crash"
        );
        assert_eq!(
            error.payload.remote_link,
            Some(row),
            "the rejection carries the link row the crash left"
        );
    });
}
