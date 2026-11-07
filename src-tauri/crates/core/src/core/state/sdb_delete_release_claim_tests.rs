//! [#576 RV5-F4-4] Deleting the OPEN project hands back only a lock THIS app
//! session acquired.
//!
//! Decision RV2-R-13: "Release only when this app session acquired the lock". The
//! silent releases (close, switch, quit, logout) check the claim since RV4-R-S2,
//! but the delete of the open project took the ABANDON snapshot, which skipped it:
//! a second window or computer on the same account whose gate was opened by the
//! status poll's inherited-lock reopen (no claim) freed the lock the other session
//! is still editing under, through a verb that is about the local file.
//!
//! The delete keeps the abandon half (no un-pushed-edits veto: the user is
//! destroying the local copy those edits live in) and gains the claim check.
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
fn deleting_the_open_project_never_releases_a_lock_this_session_did_not_take() {
    crate::test_helpers::with_isolated_config(|| {
        let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
        let (_temp, state) = editing_fixture(&base);
        // An inherited edit session: gate open, the lock is the account's, no claim.
        let _ = state.speleodb_forget_lock_acquired_for(REMOTE_ID);
        assert!(state.edit_enabled() && !state.speleodb_lock_acquired_here(REMOTE_ID));
        let path = state.open_project_path().unwrap();

        let result = state.delete_project(path.clone());
        let released = release_requests(&seen);
        println!(
            "inherited lock, no session claim: delete success={} file gone={}; release \
             requests after the delete: {} {released:?}",
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
        assert!(
            released.is_empty(),
            "the delete freed a lock this session did not take: {released:?}"
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
