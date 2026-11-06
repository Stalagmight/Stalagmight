//! [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] A save or an edit names the project it was
//! made for ([`for_project`]); a body run for another project than the one open
//! when it holds `command_lock` is refused with `PROJECT_CHANGED`, nothing
//! written.
//!
//! The orderings are forced with the load's `BEFORE_SESSION_COMMIT` hook (it runs
//! on the loading thread holding `command_lock`), so none depends on timing. All
//! data is synthetic, in a temp dir.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use tempfile::TempDir;

use super::project_load::BEFORE_SESSION_COMMIT;
use super::{for_project, AppState, ProjectIdentity};
use crate::core::{Command, CommandErrorCode};
use crate::models::{Project, SectionData, ShotData};
use crate::test_fixtures::state_with_null_sink;

pub(super) fn saved(dir: &Path, file: &str, name: &str) -> PathBuf {
    let path = dir.join(file);
    let mut project = Project::new(name.to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        shots: vec![ShotData {
            length: 3.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = crate::persistence::Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    path
}

/// What the frontend names the open project as, now.
pub(super) fn identity_of(state: &AppState) -> Option<ProjectIdentity> {
    state.project_snapshot().map(|p| ProjectIdentity {
        created_at: p.created_at.as_str().to_owned(),
        file_path: p
            .file_path
            .as_ref()
            .map(|f| f.to_string_lossy().into_owned()),
    })
}

pub(super) fn new_section(name: &str) -> Command {
    Command::CreateSection {
        name: Some(name.to_owned()),
        surveyor: None,
        date: None,
        color: "#336699".to_owned(),
        direction: None,
        shots: vec![
            ShotData {
                length: 2.0,
                ..Default::default()
            };
            2
        ],
    }
}

pub(super) fn section_names(state: &AppState) -> Vec<String> {
    state
        .project_snapshot()
        .map(|p| p.sections.iter().filter_map(|s| s.name.clone()).collect())
        .unwrap_or_default()
}

pub(super) fn names_on_disk(path: &Path) -> Vec<String> {
    crate::persistence::Database::open_readonly(path)
        .and_then(|db| db.load_project())
        .map(|p| p.sections.iter().filter_map(|s| s.name.clone()).collect())
        .unwrap_or_default()
}

/// [FZ-FZ-INT-7] An edit made for CaveA that reaches `command_lock` only after a
/// load of CaveB committed is refused, and CaveB does not gain its section. The
/// edit thread is started while the load holds `command_lock` at its commit, so
/// it can only run after that commit.
#[test]
fn an_edit_made_for_the_project_a_switch_replaced_is_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let b = saved(temp.path(), "CaveB.stalx", "CaveB");
    let state = state_with_null_sink();
    state.do_load_project_full(a.clone()).unwrap();
    let made_for_a = identity_of(&state);

    let (at_commit, reached) = mpsc::channel::<()>();
    let (go, wait) = mpsc::channel::<()>();
    let (edit, load) = std::thread::scope(|scope| {
        let load = scope.spawn(|| {
            BEFORE_SESSION_COMMIT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    at_commit.send(()).unwrap();
                    wait.recv().unwrap();
                }));
            });
            state.do_load_project_full(b.clone())
        });
        reached.recv().unwrap();
        let edit = scope.spawn(|| {
            for_project(made_for_a.clone(), || {
                state.execute(new_section("EditForCaveA"))
            })
        });
        go.send(()).unwrap();
        (edit.join().unwrap(), load.join().unwrap())
    });

    let open = state.project_snapshot().map(|p| p.name.clone());
    println!(
        "load CaveB -> {}; edit made for CaveA -> {} ({:?}); open {open:?}, its sections {:?}",
        if load.is_ok() { "Ok" } else { "Err" },
        if edit.success { "Ok" } else { "refused" },
        edit.code,
        section_names(&state)
    );
    assert!(load.is_ok(), "the load commits");
    assert_eq!(open.as_deref(), Some("CaveB"));
    assert!(!edit.success, "the edit made for CaveA ran in CaveB");
    assert_eq!(edit.code, Some(CommandErrorCode::ProjectChanged));
    assert!(!section_names(&state).contains(&"EditForCaveA".to_owned()));
}

