//! #191 read-only edit gate and dirty bit; Save a Copy of a linked project.
//!
//! Lines 23768-24770 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// ===========================================================================
// #191 - read-only edit gate + dirty bit + chokepoint enumeration (Track B).
// Stamped-remote fixture: a temp .stalx carrying a speleo_link row, opened so
// edit_enabled flips false (remote-open-without-lock). See guard_mutation /
// mark_remote_dirty / is_lifecycle_passthrough.
// ===========================================================================

#[test]
fn unwound_pushes_require_an_explicit_backend_acknowledgement() {
    let (state, _sid, _shid, _temp) = remote_fixture_editable();
    state.mark_remote_dirty();
    state.speleodb_set_unwound_from(Some("abc123"));

    let refused = state
        .speleodb_push_only_acknowledged(None, false)
        .expect_err("unacknowledged older-version push must fail closed");
    assert!(
        refused.message.contains("older unwound revision"),
        "unexpected refusal: {refused:?}"
    );

    let acknowledged = state
        .speleodb_push_only_acknowledged(None, true)
        .expect_err("fixture is deliberately signed out");
    assert!(
        !acknowledged.message.contains("older unwound revision"),
        "the explicit acknowledgement must pass the older-version guard"
    );
}

#[test]
fn edit_enabled_defaults_true_and_local_project_passes() {
    // A fresh local-only project (no speleo_link) is always editable; the gate is
    // a no-op off-SpeleoDB.
    let state = AppState::new().unwrap();
    assert!(state.edit_enabled(), "default-true");
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Local".to_string(),
                location: None,
            })
            .success
    );
    assert!(state.edit_enabled(), "create keeps edit enabled");
    let result = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#abcdef".to_string(),
        direction: None,
        shots: vec![shot(1.0)],
    });
    assert!(result.success, "local project edits succeed");
}

#[test]
fn remote_open_without_lock_disables_edit() {
    let (state, _sid, _shid, _temp) = remote_fixture();
    assert!(!state.edit_enabled(), "remote project opens read-only");
    // Closing resets the gate so the next open isn't stuck.
    assert!(state.close_project().success);
    assert!(state.edit_enabled(), "close resets the gate");
}

#[test]
fn logout_closes_edit_gate_on_remote_edit_session() {
    // #197: logging out mid-edit on a remote project must drop the edit gate (so a
    // token-less session can't keep mutating a project it can never push) - the
    // lock release is best-effort and needs no token in this offline fixture.
    //
    // Under `with_isolated_config` because logout WRITES the config (it clears the
    // persisted account email, the rehydrate seed) and now REPORTS a failure to do
    // so, so the write needs a config file of its own to land in.
    crate::test_helpers::with_isolated_config(|| {
        let (state, _sid, _shid, _temp) = remote_fixture_editable();
        assert!(
            state.edit_enabled(),
            "precondition: remote edit session, gate on"
        );
        state.speleodb_logout().expect("logout");
        assert!(
            !state.edit_enabled(),
            "logout must drop the edit gate on a remote edit session"
        );
    });
}

#[test]
fn logout_leaves_local_project_editable() {
    // Logout must NOT touch a local-only project's gate (no remote link → no lock).
    //
    // Isolated for the same reason as `logout_closes_edit_gate_on_remote_edit_session`:
    // logout writes the config to clear the rehydrate seed and now reports a failure
    // to do so, so it needs a config file of its own.
    crate::test_helpers::with_isolated_config(|| {
        let state = state_with_project("Local");
        assert!(state.edit_enabled(), "local project editable");
        state.speleodb_logout().expect("logout");
        assert!(
            state.edit_enabled(),
            "logout must not disable editing on a local-only project"
        );
    });
}

