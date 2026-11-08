//! [#576 SDB-QUIT-D1] The silent releases (close, project switch, quit, logout) hand
//! back the lock this session edits under, however the account came to hold it.
//!
//! Decision RV2-R-13 ("Release only when this app session acquired the lock") made
//! them ask for this session's claim (#576 RV4-R-S2), and the status poll's
//! inherited-lock reopen opened the gate without one: a lock the account held from
//! another Stalagmight session, another machine or the SpeleoDB website was left held
//! by a clean close or a quit. Decisions SDB-ACCT-D1 and SDB-QUIT-D1 supersede it:
//! such a lock is the user's, editing under it is an edit session, and quitting
//! releases any lock this session edits under, adopted ones included. Driven against
//! the loopback recording stub; all data is synthetic.

use std::time::Duration;

use super::speleodb::{
    authed_session_at, editing_fixture, recording_stub, remote_readonly_fixture, routing_stub,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::AppState;

fn release_requests(seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("/release/"))
        .cloned()
        .collect()
}

/// A clean, signed-in copy whose lock the ACCOUNT holds from elsewhere, read-only
/// until the status poll adopts it.
fn adopted_by_the_poll(base: &str) -> (tempfile::TempDir, AppState) {
    let (temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(base);
    super::speleodb::link_open_copy_to(&state, base);
    let polled = state.speleodb_link_status().map(|s| s.edit_enabled);
    println!(
        "[SDB-ACCT-D1] the account holds the lock, taken elsewhere: poll -> edit_enabled \
         {polled:?}; claim {}; taken here {}; adopted (resumed) {}",
        state.speleodb_lock_acquired_here(REMOTE_ID),
        state.speleodb_lock_taken_here(REMOTE_ID),
        state.speleodb_lock_resumed_here(REMOTE_ID),
    );
    assert!(
        matches!(polled, Ok(true)),
        "premise: the poll opens the gate over the account's lock"
    );
    (temp, state)
}

/// The poll adopts the account's lock: an edit session with a claim of its own, for
/// the release only (no authorship), and the silent releases hand it back. Before
/// SDB-ACCT-D1 the reopen recorded no claim and close, quit and logout sent nothing.
#[test]
fn close_quit_and_logout_hand_back_a_lock_the_poll_adopted() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);

    let (_close_dir, closing) = adopted_by_the_poll(&base);
    let adopted = closing.speleodb_lock_acquired_here(REMOTE_ID)
        && !closing.speleodb_lock_taken_here(REMOTE_ID);
    closing.speleodb_release_held_lock_best_effort();
    let after_close = release_requests(&seen).len();

    let (_quit_dir, quitting) = adopted_by_the_poll(&base);
    quitting.speleodb_release_held_lock_bounded(Duration::from_secs(5));
    let after_quit = release_requests(&seen).len();

    let (_logout_dir, leaving) = adopted_by_the_poll(&base);
    let logout = leaving.speleodb_logout(true).map_err(|e| e.message);
    let after_logout = release_requests(&seen).len();
    println!(
        "[SDB-QUIT-D1] adopted lock: claimed for the release only {adopted}; release \
         requests after close {after_close}, after quit {after_quit}, after logout \
         {after_logout} ({logout:?})"
    );
    assert!(
        adopted,
        "the adopted lock is claimed, without the authorship of a lock taken here"
    );
    assert_eq!(after_close, 1, "a clean close hands the adopted lock back");
    assert_eq!(after_quit, 2, "so does the quit");
    assert_eq!(after_logout, 3, "and the logout");
}

/// [#579 x #576 SDB-QUIT-D1] An update restart exits through the quit's release
/// (`release_held_lock_on_exit`: `RunEvent::ExitRequested`, or on Windows the
/// updater's `on_before_exit` hook; the hook exits the process, so one of the two
/// runs). After the quit prompt's "Discard & release" (`speleodb_release_current_lock`,
/// which shuts the gate), that exit release finds nothing to hand back: the lock is
/// released once, not again on the way out.
#[test]
fn the_exit_release_after_discard_and_release_sends_nothing_more() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_dir, restarting) = editing_fixture(&base);

    let released = restarting
        .speleodb_release_current_lock()
        .map_err(|e| e.message);
    let after_discard = release_requests(&seen).len();
    restarting.speleodb_release_held_lock_bounded(Duration::from_secs(5));
    let after_exit = release_requests(&seen).len();
    println!(
        "[SDB-QUIT-D1] update restart after Discard & release ({released:?}): release \
         requests {after_discard}, after the exit release {after_exit}"
    );
    assert!(released.is_ok(), "{released:?}");
    assert_eq!(after_discard, 1, "Discard & release hands the lock back");
    assert_eq!(after_exit, 1, "and the exit release does not again");
}

