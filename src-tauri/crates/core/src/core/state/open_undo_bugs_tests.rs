//! Regression tests for the undo/redo bugs the agent-harness fuzzer and the
//! Rust undo fuzzer found in 0.6.0 (#413 and its findings, #469, #557), on
//! the changeset engine.
//!
//! # The rule
//!
//! A test here is committed `#[ignore]`d with the bug it pins while the bug
//! is open, so the default suite stays green while `cargo test -- --ignored`
//! runs it RED; when the bug is fixed the `#[ignore]` goes and the test is its
//! regression guard. **Nothing here is `#[ignore]`d**: every bug this module
//! pinned is fixed, or its mechanism is gone with the machinery the redesign
//! deleted.
//!
//! # What the port kept, and what it retired
//!
//! Each case keeps the property the user could see, rewritten on the station
//! graph: a shot named by its uuid after a reopen, a line or a datum where it
//! was, an order restored, a history that never wedges, a save that heals
//! nothing an undo could later revert, a Computed View that is a function of
//! the survey. Most cases also gain the engine's own oracle: one undo and one
//! redo land on the recorded states to the bit.
//!
//! Retired, because they pinned machinery the redesign deleted (each with
//! its reason at its old place below): the gauge-pair anchors
//! (`gauge_shot_uuid`, captured on save), the legacy `ZeroDatumInfo` undo
//! arm, the `restore_snapshot_into` hostile shot counter (the counter is a
//! session value now, never read from a file), and the per-line stored
//! `created_at` (SQLite `sections.created_at`). `line_offsets` became the
//! `Placement` of a datum-less piece; the cases about an offset's key and
//! value are kept on placements.
//!
//! # Data
//!
//! Everything is built in code. No fixture is read, copied or derived from.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::topology_kit::{assert_round_trip, assert_same, snapshot, Snapshot};
use super::AppState;
use crate::core::Command;
use crate::models::survey::changeset::{decode_entry, EntryLimits};
use crate::models::survey::Project;
use crate::models::{
    gps_to_local, ClosureStrategy, Coordinates, GpsCoordinates, LineId, LineUpdates, ShotData,
    ShotId, ShotKind, ShotUpdates, StationId, StationRef,
};
use crate::persistence::stalx_v2::{self, DecodeOptions, RawJson};

/// A shot with distinct, easily-recognised geometry.
fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    }
}

/// An `AppState` with an empty project open, ready for commands.
fn state_with_project() -> AppState {
    let state = AppState::new().unwrap();
    let result = state.execute(Command::CreateProject {
        name: "OpenUndoBugs".to_string(),
        location: None,
    });
    assert!(result.success, "CreateProject failed: {:?}", result.error);
    state
}

fn create_line(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    let result = state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#3388FF".to_string(),
        direction: None,
        shots,
        cave_section_id: None,
    });
    assert!(result.success, "CreateLine failed: {:?}", result.error);
    result.line_id.expect("CreateLine returned no line id")
}

fn project(state: &AppState) -> Project {
    state.get_project_state().expect("no project open")
}

fn shot_uuids(state: &AppState, line_id: LineId) -> Vec<String> {
    project(state)
        .line(line_id)
        .expect("line missing")
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect()
}

fn shot_ids(state: &AppState, line_id: LineId) -> Vec<ShotId> {
    project(state)
        .line(line_id)
        .expect("line missing")
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

fn shot_kinds(state: &AppState, line_id: LineId) -> Vec<ShotKind> {
    project(state)
        .line(line_id)
        .expect("line missing")
        .shots
        .iter()
        .map(|s| s.kind)
        .collect()
}

fn shot_lengths(state: &AppState, line_id: LineId) -> Vec<f64> {
    project(state)
        .line(line_id)
        .expect("line missing")
        .shots
        .iter()
        .map(|s| s.length)
        .collect()
}

/// The raw (measured) `(x, y)` of a slot.
fn xy(state: &AppState, line: LineId, index: u32) -> (f64, f64) {
    let p = project(state);
    let [x, y, _] = p
        .raw_positions()
        .slot(&p, StationRef::new(line, index))
        .expect("station has no raw position");
    (x, y)
}

fn gap(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// The raw `(x, y)` of every slot.
fn all_slot_xy(state: &AppState) -> Vec<(StationRef, (f64, f64))> {
    let p = project(state);
    p.raw_positions()
        .slots(&p)
        .into_iter()
        .map(|s| (s.slot, (s.x, s.y)))
        .collect()
}

fn station_at(p: &Project, line: LineId, index: u32) -> StationId {
    p.topology()
        .station_at(StationRef::new(line, index))
        .expect("no station at the slot")
}

fn save(state: &AppState, path: &Path) {
    let saved = state.execute(Command::SaveProject {
        path: Some(path.to_path_buf()),
    });
    assert!(saved.success, "save failed: {:?}", saved.error);
}

fn reopen(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let opened = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(opened.success, "reopen failed: {:?}", opened.error);
    state
}

/// The whole survey as the file writes it (no session ids: a reopened
/// project has its own).
fn text(state: &AppState) -> String {
    snapshot(state).0
}

fn set_datum(state: &AppState, at: StationRef, latitude: f64, longitude: f64) {
    let r = state.execute(Command::SetZeroDatum {
        station_ref: at,
        coordinates: GpsCoordinates::new(latitude, longitude, 0.0).unwrap(),
        accuracy_m: None,
        entrance: None,
    });
    assert!(r.success, "SetZeroDatum failed: {:?}", r.error);
}

fn set_length(state: &AppState, line_id: LineId, shot_id: ShotId, length: f64) {
    let r = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(length),
            ..Default::default()
        },
    });
    assert!(r.success, "UpdateShot failed: {:?}", r.error);
}

// ===========================================================================
// Bug 1 (FIXED): redo of CreateLine re-minted every shot uuid
// ===========================================================================

/// 0.6.0's `CreatedLineSnapshot` held a `LineData` with no uuid, so a redo
/// minted fresh ones and dangled every anchor keyed on the shots. A
/// changeset records the line's whole image, uuids included.
#[test]
fn redo_of_create_line_preserves_shot_uuids() {
    let state = state_with_project();
    let line_id = create_line(&state, "Main", vec![shot(0.0, 5.0), shot(90.0, 4.0)]);
    let before = shot_uuids(&state, line_id);
    assert!(before.iter().all(|u| !u.is_empty()), "setup: uuids");
    assert!(state.undo().success, "undo of CreateLine failed");
    assert!(state.redo().success, "redo of CreateLine failed");
    assert_eq!(
        shot_uuids(&state, line_id),
        before,
        "redo of CreateLine re-minted the shot uuids"
    );
}

// `redo_of_create_line_preserves_gauge_anchors`: RETIRED. It pinned the
// least-squares gauge pair (`gauge_shot_uuid` / `gauge_neighbour_uuid`) a
// save captured on each line. The station graph has no gauge pair: the
// gauge is the piece's `Placement` station (plan 3.7, W5), recorded in the
// changeset like every other list, so a redo puts it back with the line.
// `undo_of_a_pre_save_entry_is_route_independent` keeps the half that
// mattered: a save heals nothing an undo could revert.

// ===========================================================================
// Bug 5 (FIXED): undo of UpdateLineMetadata could not restore a null field
// ===========================================================================

/// `LineUpdates` keeps "clear this field" (`Some(None)`) apart from "leave
/// it alone" (`None`) through serde: the IPC door reads it from the
/// frontend's JSON, where a cleared surveyor is an explicit `null`.
#[test]
fn line_updates_round_trip_preserves_clear_to_null() {
    let clear_the_surveyor = LineUpdates {
        surveyor: Some(None),
        ..Default::default()
    };
    let json = serde_json::to_string(&clear_the_surveyor).unwrap();
    let restored: LineUpdates = serde_json::from_str(&json).unwrap();
    assert_eq!(
        restored.surveyor,
        Some(None),
        "LineUpdates lost \"clear this field\" through serde: {json}"
    );
    let leave: LineUpdates = serde_json::from_str("{}").unwrap();
    assert_eq!(leave.surveyor, None, "an absent field is left alone");
}

/// Set a surveyor on a line that had none, undo: the line has none again.
#[test]
fn undo_restores_a_previously_null_surveyor() {
    let state = state_with_project();
    let line_id = create_line(&state, "NoSurveyor", vec![shot(0.0, 5.0)]);
    let before = snapshot(&state);
    assert!(
        project(&state).line(line_id).unwrap().shots[0]
            .credit
            .is_none(),
        "setup: no surveyor"
    );
    let updated = state.execute(Command::UpdateLineMetadata {
        line_id,
        updates: LineUpdates {
            surveyor: Some(Some("Someone".to_string())),
            ..Default::default()
        },
    });
    assert!(updated.success, "{:?}", updated.error);
    assert!(project(&state).line(line_id).unwrap().shots[0]
        .credit
        .is_some());
    assert_round_trip(&state, &before, "set a surveyor");
    assert!(state.undo().success);
    let p = project(&state);
    let line = p.line(line_id).unwrap();
    assert!(
        line.shots[0].credit.is_none() && line.legacy_surveyor.is_none(),
        "undo left the new surveyor in place instead of clearing it back"
    );
}

// ===========================================================================
// Bug 8 / findings 12, 13, 16 (FIXED): after save/reload, an undo hit the
// wrong shot, or wedged the history
// ===========================================================================

