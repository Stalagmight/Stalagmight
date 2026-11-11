//! The edit gate is the lock claim (data#3): pins that no path can leave a
//! SpeleoDB project editable without a claim of the signed-in account bound to
//! the current session, and that every way out of an edit session leaves the gate
//! shut once no such claim remains. Synthetic throughout: a dead loopback
//! endpoint (or a recording one) and databases built here.

use std::path::{Path, PathBuf};

use super::speleodb_edit_gate::OpenProjectGate;
use super::AppState;
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;

const ID: &str = "33333333-3333-3333-3333-333333333333";
const ME: &str = "diver@example.com";
/// Nothing listens here: token resolution succeeds locally, every request fails
/// fast, so a holder-checked release keeps its claim (it cannot read the holder).
const UNREACHABLE: &str = "http://127.0.0.1:1/";

fn seed_linked(path: &Path, endpoint: &str, name: &str) {
    let mut db = Database::open(path).expect("open db");
    db.save_project(&Project::new(name.to_owned(), None))
        .expect("save project");
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: endpoint.to_owned(),
        project_id: ID.to_owned(),
        project_name: name.to_owned(),
        base_commit_sha: Some("abc123".to_owned()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }))
    .expect("save link");
}

fn sign_in(state: &AppState, endpoint: &str) {
    let mut session = state.speleodb_session_lock().expect("session");
    session.endpoint = Some(endpoint.to_owned());
    session.email = Some(ME.to_owned());
    session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token("tok");
    session.rehydrate_attempted = true;
}

/// A linked project open from `dir`, signed in on `endpoint`, read-only.
fn open_read_only(dir: &Path, endpoint: &str) -> (AppState, PathBuf) {
    let path = dir.join("linked.stalx");
    seed_linked(&path, endpoint, "Linked");
    let state = AppState::new().expect("state");
    sign_in(&state, endpoint);
    assert!(state.open_project(path.clone()).success, "open");
    (state, path)
}

/// [`open_read_only`] plus Start Editing's commit (the probe found the mutex
/// free, so the claim is authored).
fn open_editing(dir: &Path, endpoint: &str) -> (AppState, PathBuf) {
    let (state, path) = open_read_only(dir, endpoint);
    let (generation, link) = state.speleodb_session_snapshot().expect("snapshot");
    state
        .speleodb_commit_editing(Some(generation), &link.project_id, true)
        .expect("commit");
    assert!(state.edit_enabled(), "fixture: an edit session");
    (state, path)
}

/// THE invariant, checked from the outside: a SpeleoDB project's gate is open
/// only while the signed-in account has a claim on that project bound to the
/// current session generation.
fn assert_gate_needs_a_bound_claim(state: &AppState, label: &str) {
    let OpenProjectGate::Managed { project_id } = state.speleodb_open_gate() else {
        assert!(
            state.edit_enabled(),
            "{label}: an unmanaged project is editable"
        );
        return;
    };
    let generation = state.speleodb_session_generation();
    let session = state.speleodb_session_lock().expect("session");
    let bound = session.locks_acquired_here.iter().any(|claim| {
        claim.project_id == project_id
            && claim.editing() == Some(generation)
            && claim.taken_by(session.endpoint.as_deref(), session.email.as_deref())
    });
    drop(session);
    assert_eq!(
        state.edit_enabled(),
        bound,
        "{label}: the gate is open exactly when a claim is bound to this session"
    );
}

/// The open project's bytes and generation, for a prepared in-place reload.
fn reload_of(state: &AppState, dir: &Path, name: &str) -> (PathBuf, PathBuf, (u64, Vec<u8>)) {
    let dest = state.open_project_path().expect("open path");
    let source = dir.join(format!("{name}.download"));
    seed_linked(&source, UNREACHABLE, name);
    let expected = (
        state.speleodb_session_generation(),
        serde_json::to_vec(&*state.project.lock().expect("project")).expect("snapshot"),
    );
    (source, dest, expected)
}

