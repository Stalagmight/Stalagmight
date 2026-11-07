//! [#576 RV5-PI-2] After a crash, a save does not write the Undo history.
//!
//! A panic while a command held `command_lock`, `project` or `undo_stack` can
//! leave the history out of step with the survey, which is why undo and redo
//! refuse for the rest of the session (`AppState::refuse_after_crash`). The save
//! the banner asks for used to persist that same history, and after the restart
//! the banner also asks for, a fresh session has nothing poisoned: Undo replayed
//! exactly the inverses the refusal had said must not be replayed. The save now
//! writes the survey with an empty history and says so with the
//! `UNDO_HISTORY_DROPPED` warning; with no crash the history is saved as before.
//!
//! All data is synthetic, built in memory or in a temp dir.

use std::path::Path;

use super::save_sidecars::UNDO_HISTORY_DROPPED_CODE;
use super::AppState;
use crate::core::Command;
use crate::models::poison_policy::LockCell;
use crate::models::ShotData;
use crate::test_fixtures::state_with_project;

/// Poison `cell` the way a crash does: a thread panics while holding it.
fn poison(cell: &dyn LockCell) {
    let crashed = std::thread::scope(|scope| {
        scope
            .spawn(|| cell.hold(&mut || panic!("synthetic panic in a cell (#576 RV5-PI-2 test)")))
            .join()
    });
    assert!(crashed.is_err(), "the helper thread panicked on purpose");
    assert!(cell.cell_poisoned());
}

/// A project with one section and one more shot added through `execute`, so the
/// Undo stack holds two entries.
fn edited_project() -> AppState {
    let state = state_with_project("CaveA");
    let created = state.execute(Command::CreateSection {
        name: Some("SectionA".to_owned()),
        surveyor: None,
        date: None,
        color: "#336699".to_owned(),
        direction: None,
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
    });
    assert!(created.success, "{:?}", created.error);
    let added = state.execute(Command::CreateShot {
        section_id: created.section_id.unwrap(),
        shot: ShotData {
            length: 2.5,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(added.success, "{:?}", added.error);
    assert!(state.can_undo());
    state
}

/// Save to `dir`, reopen in a fresh session (the restart), and report the save's
/// warning codes and whether Undo is offered after the reopen.
fn save_and_restart(state: &AppState, dir: &Path) -> (Vec<String>, bool, usize) {
    let path = dir.join("CaveA.stalx");
    let saved = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(
        saved.success,
        "the save itself goes through: {:?}",
        saved.error
    );
    let codes = saved.warnings.iter().map(|w| w.code.clone()).collect();
    let restarted = AppState::new().unwrap();
    let opened = restarted.open_project(path);
    assert!(opened.success, "{:?}", opened.error);
    (
        codes,
        restarted.can_undo(),
        restarted.get_undo_stack().len(),
    )
}

#[test]
fn a_save_after_a_crash_drops_the_history_and_says_so() {
    for cell in ["command_lock", "project", "undo_stack"] {
        let _cfg = crate::test_helpers::isolated_config();
        let dir = tempfile::TempDir::new().unwrap();
        let state = edited_project();
        match cell {
            "command_lock" => poison(&state.command_lock),
            "project" => poison(&state.project),
            _ => poison(&state.undo_stack),
        }
        assert!(
            !state.undo().success,
            "{cell}: undo refuses in this session"
        );
        let (codes, undo_after_restart, entries) = save_and_restart(&state, dir.path());
        println!(
            "{cell} poisoned: save warnings {codes:?}; after the restart Undo offered: \
             {undo_after_restart} ({entries} entries)"
        );
        assert!(
            !undo_after_restart,
            "{cell}: the restarted session offers Undo over the crashed history"
        );
        assert!(
            codes.iter().any(|c| c == UNDO_HISTORY_DROPPED_CODE),
            "{cell}: the save says the history was dropped: {codes:?}"
        );
    }
}

/// DO-NOT-REGRESS: with nothing poisoned the history is saved and reopened.
#[test]
fn a_save_with_no_crash_keeps_the_history() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = tempfile::TempDir::new().unwrap();
    let state = edited_project();
    let (codes, undo_after_restart, entries) = save_and_restart(&state, dir.path());
    println!(
        "no crash: save warnings {codes:?}; after the restart Undo offered: \
         {undo_after_restart} ({entries} entries)"
    );
    assert!(undo_after_restart, "the history survives a clean save");
    assert!(!codes.iter().any(|c| c == UNDO_HISTORY_DROPPED_CODE));
}

/// The undo rows in the file at `path`.
fn undo_rows_on_disk(path: &Path) -> usize {
    crate::persistence::Database::open_for_metadata(path)
        .unwrap()
        .load_undo_stack()
        .unwrap()
        .len()
}

/// [#576 RV6a-1] The confirmed-push cleanup is the save's second writer of the
/// history: after a SpeleoDB upload lands it rewrites the open file's Undo rows
/// from the stack, keeping the entries recorded after the snapshot. The upload
/// releases `command_lock` while it is on the network, so an edit can crash in
/// that window; the cleanup then wrote the crashed session's history into the
/// file the restarted session replays. Now it clears the history there instead,
/// like the save after a crash.
#[test]
fn the_push_cleanup_after_a_crash_drops_the_history_from_the_file() {
    for cell in ["command_lock", "project", "undo_stack"] {
        let _cfg = crate::test_helpers::isolated_config();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("CaveA.stalx");
        let state = edited_project();
        let saved = state.execute(Command::SaveProject {
            path: Some(path.clone()),
        });
        assert!(saved.success, "{:?}", saved.error);
        // The snapshot the upload carried, then an edit while it was on the network.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let cutoff = chrono::Utc::now().to_rfc3339();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let section_id = state.get_project_state().unwrap().sections[0].id;
        let late = state.execute(Command::CreateShot {
            section_id,
            shot: ShotData {
                length: 1.5,
                ..Default::default()
            },
            after_shot_id: None,
            before_shot_id: None,
        });
        assert!(late.success, "{:?}", late.error);
        match cell {
            "command_lock" => poison(&state.command_lock),
            "project" => poison(&state.project),
            _ => poison(&state.undo_stack),
        }
        assert!(state.refuse_after_crash("probe").is_err());

        state.speleodb_clear_undo_after_push(&cutoff, None);

        let rows = undo_rows_on_disk(&path);
        println!("{cell} poisoned, then the push landed: undo rows left in the file: {rows}");
        assert_eq!(
            rows, 0,
            "{cell}: the crashed history is not written into the file"
        );
    }
}

/// DO-NOT-REGRESS: with nothing poisoned the cleanup keeps the late edit's entry.
#[test]
fn the_push_cleanup_with_no_crash_keeps_the_late_entry() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("CaveA.stalx");
    let state = edited_project();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone()),
            })
            .success
    );
    std::thread::sleep(std::time::Duration::from_millis(2));
    let cutoff = chrono::Utc::now().to_rfc3339();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let section_id = state.get_project_state().unwrap().sections[0].id;
    assert!(
        state
            .execute(Command::CreateShot {
                section_id,
                shot: ShotData {
                    length: 1.5,
                    ..Default::default()
                },
                after_shot_id: None,
                before_shot_id: None,
            })
            .success
    );

    state.speleodb_clear_undo_after_push(&cutoff, None);

    let rows = undo_rows_on_disk(&path);
    println!("no crash, then the push landed: undo rows left in the file: {rows}");
    assert_eq!(
        rows, 1,
        "the entry recorded after the snapshot rides the next push"
    );
}