/// Shot ids are session values: a reopen gives every line `line_id *
/// FEATURE_ID_STRIDE + index`. A persisted entry names shots by uuid and
/// checks the content hash of every line it rebuilds, so a stale id can
/// neither collide with another shot nor miss.
#[test]
fn undo_after_reload_deletes_the_shot_it_created() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("renumbered.stalx");
    let (inserted, original);
    {
        let state = state_with_project();
        let line_id = create_line(&state, "Diverged", vec![shot(0.0, 5.0)]);
        let first = shot_ids(&state, line_id)[0];
        // Insert BEFORE: the new shot takes position 0, its id the next one.
        let created = state.execute(Command::CreateShot {
            line_id,
            shot: shot(180.0, 7.0),
            after_shot_id: None,
            before_shot_id: Some(first),
        });
        assert!(created.success, "CreateShot failed: {:?}", created.error);
        let uuids = shot_uuids(&state, line_id);
        inserted = uuids[0].clone();
        original = uuids[1].clone();
        save(&state, &path);
    }
    let reopened = reopen(&path);
    let undone = reopened.undo();
    assert!(undone.success, "undo after reopen failed: {:?}", undone.error);
    let remaining: Vec<String> = project(&reopened)
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
        .collect();
    assert_eq!(
        remaining,
        vec![original],
        "the undo removed exactly the shot it created ({inserted})"
    );
}

/// A line whose shot ids no longer match positional order: three shots and
/// an insert BEFORE the first.
fn diverged_line(state: &AppState) -> LineId {
    let line_id = create_line(
        state,
        "Diverged",
        vec![shot(0.0, 5.0), shot(90.0, 6.0), shot(180.0, 7.0)],
    );
    let first = shot_ids(state, line_id)[0];
    let created = state.execute(Command::CreateShot {
        line_id,
        shot: shot(270.0, 8.0),
        after_shot_id: None,
        before_shot_id: Some(first),
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    let ids = shot_ids(state, line_id);
    assert_ne!(
        ids,
        (0..ids.len())
            .map(|i| line_id * crate::models::FEATURE_ID_STRIDE + i as u32)
            .collect::<Vec<_>>(),
        "setup: the ids no longer match positional order"
    );
    line_id
}

#[test]
fn undo_of_set_shot_kind_after_reload_hits_the_shot_it_changed() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("wrong-shot-kind.stalx");
    let (marked_uuid, line_id);
    {
        let state = state_with_project();
        line_id = diverged_line(&state);
        let target = shot_ids(&state, line_id)[0];
        marked_uuid = shot_uuids(&state, line_id)[0].clone();
        let marked = state.execute(Command::SetShotKind {
            line_id,
            shot_id: target,
            kind: ShotKind::Jump,
        });
        assert!(marked.success, "SetShotKind failed: {:?}", marked.error);
        assert_eq!(shot_kinds(&state, line_id)[0], ShotKind::Jump);
        save(&state, &path);
    }
    let reopened = reopen(&path);
    let undone = reopened.undo();
    assert!(undone.success, "undo after reopen failed: {:?}", undone.error);
    let p = project(&reopened);
    let marked = p
        .line(line_id)
        .unwrap()
        .shots
        .iter()
        .find(|s| s.uuid == marked_uuid)
        .expect("the marked shot vanished");
    assert_eq!(marked.kind, ShotKind::Normal, "the undo reset the shot it changed");
    assert!(
        shot_kinds(&reopened, line_id)
            .iter()
            .all(|k| *k == ShotKind::Normal),
        "and no other shot: {:?}",
        shot_kinds(&reopened, line_id)
    );
}

#[test]
fn undo_of_update_shot_after_reload_restores_the_shot_it_changed() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("wrong-shot-length.stalx");
    let (line_id, lengths_before);
    {
        let state = state_with_project();
        line_id = diverged_line(&state);
        lengths_before = shot_lengths(&state, line_id);
        let target = shot_ids(&state, line_id)[0];
        set_length(&state, line_id, target, 42.5);
        save(&state, &path);
    }
    let reopened = reopen(&path);
    assert!(reopened.undo().success, "undo after reopen failed");
    assert_eq!(
        shot_lengths(&reopened, line_id),
        lengths_before,
        "the undo restored the shot it changed, and only that one"
    );
    assert!(reopened.redo().success);
    assert_eq!(shot_lengths(&reopened, line_id)[0], 42.5);
}

/// A line that has had a shot deleted has handed out more ids than it holds
/// shots; after a reopen the whole history still unwinds, entry by entry,
/// to the empty project.
#[test]
fn undo_after_reload_is_not_wedged_by_an_out_of_range_shot_id() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("wedged.stalx");
    let empty;
    {
        let state = state_with_project();
        empty = snapshot(&state);
        let line_id = create_line(
            &state,
            "Wedged",
            vec![shot(0.0, 5.0), shot(90.0, 6.0), shot(180.0, 7.0)],
        );
        let middle = shot_ids(&state, line_id)[1];
        let deleted = state.execute(Command::DeleteShot {
            line_id,
            shot_id: middle,
        });
        assert!(deleted.success, "DeleteShot failed: {:?}", deleted.error);
        let first = shot_ids(&state, line_id)[0];
        let created = state.execute(Command::CreateShot {
            line_id,
            shot: shot(270.0, 8.0),
            after_shot_id: None,
            before_shot_id: Some(first),
        });
        assert!(created.success, "CreateShot failed: {:?}", created.error);
        let ids = shot_ids(&state, line_id);
        let ceiling = line_id * crate::models::FEATURE_ID_STRIDE + ids.len() as u32 - 1;
        assert!(
            ids.iter().any(|id| *id > ceiling),
            "setup: a live id past the range a reopen numbers into"
        );
        save(&state, &path);
    }
    let reopened = reopen(&path);
    assert_eq!(reopened.get_undo_stack().len(), 3);
    for step in 0..3 {
        let undone = reopened.undo();
        assert!(
            undone.success,
            "undo {step} after the reopen failed ({:?}): the history is wedged",
            undone.error
        );
    }
    assert_same(&snapshot(&reopened), &empty, "back to the empty project");
}

/// After a reopen the undo of the edit and the undo of an unrelated entry
/// below it leave every shot of the edited line as it was: nothing a stale
/// id could have misdirected survives two entries down.
#[test]
fn unwinding_past_a_stale_id_edit_leaves_no_shot_holding_it() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("stale-cascade.stalx");
    let (edited_line, lengths, other_line);
    {
        let state = state_with_project();
        edited_line = create_line(&state, "Edited", vec![shot(0.0, 5.0)]);
        other_line = create_line(&state, "Other", vec![shot(90.0, 6.0)]);
        let existing = shot_ids(&state, edited_line)[0];
        let created = state.execute(Command::CreateShot {
            line_id: edited_line,
            shot: shot(180.0, 7.0),
            after_shot_id: None,
            before_shot_id: Some(existing),
        });
        assert!(created.success, "CreateShot failed: {:?}", created.error);
        lengths = shot_lengths(&state, edited_line);
        let other_shot = shot_ids(&state, other_line)[0];
        let marked = state.execute(Command::SetShotKind {
            line_id: other_line,
            shot_id: other_shot,
            kind: ShotKind::Jump,
        });
        assert!(marked.success);
        let target = shot_ids(&state, edited_line)[0];
        set_length(&state, edited_line, target, 42.5);
        save(&state, &path);
    }
    let reopened = reopen(&path);
    assert!(reopened.undo().success, "undo of UpdateShot failed");
    assert!(reopened.undo().success, "undo of SetShotKind failed");
    assert_eq!(shot_lengths(&reopened, edited_line), lengths);
    assert_eq!(shot_kinds(&reopened, other_line), vec![ShotKind::Normal]);
}

/// An undo/redo cycle of CreateShot after a reopen leaves every shot uuid
/// unique (a repeated uuid is invalid now, so the survey must also
/// validate).
#[test]
fn redo_after_reload_does_not_duplicate_a_shot_uuid() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("duplicated.stalx");
    let line_id;
    {
        let state = state_with_project();
        line_id = diverged_line(&state);
        save(&state, &path);
    }
    let reopened = reopen(&path);
    let after = text(&reopened);
    assert!(reopened.undo().success, "undo after reopen failed");
    assert!(reopened.redo().success, "redo after reopen failed");
    let uuids = shot_uuids(&reopened, line_id);
    let mut unique = uuids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), uuids.len(), "a uuid twice: {uuids:?}");
    assert!(project(&reopened).validate().is_empty());
    assert_eq!(text(&reopened), after, "the redo landed exactly");
}

// ===========================================================================
// Bug 2 (FIXED): undo of a shot insert left an orphan offset entry
// ===========================================================================

