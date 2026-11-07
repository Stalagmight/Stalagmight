//! [#576 C5] The read-only Plan/Export workspace has a backend backstop.
//!
//! Before it, Plan and Export were read-only only because the UI hid and
//! disabled the editing controls: `guard_mutation` never knew the workspace mode,
//! so any gap in that gating became a real edit while the user was planning a
//! dive, with undo hidden. The frontend now reports its mode
//! (`set_workspace_mode`) and the chokepoint refuses every guarded entry point
//! while it is Plan or Export, except the two on the allowlist.
//!
//! The entry points come from the same hand-maintained list the SpeleoDB
//! read-only tripwire uses (`tests::MUTATION_ENTRYPOINTS`, driven by
//! `tests::probe_mutation_entrypoints`), so a new listed entry is covered here
//! too. All data is synthetic, built in memory.

use tempfile::TempDir;

use super::tests::{probe_mutation_entrypoints, MUTATION_ENTRYPOINTS};
use super::{AppState, WorkspaceMode};
use crate::core::{Command, CommandError, CommandErrorCode};
use crate::models::{Coordinates, Project, SectionData, SectionId, ShotId, StationRef};
use crate::persistence::Database;
use crate::test_fixtures::shot;

/// The entry points deliberately editable in Plan and Export: the project name
/// and validation thresholds, and the project rename (ProjectSettingsDialog keeps
/// them enabled in every mode). Everything else in `MUTATION_ENTRYPOINTS` is
/// refused there.
const WORKSPACE_ALLOWLIST: &[&str] = &["update_project_settings", "set_project_name_for_path"];

/// A LOCAL project (no SpeleoDB link, so the lock gate is open) saved to a temp
/// `.stalx` and opened, so it has the file path the by-path rename gates on.
/// Returns `(state, section_id, first_shot_id, _temp)` - keep `_temp` alive.
fn local_fixture() -> (AppState, SectionId, ShotId, TempDir) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("CaveA.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    let sid = project
        .add_section(SectionData {
            name: Some("SectionA".to_string()),
            shots: vec![shot(2.0), shot(2.0)],
            ..Default::default()
        })
        .id;
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success, "open the local fixture");
    let shid = state.get_project_state().unwrap().sections[0].shots[0].id;
    (state, sid, shid, temp)
}

/// One mutation that needs nothing but an open project: move station 1.
fn move_station(state: &AppState, sid: SectionId) -> crate::core::CommandResult {
    state.execute(Command::MoveStation {
        station_ref: StationRef::new(sid, 1),
        offset: Coordinates::new(5.0, 0.0, 0.0),
    })
}

/// [#576 C4] The workspace refusal: its code, an edit-gate one, and display text
/// with no marker in it (it used to read "EDIT_DISABLED: WORKSPACE_READ_ONLY: ...").
fn refused_by_workspace(error: &CommandError) -> bool {
    error.code == CommandErrorCode::WorkspaceReadOnly
        && error.code.is_edit_gate()
        && !error.message.contains("WORKSPACE_READ_ONLY")
        && !error.message.contains("EDIT_DISABLED")
}

#[test]
fn every_entrypoint_is_refused_in_plan_and_export_except_the_allowlist() {
    for mode in [WorkspaceMode::Plan, WorkspaceMode::Export] {
        let (state, sid, shid, _temp) = local_fixture();
        state.set_workspace_mode(mode);
        let modified_before = state.get_project_state().unwrap().modified_at;
        let undo_before = state.get_undo_stack().len();

        let outcomes = probe_mutation_entrypoints(&state, sid, shid);
        let probed: Vec<&str> = outcomes.iter().map(|(label, _)| *label).collect();
        assert_eq!(
            probed, MUTATION_ENTRYPOINTS,
            "the probe covers the whole list"
        );

        for (label, outcome) in &outcomes {
            println!("{mode:?}: {label} -> {outcome:?}");
            if WORKSPACE_ALLOWLIST.contains(label) {
                assert!(
                    outcome.is_ok(),
                    "{label} is on the read-only-workspace allowlist and must succeed in {mode:?}, got {outcome:?}"
                );
            } else {
                let error = outcome
                    .as_ref()
                    .err()
                    .unwrap_or_else(|| panic!("{label} must be refused in {mode:?}"));
                assert!(
                    refused_by_workspace(error),
                    "{label} must carry WORKSPACE_READ_ONLY in {mode:?}, got {error:?}"
                );
            }
        }

        // Only the allowlisted metadata changed: the survey is untouched.
        let after = state.get_project_state().unwrap();
        assert_eq!(after.name, "Renamed Again", "the allowlisted rename landed");
        assert_eq!(after.sections.len(), 1, "no import, split or delete landed");
        assert_eq!(
            after.sections[0].shots.len(),
            2,
            "no shot was split or deleted"
        );
        // update_project_settings records its own snapshot-pair undo entry; nothing
        // else may have added one.
        assert_eq!(
            state.get_undo_stack().len(),
            undo_before + 1,
            "only the allowlisted settings save recorded an undo entry"
        );
        assert_ne!(
            after.modified_at, modified_before,
            "the allowlisted saves did run"
        );
    }
}

