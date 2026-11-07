//! [#576 RV5-PI-4] A crash inside a read-only `settled_read` does not disable the
//! history for the rest of the session.
//!
//! `settled_read` runs its read while holding `command_lock`, so the read is never
//! caught in the middle of a command. A panic in that read used to unwind through
//! the guard and poison `command_lock`, and `refuse_after_crash` then refused Undo,
//! Redo and every SpeleoDB push until restart - although nothing was mutated: the
//! refusal exists for a crash half-way through an EDIT. The read now runs inside
//! `catch_unwind`; the guard is released normally and the panic carries on.
//!
//! All data is synthetic, built in memory.

use std::panic::{catch_unwind, AssertUnwindSafe};

use super::AppState;
use crate::core::Command;
use crate::models::ShotData;
use crate::test_fixtures::state_with_project;

/// An open project with one undoable edit on the stack.
fn project_with_an_edit() -> AppState {
    let state = state_with_project("CaveA");
    let created = state.execute(Command::CreateSection {
        name: Some("SectionA".to_string()),
        surveyor: None,
        date: None,
        color: "#336699".to_string(),
        direction: None,
        shots: vec![ShotData {
            length: 10.0,
            azimuth_in: 0.0,
            azimuth_out: 180.0,
            ..Default::default()
        }],
    });
    assert!(created.success, "{:?}", created.error);
    assert!(state.can_undo());
    state
}

#[test]
fn a_panicking_settled_read_leaves_command_lock_unpoisoned_and_undo_working() {
    let state = project_with_an_edit();

    let crashed = catch_unwind(AssertUnwindSafe(|| {
        state.settled_read(|| -> usize {
            panic!("synthetic panic in a read-only query (#576 RV5-PI-4 test)")
        })
    }));
    let poisoned = state.command_lock.is_poisoned();
    let undo = state.undo();
    println!(
        "read panicked: {}; command_lock poisoned afterwards: {poisoned}; undo success: {} \
         error: {:?}",
        crashed.is_err(),
        undo.success,
        undo.error
    );
    assert!(crashed.is_err(), "the panic still reaches the caller");
    assert!(
        !poisoned,
        "a crash in a read-only query must not poison command_lock"
    );
    assert!(
        undo.success,
        "undo is not refused after a read-only crash: {:?}",
        undo.error
    );
}

#[test]
fn a_settled_read_that_returns_hands_back_its_value() {
    // DO-NOT-REGRESS: the ordinary path is unchanged.
    let state = project_with_an_edit();
    let sections = state.settled_read(|| {
        state
            .project_snapshot()
            .map_or(0, |project| project.sections.len())
    });
    println!("sections read between commands: {sections}");
    assert_eq!(sections, 1);
    assert!(!state.command_lock.is_poisoned());
}