/// 0.6.0's insert created a `line_offsets` key the undo zeroed but never
/// removed. A datum-less piece's position is its `Placement` now: the undo
/// of an insert before the first shot hands the placement back exactly,
/// key (station) and value, and leaves nothing else behind.
#[test]
fn undo_of_a_shot_insert_leaves_no_orphan_placement() {
    let state = state_with_project();
    let line_id = create_line(&state, "Inserted", vec![shot(0.0, 5.0), shot(90.0, 4.0)]);
    let first = shot_ids(&state, line_id)[0];
    let before = snapshot(&state);
    let placements = project(&state).placements;
    let created = state.execute(Command::CreateShot {
        line_id,
        shot: shot(180.0, 3.0),
        after_shot_id: None,
        before_shot_id: Some(first),
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    assert!(state.undo().success, "undo of CreateShot failed");
    assert_eq!(project(&state).placements, placements, "the placement");
    assert_same(&snapshot(&state), &before, "the whole survey");
}

// ===========================================================================
// Bug 3 / finding 15 (FIXED): the first shot of an empty line, undone,
// displaced the line, and the displacement ratcheted
// ===========================================================================

#[test]
fn undo_of_the_first_shot_in_an_empty_line_does_not_move_it() {
    let state = state_with_project();
    let line_id = create_line(&state, "Empty", vec![]);
    let start = xy(&state, line_id, 0);
    let before = snapshot(&state);
    let created = state.execute(Command::CreateShot {
        line_id,
        shot: shot(90.0, 10.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    assert_eq!(xy(&state, line_id, 0), start, "an append moves no station");
    assert!(state.undo().success, "undo of CreateShot failed");
    assert_eq!(xy(&state, line_id, 0), start, "the undo moved the line");
    assert_same(&snapshot(&state), &before, "exactly");
}

#[test]
fn the_empty_line_displacement_does_not_ratchet() {
    let state = state_with_project();
    let line_id = create_line(&state, "Ratchet", vec![]);
    let before = snapshot(&state);
    let created = state.execute(Command::CreateShot {
        line_id,
        shot: shot(90.0, 10.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success);
    let after = snapshot(&state);
    for cycle in 0..3 {
        assert!(state.undo().success, "undo {cycle}");
        assert_same(&snapshot(&state), &before, &format!("undo {cycle}"));
        assert!(state.redo().success, "redo {cycle}");
        assert_same(&snapshot(&state), &after, &format!("redo {cycle}"));
    }
}

/// Unwinding the whole history returns the project to where it started:
/// nothing an undo of an appended first shot did survives an unrelated undo
/// below it.
#[test]
fn unwinding_the_history_leaves_no_displaced_line_behind() {
    let state = state_with_project();
    let keeper = create_line(&state, "Keeper", vec![shot(0.0, 5.0)]);
    let empty = create_line(&state, "Empty", vec![]);
    let before = snapshot(&state);
    let start = xy(&state, empty, 0);
    let keeper_shot = shot_ids(&state, keeper)[0];
    set_length(&state, keeper, keeper_shot, 9.0);
    let created = state.execute(Command::CreateShot {
        line_id: empty,
        shot: shot(90.0, 10.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success);
    assert!(state.undo().success, "undo of CreateShot failed");
    assert!(state.undo().success, "undo of UpdateShot failed");
    assert_eq!(xy(&state, empty, 0), start, "the empty line is where it was");
    assert_same(&snapshot(&state), &before, "the whole survey");
}

// ===========================================================================
// Bug 4 / finding 17 (FIXED): undo appended a closure / a datum instead of
// restoring its index
// ===========================================================================

/// Two lines joined at a junction, with a zero datum on the first, so a
/// loop closure between them is legal. Returns `(a, b)`: A has two shots, B
/// one, and A:2 is B:0.
fn two_merged_lines(state: &AppState) -> (LineId, LineId) {
    let a = create_line(state, "A", vec![shot(0.0, 5.0), shot(0.0, 5.0)]);
    let b = create_line(state, "B", vec![shot(90.0, 5.0)]);
    set_datum(state, StationRef::new(a, 0), 10.0, 20.0);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    (a, b)
}

fn closure_ids(state: &AppState) -> Vec<u64> {
    project(state).closures.iter().map(|c| c.id).collect()
}

/// The closure list order is persisted and is what the sweeps and the
/// across-one-shot skip set read in order: an undo of a removal puts the
/// closure back where it was.
#[test]
fn undo_of_remove_loop_closure_restores_the_original_order() {
    let state = state_with_project();
    let (a, b) = two_merged_lines(&state);
    let first = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(first.success, "first closure failed: {:?}", first.error);
    // A second loop: a side passage C off A:1, closed to B:1.
    let c = create_line(&state, "C", vec![shot(45.0, 5.0)]);
    let side = state.execute(Command::MergeStations {
        station_a: StationRef::new(c, 0),
        station_b: StationRef::new(a, 1),
    });
    assert!(side.success, "MergeStations failed: {:?}", side.error);
    let second = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(c, 1),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(second.success, "second closure failed: {:?}", second.error);
    let before_ids = closure_ids(&state);
    assert_eq!(before_ids.len(), 2, "setup: two closures");
    let before = snapshot(&state);
    let removed = state.execute(Command::RemoveLoopClosure {
        closure_id: before_ids[0],
    });
    assert!(removed.success, "RemoveLoopClosure failed: {:?}", removed.error);
    assert_round_trip(&state, &before, "remove the first closure");
    assert!(state.undo().success);
    assert_eq!(closure_ids(&state), before_ids, "back at its index");
}

fn datum_stations(state: &AppState) -> Vec<StationId> {
    project(state).datums.iter().map(|d| d.station).collect()
}

/// The datum list order is persisted and decides which datum is the frame
/// and which places its piece: an undo of a removal puts it back at its
/// index (finding 17). The FIRST of two is removed, so an append would show.
#[test]
fn undo_of_remove_zero_datum_restores_the_original_order() {
    let state = state_with_project();
    let first = create_line(&state, "First", vec![shot(0.0, 5.0)]);
    let second = create_line(&state, "Second", vec![shot(90.0, 5.0)]);
    set_datum(&state, StationRef::new(first, 0), 10.0, 20.0);
    set_datum(&state, StationRef::new(second, 0), 11.0, 20.0);
    let order = datum_stations(&state);
    assert_eq!(order.len(), 2, "setup: two datums");
    let before = snapshot(&state);
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(first, 0),
    });
    assert!(removed.success, "RemoveZeroDatum failed: {:?}", removed.error);
    assert_round_trip(&state, &before, "remove the first datum");
    assert!(state.undo().success);
    assert_eq!(datum_stations(&state), order, "back at its index");
}

// ===========================================================================
// Bug 6 (FIXED): a CreateShot permitted forward that could not be undone
// ===========================================================================

/// 0.6.0's undo of CreateShot ran the user's delete, guards and all, so a
/// shot inserted into a datum-anchored loop could not be undone and wedged
/// the history. A step applies the recorded changeset and consults no
/// user-intent guard.
#[test]
fn undo_of_create_shot_in_an_anchored_loop_succeeds() {
    let state = state_with_project();
    let (a, b) = two_merged_lines(&state);
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closed.success, "loop-closing closure failed: {:?}", closed.error);
    let before = snapshot(&state);
    let first_shot_of_a = shot_ids(&state, a)[0];
    let created = state.execute(Command::CreateShot {
        line_id: a,
        shot: shot(45.0, 3.0),
        after_shot_id: Some(first_shot_of_a),
        before_shot_id: None,
    });
    assert!(
        created.success,
        "setup: the forward CreateShot is permitted: {:?}",
        created.error
    );
    let undone = state.undo();
    assert!(
        undone.success,
        "undo of a PERMITTED CreateShot was refused ({:?}): the history is wedged",
        undone.error
    );
    assert_same(&snapshot(&state), &before, "exactly");
}

/// Taking the guards off the UNDO path left them on the USER's: a delete
/// of a shot on an anchored loop is refused, by the frozen check when the
/// closure is aligned (the loop is rigid), by the loop guard otherwise.
#[test]
fn the_delete_guards_still_refuse_a_user_delete() {
    for (strategy, expected) in [
        (
            ClosureStrategy::Move,
            "Cannot delete a frozen shot. This shot is part of an adjusted survey network.",
        ),
        (ClosureStrategy::Batch, "Use split instead."),
    ] {
        let state = state_with_project();
        let (a, b) = two_merged_lines(&state);
        let first_shot_of_a = shot_ids(&state, a)[0];
        let created = state.execute(Command::CreateShot {
            line_id: a,
            shot: shot(45.0, 3.0),
            after_shot_id: Some(first_shot_of_a),
            before_shot_id: None,
        });
        assert!(created.success, "setup ({strategy:?}): {:?}", created.error);
        let inserted = shot_ids(&state, a)[1];
        if strategy == ClosureStrategy::Move {
            // Snap the far end onto the datum station first, as the
            // frontend's snap-to-merge does before it sends `Move`.
            snap(&state, (b, 1), (a, 0));
        }
        let closed = state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(a, 0),
            station_b: StationRef::new(b, 1),
            closure_strategy: strategy,
        });
        assert!(closed.success, "setup ({strategy:?}): {:?}", closed.error);
        assert_eq!(
            closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
            Some(strategy == ClosureStrategy::Move),
            "setup ({strategy:?}): the row's premise is the closure's alignment"
        );
        let before = snapshot(&state);
        let deleted = state.execute(Command::DeleteShot {
            line_id: a,
            shot_id: inserted,
        });
        assert!(
            !deleted.success,
            "a USER delete of a shot on an anchored loop ({strategy:?}) was permitted"
        );
        let error = deleted.error.unwrap_or_default();
        assert!(
            error.contains(expected),
            "the {strategy:?} delete was refused with {error:?}, not {expected:?}"
        );
        assert_same(&snapshot(&state), &before, "a refusal changes nothing");
    }
}

// ===========================================================================
// Finding 10 (FIXED): a delete stranded a loop closure, and save + reopen
// was not state-preserving
// ===========================================================================

/// A - bridge - B with a closure from A to B: deleting the bridge leaves
/// the closure the sole bridge between a datum piece and a free one, which
/// the sweep promotes to a junction at once (plan 5.5). What the delete
/// leaves in memory is what the file gives back.
#[test]
fn save_and_reopen_preserves_an_orphan_loop_closure() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("orphan-closure.stalx");
    let before_save;
    {
        let state = state_with_project();
        let a = create_line(&state, "A", vec![shot(0.0, 5.0)]);
        let bridge = create_line(&state, "Bridge", vec![shot(90.0, 5.0)]);
        let b = create_line(&state, "B", vec![shot(180.0, 5.0)]);
        set_datum(&state, StationRef::new(a, 0), 10.0, 20.0);
        for (left, right) in [((a, 1), (bridge, 0)), ((bridge, 1), (b, 0))] {
            let merged = state.execute(Command::MergeStations {
                station_a: StationRef::new(left.0, left.1),
                station_b: StationRef::new(right.0, right.1),
            });
            assert!(merged.success, "MergeStations failed: {:?}", merged.error);
        }
        let closure = state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(a, 0),
            station_b: StationRef::new(b, 1),
            closure_strategy: ClosureStrategy::default(),
        });
        assert!(closure.success, "CreateLoopClosure failed: {:?}", closure.error);
        let undo_to = snapshot(&state);
        let bridge_shot = shot_ids(&state, bridge)[0];
        let deleted = state.execute_delete_last_shot_and_line(bridge, bridge_shot);
        assert!(deleted.success, "delete failed: {:?}", deleted.error);
        let p = project(&state);
        assert!(p.closures.is_empty(), "the orphaned closure was promoted");
        assert_eq!(
            station_at(&p, a, 0),
            station_at(&p, b, 1),
            "into a junction of its two ends"
        );
        assert!(p.validate().is_empty());
        before_save = text(&state);
        save(&state, &path);
        assert_round_trip(&state, &undo_to, "the delete and its promotion");
    }
    let reopened = reopen(&path);
    assert_eq!(
        text(&reopened),
        before_save,
        "save + reopen changed what the delete left"
    );
}

// ===========================================================================
// Finding 11 (FIXED): a save's heals were reverted by undoing an entry
// recorded before it
// ===========================================================================

/// 0.6.0's save healed the live project without an entry of its own
/// (uuids, the gauge pair, orphan offsets), so undoing an entry recorded
/// before the save reverted the heal, and the state at an undo depth
/// depended on the route taken to it. A save changes nothing now: the
/// survey after the save is the survey before it, and the undo lands on the
/// recorded state whichever route reaches that depth.
#[test]
fn undo_of_a_pre_save_entry_is_route_independent() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pre-save-entry.stalx");
    let state = state_with_project();
    let line_id = create_line(&state, "Gauged", vec![shot(0.0, 5.0), shot(90.0, 4.0)]);
    let depth_1 = snapshot(&state);
    let moved = state.execute(Command::MoveStation {
        station_ref: StationRef::new(line_id, 1),
        offset: Coordinates {
            x: 1.0,
            y: 2.0,
            z: 0.0,
        },
    });
    assert!(moved.success, "MoveStation failed: {:?}", moved.error);
    let depth_2 = snapshot(&state);
    save(&state, &path);
    assert_same(&snapshot(&state), &depth_2, "the save healed nothing");
    assert!(state.undo().success, "undo of MoveStation failed");
    assert_same(&snapshot(&state), &depth_1, "undo after a save");
    // The other route to the same depth: reopen the saved file and undo.
    let reopened = reopen(&path);
    assert!(reopened.undo().success);
    assert_eq!(text(&reopened), depth_1.0, "the same state by the other route");
}

