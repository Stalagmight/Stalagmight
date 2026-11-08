//! [#576 SDB-2MACHINE-D1] Two computers signed in to one account: computer A edits
//! under the project's write lock, and on computer B the status poll sees the
//! account holding it and opens editing over it (the adoption of SDB-ACCT-D1). B's
//! clean close, switch, quit or Done used to hand that lock back while A was still
//! editing under it.
//!
//! Decided: an ADOPTED lock (the account's, not taken by this session) is released by
//! close, switch, quit and Done only once this session has made an edit under it. A
//! lock this session took is released as before. The sign-out prompt still lists an
//! adopted lock: one with no edit made here apart, as in use elsewhere, released only
//! when the user picks Release. Deleting the open project still releases it.
//!
//! Every test here plays computer B against the loopback recording stub, which
//! answers each request as the project with the account holding the lock (A's
//! session). All data is synthetic.

use std::time::{Duration, Instant};

use super::speleodb::{
    authed_session_at, editing_fixture, link_open_copy_to, recording_stub, remote_readonly_fixture,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::AppState;
use crate::core::Command;

type Seen = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

/// How many release requests reached the server so far.
fn releases(seen: &Seen) -> usize {
    seen.lock()
        .map(|lines| lines.iter().filter(|r| r.contains("/release/")).count())
        .unwrap_or_default()
}

/// [`releases`] after a release that may run on a detached thread (the project
/// switch's): waits up to 1.5 s for the count to pass `before`.
fn releases_after_detached(seen: &Seen, before: usize) -> usize {
    let deadline = Instant::now() + Duration::from_millis(1500);
    loop {
        let now = releases(seen);
        if now > before || Instant::now() >= deadline {
            return now;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Computer B: the clean copy is open read-only and signed in, the account holds
/// the lock (A edits under it), and the status poll adopts it.
fn adopted_by_the_poll(base: &str) -> (tempfile::TempDir, AppState) {
    let (temp, state) = remote_readonly_fixture();
    *state.speleodb.lock() = authed_session_at(base);
    link_open_copy_to(&state, base);
    let polled = state.speleodb_link_status().map(|s| s.edit_enabled);
    assert!(
        matches!(polled, Ok(true))
            && state.speleodb_lock_resumed_here(REMOTE_ID)
            && !state.speleodb_lock_taken_here(REMOTE_ID),
        "premise: the poll adopts the account's lock ({polled:?})"
    );
    (temp, state)
}

/// One survey edit made on B under the lock, then the copy marked clean, as the
/// push that uploads it leaves the link (an un-pushed copy keeps its lock by the
/// un-pushed-edits veto whatever the claim says, so the rule shows on a clean one).
fn edit_here_then_push(state: &AppState) {
    let edited = state.execute(Command::CreateSection {
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

/// How many claims this process still records.
fn claims(state: &AppState) -> usize {
    state.speleodb.lock().locks_acquired_here.len()
}

/// The project switch: open a local project over the open copy, through the full
/// load (its release of the outgoing lock runs on a detached thread). The load
/// reports progress, so it needs the host-less event sink `state_with_null_sink`
/// installs; that sink lends no managed state, so the release sends its raw POST.
fn switch_to_another_project(state: &AppState, dir: &std::path::Path) -> Result<(), String> {
    let other = super::switch_identity_tests::saved(dir, "Other.stalx", "Other");
    state.set_event_sink(std::sync::Arc::new(crate::core::events::NullEventSink));
    state
        .do_load_project_full(other)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// The (project id, name) pairs one list of the sign-out prompt names, read off the
/// IPC payload (`SpeleoSignOutLocks`).
fn listed(locks: &serde_json::Value, list: &str) -> Vec<(String, String)> {
    locks
        .get(list)
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|e| {
                    (
                        e["project_id"].as_str().unwrap_or_default().to_owned(),
                        e["project_name"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn remote() -> Vec<(String, String)> {
    vec![(REMOTE_ID.to_owned(), "Remote".to_owned())]
}

// --- Adopted, no edit made here: the lock stays with computer A -----------------

#[test]
fn a_clean_close_leaves_an_adopted_lock_with_no_edit_made_here_held() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_dir, b) = adopted_by_the_poll(&base);

    // `close_project` with releaseLock true, as the command runs it.
    b.speleodb_release_held_lock_best_effort();
    let closed = b.execute(Command::CloseProject);
    let after_close = releases(&seen);
    // The next status poll, with no project open, runs the stranded-claim janitor.
    let _ = b.speleodb_link_status();
    let after_janitor = releases(&seen);
    println!(
        "[SDB-2MACHINE-D1] B adopted the lock, made no edit, closed: close success={}; \
         release requests after the close {after_close}, after the janitor's poll \
         {after_janitor}; claims left {}",
        closed.success,
        claims(&b)
    );
    assert!(closed.success, "{:?}", closed.error);
    assert_eq!(after_close, 0, "the close sends no release");
    assert_eq!(
        after_janitor, 0,
        "nor does the stranded-claim janitor afterwards"
    );
    assert_eq!(claims(&b), 0, "the claim ended with B's session");
}

#[test]
fn a_project_switch_leaves_an_adopted_lock_with_no_edit_made_here_held() {
    crate::test_helpers::with_isolated_config(|| {
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (dir, b) = adopted_by_the_poll(&base);

        let switched = switch_to_another_project(&b, dir.path());
        let after_switch = releases_after_detached(&seen, 0);
        println!(
            "[SDB-2MACHINE-D1] B adopted the lock, made no edit, opened another project: \
             load {switched:?}; release requests {after_switch}; claims left {}",
            claims(&b)
        );
        assert!(
            switched.is_ok(),
            "premise: the switch loads the other project"
        );
        assert_eq!(after_switch, 0, "the switch sends no release");
        assert_eq!(claims(&b), 0, "the claim ended with B's session");
    });
}

#[test]
fn the_quit_leaves_an_adopted_lock_with_no_edit_made_here_held() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_dir, b) = adopted_by_the_poll(&base);

    b.speleodb_release_held_lock_bounded(Duration::from_secs(5));
    let after_quit = releases(&seen);
    println!(
        "[SDB-2MACHINE-D1] B adopted the lock, made no edit, quit: release requests \
         {after_quit}"
    );
    assert_eq!(after_quit, 0, "the quit sends no release");
}

#[test]
fn the_janitor_drops_a_stranded_adopted_claim_with_no_edit_made_here_unreleased() {
    // A 401 drop shuts the gate and keeps the claims, so the close after it has no
    // edit session to decide on and leaves the claim stranded. The first status poll
    // after signing back in, with no project open, runs the stranded-claim janitor.
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_dir, b) = adopted_by_the_poll(&base);

    b.speleodb_forget();
    b.speleodb_release_held_lock_best_effort();
    let closed = b.execute(Command::CloseProject);
    let stranded = claims(&b);
    // Signed back in as the same account, the claims carried over.
    let carried = std::mem::take(&mut b.speleodb.lock().locks_acquired_here);
    let mut again = authed_session_at(&base);
    again.locks_acquired_here = carried;
    *b.speleodb.lock() = again;
    let _ = b.speleodb_link_status();
    let after_janitor = releases(&seen);
    println!(
        "[SDB-2MACHINE-D1] B adopted the lock, made no edit, 401 drop, closed: close \
         success={}; claims stranded {stranded}; release requests after the janitor's \
         poll {after_janitor}; claims left {}",
        closed.success,
        claims(&b)
    );
    assert!(closed.success, "{:?}", closed.error);
    assert_eq!(stranded, 1, "premise: the claim outlived the gate");
    assert_eq!(after_janitor, 0, "the janitor sends no release");
    assert_eq!(claims(&b), 0, "the claim ended with B's session");
}

#[test]
fn done_finishes_here_and_leaves_an_adopted_lock_with_no_edit_made_here_held() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_dir, b) = adopted_by_the_poll(&base);

    let done = b.speleodb_done(None).map_err(|e| e.message);
    let after_done = releases(&seen);
    let editing_after_done = b.edit_enabled();
    // The status poll sees the account's lock over a shut gate again: it must not
    // switch editing back on in this session.
    let polled = b.speleodb_link_status().map(|s| s.edit_enabled);
    // An explicit Start editing is the way back in.
    let restarted = b
        .speleodb_start_editing()
        .map(|r| format!("{r:?}"))
        .map_err(|e| e.message);
    let polled_after_start = b.speleodb_link_status().map(|s| s.edit_enabled);
    println!(
        "[SDB-2MACHINE-D1] B adopted the lock, made no edit, Done -> {done:?}; release \
         requests {after_done}; editing after Done {editing_after_done}; the next poll \
         -> edit_enabled {polled:?}; Start editing -> {restarted:?}, then the poll -> \
         {polled_after_start:?}"
    );
    assert_eq!(
        format!("{done:?}"),
        "Ok(LockHeldElsewhere)",
        "Done finishes here and says the lock stays with the account elsewhere"
    );
    assert_eq!(after_done, 0, "Done sends no release");
    assert!(!editing_after_done, "editing has ended here");
    assert!(
        matches!(polled, Ok(false)),
        "the poll does not reopen editing after that Done"
    );
    assert_eq!(
        restarted.as_deref(),
        Ok("Editing"),
        "Start editing resumes it"
    );
    assert!(matches!(polled_after_start, Ok(true)));
    assert_eq!(releases(&seen), 0, "and nothing was released on the way");
}

// --- Adopted, then one edit made here: the lock is B's to hand back -------------

#[test]
fn a_clean_close_releases_an_adopted_lock_after_one_edit_made_here() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_dir, b) = adopted_by_the_poll(&base);
    edit_here_then_push(&b);

    b.speleodb_release_held_lock_best_effort();
    let after_close = releases(&seen);
    println!(
        "[SDB-2MACHINE-D1] B adopted the lock, made one edit (pushed), closed: release \
         requests {after_close}"
    );
    assert_eq!(after_close, 1, "the close hands the lock back");
}

#[test]
fn switch_quit_and_done_release_an_adopted_lock_after_one_edit_made_here() {
    crate::test_helpers::with_isolated_config(|| {
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);

        let (dir, switching) = adopted_by_the_poll(&base);
        edit_here_then_push(&switching);
        let switched = switch_to_another_project(&switching, dir.path());
        let after_switch = releases_after_detached(&seen, 0);

        let (_quit_dir, quitting) = adopted_by_the_poll(&base);
        edit_here_then_push(&quitting);
        quitting.speleodb_release_held_lock_bounded(Duration::from_secs(5));
        let after_quit = releases(&seen);

        let (_done_dir, finishing) = adopted_by_the_poll(&base);
        edit_here_then_push(&finishing);
        let done = finishing.speleodb_done(None).map_err(|e| e.message);
        let after_done = releases(&seen);
        println!(
            "[SDB-2MACHINE-D1] adopted, one edit made here: switch ({switched:?}) -> \
             release requests {after_switch}; quit -> {after_quit}; Done -> {done:?}, \
             {after_done}"
        );
        assert!(
            switched.is_ok(),
            "premise: the switch loads the other project"
        );
        assert_eq!(after_switch, 1, "the switch hands the lock back");
        assert_eq!(after_quit, 2, "so does the quit");
        assert_eq!(format!("{done:?}"), "Ok(Released)");
        assert_eq!(after_done, 3, "and Done");
    });
}

#[test]
fn an_undo_history_setting_change_is_an_edit_made_here() {
    // The other dirty-marking write (`speleodb_set_undo_mode`): the setting rides the
    // next push, so it is an edit of the project like a survey edit.
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_dir, b) = adopted_by_the_poll(&base);
    let set = b
        .speleodb_set_undo_mode(crate::models::UndoMode::Disabled)
        .map_err(|e| e.message);
    b.speleodb_mutate_link(|link| link.has_unpushed_edits = false)
        .expect("mark the copy clean, as a landed push does");

    b.speleodb_release_held_lock_best_effort();
    let after_close = releases(&seen);
    println!(
        "[SDB-2MACHINE-D1] adopted, undo-history setting changed ({set:?}), closed: \
         release requests {after_close}"
    );
    assert!(set.is_ok(), "{set:?}");
    assert_eq!(after_close, 1, "the close hands the lock back");
}

#[test]
fn an_edit_made_here_survives_the_adopted_claim_being_recorded_again() {
    // The update's fast-forward and a revision Restore reload the copy in place and
    // record the adopted claim again: the edit made before the reload still counts.
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_dir, b) = adopted_by_the_poll(&base);
    edit_here_then_push(&b);
    b.speleodb_note_resumed_lock(REMOTE_ID);

    b.speleodb_release_held_lock_best_effort();
    let after_close = releases(&seen);
    println!(
        "[SDB-2MACHINE-D1] adopted, one edit, the claim recorded again, closed: release \
         requests {after_close}"
    );
    assert_eq!(after_close, 1, "the close hands the lock back");
}