#[test]
fn a_linked_project_opens_read_only_and_only_a_bound_claim_opens_it() {
    let temp = tempfile::TempDir::new().expect("temp");
    let (state, _path) = open_read_only(temp.path(), UNREACHABLE);
    assert!(!state.edit_enabled(), "a linked open is read-only");
    assert_gate_needs_a_bound_claim(&state, "open");

    // A claim on its own - one left over from an earlier session, or the
    // provisional pointer of an ambiguous acquire - opens nothing.
    state.speleodb_record_unbound_claim(ID);
    assert!(state.speleodb_holds_claim(ID));
    assert!(
        !state.edit_enabled(),
        "an unbound claim is a pointer, not a session"
    );
    assert_gate_needs_a_bound_claim(&state, "unbound claim");

    // Start Editing's commit, inherited and authored alike, binds one.
    for authored in [false, true] {
        let (generation, link) = state.speleodb_session_snapshot().expect("snapshot");
        state
            .speleodb_commit_editing(Some(generation), &link.project_id, authored)
            .expect("commit");
        assert!(state.edit_enabled(), "authored={authored}: editing");
        assert_gate_needs_a_bound_claim(&state, "commit");
        assert_eq!(
            state.speleodb_lock_acquired_here(ID),
            authored,
            "only a lock this process took licenses authorship"
        );
        state.speleodb_forget_lock_acquired_for(ID);
    }
}

#[test]
fn every_way_out_of_an_edit_session_leaves_the_gate_shut() {
    type WayOut = fn(&AppState);
    let ways_out: [(&str, WayOut); 9] = [
        ("the claim ends (a release ended it)", |state| {
            state.speleodb_forget_lock_acquired_for(ID);
        }),
        (
            "lost or unverifiable ownership",
            AppState::speleodb_shut_edit_gate,
        ),
        ("logout", |state| {
            crate::test_helpers::with_isolated_config(|| {
                let _ = state.speleodb_logout();
            });
        }),
        ("account switch", |state| {
            let _ = state.speleodb_logout_drop_gates_deferring_release();
        }),
        ("a 401 drops the token", |state| {
            crate::test_helpers::with_isolated_config(|| state.speleodb_forget());
        }),
        ("sign in as somebody else", |state| {
            {
                let _project = state.project.lock().expect("project");
                state.bump_speleodb_session();
            }
            let mut session = state.speleodb_session_lock().expect("session");
            session.email = Some("someone.else@example.com".to_owned());
        }),
        ("close", |state| {
            assert!(state.close_project().success);
        }),
        ("lifecycle release on close", |state| {
            let _ = state.speleodb_release_held_lock_on_close();
        }),
        ("reopen the same file", |state| {
            let path = state.open_project_path().expect("path");
            assert!(state.open_project(path).success);
        }),
    ];
    for (label, way_out) in ways_out {
        let temp = tempfile::TempDir::new().expect("temp");
        let (state, _path) = open_editing(temp.path(), UNREACHABLE);
        way_out(&state);
        if state.get_project_state().is_some() {
            assert!(!state.edit_enabled(), "{label}: the gate is shut");
        }
        assert_gate_needs_a_bound_claim(&state, label);
    }
}

#[test]
fn a_reload_carries_an_edit_session_only_while_its_claim_is_bound() {
    // data#3, structurally: the in-place reload behind a fast-forward, Reload,
    // Restore and Take remote may CARRY a session into the generation it bumps to,
    // never start one. Take remote used to release the lock and then reload with
    // "editing" copied across - and the project came back editable with no lock.
    fn released(state: &AppState) {
        state.speleodb_forget_lock_acquired_for(ID);
    }
    type EndTheClaim = Option<fn(&AppState)>;
    let cases: [(&str, EndTheClaim, bool); 3] = [
        ("an edit session", None, true),
        ("after its lock was released", Some(released), false),
        (
            "after its gate was shut",
            Some(AppState::speleodb_shut_edit_gate),
            false,
        ),
    ];
    for (label, end_the_claim, carried) in cases {
        let temp = tempfile::TempDir::new().expect("temp");
        let (state, _path) = open_editing(temp.path(), UNREACHABLE);
        if let Some(end_the_claim) = end_the_claim {
            end_the_claim(&state);
        }
        let (source, dest, expected) = reload_of(&state, temp.path(), "Server version");
        let before = expected.0;
        // `true`: the caller asks to keep editing, as Take remote's reload did.
        assert!(
            state
                .open_project_download(source, dest, expected, true)
                .expect("install")
                .success,
            "{label}: the reload itself succeeds"
        );
        assert_ne!(state.speleodb_session_generation(), before, "a boundary");
        assert_eq!(
            state.get_project_state().expect("open").name,
            "Server version"
        );
        assert_eq!(state.edit_enabled(), carried, "{label}");
        assert_gate_needs_a_bound_claim(&state, label);
    }
}