// ===========================================================================
// Finding 20 (FIXED): a shot edit inside an unanchored merge left it apart
// ===========================================================================

/// Two lines joined at a junction and anchored by nothing. Returns `(a,
/// b)`: A has two shots, B one, A:1 is B:0.
fn two_unanchored_merged_lines(state: &AppState) -> (LineId, LineId) {
    let a = create_line(state, "A", vec![shot(0.0, 5.0), shot(0.0, 5.0)]);
    let b = create_line(state, "B", vec![shot(90.0, 5.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    assert!(gap(xy(state, a, 1), xy(state, b, 0)) < 1e-9);
    (a, b)
}

/// A merge is one station now, so its two slots cannot drift apart: a
/// shot edit in an unanchored group moves the junction and the line hanging
/// off it together, by the edit.
#[test]
fn a_shot_edit_keeps_an_unanchored_merge_aligned() {
    let state = state_with_project();
    let (a, b) = two_unanchored_merged_lines(&state);
    let b_end = xy(&state, b, 1);
    let first_shot = shot_ids(&state, a)[0];
    let before = snapshot(&state);
    set_length(&state, a, first_shot, 5.02);
    let apart = gap(xy(&state, a, 1), xy(&state, b, 0));
    assert!(apart <= 0.0, "the merged pair is {apart} m apart");
    let moved = gap(xy(&state, b, 1), b_end);
    assert!(
        (moved - 0.02).abs() < 1e-9,
        "the line off the junction moved with it, by the edit: {moved}"
    );
    assert_round_trip(&state, &before, "the edit");
}

/// The drift was a landmine for a later, unrelated command; a later command
/// on a third line runs cleanly and the survey validates.
#[test]
fn a_shot_edit_does_not_arm_an_alignment_assert_for_a_later_command() {
    let state = state_with_project();
    let (a, _b) = two_unanchored_merged_lines(&state);
    let elsewhere = create_line(&state, "Elsewhere", vec![shot(45.0, 5.0)]);
    let first_shot = shot_ids(&state, a)[0];
    set_length(&state, a, first_shot, 5.02);
    let anchor = shot_ids(&state, elsewhere)[0];
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.execute(Command::CreateShot {
            line_id: elsewhere,
            shot: shot(10.0, 3.0),
            after_shot_id: Some(anchor),
            before_shot_id: None,
        })
    }));
    let result = outcome.expect("an unrelated insert panicked");
    assert!(result.success, "{:?}", result.error);
    assert!(project(&state).validate().is_empty());
}

// ===========================================================================
// Finding 21 (FIXED): the adjusted geometry was not a function of the
// project
// ===========================================================================

