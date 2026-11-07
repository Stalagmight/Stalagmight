use super::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::core::undo::{UndoData, UndoEntry};
use crate::core::{Command, CommandResult, PersistStack, UndoStack};
use crate::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, ImportPlacement, ImportSectionConfig,
    LoopClosure, Project, SectionData, SectionId, SectionUpdates, ShotData, ShotId, ShotKind,
    ShotUpdates, StationAnchor, StationKey, StationMerge, StationRef, SurveyDirection,
    MIN_SPLIT_CHILD_LENGTH_M,
};
use crate::persistence::Database;
use crate::test_fixtures::shot;
use tempfile::TempDir;

fn assert_coords_close(a: Coordinates, b: Coordinates) {
    let eps = 1e-6;
    assert!((a.x - b.x).abs() < eps, "x mismatch: {} vs {}", a.x, b.x);
    assert!((a.y - b.y).abs() < eps, "y mismatch: {} vs {}", a.y, b.y);
    assert!((a.z - b.z).abs() < eps, "z mismatch: {} vs {}", a.z, b.z);
}

/// Like [`assert_coords_close`] but only the horizontal (map) position. Section
/// offsets are 2D, so gap-close deletes preserve x/y but not section-relative z.
fn assert_xy_close(a: Coordinates, b: Coordinates) {
    let eps = 1e-6;
    assert!((a.x - b.x).abs() < eps, "x mismatch: {} vs {}", a.x, b.x);
    assert!((a.y - b.y).abs() < eps, "y mismatch: {} vs {}", a.y, b.y);
}

/// Absolute (section-offset-applied) coordinates of every station, keyed by ref.
/// Used to assert that an edit shifts only the affected geometry and leaves the
/// rest of the map put (#41 regression sentinels).
fn abs_coord_map(project: &Project) -> HashMap<StationRef, Coordinates> {
    project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, s.coordinates))
        .collect()
}

fn poison_mutex<T>(mutex: &crate::models::poison_policy::RecoveringMutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock();
        panic!("poison mutex for lock failure coverage");
    }));
}

/// [#576 A11] What a refusing site (undo / redo, see `AppState::refuse_after_crash`)
/// answers after a crash poisoned one of the cells it refuses on.
fn refused_after_crash(what: &str) -> String {
    format!(
        "{what} is unavailable after an internal error. Save your work and restart Stalagmight."
    )
}

use crate::core::CommandError;
use crate::models::{CommandErrorCode, SpeleoLink};

/// A SpeleoLink fixture (clean: no unpushed edits).
fn fixture_speleo_link() -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://10.10.0.220:8000/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "Remote Cave".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

/// Build an AppState with an OPEN, STAMPED-REMOTE project: a temp `.stalx` saved
/// with one section + one shot, stamped with a speleo_link row, then opened via
/// `open_project` so `edit_enabled` flips false (remote-open-without-lock).
/// Returns `(state, section_id, shot_id, _temp)` - keep `_temp` alive.
fn remote_fixture() -> (AppState, SectionId, ShotId, TempDir) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("remote.stalx");

    let mut project = Project::new("Remote Cave".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(2.0), shot(2.0)],
            ..Default::default()
        })
        .id;

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
    drop(db);

    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success, "open remote fixture");
    // open_project should have flipped edit_enabled false (link present).
    assert!(!state.edit_enabled(), "remote-open disables editing");

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;
    (state, section_id, shot_id, temp)
}

/// Build the same fixture but force editing ENABLED (simulating a later
/// lock-acquire) so the dirty-bit plumbing can be exercised.
fn remote_fixture_editable() -> (AppState, SectionId, ShotId, TempDir) {
    let (state, sid, shid, temp) = remote_fixture();
    state.set_edit_enabled(true);
    (state, sid, shid, temp)
}

/// Read has_unpushed_edits straight from the open database.
fn dirty_bit(state: &AppState) -> bool {
    let guard = state.database.lock();
    guard
        .as_ref()
        .unwrap()
        .load_speleo_link()
        .unwrap()
        .unwrap()
        .has_unpushed_edits
}

