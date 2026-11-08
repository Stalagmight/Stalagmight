//! [#576 RV2-R-15] Restore revision and Unwind replace the survey, so the read-only
//! Plan and Export workspaces refuse them like every other survey change (C5).
//!
//! Decided: "Gate Restore revision + Unwind in Plan/Export; imported-file record
//! stays ungated". View (a read-only peek) is not a survey change and stays allowed.

use super::speleodb::{
    editing_fixture, recording_stub, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::WorkspaceMode;
use crate::core::CommandError;
use crate::models::CommandErrorCode;

const SHA: &str = "abc1230000000000000000000000000000000000";

/// One Restore (`keep_undo_history == false`) or Unwind (`true`) request, the way
/// the dialogs send it.
fn restore(state: &super::AppState, keep_undo_history: bool) -> Result<(), CommandError> {
    state
        .speleodb_restore_revision(
            REMOTE_ID.to_owned(),
            "Remote".to_owned(),
            SHA.to_owned(),
            true,
            false,
            None,
            keep_undo_history,
            true,
            false,
        )
        .map(|_| ())
        .map_err(CommandError::from)
}

#[test]
fn restore_and_unwind_are_refused_in_plan_and_export() {
    for mode in [WorkspaceMode::Plan, WorkspaceMode::Export] {
        for (what, keep_undo_history) in [("Restore", false), ("Unwind", true)] {
            let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
            let (_temp, state) = editing_fixture(&base);
            state.set_workspace_mode(mode);

            let result = restore(&state, keep_undo_history);

            let requests = seen.lock().expect("request log").len();
            println!("{what} in {mode:?} -> {result:?}; requests sent: {requests}");
            let refusal = result.expect_err("a survey-replacing swap is refused here");
            // [#576 C4] The workspace gate's own code, not SPELEODB_FATAL, and no
            // marker in the text.
            assert!(
                refusal.code == CommandErrorCode::WorkspaceReadOnly
                    && !refusal.message.contains("WORKSPACE_READ_ONLY"),
                "{what} in {mode:?}: refused with the workspace notice, got: {refusal:?}"
            );
            assert_eq!(requests, 0, "refused before any download");
            assert!(state.edit_enabled(), "the edit session itself is untouched");
        }
    }
}

#[test]
fn view_is_not_a_survey_change_and_is_not_refused_by_the_workspace() {
    let (base, _listener, _seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    state.set_workspace_mode(WorkspaceMode::Plan);

    let result = state.speleodb_restore_revision(
        REMOTE_ID.to_owned(),
        "Remote".to_owned(),
        SHA.to_owned(),
        false,
        false,
        None,
        false,
        false,
        false,
    );

    // The download itself needs the app cache dir only the live tests set up, so the
    // View fails further on - but never on the workspace gate.
    if let Err(e) = result {
        assert!(
            e.code != CommandErrorCode::WorkspaceReadOnly,
            "a View is not refused by the workspace gate: {}",
            e.message
        );
    }
}

/// [#576 FZ-SDB-4] View is the read-only, no-lock peek, so it is refused while the
/// app is editing under the write lock. Accepted, its reload shut the gate without
/// handing the lock claim back, the next status poll forgot the claim, and the
/// quit-time silent release then left the lock held (fuzzer: ResumeHeldLock ->
/// View -> Quit). Refused before anything is touched: no request, gate and claim
/// kept, the project still open. [#576 SDB-ACCT-D1] The refusal reads the edit gate,
/// which is local state, so deciding it needs no request either.
#[test]
fn view_is_refused_while_editing_under_the_write_lock() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    assert!(
        state.edit_enabled()
            && !state.force_editing()
            && state.speleodb_lock_acquired_here(REMOTE_ID),
        "precondition: an edit session that took the lock"
    );

    let result = state
        .speleodb_restore_revision(
            REMOTE_ID.to_owned(),
            "Remote".to_owned(),
            SHA.to_owned(),
            false,
            false,
            None,
            false,
            false,
            false,
        )
        .map(|_| ())
        .map_err(CommandError::from);

    let requests = seen.lock().expect("request log").clone();
    println!("View while editing -> {result:?}; requests sent: {requests:?}");
    let refusal = result.expect_err("a View under the write lock is refused");
    assert!(
        refusal.code == CommandErrorCode::SpeleodbFatal
            && refusal.message.contains("release the write lock"),
        "refused with the coded View refusal, got: {refusal:?}"
    );
    assert!(
        requests.is_empty(),
        "refused before any request: {requests:?}"
    );
    assert!(
        state.edit_enabled() && !state.force_editing(),
        "the edit gate is still open"
    );
    assert!(
        state.speleodb_lock_acquired_here(REMOTE_ID),
        "the lock claim is still held"
    );
    assert!(
        state
            .speleodb_open_link()
            .is_ok_and(|link| link.project_id == REMOTE_ID),
        "the linked project is still open"
    );
}

/// A View request, the way the revision dialog sends it.
fn view(state: &super::AppState) -> Result<(), CommandError> {
    state
        .speleodb_restore_revision(
            REMOTE_ID.to_owned(),
            "Remote".to_owned(),
            SHA.to_owned(),
            false,
            false,
            None,
            false,
            false,
            false,
        )
        .map(|_| ())
        .map_err(CommandError::from)
}

/// [#576 SDB-ACCT-D1, SDB-QUIT-D1] The FZ-SDB-4 refusal holds for a lock this session
/// ADOPTED from the account too. #576 RVF-b let a View through over a lock this
/// session had no claim on (the status poll's inherited-lock reopen), since no release
/// was going to hand that lock back. The quit hands such a lock back now, so a View's
/// reload over it would strand it exactly as over a lock taken here. Both shapes are
/// refused before any request: the poll's adopted lock, and an edit gate open over the
/// account's lock with no claim at all.
#[test]
fn view_is_refused_over_a_lock_this_session_adopted() {
    let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_adopted_dir, adopted) = super::speleodb::remote_readonly_fixture();
    *adopted.speleodb.lock() = super::speleodb::authed_session_at(&base);
    super::speleodb::link_open_copy_to(&adopted, &base);
    let polled = adopted.speleodb_link_status().map(|s| s.edit_enabled);
    assert!(
        matches!(polled, Ok(true)),
        "precondition: the poll adopts the account's lock, got {polled:?}"
    );
    let before_view = seen.lock().expect("request log").len();
    let over_adopted = view(&adopted);
    let after_adopted = seen.lock().expect("request log").len();

    let (_unclaimed_dir, unclaimed) = editing_fixture(&base);
    let _ = unclaimed.speleodb_forget_lock_acquired_for(REMOTE_ID);
    assert!(
        unclaimed.edit_enabled() && !unclaimed.speleodb_lock_acquired_here(REMOTE_ID),
        "precondition: an edit gate open over a lock this session does not claim"
    );
    let over_unclaimed = view(&unclaimed);
    let after_unclaimed = seen.lock().expect("request log").len();

    println!(
        "[SDB-ACCT-D1] View over the poll's adopted lock -> {over_adopted:?}; over an \
         unclaimed one -> {over_unclaimed:?}; requests sent by the Views: {}",
        after_unclaimed - before_view
    );
    for (what, result) in [("adopted", over_adopted), ("unclaimed", over_unclaimed)] {
        let refusal = result.expect_err("a View under the account's write lock is refused");
        assert!(
            refusal.code == CommandErrorCode::SpeleodbFatal
                && refusal.message.contains("release the write lock"),
            "{what}: refused with the coded View refusal, got: {refusal:?}"
        );
    }
    assert_eq!(
        (after_adopted, after_unclaimed),
        (before_view, before_view),
        "both refused before any request"
    );
    assert!(
        adopted.edit_enabled() && adopted.speleodb_lock_acquired_here(REMOTE_ID),
        "the adopted edit session and its claim are untouched"
    );
}

#[test]
fn the_imported_file_record_stays_ungated() {
    // Only reached inside an import flow, which is itself gated; recording the hash
    // is bookkeeping, not a survey change.
    let (base, _listener, _seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
    let (_temp, state) = editing_fixture(&base);
    state.set_workspace_mode(WorkspaceMode::Export);
    assert!(state.track_imported_file("hash-a".to_owned()).success);
}
