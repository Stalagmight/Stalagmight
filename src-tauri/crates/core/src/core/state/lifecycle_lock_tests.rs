//! [#576 A1] A project-slot commit never lands inside an edit.
//!
//! `execute` holds `command_lock` for a whole command, but it takes and releases
//! the `project` mutex several times inside it: the undo data is read off the
//! project, the mutation runs, the epilogue runs, and only then is the undo entry
//! pushed. A load, create, close or delete that committed the project slot
//! without `command_lock` could land in between, so the edit's undo entry - which
//! addresses a shot of the OLD project by uuid - was pushed onto the NEW
//! project's freshly reset stack (the #413 bug class), or onto the stack of a
//! session that no longer has a project at all.
//!
//! Each test edits project A continuously on a worker thread while this thread
//! runs one slot commit, then checks the stack the commit left: every entry must
//! address a shot of the project now open, and with no project open there must be
//! no entry at all. Repeated over many rounds, because the interleaving is the
//! scheduler's to choose.
//!
//! All data is synthetic: projects built in memory and saved by this build.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData, ShotUpdates};
use crate::test_fixtures::state_with_null_sink;

const ROUNDS: usize = 40;

fn saved_project(dir: &Path, file: &str, name: &str) -> PathBuf {
    let path = dir.join(file);
    let mut project = Project::new(name.to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        shots: (0..4)
            .map(|i| ShotData {
                length: 3.0 + f64::from(i),
                azimuth_in: 10.0 * f64::from(i),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    let mut db = crate::persistence::Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    path
}

/// Entries on the stack that address a shot the open project does not have; with
/// no project open, every entry counts.
fn foreign_entries(state: &AppState) -> usize {
    let project = state.get_project_state();
    let mut stack = state.undo_stack.lock();
    match project {
        Some(project) => stack.drop_entries_addressing_a_missing_shot(&project),
        None => stack.len() + stack.redo_len(),
    }
}

/// Run `commit` once per round while a worker edits project A back to back, and
/// count the rounds that left a foreign undo entry behind.
fn rounds_with_a_foreign_entry(kind: &str, commit: impl Fn(&AppState, &Path, &Path)) -> usize {
    let _cfg = crate::test_helpers::isolated_config();
    let mut bad_rounds = 0;
    let mut foreign_total = 0;
    for round in 0..ROUNDS {
        let temp = TempDir::new().unwrap();
        let a = saved_project(temp.path(), "a.stalx", "CaveA");
        let b = saved_project(temp.path(), "b.stalx", "CaveB");
        let state = state_with_null_sink();
        assert!(state.open_project(a.clone()).success, "project A opens");
        let (section_id, shot_id) = {
            let project = state.get_project_state().unwrap();
            let section = &project.sections[0];
            (section.id, section.shots[1].id)
        };

        let stop = AtomicBool::new(false);
        let edits = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut length = 5.0;
                while !stop.load(Ordering::Relaxed) {
                    length += 0.01;
                    let _ = state.execute(Command::UpdateShot {
                        section_id,
                        shot_id,
                        updates: ShotUpdates {
                            length: Some(length),
                            ..ShotUpdates::default()
                        },
                    });
                    std::thread::yield_now();
                    // Every other round trip also undoes: an undo pops its entry,
                    // applies it, and only then pushes it on - a commit landing
                    // between the two left the entry on the new session's stacks.
                    if edits.load(Ordering::Relaxed) % 2 == 1 {
                        let _ = state.undo();
                    }
                    edits.fetch_add(1, Ordering::Relaxed);
                    // Let the committing thread in between commands: what is under
                    // test is a commit landing INSIDE one, not a starved commit.
                    std::thread::yield_now();
                }
            });
            // Commit once the edits are flowing, so it lands among them.
            while edits.load(Ordering::Relaxed) < 2 + round % 5 {
                std::thread::yield_now();
            }
            commit(&state, &a, &b);
            // A few more edits after the commit, on whatever is open now.
            let after = edits.load(Ordering::Relaxed) + 3;
            while edits.load(Ordering::Relaxed) < after {
                std::thread::yield_now();
            }
            stop.store(true, Ordering::Relaxed);
        });

        let foreign = foreign_entries(&state);
        if foreign > 0 {
            bad_rounds += 1;
            foreign_total += foreign;
        }
    }
    println!(
        "{kind}: {bad_rounds} of {ROUNDS} round(s) left {foreign_total} undo entr(y/ies) \
         addressing a shot the open project does not have"
    );
    bad_rounds
}

#[test]
fn a_load_never_lands_inside_an_edit() {
    let full = rounds_with_a_foreign_entry("load_project_full", |state, _, b| {
        state.do_load_project_full(b.to_path_buf()).unwrap();
    });
    let legacy = rounds_with_a_foreign_entry("open_project", |state, _, b| {
        assert!(state.open_project(b.to_path_buf()).success);
    });
    assert_eq!(
        (full, legacy),
        (0, 0),
        "an edit of project A pushed its undo entry onto project B's stack"
    );
}

#[test]
fn a_create_never_lands_inside_an_edit() {
    let created = rounds_with_a_foreign_entry("create_project_full", |state, a, _| {
        let path = a.with_file_name("new.stalx");
        state
            .do_create_project_full("CaveNew".to_string(), None, None, Some(path), false)
            .unwrap();
    });
    assert_eq!(
        created, 0,
        "an edit of the previous project pushed its undo entry onto the new project's stack"
    );
}

#[test]
fn a_close_or_delete_never_lands_inside_an_edit() {
    let closed = rounds_with_a_foreign_entry("close_project", |state, _, _| {
        assert!(state.close_project().success);
    });
    let deleted = rounds_with_a_foreign_entry("delete_project", |state, a, _| {
        assert!(state.delete_project(a.to_path_buf()).success);
    });
    let cache_closed = rounds_with_a_foreign_entry("close_project_if_open_at", |state, a, _| {
        assert!(state.close_project_if_open_at(a));
    });
    assert_eq!(
        (closed, deleted, cache_closed),
        (0, 0, 0),
        "an edit pushed its undo entry onto the stack of a session with no project"
    );
}