fn adjusted_bits(state: &AppState) -> Vec<(String, [u64; 3])> {
    let mut rows: Vec<(String, [u64; 3])> = state
        .get_adjusted_stations()
        .expect("no adjusted stations")
        .iter()
        .map(|station| {
            (
                station.station_ref.to_string(),
                [
                    station.coordinates.x.to_bits(),
                    station.coordinates.y.to_bits(),
                    station.coordinates.z.to_bits(),
                ],
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Re-solving the same survey gives the same bits, every time (0.6.0
/// numbered its least-squares unknowns in `HashMap` order). Each re-solve
/// is forced by a new revision, so the memo cannot answer for it; the
/// project is measured not to move.
#[test]
fn the_adjusted_solve_is_a_function_of_the_project() {
    let state = state_with_project();
    let a = create_line(
        &state,
        "A",
        vec![shot(0.0, 5.0), shot(0.0, 5.0), shot(10.0, 4.0)],
    );
    let b = create_line(&state, "B", vec![shot(90.0, 5.0), shot(95.0, 6.0)]);
    set_datum(&state, StationRef::new(a, 0), 10.0, 20.0);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    // A least-squares closure, so the Computed View really solves.
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 2),
        closure_strategy: ClosureStrategy::LeastSquaresWeighted,
    });
    assert!(closure.success, "CreateLoopClosure failed: {:?}", closure.error);
    let survey_before = snapshot(&state);
    let base = adjusted_bits(&state);
    let raw: std::collections::HashMap<StationRef, (f64, f64)> =
        all_slot_xy(&state).into_iter().collect();
    let solved = state.get_adjusted_stations().unwrap().iter().any(|s| {
        gap(raw[&s.station_ref], (s.coordinates.x, s.coordinates.y)) > 1e-6
    });
    assert!(solved, "setup: the closure moves something, so a solve ran");
    let mut differing = 0;
    for _ in 0..64 {
        state.bump_revision();
        if adjusted_bits(&state) != base {
            differing += 1;
        }
    }
    assert_same(&snapshot(&state), &survey_before, "the survey did not move");
    assert_eq!(differing, 0, "{differing} of 64 re-solves differed");
}

// ===========================================================================
// Finding 22 (RESOLVED as intended): an atomic range delete keeps a
// cut-loose partner put
// ===========================================================================

/// A four-shot line merged at its far end to a one-shot line, anchored by
/// nothing. Returns `(a, b)`.
fn merged_chain_for_range_delete(state: &AppState) -> (LineId, LineId) {
    let a = create_line(
        state,
        "A",
        vec![
            shot(0.0, 5.0),
            shot(30.0, 6.0),
            shot(60.0, 7.0),
            shot(90.0, 8.0),
        ],
    );
    let b = create_line(state, "B", vec![shot(120.0, 5.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 4),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    (a, b)
}

fn line_xy(state: &AppState, line: LineId) -> Vec<(f64, f64)> {
    let n = project(state).line(line).unwrap().shots.len() as u32;
    (0..=n).map(|i| xy(state, line, i)).collect()
}

/// Both routes over `[lo..=hi]`, each on its own copy of ONE saved
/// [`merged_chain_for_range_delete`] (so the two start byte-identical, uuids
/// included): the atomic range delete, and the same shots deleted one
/// `DeleteShot` at a time.
fn both_routes_over(lo: usize, hi: usize) -> (AppState, AppState, LineId, LineId) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("chain.stalx");
    let source = state_with_project();
    let (a, b) = merged_chain_for_range_delete(&source);
    save(&source, &path);
    let (ranged, singles) = (reopen(&path), reopen(&path));
    assert_eq!(text(&ranged), text(&singles), "the twins start equal");
    let ids = shot_ids(&ranged, a);
    let result = ranged.execute_delete_shot_range_shift(a, ids[lo], ids[hi]);
    assert!(result.success, "range [{lo}..={hi}]: {:?}", result.error);
    for step in lo..=hi {
        let live = shot_ids(&singles, a);
        let single = singles.execute(Command::DeleteShot {
            line_id: a,
            shot_id: live[lo],
        });
        assert!(single.success, "single {step}: {:?}", single.error);
    }
    (ranged, singles, a, b)
}

/// The atomic range delete settles the anchors once, after the whole range
/// is gone, so a partner cut loose by the range stays where the surveyor
/// put it; the single deletes drag it while the junction is still on the
/// line (the first delete closes the gap under it) and then leave it there.
/// On every range that removes no junction the two routes agree.
#[test]
fn a_doomed_anchor_range_shift_keeps_the_partner_in_place() {
    // Half 1: the suffix range [2..=3] takes A:3 and the junction A:4.
    let reference = state_with_project();
    let (_, b) = merged_chain_for_range_delete(&reference);
    let b_before = line_xy(&reference, b);
    let (ranged, singles, a, b2) = both_routes_over(2, 3);
    assert_eq!(b2, b);
    let p = project(&ranged);
    assert!(
        !p.topology().is_junction(station_at(&p, b, 0)),
        "the range cut B loose"
    );
    assert!(p.datums.is_empty(), "B is held by nothing");
    let b_ranged = line_xy(&ranged, b);
    for (now, was) in b_ranged.iter().zip(&b_before) {
        assert!(
            gap(*now, *was) <= 1e-9,
            "the range delete moved the cut-loose partner: {was:?} -> {now:?}"
        );
    }
    // The singles: the first delete closes shot 2's gap (7 m on 60 degrees)
    // under the junction, and B rides along.
    let shift = (
        -7.0 * 60f64.to_radians().sin(),
        -7.0 * 60f64.to_radians().cos(),
    );
    let b_singles = line_xy(&singles, b);
    for (now, was) in b_singles.iter().zip(&b_before) {
        let moved = (now.0 - was.0, now.1 - was.1);
        assert!(
            gap(moved, shift) <= 1e-9,
            "the single deletes were expected to strand B by {shift:?}, moved it {moved:?}"
        );
    }
    let _ = a;
    // Half 2: a prefix and an interior range remove no junction, and there
    // the two routes agree to the last bit.
    for (label, lo, hi) in [("prefix", 0, 1), ("interior", 1, 2)] {
        let (ranged, singles, a, b) = both_routes_over(lo, hi);
        assert_eq!(
            text(&ranged),
            text(&singles),
            "a {label} range [{lo}..={hi}] leaves the same survey both ways"
        );
        for line in [a, b] {
            assert_eq!(
                line_xy(&ranged, line),
                line_xy(&singles, line),
                "a {label} range put line {line} in two places"
            );
        }
    }
}

// ===========================================================================
// Finding 24 (FIXED): describing a command with an out-of-range shot id
// overflowed
// ===========================================================================

/// The label of a command is worded before it runs; a shot id no line
/// holds must be refused, not overflow the display-position arithmetic.
#[test]
fn describing_a_command_with_an_out_of_range_shot_id_does_not_overflow() {
    let state = state_with_project();
    let line_id = create_line(&state, "A", vec![shot(0.0, 5.0)]);
    let before = snapshot(&state);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.execute(Command::SetShotKind {
            line_id,
            shot_id: ShotId::MAX,
            kind: ShotKind::Jump,
        })
    }));
    let result = outcome.expect("SetShotKind with an out-of-range id panicked");
    assert!(!result.success, "a shot id no line holds is refused");
    assert_same(&snapshot(&state), &before, "and nothing changed");
    let r = state.execute(Command::UpdateShot {
        line_id: u32::MAX,
        shot_id: ShotId::MAX,
        updates: ShotUpdates::default(),
    });
    assert!(!r.success);
}

// ===========================================================================
// Finding 25 / #469 item 1 (FIXED): removing the primary datum moved the
// frame and nothing re-anchored
// ===========================================================================

const ORIGIN_GPS: (f64, f64) = (20.4, -87.3);
/// About 6.4 km from [`ORIGIN_GPS`].
const FAR_GPS: (f64, f64) = (20.4358, -87.3479);
/// About 400 m from [`ORIGIN_GPS`].
const NEAR_GPS: (f64, f64) = (20.4027, -87.2973);

/// Origin (two shots, the first datum on its station 0) and Anchored (two
/// shots, a datum `second` on its station 1); with `floater`, a third,
/// free line.
fn two_datum_survey(state: &AppState, second: (f64, f64), floater: bool) -> (LineId, LineId) {
    let origin = create_line(state, "Origin", vec![shot(10.0, 3.0), shot(20.0, 4.0)]);
    let anchored = create_line(state, "Anchored", vec![shot(70.0, 7.0), shot(100.0, 8.0)]);
    if floater {
        create_line(state, "Floater", vec![shot(300.0, 12.0)]);
    }
    set_datum(state, StationRef::new(origin, 0), ORIGIN_GPS.0, ORIGIN_GPS.1);
    set_datum(state, StationRef::new(anchored, 1), second.0, second.1);
    (origin, anchored)
}

fn assert_at_origin(state: &AppState, line: LineId, index: u32, what: &str) {
    let at = xy(state, line, index);
    assert!(
        at.0.abs() <= 1e-9 && at.1.abs() <= 1e-9,
        "{what}: the frame datum's station sits at local (0, 0), got {at:?}"
    );
}

/// 2 m more on the shot upstream of Anchored's datum station: the datum
/// station holds still (to the bit), and the line's start moves by the
/// edit's own 2 m.
fn assert_edit_holds_the_anchor(state: &AppState, anchored: LineId) {
    let anchor = xy(state, anchored, 1);
    let start = xy(state, anchored, 0);
    let target = shot_ids(state, anchored)[0];
    set_length(state, anchored, target, 9.0);
    let now = xy(state, anchored, 1);
    assert!(
        gap(now, anchor) <= 1e-9,
        "an UpdateShot moved the datum's own station {:.3} m",
        gap(now, anchor)
    );
    let slid = gap(xy(state, anchored, 0), start);
    assert!(
        (slid - 2.0).abs() <= 1e-9,
        "the line start moved {slid:.3} m for a 2 m lengthening upstream of its datum"
    );
}

#[test]
fn removing_the_primary_zero_datum_re_anchors_the_datums_it_leaves_behind() {
    let state = state_with_project();
    let (origin, anchored) = two_datum_survey(&state, FAR_GPS, false);
    assert!(
        gap(xy(&state, anchored, 1), (0.0, 0.0)) > 1000.0,
        "setup: the second datum sits kilometres from the first"
    );
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(origin, 0),
    });
    assert!(removed.success, "RemoveZeroDatum failed: {:?}", removed.error);
    assert_at_origin(&state, anchored, 1, "after the removal");
    assert_edit_holds_the_anchor(&state, anchored);
}

#[test]
fn undoing_the_edit_that_re_anchored_a_component_puts_it_back() {
    let state = state_with_project();
    let (origin, anchored) = two_datum_survey(&state, FAR_GPS, false);
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(origin, 0),
    });
    assert!(removed.success);
    assert_at_origin(&state, anchored, 1, "after the removal");
    let before = snapshot(&state);
    let target = shot_ids(&state, anchored)[0];
    set_length(&state, anchored, target, 9.0);
    assert_round_trip(&state, &before, "the edit after the promotion");
}

/// Nobody removed a datum: a delete of the shot whose station held the
/// primary datum promotes the next one, by a path that never mentions
/// datums.
#[test]
fn deleting_the_primary_datums_station_does_not_silently_move_the_origin() {
    let state = state_with_project();
    let (origin, anchored) = two_datum_survey(&state, FAR_GPS, false);
    let first_shot = shot_ids(&state, origin)[0];
    let before = snapshot(&state);
    let deleted = state.execute(Command::DeleteShot {
        line_id: origin,
        shot_id: first_shot,
    });
    assert!(deleted.success, "DeleteShot failed: {:?}", deleted.error);
    assert_eq!(project(&state).datums.len(), 1, "the datum left with its station");
    assert_at_origin(&state, anchored, 1, "after the delete");
    assert_round_trip(&state, &before, "the delete");
    assert_edit_holds_the_anchor(&state, anchored);
}

