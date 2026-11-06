//! [#576 FZ-SDB-3-D1] A sign-in that replaces a live session hands it back first.
//!
//! Signing in as another account (or on another server) used to forget the old
//! session with its write lock still held and its edit gate open: the project kept
//! editing under a lock the new credentials do not hold, and Done then went to the
//! new server. The login now runs logout's gate half for the session it replaces -
//! the held lock released best-effort with the OLD token, the edit and force-edit
//! gates shut, the session boundary bumped - before it forgets the old session.
//!
//! Driven against two loopback stubs that answer like SpeleoDB: server A, where the
//! open project's lock is held by our account (`u@x`), and server B, whose whoami
//! names another account. Three cases: a switch to B (hand-back), a same-account
//! re-login to A (no hand-back), and a sign-in with no session at all over a local
//! project (nothing to hand back). Under `with_isolated_config`, because a login
//! writes the account record to the config; every session here is backed by an
//! in-memory token store, so no real keychain is touched.
//!
//! All data is synthetic, built in code.

use super::speleodb::{
    authed_session_at, editing_fixture, routing_stub, FREE_MUTEX_AT_BASE_BODY,
    HELD_BY_SAME_ACCOUNT_AT_BASE_BODY, REMOTE_ID,
};
use super::take_remote_success_tests::project_file;
use super::AppState;

/// Whether `line` is the whoami read a pasted-token login makes.
fn is_whoami(line: &str) -> bool {
    line.starts_with("GET /api/v2/user/ ")
}