/// The rule is the edit session, not the claim: an open gate (not force-editing)
/// over the account's lock with no claim at all is released on close and on quit
/// too. It used to be skipped ("this session did not take the lock").
#[test]
fn close_and_quit_release_a_lock_this_session_edits_under_without_a_claim() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let unclaimed = || {
        let (temp, state) = editing_fixture(&base);
        let _ = state.speleodb_forget_lock_acquired_for(REMOTE_ID);
        assert!(state.edit_enabled() && !state.speleodb_lock_acquired_here(REMOTE_ID));
        (temp, state)
    };

    let (_close_dir, closing) = unclaimed();
    let params = closing.speleodb_held_lock_release_params().is_some();
    closing.speleodb_release_held_lock_best_effort();
    let after_close = release_requests(&seen);

    let (_quit_dir, quitting) = unclaimed();
    quitting.speleodb_release_held_lock_bounded(Duration::from_secs(5));
    let after_quit = release_requests(&seen);
    println!(
        "[SDB-QUIT-D1] editing with no claim: release params offered: {params}; release \
         requests after close: {}; after quit: {}; {after_quit:?}",
        after_close.len(),
        after_quit.len()
    );
    assert!(params, "the silent release is offered for the edit session");
    assert_eq!(after_close.len(), 1, "close hands the lock back");
    assert_eq!(after_quit.len(), 2, "quit hands the lock back");
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

/// Every lock release answered 502; every other request answered as the project with
/// our account holding the lock (fuzz seed 615006's shape).
fn releases_fail(line: &str) -> (u16, &'static str) {
    if line.starts_with("POST ") && line.contains("/release/") {
        (502, r#"{"detail":"Bad gateway"}"#)
    } else {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    }
}

/// [#576 Q-W61-3-D1] A logout whose release the server refuses signs out anyway and
/// names the project whose lock is still held, for the frontend's warning. It used to
/// answer `Ok(())` with the lock held and nothing said.
#[test]
fn a_logout_whose_release_fails_signs_out_and_names_the_lock_left_held() {
    let (base, _listener, seen) = routing_stub(releases_fail);
    let (_temp, state) = editing_fixture(&base);
    let report = state.speleodb_logout(true).map_err(|e| e.message);
    let tried = release_requests(&seen).len();
    let signed_in = state
        .speleodb_session_status()
        .map(|status| status.logged_in)
        .map_err(|e| e.message);
    let left: Vec<(String, String)> = report
        .iter()
        .flat_map(|report| report.locks_left_held.iter())
        .map(|left| (left.project_id.clone(), left.project_name.clone()))
        .collect();
    println!(
        "[Q-W61-3-D1] logout, every release answered 502 -> {report:?}; release requests \
         {tried}; still signed in {signed_in:?}; editing {}",
        state.edit_enabled()
    );
    assert!(tried >= 1, "the release was tried");
    assert_eq!(
        left,
        vec![(REMOTE_ID.to_owned(), "Remote".to_owned())],
        "the answer names the project whose lock is still held"
    );
    assert_eq!(signed_in, Ok(false), "the sign-out still happened");
    assert!(!state.edit_enabled(), "and editing is off");
}

/// Control: a release that lands leaves nothing to warn about.
#[test]
fn a_logout_whose_release_lands_names_no_lock() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    let report = state.speleodb_logout(true).map_err(|e| e.message);
    println!(
        "[Q-W61-3-D1] logout, the release answered 200 -> {report:?}; release requests {}",
        release_requests(&seen).len()
    );
    assert_eq!(release_requests(&seen).len(), 1);
    assert!(
        report.is_ok_and(|report| report.locks_left_held.is_empty()),
        "no lock is named"
    );
}