#[test]
fn done_without_edit_session_is_noop_not_a_release() {
    // #197 review: Done on a remote project opened READ-ONLY (never Started editing →
    // edit_enabled false, no lock held this session) must NOT POST /release/ for a
    // mutex it never acquired. It short-circuits with no server call - the offline
    // fixture has no live endpoint, so reaching active_token/release would error; the
    // `Ok` proves the guard fired before any network work.
    //
    // UPDATED: the arm was `Released`. That conflated "your edits are on the server,
    // the lock is gone" with "there was no edit session at all", and the two diverge
    // whenever the gate is closed over a project that is still dirty (a 401 auto-drop
    // closes the gate and deliberately keeps the dirty bit). The frontend clears its
    // `hasUnpushedEdits` on `Released`, which disarmed the quit guard over work
    // nothing had uploaded - so the no-session case now has its own arm.
    let (state, _sid, _shid, _temp) = remote_fixture();
    assert!(
        !state.edit_enabled(),
        "precondition: remote read-only, no lock held"
    );
    let done = state
        .speleodb_done(None)
        .expect("non-dirty no-session Done is a no-op, not an error");
    assert!(
        matches!(done, crate::speleodb::models::DoneResult::NotEditing),
        "Done without an edit session reports NotEditing, not a release, got {done:?}"
    );
    assert!(
        !state.edit_enabled(),
        "still read-only after the no-op Done"
    );
}

#[test]
fn gate_rejects_every_mutation_entrypoint_with_edit_disabled_code() {
    // Tripwire doc: when a new state-mutating entry is added, route it through
    // guard_mutation AND add its name to MUTATION_ENTRYPOINTS + a call in
    // probe_mutation_entrypoints.
    assert_eq!(
        MUTATION_ENTRYPOINTS.len(),
        25,
        "update both the routing and this list when adding a mutation entry"
    );

    let (state, sid, shid, _temp) = remote_fixture();
    let outcomes = probe_mutation_entrypoints(&state, sid, shid);
    let probed: Vec<&str> = outcomes.iter().map(|(label, _)| *label).collect();
    assert_eq!(
        probed, MUTATION_ENTRYPOINTS,
        "probe_mutation_entrypoints must call exactly the listed entry points, in order"
    );
    for (label, outcome) in &outcomes {
        // Repair included: since #576 D39(a) its refusal is the coded error itself,
        // not a zero result that read as a clean repair.
        let error = outcome
            .as_ref()
            .err()
            .unwrap_or_else(|| panic!("{label} must be rejected when edit disabled"));
        assert!(
            error.code == CommandErrorCode::EditDisabled
                && !error.message.contains("EDIT_DISABLED"),
            "{label} must carry the EDIT_DISABLED code (and no marker), got: {error:?}"
        );
    }
    // The gate fires BEFORE any DB write, so the dirty bit must NOT have flipped.
    assert!(
        !dirty_bit(&state),
        "a rejected mutation must not dirty the project"
    );
}

#[test]
fn lifecycle_commands_pass_through_gate() {
    // Save / Close must NOT be blocked on a read-only remote project (autosave
    // must keep working). Open/Create/MergeProjects are lifecycle too.
    let (state, _sid, _shid, _temp) = remote_fixture();
    assert!(!state.edit_enabled());

    // Autosave (SaveProject) passes the gate.
    let save = state.execute(Command::SaveProject { path: None });
    assert!(save.success, "autosave must pass through the gate");

    // The dirty bit is NOT set by a local save.
    assert!(!dirty_bit(&state), "save/autosave never sets the dirty bit");

    // Close passes through (and resets the gate).
    assert!(state.close_project().success);
}

