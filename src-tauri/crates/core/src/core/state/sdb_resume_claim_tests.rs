//! [#576 RV2-R-13-RESTART] The held-lock prompt's "Continue editing" claims the
//! lock it resumes, so Done releases it.
//!
//! RV2-R-13 made Done release only a lock this app session holds a claim on. A user
//! who took the lock, restarted the app and resumed through the held-lock prompt at
//! sign-in got an INHERITED edit session with no claim, so their Done left the lock
//! held. Decided: "Continue editing" at open records the claim for this session.
//! Plain Start editing over an inherited lock still claims nothing (the second
//! window case RV2-R-13 closed).
//!
//! Driven against the loopback stub of `sdb_lock_rule_tests` (one project body for
//! every request, naming the holder; each request line recorded).

use super::speleodb::{
    authed_session_at, recording_stub, remote_readonly_fixture, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY,
    REMOTE_ID,
};
use crate::speleodb::StartEditingResult;

fn releases(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> usize {
    seen.lock()
        .expect("request log")
        .iter()
        .filter(|r| r.contains("/release/"))
        .count()
}

#[test]
fn continue_editing_claims_the_resumed_lock_so_done_releases_it() {
    // Our own account holds the lock at sign-in (this app before a restart).
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);

    let started = state
        .speleodb_resume_held_lock_editing()
        .expect("continue editing");
    let claimed = state.speleodb_lock_acquired_here(REMOTE_ID);
    println!(
        "Continue editing over u@x's own lock -> {started:?}; editing on: {}; claim \
         recorded: {claimed}",
        state.edit_enabled()
    );
    assert!(matches!(started, StartEditingResult::Editing));
    assert!(state.edit_enabled());
    assert!(claimed, "the resumed lock is claimed for this session");

    let done = state.speleodb_done(None);
    let released = releases(&seen);
    println!("clean Done -> {done:?}; release requests sent: {released}");
    assert_eq!(
        format!("{done:?}"),
        "Ok(Released)",
        "Done releases as usual"
    );
    assert_eq!(released, 1, "the lock is handed back once");
    assert!(!state.edit_enabled());
}

#[test]
fn plain_start_editing_over_an_inherited_lock_still_claims_nothing() {
    // DO-NOT-REGRESS (RV2-R-13): only the explicit resume claims; a second window
    // that merely starts editing under the account's lock does not.
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);

    let started = state.speleodb_start_editing().expect("start editing");
    assert!(matches!(started, StartEditingResult::Editing));
    assert!(!state.speleodb_lock_acquired_here(REMOTE_ID));

    let done = state.speleodb_done(None);
    println!(
        "plain Start editing, clean Done -> {done:?}; release requests sent: {}",
        releases(&seen)
    );
    assert_eq!(format!("{done:?}"), "Ok(LockHeldElsewhere)");
    assert_eq!(releases(&seen), 0);
}
