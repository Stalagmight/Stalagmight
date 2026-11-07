//! [#576 RV5-F4-4, SDB-ACCT-D1] Deleting the OPEN project hands back the lock this
//! session edits under.
//!
//! The delete takes the abandon half of the release (no un-pushed-edits veto: the
//! user is destroying the local copy those edits live in). RV5-F4-4 gave it the
//! RV2-R-13 claim check as well, so a lock adopted from the account (the status
//! poll's inherited-lock reopen) was left held. Decision SDB-ACCT-D1: a lock the
//! account holds is the user's however they acquired it, so an edit session under it
//! is released like one under a lock this session took.
//! Driven against the loopback recording stub; all data is synthetic.

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
fn deleting_the_open_project_releases_a_lock_it_edits_under_without_a_claim() {
    crate::test_helpers::with_isolated_config(|| {
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);
        // An edit session under the account's lock with no claim of this session's.
        let _ = state.speleodb_forget_lock_acquired_for(REMOTE_ID);
        assert!(state.edit_enabled() && !state.speleodb_lock_acquired_here(REMOTE_ID));
        let path = state.open_project_path().unwrap();

        let result = state.delete_project(path.clone());
        let released = release_requests(&seen);
        println!(
            "[SDB-ACCT-D1] editing under the account's lock, no claim: delete success={} \
             file gone={}; release requests after the delete: {} {released:?}",
            result.success,
            !path.exists(),
            released.len()
        );
        assert!(
            result.success,
            "the delete itself succeeds: {:?}",
            result.error
        );
        assert!(!path.exists(), "and the file is gone");
        assert_eq!(
            released.len(),
            1,
            "the delete hands back the lock this session edits under: {released:?}"
        );
    });
}

#[test]
fn deleting_the_open_project_still_releases_a_lock_this_session_took() {
    // DO-NOT-REGRESS: with the claim, the delete hands the lock back.
    crate::test_helpers::with_isolated_config(|| {
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);
        assert!(state.speleodb_lock_acquired_here(REMOTE_ID));
        let path = state.open_project_path().unwrap();

        let result = state.delete_project(path);
        let released = release_requests(&seen);
        println!(
            "own lock: delete success={}; release requests after the delete: {}",
            result.success,
            released.len()
        );
        assert!(result.success, "{:?}", result.error);
        assert_eq!(released.len(), 1, "our own lock is released on delete");
    });
}

#[test]
fn deleting_the_open_project_releases_its_own_lock_over_unpushed_edits() {
    // DO-NOT-REGRESS: the delete is still an abandon - destroying the file forfeits
    // the un-pushed edits by the user's own instruction, so the silent release's
    // veto does not apply and the lock is not stranded.
    crate::test_helpers::with_isolated_config(|| {
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);
        // An ordinary edit marks the copy as holding un-pushed work.
        let edited = state.execute(crate::core::Command::CreateSection {
            name: Some("SectionA".to_string()),
            surveyor: None,
            date: None,
            color: "#336699".to_string(),
            direction: None,
            shots: Vec::new(),
        });
        assert!(edited.success, "{:?}", edited.error);
        assert!(state.speleodb_open_link().unwrap().has_unpushed_edits);
        let path = state.open_project_path().unwrap();

        let result = state.delete_project(path);
        let released = release_requests(&seen);
        println!(
            "own lock, un-pushed edits: delete success={}; release requests: {}",
            result.success,
            released.len()
        );
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            released.len(),
            1,
            "the delete still releases a dirty project's lock"
        );
    });
}
