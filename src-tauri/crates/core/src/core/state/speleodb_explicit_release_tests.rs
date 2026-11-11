//! "Never release someone else's lock, even as an admin" (lock#4) for the verbs a
//! user aims at a lock on purpose: Done with nothing to upload, the per-project
//! Release, the stale-context unwind and Switch to Read Only. SpeleoDB lets an
//! administrator release anybody's mutex, so each of these asks who holds the lock
//! first: another account (or nobody) ends the local claim and shuts the gate with
//! nothing sent, an unreadable holder sends nothing and keeps the claim, and only
//! this account's own lock is released. Synthetic throughout.

use std::path::Path;
use std::sync::{Arc, Mutex};

use super::tests_support::{lock_server, lock_server_unreadable, release_posts};
use super::AppState;
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;
use crate::speleodb::DoneResult;

const ID: &str = "66666666-6666-6666-6666-666666666666";
/// The signed-in account: an administrator of the project, as far as the server
/// is concerned - which is exactly why the client may not rely on the server to
/// refuse.
const ADMIN: &str = "admin@example.com";
const OTHER: &str = "someone.else@example.com";

type Log = Arc<Mutex<Vec<String>>>;

/// An edit session on a clean linked project, signed in as [`ADMIN`] against
/// `server`, the lock taken by this process (an authored, bound claim).
fn editing_on(dir: &Path, server: &str) -> AppState {
    let path = dir.join("linked.stalx");
    let mut db = Database::open(&path).expect("open db");
    db.save_project(&Project::new("Linked".to_owned(), None))
        .expect("save project");
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: server.to_owned(),
        project_id: ID.to_owned(),
        project_name: "Linked".to_owned(),
        base_commit_sha: Some("abc123".to_owned()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }))
    .expect("save link");
    drop(db);
    let state = AppState::new().expect("state");
    {
        let mut session = state.speleodb_session_lock().expect("session");
        session.endpoint = Some(server.to_owned());
        session.email = Some(ADMIN.to_owned());
        session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token("tok");
        session.rehydrate_attempted = true;
    }
    assert!(state.open_project(path).success, "open");
    state.speleodb_begin_acquisition_flow();
    let (generation, link) = state.speleodb_session_snapshot().expect("snapshot");
    state
        .speleodb_commit_editing(Some(generation), &link.project_id, true)
        .expect("commit");
    assert!(state.edit_enabled(), "fixture: an edit session");
    state
}

/// The three servers each verb is pressed against: the lock now held by
/// somebody else, held by nobody, and a holder that cannot be read.
fn servers() -> [(&'static str, (String, Log)); 3] {
    [
        (
            "held by another account",
            lock_server(ID, Some(OTHER.to_owned())),
        ),
        ("held by nobody", lock_server(ID, None)),
        ("holder unreadable", lock_server_unreadable(ID)),
    ]
}

/// The outcome every verb must produce for `case`, given whether it reported an
/// error.
fn assert_never_blind(state: &AppState, case: &str, seen: &Log, failed: bool) {
    assert_eq!(release_posts(seen), 0, "{case}: nothing is released");
    if case == "holder unreadable" {
        assert!(failed, "{case}: the verb reports that it could not release");
        assert!(
            state.speleodb_holds_claim(ID),
            "{case}: the claim survives for a retry"
        );
    } else {
        assert!(!failed, "{case}: the session simply ends");
        assert!(!state.speleodb_holds_claim(ID), "{case}: the claim is over");
        assert!(!state.edit_enabled(), "{case}: and the gate is shut");
    }
}

#[test]
fn done_with_nothing_to_upload_never_frees_another_accounts_lock() {
    for (case, (server, seen)) in servers() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = editing_on(temp.path(), &server);
        let result = state.speleodb_done(None);
        if let Ok(done) = &result {
            assert!(
                matches!(done, DoneResult::Released { .. }),
                "{case}: the session ends as released: {done:?}"
            );
        }
        assert_never_blind(&state, case, &seen, result.is_err());
        if result.is_err() {
            assert!(
                state.edit_enabled(),
                "{case}: Done keeps editing until the release is settled"
            );
        }
    }
}

#[test]
fn the_per_project_release_never_frees_another_accounts_lock() {
    for (case, (server, seen)) in servers() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = editing_on(temp.path(), &server);
        let result = state.speleodb_release_lock(ID.to_owned());
        assert_never_blind(&state, case, &seen, result.is_err());
    }
}

#[test]
fn the_stale_context_unwind_never_frees_another_accounts_lock() {
    for (case, (server, seen)) in servers() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = editing_on(temp.path(), &server);
        let result = state.speleodb_release_lock_acquired_here();
        assert_never_blind(&state, case, &seen, result.is_err());
    }
}

#[test]
fn switching_to_read_only_never_frees_another_accounts_lock() {
    // Already holder-checked before this pass; pinned with the others.
    for (case, (server, seen)) in servers() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = editing_on(temp.path(), &server);
        let (generation, link) = state.speleodb_session_snapshot().expect("snapshot");
        let expected = crate::speleodb::SpeleoReleaseContext {
            session_generation: generation.to_string(),
            project_id: link.project_id,
            endpoint: server.clone(),
            user_email: ADMIN.to_owned(),
            project_modified_at: state
                .project
                .lock()
                .expect("project")
                .as_ref()
                .expect("open")
                .modified_at
                .to_rfc3339(),
        };
        let result = state.speleodb_release_current_lock(Some(expected));
        assert_eq!(release_posts(&seen), 0, "{case}: nothing is released");
        if case == "holder unreadable" {
            assert!(result.is_err(), "{case}");
            assert!(state.edit_enabled(), "{case}: nothing changed");
        } else {
            assert!(result.is_ok(), "{case}: {result:?}");
            assert!(!state.edit_enabled(), "{case}: read-only");
            assert!(!state.speleodb_holds_claim(ID), "{case}: the claim is over");
        }
    }
}

#[test]
fn every_explicit_verb_still_releases_this_accounts_own_lock() {
    type Verb = fn(&AppState) -> bool;
    let verbs: [(&str, Verb); 3] = [
        ("done", |state| state.speleodb_done(None).is_ok()),
        ("release", |state| {
            state.speleodb_release_lock(ID.to_owned()).is_ok()
        }),
        ("stale-context unwind", |state| {
            state.speleodb_release_lock_acquired_here().is_ok()
        }),
    ];
    for (name, verb) in verbs {
        let temp = tempfile::TempDir::new().expect("temp");
        let (server, seen) = lock_server(ID, Some(ADMIN.to_owned()));
        let state = editing_on(temp.path(), &server);
        assert!(verb(&state), "{name} succeeds");
        assert_eq!(release_posts(&seen), 1, "{name}: our own lock is released");
        assert!(
            !state.speleodb_holds_claim(ID),
            "{name}: and the claim is over"
        );
        assert!(!state.edit_enabled(), "{name}: read-only afterwards");
    }
}