#[test]
fn warning_resolve_unresolve_in_edit_session_dirty_for_push() {
    // #197 data-safety: resolved_warnings PROLIFERATES to the server (it is NOT in
    // PUSH_SNAPSHOT_LOCAL_TABLES), so a resolve/unresolve made during an edit session
    // MUST mark the project dirty - otherwise Done (gated on the dirty bit) skips the
    // push and the change is silently dropped on the next pull. This guards the
    // `mark_remote_dirty()` calls in the gated wrappers against a future drop. (The
    // read-only REJECTION path is covered by
    // gate_rejects_every_mutation_entrypoint_with_edit_disabled_code.)
    let make_warning = |sid, shid| crate::persistence::ResolvedWarning {
        section_id: sid,
        shot_id: shid,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    };

    // resolve in an edit session → dirty
    {
        let (state, sid, shid, _temp) = remote_fixture_editable();
        assert!(!dirty_bit(&state), "fixture starts clean");
        state
            .save_resolved_warning_gated(&make_warning(sid, shid))
            .expect("resolve allowed in an edit session");
        assert!(
            dirty_bit(&state),
            "an edit-session resolve must dirty the project so Done pushes it"
        );
    }

    // unresolve in an edit session → dirty; and the low-level accessor stays UNGATED
    {
        let (state, sid, shid, _temp) = remote_fixture_editable();
        // Seed via the low-level accessor - it must NOT dirty (only the gated wrapper
        // does), which also pins that the backfill/test path is exempt by design.
        state
            .save_resolved_warning(&make_warning(sid, shid))
            .expect("seed a resolved row");
        assert!(
            !dirty_bit(&state),
            "the ungated low-level save_resolved_warning must not dirty the project"
        );
        state
            .delete_resolved_warning_gated(sid, shid, "SHORT_SHOT", None)
            .expect("unresolve allowed in an edit session");
        assert!(
            dirty_bit(&state),
            "an edit-session unresolve must dirty the project so Done pushes it"
        );
    }
}

#[test]
fn dirty_bit_flips_false_to_true_on_successful_remote_edit() {
    let (state, sid, shid, _temp) = remote_fixture_editable();
    assert!(!dirty_bit(&state), "starts clean");

    let r = state.execute(Command::UpdateShot {
        section_id: sid,
        shot_id: shid,
        updates: ShotUpdates {
            length: Some(9.0),
            ..Default::default()
        },
    });
    assert!(r.success);
    assert!(
        dirty_bit(&state),
        "successful remote edit flips the dirty bit"
    );
}

#[test]
fn dirty_bit_is_idempotent_and_set_only() {
    let (state, sid, shid, _temp) = remote_fixture_editable();

    // First edit sets it.
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(dirty_bit(&state));

    // A second edit + a local autosave both leave it set (idempotent; set-only -
    // a local save NEVER clears it).
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(8.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(dirty_bit(&state), "dirty bit stays set after autosave");
}

#[test]
fn dirty_bit_noop_for_local_only_project() {
    // A local-only project has no speleo_link → mark_remote_dirty is a no-op and
    // never errors.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("local.stalx");
    let mut project = Project::new("Local".to_string(), None);
    let sid = project
        .add_section(SectionData {
            shots: vec![shot(2.0)],
            ..Default::default()
        })
        .id;
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);

    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success);
    assert!(state.edit_enabled(), "local project stays editable");

    let shid = state.get_project_state().unwrap().sections[0].shots[0].id;
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(5.0),
                    ..Default::default()
                },
            })
            .success
    );
    // No speleo_link row exists; mark_remote_dirty was a silent no-op.
    let guard = state.database.lock();
    assert!(guard
        .as_ref()
        .unwrap()
        .load_speleo_link()
        .unwrap()
        .is_none());
}

// ===========================================================================
// Save a Copy: an explicit destination on a LINKED project writes a detached
// unlinked copy and leaves the session on the managed file (see save_project).
// ===========================================================================

#[test]
fn save_as_on_a_linked_project_writes_an_unlinked_copy_and_stays_put() {
    let (state, _sid, _shid, temp) = remote_fixture();
    let open_path = state.open_project_path().unwrap();
    let copy_path = temp.path().join("copy.stalx");

    let result = state.save_project(Some(copy_path.clone()));

    assert!(result.success, "copy save succeeds: {:?}", result.error);
    assert_eq!(
        result.unlinked_copy_path.as_deref(),
        Some(copy_path.display().to_string().as_str()),
        "the copy is reported via unlinked_copy_path"
    );
    assert!(
        result.saved_path.is_none(),
        "saved_path must stay None - the frontend re-points file_path on it"
    );
    assert_eq!(
        state.open_project_path().unwrap(),
        open_path,
        "the session stays on the managed file"
    );
    assert!(
        state.speleodb_open_link().is_ok(),
        "the managed file keeps its link"
    );

    let copy_db = Database::open_for_metadata(&copy_path).unwrap();
    assert!(
        copy_db.load_speleo_link().unwrap().is_none(),
        "the copy carries NO speleo_link row"
    );
    drop(copy_db);
    let copy_db = Database::open_existing(&copy_path).unwrap();
    let copied = copy_db.load_project().unwrap();
    assert_eq!(copied.sections.len(), 1, "the survey data is in the copy");
    assert_eq!(copied.sections[0].shots.len(), 2);
}