/// THE property: local (0, 0) is the frame datum's GPS, so when the frame
/// datum goes and the next is promoted, the survey is translated onto the
/// new origin, and no station moves in the WORLD (measured in the frame the
/// removed datum defined), the free line included.
#[test]
fn promoting_a_zero_datum_keeps_every_station_where_it_is_in_the_world() {
    let state = state_with_project();
    let (origin, anchored) = two_datum_survey(&state, NEAR_GPS, true);
    let before = all_slot_xy(&state);
    assert!(gap(xy(&state, anchored, 1), (0.0, 0.0)) > 100.0, "setup");
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(origin, 0),
    });
    assert!(removed.success, "RemoveZeroDatum failed: {:?}", removed.error);
    assert_at_origin(&state, anchored, 1, "the promoted datum");
    let p = project(&state);
    let frame = p.frame_datum().expect("a frame datum");
    let gps = p.datums[frame].gps;
    assert_eq!((gps.latitude, gps.longitude), NEAR_GPS, "the second was promoted");
    let shift = gps_to_local(
        &GpsCoordinates::new(NEAR_GPS.0, NEAR_GPS.1, 0.0).unwrap(),
        &GpsCoordinates::new(ORIGIN_GPS.0, ORIGIN_GPS.1, 0.0).unwrap(),
    );
    let after: std::collections::HashMap<StationRef, (f64, f64)> =
        all_slot_xy(&state).into_iter().collect();
    for (slot, world_before) in before {
        let local = after[&slot];
        let world_after = (shift.0 + local.0, shift.1 + local.1);
        assert!(
            gap(world_after, world_before) <= 1e-2,
            "slot {slot:?} moved {:.4} m in the world when the frame datum was removed",
            gap(world_after, world_before)
        );
    }
}

/// The undo half: the removal translated every piece (placements and
/// corrections), and one undo and one redo hand all of it back, to the bit.
#[test]
fn undoing_the_primary_datum_removal_restores_every_placement() {
    let state = state_with_project();
    let (origin, _) = two_datum_survey(&state, NEAR_GPS, true);
    let before = snapshot(&state);
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(origin, 0),
    });
    assert!(removed.success);
    assert_ne!(snapshot(&state).0, before.0, "setup: the removal moved things");
    assert_round_trip(&state, &before, "remove the frame datum");
}

/// 0.6.0 pinned that `ZeroDatumInfo` captured every line's offset, not the
/// datum's merge group, because the first datum re-frames every piece. On
/// the station graph that is the placement list: adding the first datum
/// translates every free piece, and the entry records each of their
/// placements before and after.
#[test]
fn the_first_datum_entry_records_every_piece_it_re_frames() {
    let state = state_with_project();
    let anchored = create_line(&state, "Anchored", vec![shot(10.0, 3.0), shot(20.0, 4.0)]);
    let one = create_line(&state, "One", vec![shot(70.0, 7.0)]);
    let two = create_line(&state, "Two", vec![shot(300.0, 12.0)]);
    let before = snapshot(&state);
    let placements = project(&state).placements;
    assert_eq!(placements.len(), 3, "setup: three free pieces");
    set_datum(&state, StationRef::new(anchored, 1), ORIGIN_GPS.0, ORIGIN_GPS.1);
    assert_at_origin(&state, anchored, 1, "the first datum");
    let digest = state
        .undo_stack
        .lock()
        .unwrap()
        .peek_undo()
        .unwrap()
        .entry()
        .changes
        .digest();
    let p = project(&state);
    for line in [one, two] {
        let station = p.line(line).unwrap().start;
        let old = placements.iter().find(|pl| pl.station == station).unwrap();
        let new = p.placements.iter().find(|pl| pl.station == station).unwrap();
        assert_ne!((old.x, old.y), (new.x, new.y), "premise: line {line} moved");
        assert!(digest.placements.removed.contains(old), "the old placement");
        assert!(digest.placements.added.contains(new), "the new placement");
    }
    assert_round_trip(&state, &before, "the first datum");
}

/// Adding the survey's first datum re-frames every piece, a free line
/// outside the datum's group included; undo and redo land exactly.
#[test]
fn adding_the_first_zero_datum_and_undoing_it_restores_every_placement() {
    let state = state_with_project();
    let anchored = create_line(&state, "Anchored", vec![shot(10.0, 3.0), shot(20.0, 4.0)]);
    let partner = create_line(&state, "Partner", vec![shot(70.0, 7.0), shot(100.0, 8.0)]);
    let floater = create_line(&state, "Floater", vec![shot(300.0, 12.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(anchored, 2),
        station_b: StationRef::new(partner, 0),
    });
    assert!(merged.success);
    let moved = state.execute(Command::MoveLine {
        line_id: anchored,
        offset: Coordinates {
            x: 13.0,
            y: -7.0,
            z: 0.0,
        },
    });
    assert!(moved.success, "MoveLine failed: {:?}", moved.error);
    let before = snapshot(&state);
    let datum_before = xy(&state, anchored, 1);
    let floater_before = xy(&state, floater, 0);
    assert!(gap(datum_before, (0.0, 0.0)) > 1.0, "setup: off the origin");
    set_datum(&state, StationRef::new(anchored, 1), ORIGIN_GPS.0, ORIGIN_GPS.1);
    assert_at_origin(&state, anchored, 1, "the first datum");
    let floater_after = xy(&state, floater, 0);
    let expected = (
        floater_before.0 - datum_before.0,
        floater_before.1 - datum_before.1,
    );
    assert!(
        gap(floater_after, expected) <= 1e-9,
        "the free line rides the establishing translation: {floater_before:?} -> \
         {floater_after:?}, expected {expected:?}"
    );
    assert_round_trip(&state, &before, "the first datum");
    let p = project(&state);
    assert_eq!(p.datums[0].station, station_at(&p, anchored, 1));
}

#[test]
fn deleting_the_primary_datums_line_renormalises_the_frame() {
    let state = state_with_project();
    let (origin, anchored) = two_datum_survey(&state, NEAR_GPS, false);
    let before = snapshot(&state);
    let deleted = state.execute(Command::DeleteLine { line_id: origin });
    assert!(deleted.success, "DeleteLine failed: {:?}", deleted.error);
    assert_at_origin(&state, anchored, 1, "after the line's delete");
    assert_round_trip(&state, &before, "the delete puts the old frame back");
}

#[test]
fn the_delete_last_shot_path_renormalises_the_frame() {
    let state = state_with_project();
    let origin = create_line(&state, "Origin", vec![shot(10.0, 3.0)]);
    let anchored = create_line(&state, "Anchored", vec![shot(70.0, 7.0), shot(100.0, 8.0)]);
    set_datum(&state, StationRef::new(origin, 0), ORIGIN_GPS.0, ORIGIN_GPS.1);
    set_datum(&state, StationRef::new(anchored, 1), NEAR_GPS.0, NEAR_GPS.1);
    let before = snapshot(&state);
    let only_shot = shot_ids(&state, origin)[0];
    let deleted = state.execute_delete_last_shot_and_line(origin, only_shot);
    assert!(deleted.success, "delete failed: {:?}", deleted.error);
    assert_at_origin(&state, anchored, 1, "after the delete");
    assert_round_trip(&state, &before, "the delete puts the old frame back");
}

#[test]
fn the_renormalised_frame_survives_save_and_reopen() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("renormalised-frame.stalx");
    let (saved, anchored);
    {
        let state = state_with_project();
        let (origin, a) = two_datum_survey(&state, NEAR_GPS, true);
        anchored = a;
        let removed = state.execute(Command::RemoveZeroDatum {
            station_ref: StationRef::new(origin, 0),
        });
        assert!(removed.success);
        save(&state, &path);
        saved = text(&state);
    }
    let reopened = reopen(&path);
    assert_eq!(text(&reopened), saved, "the loader is no second solve");
    assert_at_origin(&reopened, anchored, 1, "after the reopen");
}

// ===========================================================================
// #469 item 2 (FIXED): a closure ASSERTED that its ends coincided
// ===========================================================================
//
// Alignment is derived now (`Project::closure_alignment` on the raw layout:
// a `Move` closure within 1 cm that would not join two datum-held pieces),
// never stored, so it follows the geometry both ways and no open can
// re-assert it.

fn closure_alignment(state: &AppState, a: StationRef, b: StationRef) -> Option<bool> {
    let p = project(state);
    let (sa, sb) = (
        p.topology().station_at(a)?,
        p.topology().station_at(b)?,
    );
    let index = p
        .closures
        .iter()
        .position(|c| (c.a == sa && c.b == sb) || (c.a == sb && c.b == sa))?;
    Some(p.closure_alignment(&p.raw_positions())[index])
}

/// Move the station at `from` onto the station at `to` (the snap-to-merge
/// move the frontend sends before a `Move` closure).
fn snap(state: &AppState, from: (LineId, u32), to: (LineId, u32)) {
    let (f, t) = (xy(state, from.0, from.1), xy(state, to.0, to.1));
    let snapped = state.execute(Command::MoveStation {
        station_ref: StationRef::new(from.0, from.1),
        offset: Coordinates {
            x: t.0 - f.0,
            y: t.1 - f.1,
            z: 0.0,
        },
    });
    assert!(snapped.success, "snap failed: {:?}", snapped.error);
}

/// B's far end snapped onto A's first station and a `Move` closure over the
/// two: what the snap-to-merge workflow produces.
fn snapped_loop(state: &AppState, anchored: bool) -> (LineId, LineId) {
    let (a, b) = if anchored {
        two_merged_lines(state)
    } else {
        two_unanchored_merged_lines(state)
    };
    snap(state, (b, 1), (a, 0));
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::Move,
    });
    assert!(closed.success, "CreateLoopClosure failed: {:?}", closed.error);
    (a, b)
}

