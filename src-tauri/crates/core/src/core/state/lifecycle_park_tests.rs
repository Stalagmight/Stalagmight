//! [#576 RV4-R-NITS C-4] A close or delete never lands inside an edit - forced,
//! not left to the scheduler.
//!
//! `lifecycle_lock_tests.rs` runs each slot commit among a stream of edits and
//! counts the rounds that left a foreign undo entry. Whether a commit lands in the
//! window that matters - after an edit's mutation, before its undo push - is the
//! scheduler's choice, so with every commit stripped of `command_lock` that test
//! failed the close and delete legs one round in forty and the cache-close leg
//! never. Here the edit is PARKED in that window (the `park_after_mutation` seam
//! in `execute`) while the commit runs, so each leg fails deterministically when
//! the commit does not wait for `command_lock`, and passes when it does.
//!
//! All data is synthetic: projects built in memory and saved by this build.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use tempfile::TempDir;

use super::app_state::ParkAfterMutation;
use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData, ShotUpdates};

fn saved_project(dir: &Path) -> PathBuf {
    let path = dir.join("a.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        shots: (0..3)
            .map(|i| ShotData {
                length: 3.0 + f64::from(i),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    let mut db = crate::persistence::Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    path
}

/// Undo entries left behind that the project now open (or none) does not account for.
fn foreign_entries(state: &AppState) -> usize {
    let project = state.get_project_state();
    let mut stack = state.undo_stack.lock();
    match project {
        Some(project) => stack.drop_entries_addressing_a_missing_shot(&project),
        None => stack.len() + stack.redo_len(),
    }
}

/// Park an edit of project A between its mutation and its undo push, run `commit`
/// on another thread while it is parked, give that thread time to either commit
/// (no `command_lock`) or block on the lock, then let the edit finish. Returns the
/// foreign entries left behind.
fn foreign_after_a_commit_inside_an_edit(
    kind: &str,
    commit: impl Fn(&AppState, &Path) + Sync,
) -> usize {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let a = saved_project(temp.path());
    let state = AppState::new().unwrap();
    assert!(state.open_project(a.clone()).success, "project A opens");
    let (section_id, shot_id) = {
        let project = state.get_project_state().unwrap();
        (project.sections[0].id, project.sections[0].shots[1].id)
    };
    let (reached_tx, reached_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    *state.park_after_mutation.lock() = Some(ParkAfterMutation {
        reached: reached_tx,
        resume: resume_rx,
    });

    std::thread::scope(|scope| {
        let edit = scope.spawn(|| {
            state.execute(Command::UpdateShot {
                section_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..ShotUpdates::default()
                },
            })
        });
        reached_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the edit reaches the window");
        let committing = scope.spawn(|| commit(&state, &a));
        std::thread::sleep(Duration::from_millis(200));
        resume_tx.send(()).unwrap();
        assert!(edit.join().unwrap().success, "the edit itself succeeds");
        committing.join().unwrap();
    });
    let foreign = foreign_entries(&state);
    println!(
        "{kind} run while an edit was parked after its mutation: {foreign} foreign undo \
         entr(y/ies) left; project open after: {:?}",
        state.get_project_state().map(|p| p.name)
    );
    foreign
}

#[test]
fn a_close_inside_an_edit_waits_for_it() {
    let foreign = foreign_after_a_commit_inside_an_edit("close_project", |state, _| {
        assert!(state.close_project().success);
    });
    assert_eq!(
        foreign, 0,
        "the edit's undo entry landed on the closed session's stack"
    );
}

#[test]
fn a_delete_inside_an_edit_waits_for_it() {
    let foreign = foreign_after_a_commit_inside_an_edit("delete_project", |state, a| {
        assert!(state.delete_project(a.to_path_buf()).success);
    });
    assert_eq!(
        foreign, 0,
        "the edit's undo entry landed on the closed session's stack"
    );
}

#[test]
fn a_cache_close_inside_an_edit_waits_for_it() {
    let foreign = foreign_after_a_commit_inside_an_edit("close_project_if_open_at", |state, a| {
        assert!(state.close_project_if_open_at(a));
    });
    assert_eq!(
        foreign, 0,
        "the edit's undo entry landed on the closed session's stack"
    );
}