#[test]
fn survey_mode_refuses_nothing() {
    let (state, sid, shid, _temp) = local_fixture();
    // Through Plan and back: a survey edit after the round trip is not refused.
    state.set_workspace_mode(WorkspaceMode::Plan);
    state.set_workspace_mode(WorkspaceMode::Survey);
    let moved = move_station(&state, sid);
    println!(
        "Survey after Plan: execute(MoveStation) -> success={}",
        moved.success
    );
    assert!(
        moved.success,
        "a survey edit in Survey succeeds: {:?}",
        moved.error
    );
    assert!(state.undo().success, "and undo works in Survey");

    // Every entry point: some fail for their own reasons on this tiny fixture,
    // none because of the workspace.
    for (label, outcome) in probe_mutation_entrypoints(&state, sid, shid) {
        if let Err(error) = &outcome {
            assert!(
                error.code != CommandErrorCode::WorkspaceReadOnly,
                "{label} must not be refused by the workspace in Survey, got {error:?}"
            );
        }
    }
}

#[test]
fn every_project_slot_commit_resets_the_workspace_to_survey() {
    let (state, sid, _shid, temp) = local_fixture();
    let path = state.get_project_state().unwrap().file_path.unwrap();

    // Open: the frontend opens every project in Survey, so must the backend.
    state.set_workspace_mode(WorkspaceMode::Plan);
    assert!(state.open_project(path).success);
    assert_eq!(state.workspace_mode(), WorkspaceMode::Survey, "open resets");
    assert!(move_station(&state, sid).success, "editable after open");

    // Close.
    state.set_workspace_mode(WorkspaceMode::Export);
    assert!(state.execute(Command::CloseProject).success);
    assert_eq!(
        state.workspace_mode(),
        WorkspaceMode::Survey,
        "close resets"
    );

    // Create.
    state.set_workspace_mode(WorkspaceMode::Plan);
    assert!(
        state
            .execute(Command::CreateProject {
                name: "CaveB".to_string(),
                location: Some(temp.path().to_string_lossy().into_owned()),
            })
            .success
    );
    assert_eq!(
        state.workspace_mode(),
        WorkspaceMode::Survey,
        "create resets"
    );
}

#[test]
fn the_older_gates_still_come_first_for_the_allowlist() {
    // The allowlist only skips the WORKSPACE check: a read-only SpeleoDB project
    // still refuses a settings save in Plan, with the lock's reason, not the
    // workspace's.
    let (state, _sid, _shid, _temp) = local_fixture();
    state.set_edit_enabled(false);
    state.set_workspace_mode(WorkspaceMode::Plan);
    let settings =
        state.update_project_settings("Renamed".to_string(), state.get_config().validation, None);
    let error = settings.error.unwrap_or_default();
    assert!(!settings.success, "refused by the lock gate");
    assert_eq!(
        settings.code,
        Some(CommandErrorCode::EditDisabled),
        "the lock refusal, not the workspace one: {error:?}"
    );
    assert!(!error.contains("EDIT_DISABLED"), "no marker: {error:?}");
}

#[test]
fn the_wire_spells_modes_as_the_frontend_does() {
    for (wire, mode) in [
        ("\"survey\"", WorkspaceMode::Survey),
        ("\"plan\"", WorkspaceMode::Plan),
        ("\"export\"", WorkspaceMode::Export),
    ] {
        let parsed: WorkspaceMode = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed, mode);
    }
    assert!(serde_json::from_str::<WorkspaceMode>("\"closures\"").is_err());
}