#[test]
fn a_move_closure_over_a_real_gap_is_not_born_aligned() {
    let state = state_with_project();
    let (a, b) = two_merged_lines(&state);
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::Move,
    });
    assert!(closed.success, "CreateLoopClosure failed: {:?}", closed.error);
    let apart = gap(xy(&state, a, 0), xy(&state, b, 1));
    assert!(apart > 1.0, "setup: the ends are metres apart ({apart} m)");
    assert_eq!(
        closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
        Some(false),
        "a closure cannot claim a coincidence the survey refutes"
    );
    let p = project(&state);
    let metrics = p.closures_metrics(&p.raw_positions());
    let misclosure = metrics[0].as_ref().expect("metrics").misclosure_distance;
    assert!(
        (misclosure - apart).abs() < 1e-6,
        "the distance is the misclosure the user reads ({misclosure} vs {apart})"
    );
    assert!(p.validate().is_empty(), "and the survey is sound");
}

#[test]
fn the_snap_to_merge_batch_still_creates_an_aligned_closure() {
    let state = state_with_project();
    let (a, b) = two_merged_lines(&state);
    let (f, t) = (xy(&state, b, 1), xy(&state, a, 0));
    let batched = state.execute(Command::Batch {
        commands: vec![
            Command::MoveStation {
                station_ref: StationRef::new(b, 1),
                offset: Coordinates {
                    x: t.0 - f.0,
                    y: t.1 - f.1,
                    z: 0.0,
                },
            },
            Command::CreateLoopClosure {
                station_a: StationRef::new(a, 0),
                station_b: StationRef::new(b, 1),
                closure_strategy: ClosureStrategy::Move,
            },
        ],
        label: None,
    });
    assert!(batched.success, "snap-to-merge batch failed: {:?}", batched.error);
    assert!(gap(xy(&state, a, 0), xy(&state, b, 1)) < 1e-6);
    assert_eq!(
        closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
        Some(true),
        "a measured coincidence is an alignment"
    );
}

/// Tearing an aligned closure (a narrow shot edit on an unanchored loop, a
/// station move on an anchored one) makes it unaligned, and the undo
/// restores the geometry and with it the alignment.
#[test]
fn tearing_an_aligned_closure_demotes_it_and_the_undo_restores_the_claim() {
    for narrow in [false, true] {
        let state = state_with_project();
        let (a, b) = snapped_loop(&state, !narrow);
        let closed = all_slot_xy(&state);
        let before = snapshot(&state);
        let ends = (StationRef::new(a, 0), StationRef::new(b, 1));
        assert_eq!(closure_alignment(&state, ends.0, ends.1), Some(true), "setup");
        let torn = if narrow {
            state.execute(Command::UpdateShot {
                line_id: b,
                shot_id: shot_ids(&state, b)[0],
                updates: ShotUpdates {
                    length: Some(105.0),
                    ..Default::default()
                },
            })
        } else {
            state.execute(Command::MoveStation {
                station_ref: StationRef::new(b, 1),
                offset: Coordinates {
                    x: 100.0,
                    y: 0.0,
                    z: 0.0,
                },
            })
        };
        assert!(torn.success, "narrow={narrow}: {:?}", torn.error);
        assert!(gap(xy(&state, a, 0), xy(&state, b, 1)) > 1.0, "torn open");
        assert_eq!(closure_alignment(&state, ends.0, ends.1), Some(false));
        assert!(project(&state).validate().is_empty());
        assert!(state.undo().success, "narrow={narrow}: undo");
        assert_eq!(all_slot_xy(&state), closed, "the geometry is back");
        assert_eq!(
            closure_alignment(&state, ends.0, ends.1),
            Some(true),
            "and the claim with it (narrow={narrow})"
        );
        assert_same(&snapshot(&state), &before, "exactly");
    }
}

#[test]
fn drifting_back_into_coincidence_re_promotes_the_closure() {
    let state = state_with_project();
    let (a, b) = snapped_loop(&state, true);
    for (offset, expected) in [(100.0, false), (-100.0, true)] {
        let moved = state.execute(Command::MoveStation {
            station_ref: StationRef::new(b, 1),
            offset: Coordinates {
                x: offset,
                y: 0.0,
                z: 0.0,
            },
        });
        assert!(moved.success, "MoveStation({offset}) failed: {:?}", moved.error);
        assert_eq!(
            closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
            Some(expected),
            "after moving by {offset} m the gap is {} m",
            gap(xy(&state, a, 0), xy(&state, b, 1))
        );
    }
}

/// Two GPS-anchored lines whose stations coincide: aligning the closure
/// would weld two datum-held pieces into one rigid group, so it is never
/// aligned, before or after a later command.
#[test]
fn a_coincident_closure_between_two_anchored_groups_is_never_aligned() {
    let state = state_with_project();
    let a = create_line(&state, "A", vec![shot(0.0, 5.0)]);
    let c = create_line(&state, "C", vec![shot(90.0, 5.0)]);
    for line in [a, c] {
        set_datum(&state, StationRef::new(line, 0), ORIGIN_GPS.0, ORIGIN_GPS.1);
    }
    assert!(gap(xy(&state, a, 0), xy(&state, c, 0)) < 1e-6, "setup: one point");
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(c, 0),
        closure_strategy: ClosureStrategy::Move,
    });
    assert!(closed.success, "CreateLoopClosure failed: {:?}", closed.error);
    let ends = (StationRef::new(a, 0), StationRef::new(c, 0));
    assert_eq!(closure_alignment(&state, ends.0, ends.1), Some(false));
    let commented = state.execute(Command::SetStationComment {
        station_ref: StationRef::new(c, 1),
        comment: Some("a later edit".to_string()),
    });
    assert!(commented.success);
    assert_eq!(closure_alignment(&state, ends.0, ends.1), Some(false));
}

#[test]
fn a_demoted_alignment_claim_survives_save_and_reopen() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("demoted-claim.stalx");
    let (a, b, saved);
    {
        let state = state_with_project();
        (a, b) = snapped_loop(&state, true);
        let torn = state.execute(Command::MoveStation {
            station_ref: StationRef::new(b, 1),
            offset: Coordinates {
                x: 100.0,
                y: 0.0,
                z: 0.0,
            },
        });
        assert!(torn.success);
        assert_eq!(
            closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
            Some(false)
        );
        save(&state, &path);
        saved = text(&state);
    }
    let reopened = reopen(&path);
    assert_eq!(text(&reopened), saved);
    assert_eq!(
        closure_alignment(&reopened, StationRef::new(a, 0), StationRef::new(b, 1)),
        Some(false),
        "the open cannot re-assert the claim"
    );
    assert!(project(&reopened).validate().is_empty());
}

// ===========================================================================
// Finding 5 on the command side (FIXED): a reloaded clear of a line field
// redid as "leave it alone"
// ===========================================================================

/// A line's surveyor (its shots' credit), rendered so that a cleared field
/// and a missing line cannot read as the same thing.
fn surveyor_of(state: &AppState, line_id: LineId) -> String {
    let p = project(state);
    let line = p.line(line_id).expect("line missing");
    line.shots
        .first()
        .and_then(|s| s.credit.as_deref())
        .map(|c| {
            c.surveyors
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|names| !names.is_empty())
        .or_else(|| line.legacy_surveyor.clone())
        .unwrap_or_else(|| CLEARED.to_string())
}

const CLEARED: &str = "<cleared>";

fn surveyed_line(state: &AppState) -> LineId {
    let created = state.execute(Command::CreateLine {
        name: Some("Sump".to_string()),
        surveyor: Some("Ferreira".to_string()),
        date: None,
        color: "#3388FF".to_string(),
        direction: None,
        shots: vec![shot(0.0, 5.0)],
        cave_section_id: None,
    });
    assert!(created.success, "CreateLine failed: {:?}", created.error);
    created.line_id.unwrap()
}

fn clear_surveyor(state: &AppState, line_id: LineId) {
    let cleared = state.execute(Command::UpdateLineMetadata {
        line_id,
        updates: LineUpdates {
            surveyor: Some(None),
            ..Default::default()
        },
    });
    assert!(cleared.success, "clearing failed: {:?}", cleared.error);
    assert_eq!(surveyor_of(state, line_id), CLEARED, "setup: cleared");
}

#[test]
fn redo_after_reload_clears_the_line_field_it_cleared() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("cleared-field.stalx");
    let line_id;
    {
        let state = state_with_project();
        line_id = surveyed_line(&state);
        clear_surveyor(&state, line_id);
        save(&state, &path);
    }
    let reopened = reopen(&path);
    assert!(reopened.undo().success, "undo after reopen failed");
    assert_eq!(surveyor_of(&reopened, line_id), "Ferreira");
    assert!(reopened.redo().success, "redo after reopen failed");
    assert_eq!(
        surveyor_of(&reopened, line_id),
        CLEARED,
        "the redo of a reloaded clear left the surveyor in place"
    );
}