#[test]
fn save_a_copy_onto_the_managed_file_itself_is_refused() {
    let (state, _sid, _shid, _temp) = remote_fixture();
    let open_path = state.open_project_path().unwrap();

    let result = state.save_project(Some(open_path.clone()));

    assert!(
        !result.success,
        "writing an unlinked copy over the managed file would strip its link in place"
    );
    let db = Database::open_for_metadata(&open_path).unwrap();
    assert!(
        db.load_speleo_link().unwrap().is_some(),
        "the refusal must not have touched the file"
    );
}

#[test]
fn implicit_save_on_a_linked_project_still_carries_the_link_and_the_dirty_bit() {
    // Pins speleodb_upload's pre-push flush (`save_project(None)`): an implicit
    // save is NOT a copy save - it must keep the link and the dirty bit.
    let (state, sid, shid, _temp) = remote_fixture_editable();
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(dirty_bit(&state), "the edit set the dirty bit");

    let result = state.save_project(None);

    assert!(result.success);
    assert!(
        result.saved_path.is_some(),
        "an implicit save reports saved_path"
    );
    assert!(
        result.unlinked_copy_path.is_none(),
        "an implicit save is never a copy save"
    );
    assert!(
        state.speleodb_open_link().is_ok(),
        "the link survived the save"
    );
    assert!(
        dirty_bit(&state),
        "a local save never clears the dirty bit - only a push does"
    );
}

/// A linked, edited (dirty) session whose `speleo_link` row is still on disk but no
/// longer reads: the column the reader needs was renamed, an error confined to the
/// link table. Returns `(state, open_path, _temp)`.
fn linked_session_with_an_unreadable_link() -> (AppState, PathBuf, TempDir) {
    let (state, sid, shid, temp) = remote_fixture_editable();
    let open_path = state.open_project_path().unwrap();
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(dirty_bit(&state), "the edit set the dirty bit");
    rusqlite::Connection::open(&open_path)
        .unwrap()
        .execute_batch("ALTER TABLE speleo_link RENAME COLUMN project_name TO project_title;")
        .unwrap();
    assert!(Database::open_for_metadata(&open_path)
        .unwrap()
        .load_speleo_link()
        .is_err());
    (state, open_path, temp)
}

/// [#576 B35(b)] A link that cannot be read must not be "no link". The save
/// rebuilds the managed file from a fresh temp, so treating the read error as an
/// unlinked project wrote the file without its `speleo_link` row: the project
/// silently became a plain local file, its base revision and dirty bit gone.
#[test]
fn an_unreadable_link_refuses_the_save_instead_of_unlinking_the_project() {
    let (state, open_path, _temp) = linked_session_with_an_unreadable_link();
    let on_disk = std::fs::read(&open_path).unwrap();

    let result = state.save_project(None);

    let link_now = Database::open_for_metadata(&open_path)
        .unwrap()
        .load_speleo_link()
        .map(|link| link.map(|l| (l.project_id, l.has_unpushed_edits)));
    eprintln!("save success: {} error: {:?}", result.success, result.error);
    eprintln!("speleo_link in the managed file after the save: {link_now:?}");
    assert!(!result.success, "the save must be refused");
    assert!(
        std::fs::read(&open_path).unwrap() == on_disk,
        "the refused save rewrote the managed file"
    );
    assert!(!open_path.with_extension("stalx.tmp").exists());
    assert_eq!(state.open_project_path().unwrap(), open_path);
}

/// [#576 B35(b)] The way out: a Save As to another file still works, and the
/// managed file is left as it was.
#[test]
fn an_unreadable_link_still_allows_save_as_to_another_file() {
    let (state, open_path, temp) = linked_session_with_an_unreadable_link();
    let on_disk = std::fs::read(&open_path).unwrap();
    let rescue = temp.path().join("rescue.stalx");

    let result = state.save_project(Some(rescue.clone()));

    assert!(result.success, "Save As failed: {:?}", result.error);
    assert!(
        std::fs::read(&open_path).unwrap() == on_disk,
        "the Save As rewrote the managed file"
    );
    let saved = Database::open_existing(&rescue)
        .unwrap()
        .load_project()
        .unwrap();
    assert!((saved.sections[0].shots[0].length - 9.0).abs() < 1e-9);
}