/// The chokepoint enumeration tripwire (D-B6): every SURVEY-CONTENT mutation entry
/// listed in MUTATION_ENTRYPOINTS must reject on a read-only remote project with the
/// EDIT_DISABLED code, and (#576 C5, `workspace_mode_tests`) on a local project
/// whose workspace is Plan or Export with EDIT_DISABLED + WORKSPACE_READ_ONLY,
/// except the two entries on the read-only-workspace allowlist. Both tests drive
/// the same [`probe_mutation_entrypoints`], which calls every listed entry once.
///
/// CAVEAT - this list is hand-maintained, so it catches a forgotten guard on a KNOWN
/// entry, NOT a brand-new `execute_*`/`set_*_for_path` method added without a
/// `guard_mutation` call (that would compile and pass green). It is NOT derived
/// from the IPC command manifest (#576 C5-D3, decided when C3 landed): the manifest
/// is read from `#[tauri::command]` signatures, which say nothing about mutating (a
/// `CommandResult` return is not it - save/close/recent-list commands return one);
/// these entries are `AppState` methods and `Command` variants below the IPC surface,
/// many-to-one with commands; each still needs its hand-built probe call; and the
/// manifest is app-crate output this crate cannot read. The exhaustive
/// `is_lifecycle_passthrough` match closes the hole for new `Command` *variants*.
/// A new method that does call `guard_mutation` is refused in Plan/Export by
/// default; only an explicit `guard_metadata_mutation` caller is allowed there.
/// And note the deliberately-ungated local-only writes documented on
/// `guard_mutation` (display settings, import-dedup) are NOT in this list - but
/// warning resolve/unresolve ARE gated (#197) and appear at the end.
pub(super) const MUTATION_ENTRYPOINTS: &[&str] = &[
    // execute() + a representative spread of non-lifecycle Command variants. The
    // guard runs BEFORE the variant match, so one code path gates them all - but
    // probing the GEOMETRY variants the move/drag tools use (MoveStation/Segment/
    // Section/Batch) guards against a future misclassification into the
    // is_lifecycle_passthrough arm, which the lone UpdateShot probe would NOT
    // catch. That gap sat behind the move-shot read-only "shots vanish" report.
    "execute",
    "execute:MoveStation",
    "execute:MoveSegment",
    "execute:MoveSection",
    "execute:Batch",
    // ReverseSection renumbers stations and is newly reachable from three menus (#24), so it
    // is exactly the shape the geometry probes above exist for. Both the bare command and the
    // Batch wrapper the multi-select path uses.
    "execute:ReverseSection",
    "execute:Batch:ReverseSection",
    "execute:MoveComment",
    // history entrypoints
    "undo",
    "redo",
    "undo_to",
    "redo_to",
    // direct-lock bypass methods
    "update_project_settings",
    "set_surface_elevation",
    "repair_open_project_integrity",
    "execute_delete_shot_and_split",
    "execute_delete_shot_range_split",
    "execute_delete_shot_range_shift",
    "execute_delete_last_shot_and_section",
    "execute_split_shot_at_distance",
    "execute_create_and_merge",
    "execute_import_sections",
    // by-path / metadata methods (gated only when path == the OPEN project)
    "set_project_name_for_path",
    // warning resolve/unresolve (#197): gated because resolved_warnings proliferates
    // to the server (NOT stripped from the push snapshot).
    "save_resolved_warning_gated",
    "delete_resolved_warning_gated",
];

