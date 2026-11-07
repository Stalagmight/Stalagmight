//! [#576 RV4-R-S1] A Done that finished editing HERE stays finished.
//!
//! Done in a session that did not take the account's lock ends editing locally
//! and leaves the lock with the account (`LockHeldElsewhere`, RV2-R-13). The
//! status poll's inherited-lock rule reopens a shut gate whenever the server says
//! the account holds the lock, and could not tell "finished here by Done" from
//! "reopened after a restart": the next poll put the session back to editing
//! under the other session's lock. The Done now leaves a mark the poll honours
//! until an explicit Start editing or a new open.
//!
//! Driven against the loopback recording stub; all data is synthetic.

use super::speleodb::{
    editing_fixture, recording_stub, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};

/// An inherited edit session that Done finished here: gate shut, lock left with
/// the account, no claim.
fn finished_here(base: &str) -> (tempfile::TempDir, super::AppState) {
    let (temp, state) = editing_fixture(base);
    let _ = state.speleodb_forget_lock_acquired_for(REMOTE_ID);
    let done = state.speleodb_done(None);
    assert_eq!(format!("{done:?}"), "Ok(LockHeldElsewhere)");
    assert!(!state.edit_enabled(), "Done ended editing here");
    (temp, state)
}

#[test]
fn the_poll_does_not_reopen_editing_after_done_finished_here() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = finished_here(&base);

    let polled = state.speleodb_link_status().map(|s| s.edit_enabled).ok();
    let editing_after_poll = state.edit_enabled();
    let releases = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("/release/"))
        .count();
    println!(
        "after Done -> LockHeldElsewhere: poll edit_enabled {polled:?}; editing after \
         the poll: {editing_after_poll}; release requests: {releases}"
    );
    assert_eq!(polled, Some(false), "the poll reports editing still off");
    assert!(
        !editing_after_poll,
        "the inherited-lock rule must not reopen a session Done finished here"
    );
    assert_eq!(releases, 0, "and the account's lock is still left alone");
}

#[test]
fn a_new_open_lets_the_poll_resume_the_inherited_lock_again() {
    // DO-NOT-REGRESS: the mark is about THIS open. Reopening the copy (as after a
    // restart) is exactly what the inherited-lock reopen exists for.
    let (base, _listener, _seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (temp, state) = finished_here(&base);
    assert!(
        state.open_project(temp.path().join("remote.stalx")).success,
        "reopen the copy"
    );
    assert!(!state.edit_enabled(), "a remote copy opens read-only");

    let polled = state.speleodb_link_status().map(|s| s.edit_enabled).ok();
    println!(
        "after a reopen: poll edit_enabled {polled:?}; editing: {}",
        state.edit_enabled()
    );
    assert_eq!(polled, Some(true), "a new open resumes the account's lock");
    assert!(state.edit_enabled());
}

#[test]
fn start_editing_after_done_finished_here_edits_again_and_the_poll_keeps_it() {
    // DO-NOT-REGRESS: an explicit Start editing is the user's way back in, and
    // the poll must not undo it either.
    let (base, _listener, _seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = finished_here(&base);

    let started = state.speleodb_start_editing().map(|r| format!("{r:?}"));
    let polled = state.speleodb_link_status().map(|s| s.edit_enabled).ok();
    println!(
        "Start editing after Done: {started:?}; poll edit_enabled {polled:?}; editing: {}",
        state.edit_enabled()
    );
    assert!(started.is_ok(), "{started:?}");
    assert_eq!(polled, Some(true));
    assert!(state.edit_enabled());
}
