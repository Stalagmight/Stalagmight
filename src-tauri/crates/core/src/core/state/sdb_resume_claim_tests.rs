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
//! [#576 SDB-2MACHINE-D1] Once an edit has been made under it here: an adopted lock
//! this session has only seen may be another computer's of the account, so a clean
//! Done with no edit made here finishes editing and leaves the lock held
//! (`LockHeldElsewhere`).
//!
//! Driven against the loopback stub of `sdb_lock_rule_tests` (one project body for
//! every request, naming the holder; each request line recorded).

use std::sync::atomic::{AtomicBool, Ordering};

use super::speleodb::{
    authed_session_at, link_open_copy_to, recording_stub, remote_readonly_fixture, routing_stub,
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

/// [#576 SDB-2MACHINE-D1] One survey edit made here under the lock, then the copy
/// marked clean, as the push that uploads it leaves the link.
fn edit_here_then_push(state: &super::AppState) {
    let edited = state.execute(crate::core::Command::CreateSection {
        name: Some("SectionB".to_owned()),
        surveyor: None,
        date: None,
        color: "#336699".to_owned(),
        direction: None,
        shots: Vec::new(),
    });
    assert!(
        edited.success,
        "premise: the edit lands: {:?}",
        edited.error
    );
    state
        .speleodb_mutate_link(|link| link.has_unpushed_edits = false)
        .expect("mark the copy clean, as a landed push does");
}

#[test]
fn continue_editing_claims_the_resumed_lock_so_done_releases_it_after_an_edit() {
    // Our own account holds the lock at sign-in (this app before a restart, or
    // another computer of the account).
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let resume = || {
        let (temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&base);
        link_open_copy_to(&state, &base);
        let started = state
            .speleodb_resume_held_lock_editing()
            .expect("continue editing");
        assert!(matches!(started, StartEditingResult::Editing));
        (temp, state)
    };

    let (_seen_dir, only_seen) = resume();
    let claimed = only_seen.speleodb_lock_acquired_here(REMOTE_ID);
    println!(
        "Continue editing over u@x's own lock: editing on: {}; claim recorded: {claimed}",
        only_seen.edit_enabled()
    );
    assert!(only_seen.edit_enabled());
    assert!(claimed, "the resumed lock is claimed for this session");
    let done_unused = only_seen.speleodb_done(None);
    let after_unused = releases(&seen);

    let (_edited_dir, edited) = resume();
    edit_here_then_push(&edited);
    let done_edited = edited.speleodb_done(None);
    let after_edited = releases(&seen);
    println!(
        "[SDB-2MACHINE-D1] clean Done, no edit made here -> {done_unused:?}, release \
         requests {after_unused}; after one edit -> {done_edited:?}, {after_edited}"
    );
    assert_eq!(
        format!("{done_unused:?}"),
        "Ok(LockHeldElsewhere)",
        "with no edit made here the lock stays with the account"
    );
    assert_eq!(after_unused, 0, "nothing is released");
    assert_eq!(
        format!("{done_edited:?}"),
        "Ok(Released)",
        "Done releases as usual"
    );
    assert_eq!(after_edited, 1, "the lock is handed back once");
    assert!(!only_seen.edit_enabled() && !edited.edit_enabled());
}

#[test]
fn plain_start_editing_over_the_accounts_lock_adopts_it_so_done_releases_it_after_an_edit() {
    // [#576 SDB-ACCT-D1] Another window, another computer or the website took the
    // lock on this account; a plain Start editing here adopts it. [#576
    // SDB-2MACHINE-D1] Done hands it back once an edit has been made under it here,
    // and before that finishes here and leaves it held (LockHeldElsewhere).
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let start = || {
        let (temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&base);
        link_open_copy_to(&state, &base);
        let started = state.speleodb_start_editing().expect("start editing");
        assert!(matches!(started, StartEditingResult::Editing));
        (temp, state)
    };

    let (_seen_dir, only_seen) = start();
    let (claimed, taken) = (
        only_seen.speleodb_lock_acquired_here(REMOTE_ID),
        only_seen.speleodb_lock_taken_here(REMOTE_ID),
    );
    let done_unused = only_seen.speleodb_done(None);
    let after_unused = releases(&seen);

    let (_edited_dir, edited) = start();
    edit_here_then_push(&edited);
    let done_edited = edited.speleodb_done(None);
    println!(
        "[SDB-ACCT-D1] plain Start editing over u@x's lock: claimed {claimed}; taken here \
         {taken}; clean Done, no edit made here -> {done_unused:?}, release requests \
         {after_unused}; after one edit -> {done_edited:?}, {}",
        releases(&seen)
    );
    assert!(
        claimed && !taken,
        "the account's lock is adopted: claimed for the release, not as taken here"
    );
    assert_eq!(format!("{done_unused:?}"), "Ok(LockHeldElsewhere)");
    assert_eq!(
        after_unused, 0,
        "with no edit made here nothing is released"
    );
    assert_eq!(
        format!("{done_edited:?}"),
        "Ok(Released)",
        "Done releases as usual"
    );
    assert_eq!(releases(&seen), 1, "the lock is handed back once");
    assert!(!only_seen.edit_enabled() && !edited.edit_enabled());
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
    link_open_copy_to(&state, &base);

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

#[test]
fn done_leaves_a_lock_the_poll_adopted_held_only_by_its_adopted_claim() {
    // [#576 SDB-ACCT-D1] The status poll adopts the account's lock (another window,
    // another computer, the website). Done keys on the edit gate, as the silent
    // releases do: with no claim at all it releases. [#576 SDB-2MACHINE-D1] The one
    // claim it reads is the adopted one with no edit made here: then it finishes
    // here as `LockHeldElsewhere` and leaves the lock with the session using it.
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let adopt = || {
        let (temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&base);
        link_open_copy_to(&state, &base);
        let polled = state.speleodb_link_status().map(|s| s.edit_enabled);
        assert!(
            matches!(polled, Ok(true)) && state.speleodb_lock_resumed_here(REMOTE_ID),
            "premise: the poll adopts the account's lock ({polled:?})"
        );
        (temp, state)
    };

    let (_claimed_dir, claimed) = adopt();
    let with_claim = claimed.speleodb_done(None);
    let after_claimed = releases(&seen);

    let (_unclaimed_dir, unclaimed) = adopt();
    let _ = unclaimed.speleodb_forget_lock_acquired_for(REMOTE_ID);
    let gate_without_claim =
        unclaimed.edit_enabled() && !unclaimed.speleodb_lock_acquired_here(REMOTE_ID);
    let without_claim = unclaimed.speleodb_done(None);
    let after_unclaimed = releases(&seen);
    println!(
        "[SDB-ACCT-D1] Done over a lock the poll adopted -> {with_claim:?}, release \
         requests {after_claimed}; with the claim gone (gate open {gate_without_claim}) \
         -> {without_claim:?}, release requests {after_unclaimed}"
    );
    assert_eq!(format!("{with_claim:?}"), "Ok(LockHeldElsewhere)");
    assert_eq!(
        after_claimed, 0,
        "the adopted lock with no edit made here is left held"
    );
    assert!(gate_without_claim, "premise: an edit session with no claim");
    assert_eq!(
        format!("{without_claim:?}"),
        "Ok(Released)",
        "Done keys on the edit gate when no claim says the lock was adopted"
    );
    assert_eq!(after_unclaimed, 1, "and hands that lock back");
    assert!(!claimed.edit_enabled() && !unclaimed.edit_enabled());
}