// --- Control: a lock this session took is released as before -------------------

#[test]
fn close_switch_quit_and_done_release_a_lock_this_session_took() {
    crate::test_helpers::with_isolated_config(|| {
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let taken = |state: &AppState| {
            assert!(
                state.speleodb_lock_taken_here(REMOTE_ID),
                "premise: this session took the lock"
            );
        };

        let (_close_dir, closing) = editing_fixture(&base);
        taken(&closing);
        closing.speleodb_release_held_lock_best_effort();
        let after_close = releases(&seen);

        let (switch_dir, switching) = editing_fixture(&base);
        taken(&switching);
        let switched = switch_to_another_project(&switching, switch_dir.path());
        let after_switch = releases_after_detached(&seen, after_close);

        let (_quit_dir, quitting) = editing_fixture(&base);
        taken(&quitting);
        quitting.speleodb_release_held_lock_bounded(Duration::from_secs(5));
        let after_quit = releases(&seen);

        let (_done_dir, finishing) = editing_fixture(&base);
        taken(&finishing);
        let done = finishing.speleodb_done(None).map_err(|e| e.message);
        let after_done = releases(&seen);
        println!(
            "[SDB-2MACHINE-D1 control] taken here, no edit: close -> release requests \
             {after_close}; switch ({switched:?}) -> {after_switch}; quit -> {after_quit}; \
             Done -> {done:?}, {after_done}"
        );
        assert_eq!(after_close, 1, "close hands it back");
        assert!(
            switched.is_ok(),
            "premise: the switch loads the other project"
        );
        assert_eq!(after_switch, 2, "so does the switch");
        assert_eq!(after_quit, 3, "and the quit");
        assert_eq!(format!("{done:?}"), "Ok(Released)");
        assert_eq!(after_done, 4, "and Done");
    });
}

