//! [#576 W2S-PUSH / RV2-R-13] Done and Push act only under a write lock that is
//! really theirs.
//!
//! Driven against a loopback stub that answers every request with one project
//! body (whose `active_mutex` names the holder) and records each request line, so
//! a test can assert which calls a verb made - and, above all, which it did not.

use super::speleodb::{
    editing_fixture, recording_stub, FREE_MUTEX_AT_BASE_BODY, HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use crate::speleodb::SpeleoDbAction;

fn lines(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock().expect("request log").clone()
}

/// Give the open copy un-pushed edits, the way an edit leaves it. Written through the
/// link rather than `mark_remote_dirty`, which the forward-edit tripwire in
/// `speleodb_revisions.rs` keeps out of every file but the production writers.
fn mark_unpushed(state: &super::AppState) {
    state
        .speleodb_mutate_link(|l| {
            l.has_unpushed_edits = true;
            l.last_edit_at = Some("2026-07-27T10:00:00Z".to_owned());
        })
        .expect("mark the copy un-pushed");
}

// --- RV2-R-13: Done releases only a lock this app session took ----------------

#[test]
fn done_never_releases_a_same_account_lock_this_session_did_not_take() {
    // [#576 RV2-R-13] The holder check is per ACCOUNT. A second window (or this app
    // after a restart) on the same account opens its edit gate through the
    // inherited-lock reopen without taking the lock, passes that check, and a clean
    // Done there released the lock the other session was editing under. Decided:
    // release only when this app session acquired it; otherwise finish locally.
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    // An inherited edit session: the gate is open, the lock is the account's, and
    // this session holds no claim on it.
    let _ = state.speleodb_forget_lock_acquired_for(REMOTE_ID);
    assert!(state.edit_enabled() && !state.speleodb_lock_acquired_here(REMOTE_ID));

    let result = state.speleodb_done(None);

    let seen = lines(&seen);
    let releases = seen.iter().filter(|r| r.contains("/release/")).count();
    let outcome = format!("{result:?}");
    println!(
        "clean Done in a session that did not take u@x's lock -> {outcome}; release \
         requests sent: {releases}; editing still on: {}",
        state.edit_enabled()
    );
    assert_eq!(
        releases, 0,
        "a lock this session did not take is never released: {seen:?}"
    );
    assert_eq!(
        outcome, "Ok(LockHeldElsewhere)",
        "Done finishes here and says the lock stays with the account elsewhere"
    );
    assert!(!state.edit_enabled(), "editing has ended in this session");
}

#[test]
fn done_still_releases_a_lock_this_session_took() {
    // [#576 RV2-R-13] DO-NOT-REGRESS: with the claim, the lock is handed back.
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    assert!(state.speleodb_lock_acquired_here(REMOTE_ID));

    let result = state.speleodb_done(None).expect("done");

    assert_eq!(format!("{result:?}"), "Released");
    assert!(
        lines(&seen).iter().any(|r| r.contains("/release/")),
        "our own lock is released"
    );
    assert!(!state.edit_enabled());
}

// --- W2S-PUSH: Push applies Done's lost-lock rule ------------------------------

#[test]
fn push_never_uploads_under_a_lock_another_account_holds() {
    // [#576 W2S-PUSH] Done already refused here (B35(e)); Push read the holder only
    // for the authorship licence, went on to attempt the upload and surfaced
    // whatever the server made of it - with editing still on over a lock that is
    // someone else's. Same rule as Done now: nothing uploaded, editing off, and the
    // message names the holder.
    let (base, _listener, seen) = recording_stub(HELD_BY_ANOTHER_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    mark_unpushed(&state);

    let result = state.speleodb_push_only(None);

    let seen = lines(&seen);
    let uploads = seen.iter().filter(|r| r.contains("/upload/")).count();
    println!(
        "push with someone.else@x holding the lock -> {:?}; uploads sent: {uploads}; \
         editing still on: {}",
        result.as_ref().map_err(|e| (e.action, e.message.clone())),
        state.edit_enabled()
    );
    assert_eq!(
        uploads, 0,
        "no upload under another account's lock: {seen:?}"
    );
    let err = result.expect_err("a lock that is not ours cannot be pushed under");
    assert_eq!(err.action, SpeleoDbAction::Fence);
    assert!(
        err.message.contains("someone.else@x"),
        "the error names who holds the lock: {}",
        err.message
    );
    assert!(!state.edit_enabled(), "editing is switched off");
    assert!(
        !state.speleodb_lock_acquired_here(REMOTE_ID),
        "and the claim on a lock we no longer hold is dropped"
    );
    assert!(
        state.speleodb_open_link().expect("link").has_unpushed_edits,
        "the un-pushed edits stay marked for the next Start editing"
    );
}

#[test]
fn push_never_uploads_when_the_lock_is_gone() {
    // [#576 W2S-PUSH] The same with nobody on the mutex: the upload needs the lock.
    let (base, _listener, seen) = recording_stub(FREE_MUTEX_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    mark_unpushed(&state);

    let result = state.speleodb_push_only(None);

    let seen = lines(&seen);
    assert!(
        !seen.iter().any(|r| r.contains("/upload/")),
        "no upload without the lock: {seen:?}"
    );
    let err = result.expect_err("there is no lock to push under");
    assert_eq!(err.action, SpeleoDbAction::Fence);
    assert!(!state.edit_enabled(), "editing is switched off");
}

#[test]
fn push_under_our_own_lock_is_not_refused_by_the_holder_check() {
    // [#576 W2S-PUSH] DO-NOT-REGRESS. The upload itself needs the app cache dir
    // only the env-gated live tests set up, so it fails further on; what matters is
    // that it got PAST the holder check: not a lock refusal, and editing stays on.
    let (base, _listener, _seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    mark_unpushed(&state);

    let result = state.speleodb_push_only(None);

    if let Err(e) = &result {
        assert_ne!(
            e.action,
            SpeleoDbAction::Fence,
            "our own lock must not be refused: {}",
            e.message
        );
    }
    assert!(state.edit_enabled(), "editing stays on under our own lock");
}