/// The newest undo entry's preview rows, by field label and new value.
fn preview_rows(state: &AppState) -> Vec<(String, String)> {
    state
        .get_undo_stack()
        .first()
        .and_then(|item| item.preview_data.as_ref())
        .map(|preview| {
            preview
                .changes
                .iter()
                .map(|change| (change.field.clone(), change.new_value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The history preview of a clear lists the cleared field, before and after
/// a reopen.
#[test]
fn the_history_preview_still_reports_a_cleared_field() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("cleared-preview.stalx");
    let before;
    {
        let state = state_with_project();
        let line_id = surveyed_line(&state);
        clear_surveyor(&state, line_id);
        before = preview_rows(&state);
        assert!(
            before.iter().any(|(field, _)| field == "Surveyor"),
            "setup: the preview lists the Surveyor row: {before:?}"
        );
        save(&state, &path);
    }
    let reopened = reopen(&path);
    assert_eq!(
        preview_rows(&reopened),
        before,
        "the reloaded entry's preview no longer lists the field it cleared"
    );
}

// ===========================================================================
// Finding 23 (FIXED): an f64 that the history wrote and could not read back
// ===========================================================================

/// The bits ryu renders as `13.565000000000001` sit on a two-decimal
/// rendering boundary: a reopened entry must show the length it was given.
#[test]
fn a_reloaded_entry_displays_the_length_it_was_given() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("ulp.stalx");
    let length = f64::from_bits(0x402b_2147_ae14_7ae2);
    let (before, line_id);
    {
        let state = state_with_project();
        line_id = create_line(&state, "Traverse", vec![shot(0.0, 5.0)]);
        let shot_id = shot_ids(&state, line_id)[0];
        set_length(&state, line_id, shot_id, length);
        before = preview_rows(&state);
        assert!(
            before.iter().any(|(_, new)| new == "13.57"),
            "setup: the preview renders 13.57: {before:?}"
        );
        save(&state, &path);
    }
    let reopened = reopen(&path);
    assert_eq!(preview_rows(&reopened), before);
    assert_eq!(
        shot_lengths(&reopened, line_id)[0].to_bits(),
        length.to_bits(),
        "the survey keeps the bits too"
    );
    assert!(reopened.undo().success);
    assert!(reopened.redo().success);
    assert_eq!(shot_lengths(&reopened, line_id)[0].to_bits(), length.to_bits());
}

// ===========================================================================
// The orphan-closure observation (FIXED): a datum removal made a closure a
// sole bridge, and the next load reclassified it
// ===========================================================================

/// Two pieces with no path between them, each with its own datum, and a
/// closure bridging them (allowed only because both are datum-held).
fn detached_anchored_closure() -> (AppState, LineId, LineId) {
    let state = state_with_project();
    let a = create_line(&state, "A", vec![shot(0.0, 5.0)]);
    let b = create_line(&state, "B", vec![shot(90.0, 5.0)]);
    for (line, latitude) in [(a, 10.0), (b, 11.0)] {
        set_datum(&state, StationRef::new(line, 0), latitude, 20.0);
    }
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closure.success, "CreateLoopClosure failed: {:?}", closure.error);
    (state, a, b)
}

/// Removing the second datum leaves the closure between a datum piece and
/// a free one, which the sweep promotes to a junction in session (plan
/// 5.5); undo puts the closure AND the datum back (at its index), redo
/// promotes again.
#[test]
fn removing_a_zero_datum_converts_the_closure_it_makes_convertible() {
    let (state, a, b) = detached_anchored_closure();
    assert_eq!(project(&state).closures.len(), 1, "setup: a closure");
    let datums = datum_stations(&state);
    let before = snapshot(&state);
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(b, 0),
    });
    assert!(removed.success, "RemoveZeroDatum failed: {:?}", removed.error);
    let p = project(&state);
    assert!(p.closures.is_empty(), "the sole bridge was promoted");
    assert_eq!(station_at(&p, a, 1), station_at(&p, b, 1), "to a junction");
    let after = snapshot(&state);
    assert!(state.undo().success, "undo of RemoveZeroDatum failed");
    assert_same(&snapshot(&state), &before, "closure and datum back");
    assert_eq!(datum_stations(&state), datums, "the datum at its index");
    assert!(state.redo().success);
    assert_same(&snapshot(&state), &after, "the redo promotes again");
}

#[test]
fn removing_a_zero_datum_leaves_nothing_for_the_next_load_to_reclassify() {
    let temp = TempDir::new().unwrap();
    let path: PathBuf = temp.path().join("converged-orphan.stalx");
    let saved;
    {
        let (state, _a, b) = detached_anchored_closure();
        let removed = state.execute(Command::RemoveZeroDatum {
            station_ref: StationRef::new(b, 0),
        });
        assert!(removed.success);
        save(&state, &path);
        saved = text(&state);
    }
    let reopened = reopen(&path);
    assert_eq!(text(&reopened), saved, "the file gives back what the session held");
}

/// The state after [`detached_anchored_closure`] lost its second datum: the
/// closure promoted, the survey converged with what a load reads.
fn converged_orphan_state() -> AppState {
    let (state, _a, b) = detached_anchored_closure();
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(b, 0),
    });
    assert!(removed.success);
    assert!(project(&state).closures.is_empty(), "setup: promoted");
    state
}

/// The undo of a shot appended into an empty line lands on the recorded
/// state, and changes nothing about closures it did not touch.
#[test]
fn undo_of_an_appended_first_shot_restores_the_recorded_closure_state() {
    let state = converged_orphan_state();
    let empty = create_line(&state, "Elsewhere", vec![]);
    let before = snapshot(&state);
    let created = state.execute(Command::CreateShot {
        line_id: empty,
        shot: shot(180.0, 4.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    assert_round_trip(&state, &before, "append the first shot");
}

#[test]
fn undo_of_an_appended_shot_restores_the_recorded_closure_state() {
    let state = converged_orphan_state();
    let elsewhere = create_line(&state, "Elsewhere", vec![shot(270.0, 6.0)]);
    let first = shot_ids(&state, elsewhere)[0];
    let before = snapshot(&state);
    let created = state.execute(Command::CreateShot {
        line_id: elsewhere,
        shot: shot(180.0, 4.0),
        after_shot_id: Some(first),
        before_shot_id: None,
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    assert_round_trip(&state, &before, "append a shot");
}

// `a_legacy_remove_zero_datum_entry_still_restores_the_datum_at_its_index`:
// RETIRED. It pinned the undo arm of a persisted 0.6.0 `ZeroDatumInfo`
// entry. Histories written before the changeset engine are dropped whole on
// open with the earlier-version banner (plan 7.2.8; `undo_after_reopen_tests`
// pins the drop), so no such entry can reach a step; the datum-order half
// is `undo_of_remove_zero_datum_restores_the_original_order` above.

// ===========================================================================
// #557 (FIXED): a crafted persisted history could construct an over-bound
// line
// ===========================================================================

/// A file-supplied history entry naming a line id past `MAX_LINE_ID` (or a
/// shot index past the line cap) is refused by the entry decoder, never
/// constructed: the open drops the history whole with the banner and the
/// survey opens as it was. Covers 0.6.0's `restore_line` and
/// `restore_snapshot_into` refusals (both were doors into the same
/// constructor; the decoder is the one door now).
#[test]
fn a_crafted_history_with_an_over_bound_line_id_is_refused() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("hostile.stalx");
    let saved;
    {
        let state = state_with_project();
        create_line(&state, "Line", vec![shot(0.0, 5.0)]);
        save(&state, &path);
        saved = text(&state);
    }
    let decoded =
        stalx_v2::decode(&std::fs::read(&path).unwrap(), &DecodeOptions::default()).unwrap();
    let mut extras = decoded.extras;
    let entry = extras.history.undo.pop().expect("the create's entry");
    let line_id = project(&reopen(&path)).lines()[0].id;
    let hostile_id = crate::models::MAX_LINE_ID + 1;
    let needle = format!("\"id\":{line_id}");
    assert!(entry.as_str().contains(&needle), "setup: {}", entry.as_str());
    let hostile_text = entry.as_str().replace(&needle, &format!("\"id\":{hostile_id}"));
    assert!(
        decode_entry(&hostile_text, &EntryLimits::default()).is_err(),
        "the decoder refuses an over-bound line id"
    );
    extras.history.undo.push(RawJson::parse(&hostile_text).unwrap());
    std::fs::write(
        &path,
        stalx_v2::encode(&decoded.project, &extras, stalx_v2::Mode::Normal).unwrap(),
    )
    .unwrap();
    let state = reopen(&path);
    assert!(state.history_discarded(), "the history is dropped with the banner");
    assert!(state.get_undo_stack().is_empty());
    assert_eq!(text(&state), saved, "the survey opens as saved");
    assert!(project(&state)
        .lines()
        .iter()
        .all(|l| l.id <= crate::models::MAX_LINE_ID));
}

// `a_snapshot_restore_with_a_hostile_shot_counter_is_refused`: RETIRED. A
// line's shot-id allocator (`next_shot_id`) is a session value now (plan
// 1.1, U8): it is never persisted, so no file can supply a hostile one, and
// a decoded entry mints ids inside the line's window (the engine's
// `session_ids_sound` property, `changeset/property_tests.rs`).

// `undo_of_a_snapshot_entry_keeps_each_lines_stored_created_at`: RETIRED. It
// pinned the per-line creation time 0.6.0 kept in the SQLite `sections`
// table (`Line::loaded_created_at`). A v2 line has no creation time (the
// project's `created_at` is the only one, file format 5.9), so there is
// nothing for a step to lose.

/// Every case above that compares a [`Snapshot`] ends on the same oracle.
#[test]
fn the_snapshot_oracle_sees_a_change_of_one_bit() {
    let state = state_with_project();
    let line_id = create_line(&state, "Bits", vec![shot(0.0, 5.0)]);
    let before: Snapshot = snapshot(&state);
    let shot_id = shot_ids(&state, line_id)[0];
    set_length(&state, line_id, shot_id, f64::from_bits(5.0f64.to_bits() + 1));
    assert_ne!(snapshot(&state).0, before.0, "one ulp is a different survey");
}