/// [FZ-FZ-INT-7] The rule itself: another project's identity, a copy's (same
/// `created_at`, other file) and "no project" are refused; the open project's -
/// and the file it was at before a Save As moved it - are not. Undo, a direct
/// entry point, is gated the same way.
#[test]
fn an_edit_is_refused_unless_it_names_the_open_project() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let state = state_with_null_sink();
    state.do_load_project_full(a.clone()).unwrap();
    let open = identity_of(&state).unwrap();
    let copy = ProjectIdentity {
        file_path: Some(temp.path().join("CaveA copy.stalx").display().to_string()),
        ..open.clone()
    };
    let other = ProjectIdentity {
        created_at: "2020-01-01T00:00:00Z".to_owned(),
        ..open.clone()
    };

    let code = |made_for: Option<ProjectIdentity>, name: &str| {
        let result = for_project(made_for, || state.execute(new_section(name)));
        result.code
    };
    let answers = [
        ("another project", code(Some(other), "ForOther")),
        ("a copy of the file", code(Some(copy), "ForCopy")),
        ("no project", code(None, "ForNone")),
        ("the open project", code(Some(open.clone()), "ForOpen")),
    ];
    println!("edit answers by identity: {answers:?}");
    for (what, answer) in &answers[..3] {
        assert_eq!(*answer, Some(CommandErrorCode::ProjectChanged), "{what}");
    }
    assert_eq!(answers[3].1, None, "the open project's own edit lands");
    assert_eq!(section_names(&state), ["SectionA", "ForOpen"]);

    let undo_for_other = for_project(
        Some(ProjectIdentity {
            created_at: "2020-01-01T00:00:00Z".to_owned(),
            file_path: open.file_path.clone(),
        }),
        || state.undo(),
    );
    assert_eq!(undo_for_other.code, Some(CommandErrorCode::ProjectChanged));
    assert_eq!(
        section_names(&state),
        ["SectionA", "ForOpen"],
        "no undo ran"
    );

    // A Save As moves the open project to another file without a new open: an
    // edit made just before it names the old file and still lands.
    let moved = temp.path().join("Moved.stalx");
    let saved_as = for_project(Some(open.clone()), || {
        state.execute(Command::SaveProject {
            path: Some(moved.clone()),
        })
    });
    assert!(saved_as.success, "{:?}", saved_as.error);
    assert_eq!(code(Some(open), "MadeBeforeSaveAs"), None);
}

/// [FZ-FZ-INT-4] A Save As made for CaveA that reaches `command_lock` after a
/// load of CaveB committed is refused: before, it wrote CaveB to the path the
/// user had chosen for CaveA.
#[test]
fn a_save_as_made_for_the_project_a_switch_replaced_writes_nothing() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved(temp.path(), "CaveA.stalx", "CaveA");
    let b = saved(temp.path(), "CaveB.stalx", "CaveB");
    let target = temp.path().join("CaveA chosen.stalx");
    let state = state_with_null_sink();
    state.do_load_project_full(a.clone()).unwrap();
    let made_for_a = identity_of(&state);

    let (at_commit, reached) = mpsc::channel::<()>();
    let (go, wait) = mpsc::channel::<()>();
    let save = std::thread::scope(|scope| {
        let load = scope.spawn(|| {
            BEFORE_SESSION_COMMIT.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    at_commit.send(()).unwrap();
                    wait.recv().unwrap();
                }));
            });
            state.do_load_project_full(b.clone())
        });
        reached.recv().unwrap();
        let save = scope.spawn(|| {
            for_project(made_for_a.clone(), || {
                state.execute(Command::SaveProject {
                    path: Some(target.clone()),
                })
            })
        });
        go.send(()).unwrap();
        load.join().unwrap().unwrap();
        save.join().unwrap()
    });

    println!(
        "Save As made for CaveA after the switch to CaveB -> {:?}; target exists: {}",
        save.code,
        target.exists()
    );
    assert!(
        !save.success,
        "the Save As wrote {:?}",
        names_on_disk(&target)
    );
    assert_eq!(save.code, Some(CommandErrorCode::ProjectChanged));
    assert!(!target.exists(), "nothing written at the chosen path");
}