/// Server A: whoami is our own account, and every project read and the release
/// answer name our account on the open project's lock.
fn server_a(line: &str) -> (u16, &'static str) {
    if is_whoami(line) {
        (200, r#"{"email":"u@x"}"#)
    } else {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    }
}

/// Server B: whoami is ANOTHER account; anything else describes a free project.
fn server_b(line: &str) -> (u16, &'static str) {
    if is_whoami(line) {
        (200, r#"{"email":"v@y"}"#)
    } else {
        (200, FREE_MUTEX_AT_BASE_BODY)
    }
}

/// The release POST for the open project.
fn release_line() -> String {
    format!("POST /api/v2/projects/{REMOTE_ID}/release/ HTTP/1.1")
}

/// The project ids this process holds a claim on.
fn claims(state: &AppState) -> Vec<String> {
    state
        .speleodb
        .lock()
        .locks_acquired_here
        .iter()
        .map(|claim| claim.project_id.clone())
        .collect()
}

/// The session's (endpoint, email).
fn session_of(state: &AppState) -> (Option<String>, Option<String>) {
    let session = state.speleodb.lock();
    (session.endpoint.clone(), session.email.clone())
}

#[test]
fn signing_in_as_another_account_hands_the_live_session_back_first() {
    crate::test_helpers::with_isolated_config(|| {
        let (a_base, _a_listener, a_seen) = routing_stub(server_a);
        let (b_base, _b_listener, b_seen) = routing_stub(server_b);
        // A live session on A, editing the remote project under a lock this process
        // took, over a CLEAN copy (the silent release is not vetoed).
        let (_temp, state) = editing_fixture(&a_base);
        assert!(
            state.edit_enabled() && !state.force_editing(),
            "premise: editing under our own lock"
        );
        assert_eq!(
            claims(&state),
            vec![REMOTE_ID.to_owned()],
            "premise: claimed"
        );
        let generation_before = state.speleodb_session_generation();

        let result = state
            .speleodb_login(Some(b_base.clone()), None, None, Some("tok-b".to_owned()))
            .map_err(|e| e.message)
            .expect("the paste login to B succeeds");

        let a_requests = a_seen.lock().expect("A's request log").clone();
        let b_requests = b_seen.lock().expect("B's request log").clone();
        let gate = state.edit_enabled();
        let force = state.force_editing();
        let claims_after = claims(&state);
        let session = session_of(&state);
        let generation_after = state.speleodb_session_generation();
        println!(
            "[FZ-SDB-3] switch A -> B: replaced_session {}; user {}; gate {gate}; \
             force-edit {force}; claims {claims_after:?}; session {session:?}; \
             generation {generation_before} -> {generation_after}",
            result.replaced_session, result.user_email
        );
        println!("[FZ-SDB-3] server A saw {a_requests:?}");
        println!("[FZ-SDB-3] server B saw {b_requests:?}");

        assert!(
            result.replaced_session,
            "a sign-in as another account replaces the live session"
        );
        assert!(
            a_requests.contains(&release_line()),
            "the replaced session's lock is released on ITS server before it is \
             forgotten: {a_requests:?}"
        );
        assert!(
            !b_requests.iter().any(|r| r.contains("/release/")),
            "nothing is released on the new server: {b_requests:?}"
        );
        assert!(
            !gate,
            "the edit gate is shut: the new account does not hold the lock"
        );
        assert!(!force, "and force-edit is off");
        assert!(
            claims_after.is_empty(),
            "no claim on the old session's lock is left: {claims_after:?}"
        );
        assert!(
            generation_after > generation_before,
            "the session boundary is bumped"
        );
        assert_eq!(
            session,
            (Some(b_base), Some("v@y".to_owned())),
            "the session is now B's"
        );
    });
}

#[test]
fn a_same_account_re_login_keeps_the_edit_session() {
    // DO-NOT-REGRESS: re-entering the same account on the same server replaces
    // nothing. The lock, the gate and the claim are still this session's.
    crate::test_helpers::with_isolated_config(|| {
        let (a_base, _a_listener, a_seen) = routing_stub(server_a);
        let (_temp, state) = editing_fixture(&a_base);

        let result = state
            .speleodb_login(Some(a_base.clone()), None, None, Some("tok-1b".to_owned()))
            .map_err(|e| e.message)
            .expect("the same-account paste login succeeds");

        let a_requests = a_seen.lock().expect("A's request log").clone();
        let gate = state.edit_enabled();
        let claims_after = claims(&state);
        println!(
            "[FZ-SDB-3] re-login A -> A: replaced_session {}; gate {gate}; claims \
             {claims_after:?}; session {:?}; server A saw {a_requests:?}",
            result.replaced_session,
            session_of(&state)
        );

        assert!(
            !result.replaced_session,
            "the same account on the same server is not a replacement"
        );
        assert!(
            !a_requests.iter().any(|r| r.contains("/release/")),
            "no release goes out: {a_requests:?}"
        );
        assert!(gate, "the edit gate stays open");
        assert_eq!(
            claims_after,
            vec![REMOTE_ID.to_owned()],
            "and the claim is still there"
        );
    });
}

#[test]
fn a_sign_in_with_no_session_leaves_a_local_projects_gate_alone() {
    // DO-NOT-REGRESS: signed out, there is no session to hand back; an open LOCAL
    // project edits freely before and after.
    crate::test_helpers::with_isolated_config(|| {
        let temp = tempfile::TempDir::new().expect("temp");
        let path = temp.path().join("local.stalx");
        project_file(&path, "Local", "SectionA");
        let state = AppState::new().expect("state");
        assert!(state.open_project(path).success, "open the local project");
        let (b_base, _b_listener, b_seen) = routing_stub(server_b);
        // Signed out, over an in-memory store so the login lands nowhere real.
        let mut signed_out = authed_session_at(&b_base);
        signed_out
            .forget_reporting()
            .expect("forget the seeded in-memory session");
        *state.speleodb.lock() = signed_out;
        assert!(
            state.edit_enabled() && session_of(&state) == (None, None),
            "premise: signed out, a local project editing"
        );
        let generation_before = state.speleodb_session_generation();

        let result = state
            .speleodb_login(Some(b_base.clone()), None, None, Some("tok-b".to_owned()))
            .map_err(|e| e.message)
            .expect("the paste login succeeds");

        let b_requests = b_seen.lock().expect("B's request log").clone();
        let gate = state.edit_enabled();
        let generation_after = state.speleodb_session_generation();
        println!(
            "[FZ-SDB-3] signed out -> B over a local project: replaced_session {}; gate \
             {gate}; force-edit {}; generation {generation_before} -> {generation_after}; \
             session {:?}; server B saw {b_requests:?}",
            result.replaced_session,
            state.force_editing(),
            session_of(&state)
        );

        assert!(
            !result.replaced_session,
            "with no session there is nothing to replace"
        );
        assert!(gate, "the local project's edit gate is untouched");
        assert_eq!(
            generation_after, generation_before,
            "no session boundary: no hand-back ran"
        );
        assert!(
            !b_requests.iter().any(|r| r.contains("/release/")),
            "and nothing is released: {b_requests:?}"
        );
    });
}