#[test]
fn save_as_on_an_unlinked_project_still_relocates_the_session() {
    // The pre-existing Save As contract for plain local projects, pinned so the
    // copy_only branch cannot leak into it.
    let temp = TempDir::new().unwrap();
    let state = state_with_project("Local");

    let dest = temp.path().join("relocated.stalx");
    let result = state.save_project(Some(dest.clone()));

    assert!(result.success);
    assert_eq!(
        result.saved_path.as_deref(),
        Some(dest.display().to_string().as_str()),
        "an unlinked Save As reports saved_path"
    );
    assert!(result.unlinked_copy_path.is_none());
    assert_eq!(
        state.open_project_path().unwrap(),
        dest,
        "an unlinked Save As relocates the session"
    );
}

#[test]
fn the_copy_carries_the_undo_history() {
    let (state, sid, shid, temp) = remote_fixture_editable();
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success
    );

    let copy_path = temp.path().join("copy.stalx");
    assert!(state.save_project(Some(copy_path.clone())).success);

    let copy_db = Database::open_existing(&copy_path).unwrap();
    assert!(
        !copy_db
            .load_undo_stack_with_timestamps()
            .unwrap()
            .is_empty(),
        "the copy carries the undo history (the detach flow reopens it and \
         must not lose the edit trail)"
    );
}

#[test]
fn edit_disabled_code_is_distinct() {
    // The EDIT_DISABLED code is distinct from the FS permission code, so the FE
    // can route on it independently (#204; a code since #576 C4).
    assert_eq!(CommandErrorCode::EditDisabled.as_str(), "EDIT_DISABLED");
    assert_ne!(
        CommandErrorCode::EditDisabled,
        CommandErrorCode::FsPermissionDenied
    );
    let (state, sid, shid, _temp) = remote_fixture();
    let refused = state.execute(Command::UpdateShot {
        section_id: sid,
        shot_id: shid,
        updates: ShotUpdates::default(),
    });
    assert_eq!(refused.code, Some(CommandErrorCode::EditDisabled));
    let err = refused.error.unwrap();
    assert!(
        !err.contains("EDIT_DISABLED") && !err.contains("FS_PERMISSION_DENIED"),
        "the message is display text with no marker: {err}"
    );
}

#[test]
fn repair_and_settings_gated_on_locked_remote() {
    let (state, _sid, _shid, _temp) = remote_fixture();
    // Repair: refused with the EDIT_DISABLED coded error (#576 D39(a)). It used to
    // return `{ fixed: 0, remaining: 0, changed: false }`, which the frontend
    // reported as "Repaired 0 issues. Save to keep the fix." for a repair that
    // never ran. The refusal fires before any mutation: no undo entry, no change.
    let modified_before = state.get_project_state().unwrap().modified_at;
    let undo_before = state.get_undo_stack().len();
    let repair = state.repair_open_project_integrity();
    println!("read-only remote project: repair_open_project_integrity() -> {repair:?}");
    let refusal = repair.expect_err("a refused repair must be an error, not a clean result");
    assert!(
        refusal.code == CommandErrorCode::EditDisabled
            && !refusal.message.contains("EDIT_DISABLED"),
        "the refusal carries the EDIT_DISABLED code, got: {refusal:?}"
    );
    assert_eq!(
        state.get_project_state().unwrap().modified_at,
        modified_before,
        "a refused repair leaves the project untouched"
    );
    assert_eq!(
        state.get_undo_stack().len(),
        undo_before,
        "and records no undo entry"
    );
    // Settings: returns the EDIT_DISABLED coded failure.
    let settings =
        state.update_project_settings("New Name".to_string(), state.get_config().validation, None);
    assert!(!settings.success);
    assert_eq!(settings.code, Some(CommandErrorCode::EditDisabled));
}
