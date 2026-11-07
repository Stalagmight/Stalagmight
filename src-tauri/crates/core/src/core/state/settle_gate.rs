//! [#576 C2] Test seam: hold `execute` after a command has mutated the project
//! and before it settles it (the corruption guard, the reconcile, the undo push),
//! with `command_lock` still held - the window an unlocked read can fall into.
//!
//! Keyed by the `AppState`, so tests running at once never hold each other's
//! commands; a state nobody parked runs straight through. Test builds only
//! (`test-support` lets the app crate's tests use it too).

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use super::AppState;

/// How long a held command waits for its release before it settles regardless,
/// so a test that forgot to release cannot hang the suite.
pub const HOLD_LIMIT: Duration = Duration::from_secs(5);

/// `(entered, released)`.
#[derive(Default)]
struct Gate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

static GATES: Mutex<Vec<(usize, Arc<Gate>)>> = Mutex::new(Vec::new());

fn key(state: &AppState) -> usize {
    std::ptr::from_ref(state) as usize
}

/// The next command `state` executes, held between its mutation and its settle
/// step; released on drop.
pub struct Parked {
    key: usize,
    gate: Arc<Gate>,
}

/// Hold the next command `state` executes before it settles.
pub fn park(state: &AppState) -> Parked {
    let gate = Arc::new(Gate::default());
    GATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((key(state), Arc::clone(&gate)));
    Parked {
        key: key(state),
        gate,
    }
}

impl Parked {
    /// Wait up to `limit` for the command to reach the gate; whether it did.
    pub fn wait_entered(&self, limit: Duration) -> bool {
        let state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .gate
            .changed
            .wait_timeout_while(state, limit, |(entered, _)| !*entered)
            .unwrap_or_else(PoisonError::into_inner);
        state.0
    }

    /// Let the held command settle.
    pub fn release(&self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.1 = true;
        self.gate.changed.notify_all();
    }
}

impl Drop for Parked {
    fn drop(&mut self) {
        self.release();
        GATES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(key, gate)| !(*key == self.key && Arc::ptr_eq(gate, &self.gate)));
    }
}

/// `execute`'s side: report the gate reached, then wait for the release (at most
/// [`HOLD_LIMIT`]) if a test parked this state. One park holds one command.
pub(super) fn hold(state: &AppState) {
    let gate = {
        let mut gates = GATES.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(at) = gates.iter().position(|(k, _)| *k == key(state)) else {
            return;
        };
        gates.remove(at).1
    };
    let mut held = gate.state.lock().unwrap_or_else(PoisonError::into_inner);
    held.0 = true;
    gate.changed.notify_all();
    let _released = gate
        .changed
        .wait_timeout_while(held, HOLD_LIMIT, |(_, released)| !*released)
        .unwrap_or_else(PoisonError::into_inner);
}
