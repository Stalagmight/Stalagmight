//! [#576 RV4-R-S2] The silent releases (close, project switch, quit, logout) hand
//! back only a lock THIS app session acquired.
//!
//! Decision RV2-R-13: "Release only when this app session acquired the lock". A
//! second window or computer on the same account opens its edit gate through the
//! status poll's inherited-lock reopen without taking the lock; closing a clean
//! project or quitting there used to free the lock the other session is editing
//! under. Driven against the loopback recording stub; all data is synthetic.

use std::time::Duration;

use super::speleodb::{
    editing_fixture, recording_stub, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};

fn release_requests(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("/release/"))
        .cloned()
        .collect()
}

#[test]
fn close_and_quit_never_release_a_lock_this_session_did_not_take() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    // An inherited edit session: gate open, the lock is the account's, no claim.
    let _ = state.speleodb_forget_lock_acquired_for(REMOTE_ID);
    assert!(state.edit_enabled() && !state.speleodb_lock_acquired_here(REMOTE_ID));

    let params = state.speleodb_held_lock_release_params().is_some();
    state.speleodb_release_held_lock_best_effort();
    let after_close = release_requests(&seen);
    state.speleodb_release_held_lock_bounded(Duration::from_secs(5));
    let after_quit = release_requests(&seen);
    println!(
        "inherited lock, no session claim: release params offered: {params}; release \
         requests after close: {}; after quit: {}; {after_quit:?}",
        after_close.len(),
        after_quit.len()
    );
    assert!(!params, "no silent release is offered without the claim");
    assert!(
        after_close.is_empty(),
        "close freed a lock this session did not take: {after_close:?}"
    );
    assert!(
        after_quit.is_empty(),
        "quit freed a lock this session did not take: {after_quit:?}"
    );
}

#[test]
fn close_still_releases_a_lock_this_session_took() {
    // DO-NOT-REGRESS: with the claim, a clean close hands the lock back.
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    assert!(state.speleodb_lock_acquired_here(REMOTE_ID));

    state.speleodb_release_held_lock_best_effort();
    let released = release_requests(&seen);
    println!("own lock: release requests after close: {}", released.len());
    assert_eq!(released.len(), 1, "our own lock is released on close");
}