/// Call every entry point in [`MUTATION_ENTRYPOINTS`] once, in list order, against
/// the OPEN project (which must have been saved: `set_project_name_for_path` is
/// gated only for the open project's own path). One outcome per entry: `Ok` when
/// the entry point accepted the call, `Err(error)` when it refused it - the
/// `CommandResult` failure as its `CommandError` (#576 C4), or the error of the
/// entries that return a `Result`. On an editable project the accepted calls really mutate it, so later
/// entries may fail for their own reasons; callers assert on the refusal codes.
pub(super) fn probe_mutation_entrypoints(
    state: &AppState,
    sid: SectionId,
    shid: ShotId,
) -> Vec<(&'static str, Result<(), CommandError>)> {
    let outcome = |r: CommandResult| {
        if r.success {
            Ok(())
        } else {
            Err(r.into_error())
        }
    };
    let open_path = state
        .get_project_state()
        .and_then(|p| p.file_path)
        .expect("the probe needs a saved, open project");
    let offset = Coordinates::new(1.0, 2.0, 0.0);
    let shot_holder = crate::core::CommentHolder::Shot {
        section_id: sid,
        shot_id: shid,
    };
    let warning = crate::persistence::ResolvedWarning {
        section_id: sid,
        shot_id: shid,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    };
    let import_config = ImportSectionConfig {
        reverse: false,
        section_index: 0,
        name: Some("Imp".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(3.0)],
        station_comments: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
    };
    vec![
        // execute() and its in-execute arms. The guard runs BEFORE the variant
        // match, so one code path gates them all - but the GEOMETRY variants the
        // move/drag tools use are probed explicitly so a future reclassification
        // into is_lifecycle_passthrough can't silently reopen the move-shot
        // read-only hole (the "shots vanish" report).
        (
            "execute",
            outcome(state.execute(Command::UpdateShot {
                section_id: sid,
                shot_id: shid,
                updates: ShotUpdates::default(),
            })),
        ),
        (
            "execute:MoveStation",
            outcome(state.execute(Command::MoveStation {
                station_ref: StationRef::new(sid, 1),
                offset,
            })),
        ),
        (
            "execute:MoveSegment",
            outcome(state.execute(Command::MoveSegment {
                section_id: sid,
                offset,
            })),
        ),
        (
            "execute:MoveSection",
            outcome(state.execute(Command::MoveSection {
                section_id: sid,
                offset,
            })),
        ),
        (
            "execute:Batch",
            outcome(state.execute(Command::Batch {
                commands: vec![Command::MoveStation {
                    station_ref: StationRef::new(sid, 1),
                    offset,
                }],
            })),
        ),
        (
            "execute:ReverseSection",
            outcome(state.execute(Command::ReverseSection {
                section_id: sid,
                swap_left_right: false,
            })),
        ),
        (
            "execute:Batch:ReverseSection",
            outcome(state.execute(Command::Batch {
                commands: vec![Command::ReverseSection {
                    section_id: sid,
                    swap_left_right: false,
                }],
            })),
        ),
        (
            "execute:MoveComment",
            outcome(state.execute(Command::MoveComment {
                source: shot_holder.clone(),
                target: shot_holder,
                swap: false,
            })),
        ),
        // history entrypoints
        ("undo", outcome(state.undo())),
        ("redo", outcome(state.redo())),
        ("undo_to", outcome(state.undo_to(0))),
        ("redo_to", outcome(state.redo_to(0))),
        // direct-lock bypass methods
        (
            "update_project_settings",
            outcome(state.update_project_settings(
                "Renamed".to_string(),
                state.get_config().validation,
                None,
            )),
        ),
        (
            "set_surface_elevation",
            outcome(state.set_surface_elevation(Some(12.0))),
        ),
        (
            "repair_open_project_integrity",
            state.repair_open_project_integrity().map(|_| ()),
        ),
        (
            "execute_delete_shot_and_split",
            outcome(state.execute_delete_shot_and_split(
                sid,
                shid,
                "A".to_string(),
                "B".to_string(),
            )),
        ),
        (
            "execute_delete_shot_range_split",
            outcome(state.execute_delete_shot_range_split(
                sid,
                shid,
                shid,
                "A".to_string(),
                "B".to_string(),
            )),
        ),
        (
            "execute_delete_shot_range_shift",
            outcome(state.execute_delete_shot_range_shift(sid, shid, shid)),
        ),
        (
            "execute_delete_last_shot_and_section",
            outcome(state.execute_delete_last_shot_and_section(sid, shid)),
        ),
        (
            "execute_split_shot_at_distance",
            outcome(state.execute_split_shot_at_distance(sid, shid, 1.0)),
        ),
        (
            "execute_create_and_merge",
            outcome(state.execute_create_and_merge(
                Some("Imp".to_string()),
                None,
                None,
                "#00ff00".to_string(),
                None,
                vec![shot(3.0)],
                0,
                StationRef::new(sid, 1),
                ClosureStrategy::Batch,
            )),
        ),
        (
            "execute_import_sections",
            outcome(state.execute_import_sections(
                vec![import_config],
                Vec::new(),
                None,
                Vec::new(),
                Vec::new(),
                VirtualShotMode::Auto,
            )),
        ),
        // by-path / metadata methods (gated only when path == the OPEN project)
        (
            "set_project_name_for_path",
            outcome(state.set_project_name_for_path(open_path, "Renamed Again".to_string())),
        ),
        // warning resolve/unresolve (#197): gated because resolved_warnings
        // proliferates to the server. They return Result<(), CommandError>, the
        // gate's own code included (#576 C4).
        (
            "save_resolved_warning_gated",
            state.save_resolved_warning_gated(&warning),
        ),
        (
            "delete_resolved_warning_gated",
            state.delete_resolved_warning_gated(sid, shid, "SHORT_SHOT", None),
        ),
    ]
}

// The topic files (#576 E1): each is one contiguous run of the former single
// file, and its module doc names the lines it came from. The helpers above are
// the ones more than one of them (or a file outside this module) uses; a helper
// only one file needs lives in that file. `#[cfg(test)]` on each declaration is
// redundant for the compiler (this whole module is test-only) but is what marks
// them as test files for the Rust file-size ratchet and the source scans that
// skip `*_tests.rs`.
#[cfg(test)]
mod closure_and_station_tests;
#[cfg(test)]
mod command_basics_tests;
#[cfg(test)]
mod command_execution_tests;
#[cfg(test)]
mod commands_coverage_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod create_shot_undo_tests;
#[cfg(test)]
mod delete_last_shot_tests;
#[cfg(test)]
mod delete_range_shift_tests;
#[cfg(test)]
mod delete_range_tests;
#[cfg(test)]
mod delete_section_cascade_tests;
#[cfg(test)]
mod history_repair_tests;
#[cfg(test)]
mod merge_chain_ops_tests;
#[cfg(test)]
mod merge_chain_tests;
#[cfg(test)]
mod merge_import_coverage_tests;
#[cfg(test)]
mod merge_point_edit_tests;
#[cfg(test)]
mod move_segment_tests;
#[cfg(test)]
mod profile_harness_tests;
#[cfg(test)]
mod project_history_tests;
#[cfg(test)]
mod project_integrity_tests;
#[cfg(test)]
mod project_io_coverage_tests;
#[cfg(test)]
mod project_lifecycle_tests;
#[cfg(test)]
mod read_only_gate_tests;
#[cfg(test)]
mod section_ops_tests;
#[cfg(test)]
mod section_undo_tests;
#[cfg(test)]
mod shot_ops_tests;
#[cfg(test)]
mod station_ref_shift_tests;
#[cfg(test)]
mod undo_failure_tests;
#[cfg(test)]
mod undo_redo_tests;
