//! [#576 RV5-PI-3] The internal-error notice can be asked for, not only pushed.
//!
//! The `internal-error` event fires once, through the event sink. A crash during
//! startup fires it before the webview listens (here: before any sink is set), and
//! a webview reload loses it; `internal_error_status` answers from the poison
//! monitor itself, so the frontend can ask once its listener is registered.

use super::AppState;
use crate::models::poison_policy::LockCell;

fn poison(cell: &dyn LockCell) {
    let crashed = std::thread::scope(|scope| {
        scope
            .spawn(|| cell.hold(&mut || panic!("synthetic panic in a cell (#576 RV5-PI-3 test)")))
            .join()
    });
    assert!(crashed.is_err(), "the helper thread panicked on purpose");
}

#[test]
fn the_status_names_the_first_poisoned_cell_with_or_without_a_listener() {
    let state = AppState::new().unwrap();
    let healthy = state.internal_error_status().map(|n| n.cell);
    // No event sink: the push has nowhere to go, as during startup.
    assert!(state.get_event_sink().is_none());
    poison(&state.project);
    // The recovery that reports it: any later lock of the cell.
    let _ = state.get_project_state();
    poison(&state.undo_stack);
    let _ = state.can_undo();
    let after = state.internal_error_status().map(|n| n.cell);
    println!("internal-error status: healthy {healthy:?}; after two crashes {after:?}");
    assert_eq!(healthy, None);
    assert_eq!(after.as_deref(), Some("project"), "the first cell wins");
}
