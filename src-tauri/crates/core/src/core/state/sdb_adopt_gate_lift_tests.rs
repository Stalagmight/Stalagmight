//! [#576 RV5-F4-6] The Ariane adoption lifts its hold on the edit gate when it
//! returns - on every exit.
//!
//! `adopt_import_and_save` shuts the gate at the end of its local span, and
//! `speleodb_adopt_ariane` lifts it through an `AdoptionEditGate` guard built
//! before that span. `sdb_adopt_edit_gate_tests` lifts the gate by hand, so
//! removing the guard from the verb passed every adoption test, and the gate would
//! then stay shut over a still-open adopted project (after a Failed hand-over, for
//! one) until the next session commit.
//!
//! Two halves, because reaching the verb's lift for real needs a server that
//! accepts the lock, serves the dump and takes the push:
//!
//! 1. the guard lifts the gate on a normal return, an early error return and a panic
//!    (below);
//! 2. the verb holds that guard from before its local span to its close - the
//!    types' job since #576 A8 (it used to be a source scan here): the span
//!    (`adopt_local_span`) borrows the guard and the close (`adopt_close`) consumes
//!    it, so it must be bound to a name, built before the span, and cannot be dropped
//!    early (a use after the move does not compile).
//!
//! All data is synthetic, built in memory.

use std::panic::{catch_unwind, AssertUnwindSafe};

use super::speleodb_adopt::AdoptionEditGate;
use super::AppState;
use crate::test_fixtures::state_with_project;

/// An open project whose gate an adoption has shut.
fn gate_shut_by_an_adoption() -> AppState {
    let state = state_with_project("CaveA");
    state.speleodb_shut_gate_for_adoption();
    assert!(state.speleodb_adoption_holds_gate());
    state
}

/// The adoption's shape: the guard first, then a step that may fail early.
fn adoption_like(state: &AppState, fail: bool) -> Result<(), String> {
    let _edit_gate = AdoptionEditGate::for_tests(state);
    if fail {
        return Err("the hand-over failed".to_string());
    }
    Ok(())
}

#[test]
fn the_adoption_guard_lifts_the_gate_on_every_exit() {
    let returned = gate_shut_by_an_adoption();
    let ok = adoption_like(&returned, false);
    let after_return = returned.speleodb_adoption_holds_gate();

    let failed = gate_shut_by_an_adoption();
    let err = adoption_like(&failed, true);
    let after_error = failed.speleodb_adoption_holds_gate();

    let crashed = gate_shut_by_an_adoption();
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _edit_gate = AdoptionEditGate::for_tests(&crashed);
        panic!("synthetic panic inside an adoption (#576 RV5-F4-6 test)");
    }));
    let after_panic = crashed.speleodb_adoption_holds_gate();

    println!(
        "gate still held after: a return ({ok:?}) -> {after_return}; an early error \
         ({err:?}) -> {after_error}; a panic ({}) -> {after_panic}",
        panicked.is_err()
    );
    assert!(!after_return && !after_error && !after_panic);
}
