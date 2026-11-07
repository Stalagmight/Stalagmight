//! [#576 RV2-R-13-RESTART] The held-lock prompt's "Continue editing" claims the
//! lock it resumes, so Done releases it.
//!
//! RV2-R-13 made Done release only a lock this app session holds a claim on. A user
//! who took the lock, restarted the app and resumed through the held-lock prompt at
//! sign-in got an INHERITED edit session with no claim, so their Done left the lock
//! held. Decided: "Continue editing" at open records the claim for this session.
//!
//! [#576 SDB-ACCT-D1] Plain Start editing over the account's lock claims it as well
//! now (it used to claim nothing - the second-window case RV2-R-13 closed): a lock
//! the account holds is the user's however they acquired it, so Done releases it.
//! Both claims are ADOPTED (resumed): the release, not the authorship shortcut.
//!
//! Driven against the loopback stub of `sdb_lock_rule_tests` (one project body for
//! every request, naming the holder; each request line recorded).

use std::sync::atomic::{AtomicBool, Ordering};

use super::speleodb::{
    authed_session_at, recording_stub, remote_readonly_fixture, routing_stub,
    FREE_MUTEX_AT_BASE_BODY, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
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
fn plain_start_editing_over_the_accounts_lock_adopts_it_so_done_releases_it() {
    // [#576 SDB-ACCT-D1] Another window, another computer or the website took the
    // lock on this account; a plain Start editing here adopts it, and Done hands it
    // back. It used to claim nothing, and Done left the lock (LockHeldElsewhere).
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);

    let started = state.speleodb_start_editing().expect("start editing");
    let (claimed, taken) = (
        state.speleodb_lock_acquired_here(REMOTE_ID),
        state.speleodb_lock_taken_here(REMOTE_ID),
    );
    let done = state.speleodb_done(None);
    println!(
        "[SDB-ACCT-D1] plain Start editing over u@x's lock -> {started:?}; claimed \
         {claimed}; taken here {taken}; clean Done -> {done:?}; release requests sent: {}",
        releases(&seen)
    );
    assert!(matches!(started, StartEditingResult::Editing));
    assert!(
        claimed && !taken,
        "the account's lock is adopted: claimed for the release, not as taken here"
    );
    assert_eq!(
        format!("{done:?}"),
        "Ok(Released)",
        "Done releases as usual"
    );
    assert_eq!(releases(&seen), 1, "the lock is handed back once");
    assert!(!state.edit_enabled());
}

/// Whether the first acquire of `free_then_ours` has run.
static TAKEN: AtomicBool = AtomicBool::new(false);

/// The lock is free until this session's first acquire, ours from then on.
fn free_then_ours(line: &str) -> (u16, &'static str) {
    if line.starts_with("POST ") && line.contains("/acquire/") {
        TAKEN.store(true, Ordering::SeqCst);
    }
    if TAKEN.load(Ordering::SeqCst) {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    } else {
        (200, FREE_MUTEX_AT_BASE_BODY)
    }
}

#[test]
fn a_second_start_editing_keeps_the_lock_this_session_took() {
    // DO-NOT-REGRESS (#576 SDB-ACCT-D1): a second Start editing over a lock this
    // session took finds it held by our account, as over an adopted one; the claim it
    // already holds (an acquisition, licensing the authorship shortcut) is kept, not
    // replaced by an adopted claim.
    let (base, _listener, _seen) = routing_stub(free_then_ours);
    let (_temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(&base);

    let first = state.speleodb_start_editing().expect("first start editing");
    let taken_first = state.speleodb_lock_taken_here(REMOTE_ID);
    let second = state
        .speleodb_start_editing()
        .expect("second start editing");
    let (taken, resumed) = (
        state.speleodb_lock_taken_here(REMOTE_ID),
        state.speleodb_lock_resumed_here(REMOTE_ID),
    );
    println!(
        "[SDB-ACCT-D1] Start editing over a free lock -> {first:?}, taken here \
         {taken_first}; again over our own lock -> {second:?}, taken here {taken}, \
         adopted {resumed}"
    );
    assert!(taken_first, "the first Start editing took the lock");
    assert!(
        taken && !resumed,
        "the second keeps the acquisition, not an adopted claim"
    );
}