// --- The sign-out prompt (SIGNOUT-PROMPT-D1/D2) ----------------------------------

#[test]
fn the_sign_out_prompt_lists_an_adopted_lock_with_no_edit_made_here_as_in_use_elsewhere() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);

    let (_unused_dir, unused) = adopted_by_the_poll(&base);
    let unused_locks =
        serde_json::to_value(unused.speleodb_sign_out_locks()).expect("the prompt payload");

    let (_used_dir, used) = adopted_by_the_poll(&base);
    edit_here_then_push(&used);
    let used_locks =
        serde_json::to_value(used.speleodb_sign_out_locks()).expect("the prompt payload");
    let asked = releases(&seen);

    // "Release and sign out" hands it back: the user picked Release.
    let released = unused.speleodb_logout(true).map_err(|e| e.message);
    let after_release = releases(&seen);
    // "Keep locks and sign out" sends nothing and keeps it on purpose.
    let (_kept_dir, keeping) = adopted_by_the_poll(&base);
    let kept = keeping.speleodb_logout(false).map_err(|e| e.message);
    let after_keep = releases(&seen);
    let marked_kept = keeping
        .speleodb
        .lock()
        .locks_acquired_here
        .iter()
        .any(|claim| claim.project_id == REMOTE_ID && claim.is_kept());
    println!(
        "[SDB-2MACHINE-D1] sign-out prompt, adopted with no edit made here: {unused_locks}; \
         after one edit: {used_locks}; release requests while asking {asked}; Release and \
         sign out ({released:?}) -> {after_release}; Keep locks and sign out ({kept:?}) -> \
         {after_keep}, kept on purpose {marked_kept}"
    );
    assert_eq!(
        (
            listed(&unused_locks, "releasable"),
            listed(&unused_locks, "in_use_elsewhere"),
            listed(&unused_locks, "unpushed"),
        ),
        (Vec::new(), remote(), Vec::new()),
        "listed apart, as in use elsewhere with no edit made here"
    );
    assert_eq!(
        (
            listed(&used_locks, "releasable"),
            listed(&used_locks, "in_use_elsewhere"),
        ),
        (remote(), Vec::new()),
        "after an edit made here it is offered for release as before"
    );
    assert_eq!(asked, 0, "asking changes nothing on the server");
    assert!(released.is_ok(), "{released:?}");
    assert_eq!(after_release, 1, "Release and sign out releases it");
    assert!(kept.is_ok(), "{kept:?}");
    assert_eq!(after_keep, 1, "Keep locks and sign out sends nothing");
    assert!(marked_kept, "and keeps it on purpose");
}

// --- Delete --------------------------------------------------------------------

#[test]
fn deleting_the_open_project_releases_an_adopted_lock_with_no_edit_made_here() {
    // Decided: the delete is a deliberate, confirmed act that destroys the copy and
    // its link (so no later path could release the lock from it), like unlink and
    // take-remote; it releases the lock the session edits under, adopted or not.
    crate::test_helpers::with_isolated_config(|| {
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (_dir, b) = adopted_by_the_poll(&base);
        let path = b.open_project_path().expect("the open copy's path");

        let result = b.delete_project(path.clone());
        let after_delete = releases(&seen);
        println!(
            "[SDB-2MACHINE-D1] adopted, no edit made here, delete: success={} file gone={}; \
             release requests {after_delete}",
            result.success,
            !path.exists()
        );
        assert!(result.success, "{:?}", result.error);
        assert_eq!(after_delete, 1, "the delete hands the lock back");
    });
}