#[test]
fn a_view_reload_never_keeps_the_gate_open() {
    // A revision View reloads with `editing: false`: whatever session was open
    // before, the view is read-only - and the claim stays as the lock's pointer.
    let temp = tempfile::TempDir::new().expect("temp");
    let (state, _path) = open_editing(temp.path(), UNREACHABLE);
    let (source, dest, expected) = reload_of(&state, temp.path(), "Revision");
    assert!(
        state
            .open_project_download(source, dest, expected, false)
            .expect("install")
            .success
    );
    assert!(!state.edit_enabled());
    assert!(
        state.speleodb_holds_claim(ID),
        "the lock's pointer survives"
    );
    assert_gate_needs_a_bound_claim(&state, "view");
}

#[test]
fn a_restored_claim_after_a_failed_release_opens_nothing() {
    // Close's release cannot read the holder (nothing listens), so it sends
    // nothing and puts the claim back for a later retry - as a pointer: a release
    // that failed must not reopen the gate it was shutting.
    let temp = tempfile::TempDir::new().expect("temp");
    let (state, _path) = open_editing(temp.path(), UNREACHABLE);
    assert!(
        state.speleodb_release_held_lock_on_close().is_err(),
        "an unreadable holder is reported, not folded into success"
    );
    assert!(state.speleodb_holds_claim(ID), "kept for a retry");
    assert!(!state.edit_enabled(), "but not as an edit session");
    assert_gate_needs_a_bound_claim(&state, "restored claim");
}

#[test]
fn a_local_project_is_always_editable_whatever_the_claims() {
    let temp = tempfile::TempDir::new().expect("temp");
    let path = temp.path().join("local.stalx");
    Database::open(&path)
        .expect("open")
        .save_project(&Project::new("Local".to_owned(), None))
        .expect("save");
    let state = AppState::new().expect("state");
    sign_in(&state, UNREACHABLE);
    assert!(state.open_project(path).success);
    state.speleodb_record_unbound_claim(ID);
    state.speleodb_shut_edit_gate();
    assert!(
        state.edit_enabled(),
        "nothing about a lock gates a local project"
    );
    assert_gate_needs_a_bound_claim(&state, "local");
}

#[test]
fn unlinking_turns_an_edit_session_into_an_editable_local_project() {
    let temp = tempfile::TempDir::new().expect("temp");
    let (state, _path) = open_editing(temp.path(), UNREACHABLE);
    state.speleodb_unlink().expect("unlink");
    assert_eq!(state.speleodb_open_gate(), OpenProjectGate::Unmanaged);
    assert!(state.edit_enabled());
    assert!(
        state.speleodb_holds_claim(ID),
        "the release could not read the holder, so the claim is kept for a retry"
    );
}

#[test]
fn the_gate_is_opened_only_through_the_claim_binding() {
    // Source tripwire over the whole state layer: no production code anywhere
    // can open the gate except by binding a claim, and the binding is reached
    // only from Start Editing's commit and Switch to Read Only's rollback.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/state");
    let mut binds = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("state dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_rust = entry.path().extension().is_some_and(|ext| ext == "rs");
        if !is_rust || name.ends_with("_tests.rs") || name == "tests.rs" {
            continue;
        }
        let src = std::fs::read_to_string(entry.path()).expect("source");
        // Production region only: the top-level test gate starts the fixtures.
        let prod = src.split("\n#[cfg(test)]").next().unwrap_or(&src);
        assert!(
            !prod.contains("set_edit_enabled"),
            "{name}: there is no production gate setter"
        );
        if name != "speleodb_edit_gate.rs" {
            assert!(
                !prod.contains("bind_editing(Some"),
                "{name}: only the edit gate module binds a claim"
            );
        }
        for _ in prod.matches("self.speleodb_bind_edit_claim(") {
            binds.push(name.clone());
        }
    }
    binds.sort();
    assert_eq!(
        binds,
        vec![
            "speleodb_admin.rs".to_owned(),
            "speleodb_session.rs".to_owned()
        ],
        "the gate opens from Start Editing's commit and Switch to Read Only's \
         rollback only"
    );
}