/// The (id, name) pairs a lock list names.
fn named(locks: &[crate::core::SpeleoLockLeftHeld]) -> Vec<(String, String)> {
    locks
        .iter()
        .map(|left| (left.project_id.clone(), left.project_name.clone()))
        .collect()
}

/// Whether the session's claim on the open project is marked kept on purpose.
fn claim_kept(state: &AppState) -> bool {
    state
        .speleodb
        .lock()
        .locks_acquired_here
        .iter()
        .any(|claim| claim.project_id == REMOTE_ID && claim.is_kept())
}

/// The (releasable, unpushed) names the sign-out prompt would show.
fn prompt_names(
    locks: &crate::core::SpeleoSignOutLocks,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    (named(&locks.releasable), named(&locks.unpushed))
}

/// [#576 SIGNOUT-PROMPT-D1/D2] The sign-out prompt names the lock this session holds:
/// the open project's, while editing under it. Over a clean copy it is offered for
/// release; over un-pushed edits it is listed apart, as kept (the veto keeps it
/// whatever the answer). Read-only or signed out, there is nothing to ask.
#[test]
fn the_sign_out_prompt_names_the_held_lock_and_whether_the_sign_out_keeps_it() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, editing) = editing_fixture(&base);
    let (_temp_ro, read_only) = remote_readonly_fixture();
    *read_only.speleodb.lock() = authed_session_at(&base);
    super::speleodb::link_open_copy_to(&read_only, &base);
    let (_temp_dirty, dirty) = super::speleodb::remote_fixture_with_dirty_bit(true);
    *dirty.speleodb.lock() = authed_session_at(&base);
    super::speleodb::link_open_copy_to(&dirty, &base);
    dirty.set_edit_enabled(true);
    let (_temp_out, signed_out) = editing_fixture(&base);
    signed_out.speleodb.lock().forget_reporting().unwrap();
    let remote = vec![(REMOTE_ID.to_owned(), "Remote".to_owned())];

    let clean = prompt_names(&editing.speleodb_sign_out_locks());
    let unpushed = prompt_names(&dirty.speleodb_sign_out_locks());
    let read_only = prompt_names(&read_only.speleodb_sign_out_locks());
    let out = prompt_names(&signed_out.speleodb_sign_out_locks());
    println!(
        "[SIGNOUT-PROMPT-D2] sign-out locks (releasable, unpushed): editing a clean copy \
         {clean:?}; editing over un-pushed edits {unpushed:?}; read-only {read_only:?}; \
         signed out {out:?}"
    );
    assert_eq!(
        clean,
        (remote.clone(), Vec::new()),
        "a clean copy's lock is offered for release"
    );
    assert_eq!(
        unpushed,
        (Vec::new(), remote),
        "a lock over un-pushed edits is named as kept, not offered for release"
    );
    assert_eq!(
        read_only,
        (Vec::new(), Vec::new()),
        "read-only: nothing to ask"
    );
    assert_eq!(out, (Vec::new(), Vec::new()), "signed out: nothing to ask");
    assert!(
        release_requests(&seen).is_empty(),
        "asking changes nothing on the server"
    );
    assert!(editing.edit_enabled(), "nor the edit session");
}

/// [#576 SIGNOUT-PROMPT-D1] "Keep locks and sign out": no release request goes out,
/// the sign-out happens, editing stops, and the lock is marked kept on purpose so
/// the stranded-claim janitor does not hand it back later.
#[test]
fn a_logout_that_keeps_its_locks_sends_no_release() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    let report = state.speleodb_logout(false).map_err(|e| e.message);
    let releases = release_requests(&seen);
    let signed_in = state
        .speleodb_session_status()
        .map(|status| status.logged_in)
        .map_err(|e| e.message);
    let kept = claim_kept(&state);
    println!(
        "[SIGNOUT-PROMPT-D1] logout keeping its locks -> {report:?}; release requests \
         {releases:?}; still signed in {signed_in:?}; editing {}; claim kept {kept}",
        state.edit_enabled()
    );
    assert!(releases.is_empty(), "no release goes out: {releases:?}");
    assert!(
        report.is_ok_and(|report| report.locks_left_held.is_empty()),
        "nothing failed, so nothing is named"
    );
    assert_eq!(signed_in, Ok(false), "signed out");
    assert!(!state.edit_enabled(), "editing is off");
    assert!(kept, "the lock is kept on purpose");
}
