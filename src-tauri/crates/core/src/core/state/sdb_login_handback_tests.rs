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
            .speleodb_login(
                Some(b_base.clone()),
                None,
                None,
                Some("tok-b".to_owned()),
                true,
            )
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

/// [#576 SDB-ACCT-D1, FZ-SDB-3-D1] The hand-back covers a lock the session ADOPTED:
/// the status poll reopened the gate over the account's lock (taken in another
/// session, on another machine or on the website), and the sign-in as another account
/// releases it on its server before forgetting the session, as it releases one taken
/// here. It used to skip it (the release asked for a claim the reopen never minted).
#[test]
fn signing_in_as_another_account_hands_an_adopted_lock_back_first() {
    crate::test_helpers::with_isolated_config(|| {
        let (a_base, _a_listener, a_seen) = routing_stub(server_a);
        let (b_base, _b_listener, b_seen) = routing_stub(server_b);
        let (_temp, state) = super::speleodb::remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(&a_base);
        let polled = state
            .speleodb_link_status()
            .map(|s| s.edit_enabled)
            .map_err(|e| e.message);
        assert_eq!(
            polled,
            Ok(true),
            "premise: the poll adopts the account's lock on A"
        );

        let result = state
            .speleodb_login(Some(b_base), None, None, Some("tok-b".to_owned()), true)
            .map_err(|e| e.message)
            .expect("the paste login to B succeeds");

        let a_requests = a_seen.lock().expect("A's request log").clone();
        let b_requests = b_seen.lock().expect("B's request log").clone();
        println!(
            "[SDB-ACCT-D1] adopted lock on A, sign-in to B: replaced_session {}; gate {}; \
             claims {:?}; A released: {}; B saw a release: {}",
            result.replaced_session,
            state.edit_enabled(),
            claims(&state),
            a_requests.contains(&release_line()),
            b_requests.iter().any(|r| r.contains("/release/")),
        );
        assert!(result.replaced_session, "the sign-in replaces the session");
        assert!(
            a_requests.contains(&release_line()),
            "the adopted lock is released on A before the session is forgotten: \
             {a_requests:?}"
        );
        assert!(
            !b_requests.iter().any(|r| r.contains("/release/")),
            "nothing is released on the new server: {b_requests:?}"
        );
        assert!(!state.edit_enabled(), "the edit gate is shut");
        assert!(claims(&state).is_empty(), "no claim is left");
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
            .speleodb_login(
                Some(a_base.clone()),
                None,
                None,
                Some("tok-1b".to_owned()),
                true,
            )
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
            .speleodb_login(
                Some(b_base.clone()),
                None,
                None,
                Some("tok-b".to_owned()),
                true,
            )
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

/// Server A, but whoami spells our account with another case.
fn server_a_other_case(line: &str) -> (u16, &'static str) {
    if is_whoami(line) {
        (200, r#"{"email":"U@x"}"#)
    } else {
        (200, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY)
    }
}

/// Re-sign in to A as `server`'s whoami names us, at `endpoint`, and report what
/// the hand-back decided: (replaced_session, gate, claims, releases A saw).
fn re_login_to_a(
    server: fn(&str) -> (u16, &'static str),
    spell_endpoint: fn(&str) -> String,
    tag: &str,
) -> (bool, bool, Vec<String>, Vec<String>) {
    let (a_base, _a_listener, a_seen) = routing_stub(server);
    let (_temp, state) = editing_fixture(&a_base);
    let result = state
        .speleodb_login(
            Some(spell_endpoint(&a_base)),
            None,
            None,
            Some("tok-1b".to_owned()),
            true,
        )
        .map_err(|e| e.message)
        .expect("the same-account paste login succeeds");
    let releases: Vec<String> = a_seen
        .lock()
        .expect("A's request log")
        .iter()
        .filter(|r| r.contains("/release/"))
        .cloned()
        .collect();
    let gate = state.edit_enabled();
    let claims_after = claims(&state);
    println!(
        "[RVF-c-B1] {tag}: replaced_session {}; gate {gate}; claims {claims_after:?}; \
         session {:?}; releases {releases:?}",
        result.replaced_session,
        session_of(&state)
    );
    (result.replaced_session, gate, claims_after, releases)
}

#[test]
fn a_same_account_re_login_with_the_email_in_another_case_keeps_the_edit_session() {
    // [#576 RVF-c-B1] The holder check (`lock_held_by_me`) and the claim identity
    // (`LockClaim::taken_by`) treat the email case-insensitively, so a re-login as
    // `U@x` over a lock `u@x` holds is the same account and replaces nothing.
    crate::test_helpers::with_isolated_config(|| {
        let (replaced, gate, claims_after, releases) =
            re_login_to_a(server_a_other_case, str::to_owned, "u@x -> U@x");
        assert!(!replaced, "same account, other case: not a replacement");
        assert!(
            releases.is_empty(),
            "no release of our own lock: {releases:?}"
        );
        assert!(gate, "the edit gate stays open");
        assert_eq!(claims_after, vec![REMOTE_ID.to_owned()], "the claim stays");
    });
}

#[test]
fn a_same_account_re_login_with_the_server_spelled_differently_keeps_the_edit_session() {
    // [#576 RVF-c-B1] The claim identity compares endpoints through
    // `normalize_endpoint`, so the same server typed with a trailing slash is the
    // same session and replaces nothing.
    crate::test_helpers::with_isolated_config(|| {
        let (replaced, gate, claims_after, releases) =
            re_login_to_a(server_a, |base| format!("{base}/"), "A -> A/");
        assert!(!replaced, "same server, trailing slash: not a replacement");
        assert!(
            releases.is_empty(),
            "no release of our own lock: {releases:?}"
        );
        assert!(gate, "the edit gate stays open");
        assert_eq!(claims_after, vec![REMOTE_ID.to_owned()], "the claim stays");
    });
}

/// Server A, but every lock release is answered 502 (fuzz seed 615006's shape, as
/// for the logout); every other read names our account on the lock.
fn server_a_release_fails(line: &str) -> (u16, &'static str) {
    if line.starts_with("POST ") && line.contains("/release/") {
        (502, r#"{"detail":"Bad gateway"}"#)
    } else {
        server_a(line)
    }
}

/// What a sign-in to B over a live editing session on A left.
struct SignInOverA {
    replaced_session: bool,
    /// The answer's locks left held, as (project id, project name).
    left_held: Vec<(String, String)>,
    /// The release requests server A saw.
    releases: Vec<String>,
    session: (Option<String>, Option<String>),
    gate: bool,
    b_base: String,
}

/// Sign in to B over a live editing session on A, A answered by `server`.
fn sign_in_to_b_over_a(server: fn(&str) -> (u16, &'static str), tag: &str) -> SignInOverA {
    let (a_base, _a_listener, a_seen) = routing_stub(server);
    let (b_base, _b_listener, _b_seen) = routing_stub(server_b);
    let (_temp, state) = editing_fixture(&a_base);
    let result = state
        .speleodb_login(
            Some(b_base.clone()),
            None,
            None,
            Some("tok-b".to_owned()),
            true,
        )
        .map_err(|e| e.message)
        .expect("the paste login to B succeeds");
    let out = SignInOverA {
        replaced_session: result.replaced_session,
        left_held: result
            .locks_left_held
            .iter()
            .map(|left| (left.project_id.clone(), left.project_name.clone()))
            .collect(),
        releases: a_seen
            .lock()
            .expect("A's request log")
            .iter()
            .filter(|r| r.contains("/release/"))
            .cloned()
            .collect(),
        session: session_of(&state),
        gate: state.edit_enabled(),
        b_base,
    };
    println!(
        "[NF-W72-1] {tag}: replaced_session {}; locks left held {:?}; A's releases \
         {:?}; session {:?}; gate {}",
        out.replaced_session, out.left_held, out.releases, out.session, out.gate
    );
    out
}

/// [#576 NF-W72-1] A sign-in that replaces a live session whose lock release the old
/// server refuses still signs in, and its answer names the project still locked, for
/// the frontend's warning (as logout does, Q-W61-3-D1). It used to be only logged.
#[test]
fn a_sign_in_whose_hand_back_release_fails_signs_in_and_names_the_lock_left_held() {
    crate::test_helpers::with_isolated_config(|| {
        let out = sign_in_to_b_over_a(server_a_release_fails, "A's release answered 502");
        assert!(
            out.replaced_session,
            "the sign-in replaces the live session"
        );
        assert!(!out.releases.is_empty(), "the hand-back tried the release");
        assert_eq!(
            out.left_held,
            vec![(REMOTE_ID.to_owned(), "Remote".to_owned())],
            "the answer names the project whose lock is still held"
        );
        assert_eq!(
            out.session,
            (Some(out.b_base.clone()), Some("v@y".to_owned())),
            "the sign-in still happened"
        );
        assert!(!out.gate, "and the old session's edit gate is shut");
    });
}

/// [#576 SIGNOUT-PROMPT-D1] The prompt before a sign-in asks only when that sign-in
/// replaces a live session holding a lock it would hand back: a pasted token for
/// another account (resolved through B's whoami) names A's lock; the same account
/// re-entered on A, or a sign-in with nobody signed in, names nothing.
#[test]
fn the_sign_in_prompt_names_the_old_sessions_lock_only_when_the_sign_in_replaces_it() {
    crate::test_helpers::with_isolated_config(|| {
        let (a_base, _a_listener, a_seen) = routing_stub(server_a);
        let (b_base, _b_listener, _b_seen) = routing_stub(server_b);
        let (_temp, state) = editing_fixture(&a_base);
        // (releasable, unpushed), as (id, name) pairs.
        let named = |locks: crate::core::SpeleoSignOutLocks| {
            let pairs = |list: Vec<crate::core::SpeleoLockLeftHeld>| -> Vec<(String, String)> {
                list.into_iter()
                    .map(|left| (left.project_id, left.project_name))
                    .collect()
            };
            (pairs(locks.releasable), pairs(locks.unpushed))
        };
        let none = (Vec::new(), Vec::new());
        let to_b = state
            .speleodb_sign_in_hand_back_locks(Some(b_base.clone()), None, Some("tok-b".to_owned()))
            .map(named)
            .map_err(|e| e.message);
        let same_account = state
            .speleodb_sign_in_hand_back_locks(Some(a_base.clone()), Some("U@x".to_owned()), None)
            .map(named)
            .map_err(|e| e.message);
        let (_temp_out, signed_out) = editing_fixture(&a_base);
        signed_out
            .speleodb
            .lock()
            .forget_reporting()
            .expect("forget the seeded in-memory session");
        let nobody = signed_out
            .speleodb_sign_in_hand_back_locks(Some(b_base.clone()), None, Some("tok-b".to_owned()))
            .map(named)
            .map_err(|e| e.message);
        let a_requests = a_seen.lock().expect("A's request log").clone();
        println!(
            "[SIGNOUT-PROMPT-D1] sign-in locks: to B {to_b:?}; same account on A \
             {same_account:?}; signed out {nobody:?}; A saw {a_requests:?}"
        );
        assert_eq!(
            to_b,
            Ok((
                vec![(REMOTE_ID.to_owned(), "Remote".to_owned())],
                Vec::new()
            )),
            "a sign-in as another account names the old session's lock"
        );
        assert_eq!(
            same_account,
            Ok(none.clone()),
            "the same account replaces nothing"
        );
        assert_eq!(nobody, Ok(none), "signed out, nothing is replaced");
        assert!(
            !a_requests.iter().any(|r| r.contains("/release/")),
            "asking releases nothing: {a_requests:?}"
        );
        assert!(state.edit_enabled(), "and leaves the edit session alone");
        assert_eq!(
            session_of(&state).1,
            Some("u@x".to_owned()),
            "and the session too"
        );
    });
}

/// [#576 SIGNOUT-PROMPT-D1] "Keep locks and sign out" for the session a sign-in
/// replaces: the sign-in happens, no release goes to the old server, the old gate
/// is shut, and the lock is marked kept on purpose.
#[test]
fn a_sign_in_that_keeps_the_old_sessions_lock_sends_no_release() {
    crate::test_helpers::with_isolated_config(|| {
        let (a_base, _a_listener, a_seen) = routing_stub(server_a);
        let (b_base, _b_listener, b_seen) = routing_stub(server_b);
        let (_temp, state) = editing_fixture(&a_base);
        let result = state
            .speleodb_login(
                Some(b_base.clone()),
                None,
                None,
                Some("tok-b".to_owned()),
                false,
            )
            .map_err(|e| e.message)
            .expect("the paste login to B succeeds");
        let a_requests = a_seen.lock().expect("A's request log").clone();
        let b_requests = b_seen.lock().expect("B's request log").clone();
        let kept = state
            .speleodb
            .lock()
            .locks_acquired_here
            .iter()
            .any(|claim| claim.project_id == REMOTE_ID && claim.is_kept());
        println!(
            "[SIGNOUT-PROMPT-D1] switch A -> B keeping the lock: replaced_session {}; \
             locks left held {:?}; gate {}; claim kept {kept}; session {:?}; A saw \
             {a_requests:?}",
            result.replaced_session,
            result.locks_left_held,
            state.edit_enabled(),
            session_of(&state)
        );
        assert!(
            result.replaced_session,
            "the sign-in replaces the live session"
        );
        assert!(
            !a_requests.iter().any(|r| r.contains("/release/"))
                && !b_requests.iter().any(|r| r.contains("/release/")),
            "no release goes out: A {a_requests:?}, B {b_requests:?}"
        );
        assert!(
            result.locks_left_held.is_empty(),
            "nothing failed, nothing named"
        );
        assert!(!state.edit_enabled(), "the old session's edit gate is shut");
        assert!(kept, "the lock is kept on purpose");
        assert_eq!(
            session_of(&state),
            (Some(b_base), Some("v@y".to_owned())),
            "the session is now B's"
        );
    });
}

/// Control: a hand-back whose release lands names no lock.
#[test]
fn a_sign_in_whose_hand_back_release_lands_names_no_lock() {
    crate::test_helpers::with_isolated_config(|| {
        let out = sign_in_to_b_over_a(server_a, "A's release answered 200");
        assert!(
            out.replaced_session,
            "the sign-in replaces the live session"
        );
        assert_eq!(
            out.releases,
            vec![release_line()],
            "one release, answered 200"
        );
        assert!(
            out.left_held.is_empty(),
            "no lock is named: {:?}",
            out.left_held
        );
        assert_eq!(
            out.session,
            (Some(out.b_base.clone()), Some("v@y".to_owned()))
        );
    });
}
