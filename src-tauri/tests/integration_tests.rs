//! Comprehensive integration tests for Stalagmight
//!
//! Tests are organized by category. Run `cargo test` to execute all tests.
//! The survey is the station-graph model (`models::survey`): stations are
//! entities, a merge is one shared station, positions are derived, and a
//! project is saved as a `.stalx` version 2 file.
#![expect(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use stalagmight_lib::config::{CacheConfig, Config, ValidationConfig};
use stalagmight_lib::core::{AppState, Command, CommandResult, UndoEntry, UndoStack, Validator};
use stalagmight_lib::models::survey::changeset::{
    capture, EntryLimits, HistoryEntry, HistoryRefs, RecordedPreview,
};
use stalagmight_lib::models::survey::{self, LineMeta, Project, StartAt};
use stalagmight_lib::models::{
    ClosureStrategy, Coordinates, DatumRole, GpsCoordinates, LineId, LineUpdates, ProjectMetadata,
    Shot, ShotData, ShotKind, ShotUpdates, StationId, StationRef,
};
use stalagmight_lib::persistence::stalx_v2::{self, DecodeOptions, Extras, Mode};
use std::collections::HashSet;
use std::path::Path;
use tempfile::TempDir;

// ============================================================================
// Helper functions
// ============================================================================

fn create_test_project() -> Project {
    Project::new("Test Cave", Some("Test Location".to_string()))
}

fn create_shot_data(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    // Use same azimuth for in/out (the function averages them)
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

fn line_meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#4ecdc4".to_string(),
        ..LineMeta::default()
    }
}

/// Add a free line (a new piece) laid out from its start at local `(x, y)`.
fn add_line_at(project: &mut Project, name: &str, shots: Vec<ShotData>, x: f64, y: f64) -> LineId {
    let id = project
        .add_line(line_meta(name), StartAt::New, shots)
        .unwrap();
    let start = project.line(id).unwrap().start;
    project.set_placement(start, x, y).unwrap();
    id
}

/// Add a free line laid out from its start at the local origin (where 0.6.0
/// put a line nothing placed).
fn add_line(project: &mut Project, name: &str, shots: Vec<ShotData>) -> LineId {
    add_line_at(project, name, shots, 0.0, 0.0)
}

/// The station at `index` along `line`.
fn station_of(project: &Project, line: LineId, index: u32) -> StationId {
    project.station_at(StationRef::new(line, index)).unwrap()
}

/// The measured (Raw Data View) position of a slot.
fn slot_xyz(project: &Project, line: LineId, index: u32) -> [f64; 3] {
    project
        .raw_positions()
        .slot(project, StationRef::new(line, index))
        .unwrap()
}

fn gps(latitude: f64, longitude: f64, depth_reference: f64) -> GpsCoordinates {
    GpsCoordinates {
        latitude,
        longitude,
        depth_reference,
    }
}

/// Set a datum on a slot through the model's command (anchor rules included).
fn set_datum(project: &mut Project, line: LineId, index: u32, at: GpsCoordinates) {
    project
        .set_datum_at(StationRef::new(line, index), at, None)
        .unwrap();
}

fn assert_valid(project: &Project) {
    let violations = project.validate();
    assert!(violations.is_empty(), "{violations:?}");
}

/// Encode as a `.stalx` version 2 file body and decode it back.
fn round_trip(project: &Project) -> Project {
    let bytes = stalx_v2::encode(project, &Extras::default(), Mode::Normal).unwrap();
    stalx_v2::decode(&bytes, &DecodeOptions::default())
        .unwrap()
        .project
}

fn write_v2(project: &Project, path: &Path) {
    let bytes = stalx_v2::encode(project, &Extras::default(), Mode::Normal).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn read_v2(path: &Path) -> Project {
    let bytes = std::fs::read(path).unwrap();
    stalx_v2::decode(&bytes, &DecodeOptions::default())
        .unwrap()
        .project
}

fn assert_success(result: &CommandResult) {
    assert!(result.success, "expected success, got {:?}", result.error);
}

/// An app with a new, unsaved project open.
fn app() -> AppState {
    let state = AppState::new().unwrap();
    assert_success(&state.execute(Command::CreateProject {
        name: "Test Cave".to_string(),
        location: None,
    }));
    state
}

fn create_line_cmd(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    let result = state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots,
        cave_section_id: None,
    });
    assert_success(&result);
    result.line_id.unwrap()
}

/// What the Recents card shows for `project` written as a file: the header
/// (`summary`) the open path writes, read back by the recent-projects list.
fn recent_card(project: &Project) -> ProjectMetadata {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("card.stalx");
    write_v2(project, &path);
    let state = AppState::new().unwrap();
    assert_success(&state.execute(Command::OpenProject { path: path.clone() }));
    let cards = state.get_recent_projects();
    cards
        .into_iter()
        .find(|card| card.file_path == path)
        .expect("the opened file is listed")
}

/// An undo entry for a real edit (`before` -> `after`), as the core records it.
fn entry_for(kind: &str, before: &Project, after: &Project) -> HistoryEntry {
    HistoryEntry {
        timestamp: chrono::Utc::now(),
        kind: kind.to_string(),
        label: format!("Test {kind}"),
        label_parts: Vec::new(),
        refs: HistoryRefs::default(),
        preview: RecordedPreview::default(),
        changes: capture(before, after),
    }
}

/// `n` undo entries, each adding one line to the survey before it.
fn line_entries(n: usize) -> Vec<HistoryEntry> {
    let mut project = create_test_project();
    (0..n)
        .map(|i| {
            let before = project.clone();
            add_line_at(
                &mut project,
                &format!("Line {i}"),
                vec![create_shot_data(0.0, 1.0, 5.0, 5.0)],
                f64::from(u32::try_from(i).unwrap()) * 10.0,
                0.0,
            );
            entry_for("create_line", &before, &project)
        })
        .collect()
}

// ============================================================================
// Station Calculation Tests (5 tests)
// ============================================================================

/// Test 3: Station coordinates - verify computed coordinates from known shots
#[test]
fn test_station_coordinates_computed_correctly() {
    let mut project = create_test_project();

    // Create shots: North 10m, then East 10m
    let shot_a = create_shot_data(0.0, 10.0, 5.0, 5.0);
    let shot_b = create_shot_data(90.0, 10.0, 5.0, 6.0);
    let main = add_line(
        &mut project,
        "Main",
        vec![
            shot_a,         // North 10m (horizontal)
            shot_b.clone(), // East 10m (slope)
        ],
    );

    let slots = project.raw_positions().slots(&project);
    assert_eq!(slots.len(), 3); // 2 shots = 3 stations

    // Station 0: origin
    let s0 = slot_xyz(&project, main, 0);
    assert!((s0[0] - 0.0).abs() < 0.001);
    assert!((s0[1] - 0.0).abs() < 0.001);

    // Station 1: after north 10m (x=0, y=10)
    let s1 = slot_xyz(&project, main, 1);
    assert!((s1[0] - 0.0).abs() < 0.001);
    assert!((s1[1] - 10.0).abs() < 0.001);

    // Station 2: after east 10m slope (x=horizontal, y=10)
    let s2 = slot_xyz(&project, main, 2);
    let expected_x = Shot::new(0, shot_b).horizontal_length();
    assert!((s2[0] - expected_x).abs() < 0.001);
    assert!((s2[1] - 10.0).abs() < 0.001);
    // Depth is the measured depth reading.
    assert!((s2[2] - 6.0).abs() < 0.001);
}

/// Test 5: Zero datum propagation - verify GPS coords propagate
#[test]
fn test_zero_datum_propagation() {
    let mut project = create_test_project();

    let main = project
        .add_line(
            line_meta("Main"),
            StartAt::New,
            vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        )
        .unwrap();

    // Set zero datum at station 0 (it holds the piece: no placement needed)
    set_datum(&mut project, main, 0, gps(45.0, -122.0, 5.0));
    assert_valid(&project);

    let state = project.project_state_wire(false);
    let station0 = state
        .stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(main, 0))
        .unwrap();

    // Station 0 should be at origin (the zero datum point)
    assert!(station0.is_zero_datum);
    assert!((station0.coordinates.x - 0.0).abs() < 0.001);
    assert!((station0.coordinates.y - 0.0).abs() < 0.001);
    let at = station0.gps_coordinates.expect("the datum's GPS");
    assert_eq!(
        (at.latitude, at.longitude, at.depth_reference),
        (45.0, -122.0, 5.0)
    );
}

/// Test 23: Station ID stability - verify station refs maintain format
#[test]
fn test_station_id_stability() {
    let mut project = create_test_project();

    let a = add_line(
        &mut project,
        "Line A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let b = add_line_at(
        &mut project,
        "Line B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
        20.0,
        0.0,
    );
    assert_eq!((a, b), (0, 1));

    // Station refs should be line_id:station_index format
    let refs: Vec<_> = project
        .raw_positions()
        .slots(&project)
        .iter()
        .map(|s| s.slot)
        .collect();

    // Line 0 has 2 stations: 0:0, 0:1
    assert!(refs.contains(&StationRef::new(0, 0)));
    assert!(refs.contains(&StationRef::new(0, 1)));

    // Line 1 has 2 stations: 1:0, 1:1
    assert!(refs.contains(&StationRef::new(1, 0)));
    assert!(refs.contains(&StationRef::new(1, 1)));

    // Each station has its own Station ID, and it survives a save and reload.
    let ids: HashSet<StationId> = refs
        .iter()
        .map(|slot| project.station_at(*slot).unwrap())
        .collect();
    assert_eq!(ids.len(), 4);
    let reloaded = round_trip(&project);
    for slot in &refs {
        assert_eq!(reloaded.station_at(*slot), project.station_at(*slot));
    }
}

/// Test 24: 3D distance calculation - verify Euclidean distance
#[test]
fn test_3d_distance_calculation() {
    let coord_a = Coordinates::new(0.0, 0.0, 0.0);
    let coord_b = Coordinates::new(3.0, 4.0, 0.0);

    let distance = coord_a.distance_to(&coord_b);
    assert!((distance - 5.0).abs() < 0.001); // 3-4-5 triangle

    let coord_c = Coordinates::new(0.0, 0.0, 5.0);
    let distance_3d = coord_a.distance_to(&coord_c);
    assert!((distance_3d - 5.0).abs() < 0.001);
}

/// Test 49: Shot pitch values - verify 3D coords account for pitch (placeholder)
#[test]
fn test_shot_pitch_coordinates() {
    // Note: Current implementation doesn't use pitch for coordinate calculation
    // This test verifies the pitch fields are stored correctly
    let shot = Shot::new(
        0,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 5.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            pitch_in: Some(30.0),
            pitch_out: Some(-30.0),
            ..Default::default()
        },
    );

    assert_eq!(shot.pitch_in, Some(30.0));
    assert_eq!(shot.pitch_out, Some(-30.0));
}

// ============================================================================
// Merge Operations Tests (6 tests)
// ============================================================================

/// Test 4: Station merge repositioning - verify line repositions correctly
#[test]
fn test_station_merge_repositioning() {
    let mut project = create_test_project();

    // Line A goes north (with zero datum to anchor it)
    let a = project
        .add_line(
            line_meta("Line A"),
            StartAt::New,
            vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        )
        .unwrap();
    set_datum(&mut project, a, 0, gps(45.0, -122.0, 5.0));

    // Line B goes east (a free piece somewhere else, will move)
    let b = add_line_at(
        &mut project,
        "Line B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
        50.0,
        -30.0,
    );

    // Merge station 0:1 (end of A at y=10) with station 1:0 (start of B)
    // Since A has zero datum, B will move to align
    let result =
        project.merge_keeping_stationary(station_of(&project, a, 1), station_of(&project, b, 0));
    assert!(result.is_ok(), "{result:?}");
    assert_valid(&project);

    // One shared station now.
    assert_eq!(station_of(&project, a, 1), station_of(&project, b, 0));

    // Line B's start should now be at end of line A (y=10 relative to zero datum at origin)
    let line_b_start = slot_xyz(&project, b, 0);
    assert!(
        (line_b_start[1] - 10.0).abs() < 0.001,
        "Line B start Y should be 10, got {}",
        line_b_start[1]
    );
    assert!(line_b_start[0].abs() < 0.001);
    // Line A did not move.
    assert!(slot_xyz(&project, a, 0)[0].abs() < 0.001);
    assert!(slot_xyz(&project, a, 0)[1].abs() < 0.001);
}

/// Test 6: Unmerge keeps position: a detach moves nothing
#[test]
fn test_unmerge_keeps_position() {
    let mut project = create_test_project();

    let a = add_line(
        &mut project,
        "Line A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let b = add_line_at(
        &mut project,
        "Line B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
        30.0,
        0.0,
    );

    // Merge then unmerge
    project
        .merge_keeping_stationary(station_of(&project, a, 1), station_of(&project, b, 0))
        .unwrap();

    let merged = station_of(&project, a, 1) == station_of(&project, b, 0);
    assert!(merged);
    let before: Vec<[f64; 3]> = [(a, 0), (a, 1), (b, 0), (b, 1)]
        .iter()
        .map(|&(line, index)| slot_xyz(&project, line, index))
        .collect();

    project
        .unmerge_station_pair(
            StationRef::new(a, 1),
            StationRef::new(b, 0),
            survey::NameAnswers::RuleL,
            None,
        )
        .unwrap();
    assert_valid(&project);

    let still_merged = station_of(&project, a, 1) == station_of(&project, b, 0);
    assert!(!still_merged);
    let after: Vec<[f64; 3]> = [(a, 0), (a, 1), (b, 0), (b, 1)]
        .iter()
        .map(|&(line, index)| slot_xyz(&project, line, index))
        .collect();
    for (was, now) in before.iter().zip(&after) {
        for axis in 0..3 {
            assert!(
                (was[axis] - now[axis]).abs() < 1e-9,
                "a detach must not move anything: {was:?} -> {now:?}"
            );
        }
    }
}

/// Test 40: Duplicate merge rejection - verify already merged stations error
#[test]
fn test_duplicate_merge_rejection() {
    let mut project = create_test_project();

    let a = add_line(
        &mut project,
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let b = add_line_at(
        &mut project,
        "B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
        30.0,
        0.0,
    );

    // First merge succeeds
    let result1 =
        project.merge_keeping_stationary(station_of(&project, a, 1), station_of(&project, b, 0));
    assert!(result1.is_ok());

    // Second merge fails: the two slots are one station now
    let result2 =
        project.merge_keeping_stationary(station_of(&project, a, 1), station_of(&project, b, 0));
    assert!(result2.is_err());
    assert!(result2.unwrap_err().to_string().contains("already merged"));
}

/// Test 43: Delete line with merges - verify merges cleaned up
#[test]
fn test_delete_line_cleanup() {
    let mut project = create_test_project();

    let a = add_line(
        &mut project,
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let b = add_line_at(
        &mut project,
        "B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
        30.0,
        0.0,
    );

    project
        .merge_keeping_stationary(station_of(&project, a, 1), station_of(&project, b, 0))
        .unwrap();

    assert_eq!(project.project_wire().station_merges.len(), 1);

    // Remove line B
    project.delete_line(b).unwrap();
    assert_valid(&project);

    // The merge went with the line: A's end is an ordinary station again,
    // and nothing references line B.
    assert_eq!(project.lines().len(), 1);
    assert!(project.project_wire().station_merges.is_empty());
    assert!(!project.topology().is_junction(station_of(&project, a, 1)));
}

/// Test 48: Multiple zero datums - two datums in disconnected lines
#[test]
fn test_multiple_zero_datums_disconnected() {
    let mut project = create_test_project();

    let a = add_line(
        &mut project,
        "Line A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let b = add_line_at(
        &mut project,
        "Line B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
        30.0,
        0.0,
    );

    // Set zero datum on line A
    set_datum(&mut project, a, 0, gps(45.0, -122.0, 5.0));

    // Set zero datum on line B (disconnected, should succeed)
    let result = project.set_datum_at(StationRef::new(b, 0), gps(46.0, -123.0, 10.0), None);
    assert!(result.is_ok());
    assert_eq!(project.datums.len(), 2);
    assert_valid(&project);
    // Each holds its own piece.
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
}

/// Test: Merge nonexistent station fails
#[test]
fn test_merge_nonexistent_station_fails() {
    let mut project = create_test_project();

    let a = add_line(
        &mut project,
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );

    // Try to merge with non-existent station
    assert!(project.station_at(StationRef::new(99, 0)).is_none());
    let result = project.merge_keeping_stationary(station_of(&project, a, 0), 9_999);
    assert!(result.is_err());
    // And through the command, which resolves the slot first
    let state = app();
    create_line_cmd(&state, "A", vec![create_shot_data(0.0, 10.0, 5.0, 5.0)]);
    let result = state.execute(Command::MergeStations {
        station_a: StationRef::new(0, 0),
        station_b: StationRef::new(99, 0), // doesn't exist,
        keep: None,
        renames: None,
    });
    assert!(!result.success);
}

// ============================================================================
// Loop Closure Tests (2 tests)
// ============================================================================

/// Test 25: Loop detection simple - two lines merged at ends form loop
#[test]
fn test_loop_detection_simple() {
    let mut project = create_test_project();

    // Create a loop: Line A (north), Line B (east), merge to form loop
    let a = add_line(
        &mut project,
        "Line A",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(0.0, 10.0, 5.0, 5.0),
        ],
    );
    let b = add_line_at(
        &mut project,
        "Line B",
        vec![
            create_shot_data(90.0, 10.0, 5.0, 5.0),
            create_shot_data(90.0, 10.0, 5.0, 5.0),
        ],
        40.0,
        0.0,
    );

    // First merge end of A to start of B (connects them)
    project
        .merge_keeping_stationary(station_of(&project, a, 2), station_of(&project, b, 0))
        .unwrap();

    // Create loop closure between start of A and end of B (already connected)
    let result = project.create_closure_at(
        StationRef::new(a, 0),
        StationRef::new(b, 2),
        survey::ClosureStrategy::Batch,
        None,
        &[],
    );

    // This should succeed and create a loop closure
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(project.closures.len(), 1);
    assert_valid(&project);
}

/// Test 26: Loop closure error - verify error % calculation
#[test]
fn test_loop_closure_error_calculation() {
    let mut project = create_test_project();

    // Create lines that form almost-closed loop
    let a = add_line(
        &mut project,
        "Line A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)], // North 10m
    );
    let b = add_line_at(
        &mut project,
        "Line B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)], // East 10m
        40.0,
        0.0,
    );

    // Connect them in a way that creates a loop
    project
        .merge_keeping_stationary(
            station_of(&project, a, 1), // End of A
            station_of(&project, b, 0), // Start of B
        )
        .unwrap();

    // Now try to close the loop by creating a loop closure (stations are already connected)
    let result = project.create_closure_at(
        StationRef::new(a, 0), // Start of A
        StationRef::new(b, 1), // End of B
        survey::ClosureStrategy::Batch,
        None,
        &[],
    );

    let created = result.unwrap().value;
    // The metrics are derived from the measured layout: the path runs along
    // the two shots (20 m) and the ends lie sqrt(200) m apart.
    let metrics = created.metrics.expect("the closure's figures");
    assert!((metrics.path_length - 20.0).abs() < 1e-9);
    assert!((metrics.misclosure_distance - 200f64.sqrt()).abs() < 1e-9);
    assert!(
        (metrics.error_percentage - 100.0 * 200f64.sqrt() / 20.0).abs() < 1e-9,
        "{metrics:?}"
    );
    // And the wire's closure carries the same figures.
    let wire = project.project_wire();
    assert_eq!(wire.loop_closures.len(), 1);
    assert!((wire.loop_closures[0].path_length - 20.0).abs() < 1e-9);
}

// ============================================================================
// Undo/Redo Tests (7 tests)
// ============================================================================

/// Test 2: Undo/redo chain - create, add, delete, undo all, redo all
#[test]
fn test_undo_redo_chain() {
    let mut stack = UndoStack::new();

    // Simulate: create line, add shot
    let mut project = create_test_project();
    let empty = project.clone();
    let line = add_line(&mut project, "Test", Vec::new());
    let with_line = project.clone();
    project
        .append_shot(line, create_shot_data(0.0, 10.0, 5.0, 5.0))
        .unwrap();
    stack
        .record(entry_for("create_line", &empty, &with_line))
        .unwrap();
    stack
        .record(entry_for("create_shot", &with_line, &project))
        .unwrap();

    assert!(stack.can_undo());
    assert!(!stack.can_redo());

    // Undo one
    let entry = stack.pop_undo().unwrap();
    assert_eq!(entry.entry().kind, "create_shot");
    stack.push_redo(entry);

    assert!(stack.can_undo());
    assert!(stack.can_redo());

    // Undo second
    let entry2 = stack.pop_undo().unwrap();
    assert_eq!(entry2.entry().kind, "create_line");
    stack.push_redo(entry2);

    assert!(!stack.can_undo());
    assert!(stack.can_redo());

    // The same chain on the open project: every step lands where it started.
    let state = app();
    let line = create_line_cmd(&state, "Test", Vec::new());
    assert_success(&state.execute(Command::CreateShot {
        line_id: line,
        shot: create_shot_data(0.0, 10.0, 5.0, 5.0),
        after_shot_id: None,
        before_shot_id: None,
    }));
    let full = state.get_project_state().unwrap();
    assert_success(&state.undo());
    assert_success(&state.undo());
    assert!(state.get_project_state().unwrap().lines().is_empty());
    assert!(!state.can_undo() && state.can_redo());
    assert_success(&state.redo());
    assert_success(&state.redo());
    let redone = state.get_project_state().unwrap();
    assert_eq!(redone.lines(), full.lines());
    assert_eq!(redone.stations, full.stations);
}

/// Test 17: Undo stack limit - 110 operations, verify only last 100
#[test]
fn test_undo_stack_limit() {
    let mut stack = UndoStack::new_with_limit(100);

    // Push 110 entries
    for entry in line_entries(110) {
        stack.record(entry).unwrap();
    }

    // Should only have 100: the newest ones
    assert_eq!(stack.len(), 100);
    assert_eq!(stack.peek_undo().unwrap().entry().label, "Test create_line");
    let oldest = stack.undo_entries().next().unwrap();
    // The oldest kept entry adds "Line 10": entries 0..9 were dropped.
    let digest = oldest.entry().changes.digest();
    let digest_text = format!("{digest:?}");
    assert!(digest_text.contains("Line 10"), "{digest_text}");
}

/// Test 18: Undo clears redo - action, undo, new action, verify redo cleared
#[test]
fn test_undo_clears_redo_stack() {
    let mut stack = UndoStack::new();
    let mut entries = line_entries(2).into_iter();

    stack.record(entries.next().unwrap()).unwrap();

    // Undo
    let entry = stack.pop_undo().unwrap();
    stack.push_redo(entry);
    assert!(stack.can_redo());

    // New action should clear redo
    stack.record(entries.next().unwrap()).unwrap();
    assert!(!stack.can_redo());

    // The same rule on the open project.
    let state = app();
    create_line_cmd(&state, "Test", vec![create_shot_data(0.0, 1.0, 5.0, 5.0)]);
    assert_success(&state.undo());
    assert!(state.can_redo());
    create_line_cmd(&state, "New", vec![create_shot_data(0.0, 1.0, 5.0, 5.0)]);
    assert!(!state.can_redo());
    assert!(state.get_redo_stack().is_empty());
}

/// Test 35: An undo entry round-trips through its persisted text
#[test]
fn test_batch_command_structure() {
    let mut project = create_test_project();
    let before = project.clone();
    add_line(
        &mut project,
        "Test",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let entry = UndoEntry::new(entry_for("create_line", &before, &project)).unwrap();

    // Verify entry can be serialized
    let text = entry.raw().as_str();
    assert!(!text.is_empty());
    assert!(text.contains("\"k\":\"create_line\""));

    // And deserialized
    let restored = UndoEntry::decode(entry.raw(), &EntryLimits::default()).unwrap();
    assert_eq!(restored.entry().kind, "create_line");
    assert_eq!(restored.entry().label, entry.entry().label);
    assert_eq!(restored.raw(), entry.raw());

    // The decoded entry undoes and redoes exactly.
    let mut stepped = project.clone();
    survey::changeset::undo(&mut stepped, &restored.entry().changes).unwrap();
    assert_eq!(stepped.lines(), before.lines());
    assert_eq!(stepped.stations, before.stations);
    survey::changeset::redo(&mut stepped, &restored.entry().changes).unwrap();
    assert_eq!(stepped.lines().len(), 1);
    assert_eq!(
        stepped.lines()[0].shots[0].uuid,
        project.lines()[0].shots[0].uuid
    );
    assert_eq!(stepped.stations, project.stations);
}

/// Test 41: Undo persistence - the stacks round-trip through the file's history
#[test]
fn test_undo_persistence() {
    let mut stack = UndoStack::new();
    for entry in line_entries(1) {
        stack.record(entry).unwrap();
    }

    // Get the history the save persists
    let history = stack.persisted();
    assert_eq!(history.undo.len(), 1);
    assert!(history.redo.is_empty());

    // Load into new stack
    let mut new_stack = UndoStack::new();
    new_stack
        .load_persisted(&history, &EntryLimits::default())
        .unwrap();
    assert_eq!(new_stack.len(), 1);
    assert_eq!(new_stack.persisted(), history);

    // Through a real save and reopen: the history comes back and undoes.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("undo.stalx");
    let state = app();
    create_line_cmd(&state, "Test", vec![create_shot_data(0.0, 1.0, 5.0, 5.0)]);
    assert_success(&state.execute(Command::SaveProject {
        path: Some(path.clone()),
    }));
    let reopened = AppState::new().unwrap();
    assert_success(&reopened.execute(Command::OpenProject { path }));
    assert_eq!(reopened.get_undo_stack().len(), 1);
    assert_success(&reopened.undo());
    assert!(reopened.get_project_state().unwrap().lines().is_empty());
}

/// Test 45: Undo zero datum - set datum, undo, verify removal; an undone
/// removal puts the datum back at its list position (#413, finding 17)
#[test]
fn test_undo_zero_datum() {
    let state = app();
    let a = create_line_cmd(&state, "A", vec![create_shot_data(0.0, 10.0, 5.0, 5.0)]);
    let b = create_line_cmd(&state, "B", vec![create_shot_data(0.0, 10.0, 5.0, 5.0)]);
    let set = |line: LineId, at: GpsCoordinates| {
        assert_success(&state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(line, 0),
            coordinates: at,
            accuracy_m: Some(2.5),
            entrance: None,
        }));
    };

    // Set, then undo: the datum is gone.
    set(a, gps(45.0, -122.0, 0.0));
    assert_eq!(state.get_project_state().unwrap().datums.len(), 1);
    assert_success(&state.undo());
    assert!(state.get_project_state().unwrap().datums.is_empty());

    // Two datums; remove the first, undo: it is back FIRST, as it was.
    set(a, gps(45.0, -122.0, 0.0));
    set(b, gps(46.0, -123.0, 0.0));
    let with_both = state.get_project_state().unwrap();
    assert_success(&state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(a, 0),
    }));
    assert_eq!(state.get_project_state().unwrap().datums.len(), 1);
    assert_success(&state.undo());
    let restored = state.get_project_state().unwrap();
    assert_eq!(restored.datums, with_both.datums);
    assert_eq!(restored.datums[0].station, station_of(&restored, a, 0));
    assert_eq!(restored.datums[0].accuracy_m, Some(2.5));
    assert_eq!(restored.placements, with_both.placements);
}

/// Test 50: Comprehensive undo - every kind of edit undoes, also after a
/// save and reopen (its persisted entry)
#[test]
fn test_comprehensive_undo_types() {
    let state = app();
    let empty = state.get_project_state().unwrap();
    let a = create_line_cmd(
        &state,
        "A",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(90.0, 10.0, 5.0, 5.0),
        ],
    );
    let b = create_line_cmd(&state, "B", vec![create_shot_data(180.0, 7.0, 5.0, 6.0)]);
    let shot = state.get_project_state().unwrap().line(a).unwrap().shots[0].id;
    let commands = vec![
        Command::UpdateShot {
            line_id: a,
            shot_id: shot,
            updates: ShotUpdates {
                length: Some(12.0),
                comment: Some(Some("recalc".to_string())),
                ..Default::default()
            },
        },
        Command::UpdateLineMetadata {
            line_id: b,
            updates: LineUpdates {
                name: Some(Some("B renamed".to_string())),
                color: Some("#ff0000".to_string()),
                ..Default::default()
            },
        },
        Command::SetZeroDatum {
            station_ref: StationRef::new(a, 0),
            coordinates: gps(45.0, -122.0, 5.0),
            accuracy_m: None,
            entrance: None,
        },
        Command::MergeStations {
            station_a: StationRef::new(a, 2),
            station_b: StationRef::new(b, 0),
            keep: None,
            renames: None,
        },
        Command::CreateLoopClosure {
            station_a: StationRef::new(a, 0),
            station_b: StationRef::new(b, 1),
            closure_strategy: ClosureStrategy::Batch,
            keep: None,
            renames: None,
        },
        Command::SetShotKind {
            line_id: b,
            shot_id: state.get_project_state().unwrap().line(b).unwrap().shots[0].id,
            kind: ShotKind::Jump,
        },
    ];
    let mut states = vec![state.get_project_state().unwrap()];
    for command in commands {
        let result = state.execute(command.clone());
        assert_success(&result);
        states.push(state.get_project_state().unwrap());
    }
    let kinds: Vec<String> = state
        .get_undo_stack()
        .iter()
        .map(|item| item.command_type.clone())
        .collect();
    assert_eq!(kinds.len(), 8, "{kinds:?}");

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("kinds.stalx");
    assert_success(&state.execute(Command::SaveProject {
        path: Some(path.clone()),
    }));

    // In the session and through the persisted entries alike, each undo
    // lands exactly on the state before its edit.
    let reopened = AppState::new().unwrap();
    assert_success(&reopened.execute(Command::OpenProject { path }));
    for app_state in [&state, &reopened] {
        for expected in states.iter().rev().skip(1) {
            assert_success(&app_state.undo());
            let now = app_state.get_project_state().unwrap();
            assert_eq!(now.lines(), expected.lines());
            assert_eq!(now.stations, expected.stations);
            assert_eq!(now.datums, expected.datums);
            assert_eq!(now.closures, expected.closures);
        }
        assert_success(&app_state.undo());
        assert_success(&app_state.undo());
        assert!(!app_state.can_undo());
        let now = app_state.get_project_state().unwrap();
        assert!(now.lines().is_empty());
        assert_eq!(now.stations, empty.stations);
    }
}

// ============================================================================
// Line Operations Tests (4 tests)
// ============================================================================

/// Test 7: Shot deletion (unit test for shot removal)
#[test]
fn test_shot_deletion() {
    let mut project = create_test_project();

    let main = add_line(
        &mut project,
        "Main",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(0.0, 10.0, 5.0, 6.0),
            create_shot_data(0.0, 10.0, 6.0, 7.0),
        ],
    );

    let line = project.line(main).unwrap();
    assert_eq!(line.shots.len(), 3);
    let kept = [line.shots[0].uuid.clone(), line.shots[2].uuid.clone()];

    // Remove middle shot
    let shot_id = line.shots[1].id;
    project.delete_shot(main, shot_id).unwrap();
    assert_valid(&project);

    let line = project.line(main).unwrap();
    assert_eq!(line.shots.len(), 2);
    assert_eq!(
        [line.shots[0].uuid.clone(), line.shots[1].uuid.clone()],
        kept
    );
    // The chain closed the gap: one station fewer.
    assert_eq!(line.station_count(), 3);
}

/// Test 19: Line metadata update - verify all fields persist
#[test]
fn test_line_metadata_update() {
    let state = app();
    let line_id = create_line_cmd(
        &state,
        "Original",
        vec![create_shot_data(0.0, 10.0, 5.0, 6.0)],
    );

    let updates = LineUpdates {
        name: Some(Some("Updated Name".to_string())),
        // Legacy text: set on every shot.
        surveyor: Some(Some("New Surveyor".to_string())),
        color: Some("#ff0000".to_string()),
        date: None,
        direction: None,
        quality: Some(1.2),
    };

    assert_success(&state.execute(Command::UpdateLineMetadata { line_id, updates }));

    let check = |project: &Project| {
        let line = project.line(line_id).unwrap();
        assert_eq!(line.name, Some("Updated Name".to_string()));
        let credit = line.shots[0].credit.as_ref().expect("the shot's credit");
        assert_eq!(credit.surveyor_names().as_deref(), Some("New Surveyor"));
        assert_eq!(line.color, "#ff0000");
        assert_eq!(line.quality, 1.2);
    };
    check(&state.get_project_state().unwrap());

    // ...and they persist.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.stalx");
    assert_success(&state.execute(Command::SaveProject {
        path: Some(path.clone()),
    }));
    check(&read_v2(&path));
}

/// Test 20: Line color auto-assign (verify unique colors)
#[test]
fn test_line_colors_unique() {
    let mut project = create_test_project();

    let colors = ["#4ecdc4", "#ff6b6b", "#ffd93d", "#6bcb77"];

    for (i, color) in colors.iter().enumerate() {
        let id = project
            .add_line(
                LineMeta {
                    name: Some(format!("Line {i}")),
                    color: (*color).to_string(),
                    ..LineMeta::default()
                },
                StartAt::New,
                Vec::new(),
            )
            .unwrap();
        let start = project.line(id).unwrap().start;
        project.set_placement(start, 0.0, 0.0).unwrap();
    }
    assert_valid(&project);

    let line_colors: HashSet<_> = project.lines().iter().map(|s| &s.color).collect();
    assert_eq!(line_colors.len(), 4); // All unique
    let reloaded = round_trip(&project);
    let reloaded_colors: Vec<_> = reloaded.lines().iter().map(|s| s.color.clone()).collect();
    assert_eq!(reloaded_colors, colors);
}

/// Test 47: Line date parsing (verify date storage)
#[test]
fn test_line_date_storage() {
    use chrono::NaiveDate;

    let state = app();
    let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

    let result = state.execute(Command::CreateLine {
        name: Some("Dated Line".to_string()),
        surveyor: None,
        date: Some(date),
        color: "#4ecdc4".to_string(),
        direction: None,
        // The legacy date is a template for the shots, so it needs one.
        shots: vec![create_shot_data(0.0, 10.0, 5.0, 6.0)],
        cave_section_id: None,
    });
    assert_success(&result);

    let project = state.get_project_state().unwrap();
    let line = project.line(result.line_id.unwrap()).unwrap();
    assert_eq!(
        line.shots[0].credit.as_ref().and_then(|c| c.date),
        Some(date)
    );
    let raw = project.raw_positions();
    assert_eq!(
        project.survey_stats(&raw, false).first_survey_date,
        Some(date)
    );
}

// ============================================================================
// Statistics Tests (2 tests)
// ============================================================================

/// Test 31: Project stats - verify calculated statistics
#[test]
fn test_project_stats() {
    let mut project = create_test_project();

    add_line(
        &mut project,
        "Line A",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 6.0),
            create_shot_data(0.0, 15.0, 6.0, 8.0),
        ],
    );
    add_line_at(
        &mut project,
        "Line B",
        vec![create_shot_data(90.0, 20.0, 10.0, 12.0)],
        100.0,
        0.0,
    );

    let raw = project.raw_positions();
    let stats = project.survey_stats(&raw, false);

    assert_eq!(stats.line_count, 2);
    assert_eq!(stats.shot_count, 3);
    assert!((stats.total_length - 45.0).abs() < 0.001); // 10 + 15 + 20
    assert_eq!(stats.station_count, 5); // 3 + 2 stations
    assert_eq!(stats.slot_count, 5);
    // The wire keeps the slot count.
    assert_eq!(stats.to_project_stats().station_count, 5);

    // Depth is the absolute station range (#481):
    // Line A stations 5, 6, 8; Line B stations 10, 12.
    assert!((stats.min_depth - 5.0).abs() < 0.001);
    assert!((stats.max_depth - 12.0).abs() < 0.001);
}

/// A junction is one station: the station count counts it once, the slot
/// count (what the wire's station count has always meant) once per line.
#[test]
fn test_project_stats_count_a_junction_once() {
    let mut project = create_test_project();
    let a = add_line(
        &mut project,
        "Line A",
        vec![create_shot_data(0.0, 10.0, 5.0, 6.0)],
    );
    let junction = station_of(&project, a, 1);
    project
        .add_line(
            line_meta("Line B"),
            StartAt::Station(junction),
            vec![create_shot_data(90.0, 20.0, 6.0, 12.0)],
        )
        .unwrap();
    assert_valid(&project);
    let raw = project.raw_positions();
    let stats = project.survey_stats(&raw, false);
    assert_eq!(stats.station_count, 3);
    assert_eq!(stats.slot_count, 4);
    assert_eq!(stats.junction_count, 1);
    // The wire counts what the file summary counts (review finding UX-8).
    assert_eq!(stats.to_project_stats().station_count, 3);
    assert_eq!(stats.to_project_stats().junction_count, 1);
}

/// Test 32: Penetration distance - linear survey
#[test]
fn test_penetration_distance() {
    let mut project = create_test_project();

    // Create a straight line north: 10m + 10m + 10m = 30m
    let main = project
        .add_line(
            line_meta("Main"),
            StartAt::New,
            vec![
                create_shot_data(0.0, 10.0, 5.0, 5.0),
                create_shot_data(0.0, 10.0, 5.0, 5.0),
                create_shot_data(0.0, 10.0, 5.0, 5.0),
            ],
        )
        .unwrap();

    // Set zero datum at start
    set_datum(&mut project, main, 0, gps(45.0, -122.0, 5.0));
    assert_valid(&project);

    // Penetration should be 30m (straight line distance)
    let penetration = project.penetration(false);
    assert!((penetration - 30.0).abs() < 0.001, "{penetration}");
}

// ============================================================================
// Move Operations Tests (2 tests)
// ============================================================================

/// Test 33: A placement says where a free piece sits
#[test]
fn test_move_line_offset() {
    let mut project = create_test_project();

    // The line's piece sits with its start at (10, 20)
    let main = add_line_at(
        &mut project,
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        10.0,
        20.0,
    );

    // First station should be at offset position
    let first = slot_xyz(&project, main, 0);
    assert!((first[0] - 10.0).abs() < 0.001);
    assert!((first[1] - 20.0).abs() < 0.001);
    let second = slot_xyz(&project, main, 1);
    assert!((second[1] - 30.0).abs() < 0.001);
}

/// Test 34: Move station connectivity - shot lengths unchanged
#[test]
fn test_move_preserves_shot_data() {
    let mut project = create_test_project();

    let main = add_line(
        &mut project,
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );

    let original = project.line(main).unwrap().shots.clone();
    let before = slot_xyz(&project, main, 1);

    // Move the line
    project.move_line(main, [100.0, 100.0, 0.0]).unwrap();
    assert_valid(&project);

    // Shot data should be unchanged; the line moved as a whole
    assert_eq!(project.line(main).unwrap().shots, original);
    let after = slot_xyz(&project, main, 1);
    assert!((after[0] - (before[0] + 100.0)).abs() < 0.001);
    assert!((after[1] - (before[1] + 100.0)).abs() < 0.001);
}

// ============================================================================
// Config Tests (2 tests)
// ============================================================================

/// Test 15: Config defaults - verify default values
#[test]
fn test_config_defaults() {
    let stack = UndoStack::new();
    assert_eq!(stack.history_limit(), 100); // Default limit
}

/// Test 16: Config persistence - modify and verify
#[test]
fn test_undo_limit_config() {
    let mut stack = UndoStack::new_with_limit(50);
    assert_eq!(stack.history_limit(), 50);

    stack.set_history_limit(75);
    assert_eq!(stack.history_limit(), 75);
}

/// Test: CacheConfig defaults match SPEC.md values
#[test]
fn test_cache_config_defaults() {
    let cache = CacheConfig::default();
    assert!(
        cache.tile_cache_enabled,
        "tile_cache_enabled should default to true"
    );
    assert_eq!(
        cache.tile_cache_max_mb, 500,
        "tile_cache_max_mb should default to 500 per SPEC.md"
    );
    assert_eq!(
        cache.tile_ttl_days, 30,
        "tile_ttl_days should default to 30"
    );
}

/// Test: Config includes CacheConfig with correct defaults
#[test]
fn test_config_includes_cache_config() {
    let config = Config::default();
    assert!(config.cache.tile_cache_enabled);
    assert_eq!(config.cache.tile_cache_max_mb, 500);
    assert_eq!(config.cache.tile_ttl_days, 30);
}

/// Test: CacheConfig TOML deserialization with missing fields uses defaults
#[test]
fn test_cache_config_partial_toml_deserialization() {
    // Test partial cache config - only tile_cache_enabled specified
    let toml_str = r"
        tile_cache_enabled = false
    ";
    let cache: CacheConfig =
        toml::from_str(toml_str).expect("Should deserialize partial CacheConfig");
    assert!(
        !cache.tile_cache_enabled,
        "Explicit value should be respected"
    );
    assert_eq!(
        cache.tile_cache_max_mb, 500,
        "Missing field should use default"
    );
    assert_eq!(cache.tile_ttl_days, 30, "Missing field should use default");
}

/// Test: CacheConfig TOML deserialization with empty table uses all defaults
#[test]
fn test_cache_config_empty_toml_deserialization() {
    let toml_str = "";
    let cache: CacheConfig =
        toml::from_str(toml_str).expect("Should deserialize empty CacheConfig");
    assert!(cache.tile_cache_enabled);
    assert_eq!(cache.tile_cache_max_mb, 500);
    assert_eq!(cache.tile_ttl_days, 30);
}

/// Test: Config backwards compatibility - old config with removed fields parses correctly
#[test]
fn test_config_backwards_compat_ignores_removed_fields() {
    // Old config format with deprecated fields that were removed:
    // - display.station_size, display.line_thickness
    // - map.snap_threshold_pixels
    // - top-level [snap] line
    // Serde should ignore unknown fields (default behavior)
    let old_config_toml = r##"
[general]
auto_save = true
auto_save_interval_seconds = 60
recent_projects_max = 10

[units]
length = "meters"
depth = "feet"
angle = "degrees"
decimal_precision = 2

[display]
default_background = "#2a2a3e"
grid_color = "#444466"
station_size = 8
line_thickness = 3
font_size = 14
font_color = "#ffffff"

[display.depth_gradient]
enabled = true
shallow_color = "#00ff00"
deep_color = "#ff0000"
shallow_depth = 0.0
deep_depth = 100.0

[line_colors]
palette = ["#ff0000", "#00ff00", "#0000ff"]

[map]
satellite_provider = "esri"
satellite_enabled = true
snap_threshold_pixels = 15
zoom_default = 12
zoom_min = 5
zoom_max = 20

[map.controls]
pan_speed = 1.5
zoom_speed = 2.0
invert_scroll = true

[validation]
warn_shot_length_min = 0.5
warn_shot_length_max = 100.0
warn_depth_change_per_shot_max = 20.0
warn_azimuth_deviation_max = 45.0

[export]
default_image_format = "svg"
default_dpi = 150
default_include_satellite = false
default_include_grid = true
default_include_labels = false

[logging]
level = "debug"
max_directory_size_mb = 256

[shortcuts]
select = "A"
pan = "H"
zoom = "J"
draw = "K"
delete = "L"
merge = "G"
measure = "F"
move_line = "W"
move_station = "E"
undo = "Ctrl+U"
redo = "Ctrl+R"
save = "Ctrl+W"

[snap]
enabled_draw = false
enabled_merge = true
enabled_measure = false
threshold_pixels = 20
"##;

    let config: Config = toml::from_str(old_config_toml).expect(
        "Config with deprecated fields should parse - serde ignores unknown fields by default",
    );

    // Verify custom values were preserved
    assert_eq!(config.general.auto_save_interval_seconds, 60);
    assert_eq!(
        config.units.depth,
        stalagmight_lib::config::LengthUnit::Feet
    );
    assert_eq!(config.display.font_size, 14);
    // `[display.depth_gradient]` was removed (#380 item 1): its table above is
    // ignored, and re-serialising the config drops it.
    let resaved = toml::to_string(&config).expect("config re-serialises");
    assert!(!resaved.contains("[display.depth_gradient]"));
    assert!(!resaved.contains("shallow_color"));
    // A config from before `[map_palette]` existed loads with the theme palettes.
    assert_eq!(
        config.map_palette,
        stalagmight_lib::config::MapPaletteConfig::default()
    );
    assert_eq!(config.map.satellite_provider, "esri");
    assert!(config.map.satellite_enabled);
    assert!(config.map.controls.invert_scroll);

    // New nested configs should have defaults since they weren't in old format
    assert_eq!(config.map.station.radius, 4.0);
    assert_eq!(config.map.station.hit_area, 10.0);
    assert_eq!(config.map.shot.width, 2.0);
    assert_eq!(config.map.shot.hit_area, 8.0);
    assert!(!config.debug.map.show_hitboxes);

    // Legacy [snap] values should be migrated to map.snap
    // The test config had: enabled_draw=false, enabled_merge=true, enabled_measure=false, threshold_pixels=20
    // After migration, map.snap should reflect these values
}

// Note: Legacy snap migration tests are in src-tauri/src/config/mod.rs::tests
// where they have access to the private migrate_legacy() method

/// Test: Config normalization clamps and validates values
#[test]
fn test_config_normalize_clamps_values() {
    use stalagmight_lib::config::{MapConfig, MapSnapConfig, ShotConfig, StationConfig};

    let mut map_config = MapConfig {
        station: StationConfig {
            radius: 0.1,
            hit_area: 0.5,
            ..Default::default()
        }, // Below min
        shot: ShotConfig {
            width: 100.0,
            hit_area: 50.0,
            ..Default::default()
        }, // Above max
        snap: MapSnapConfig {
            threshold: -5.0,
            enabled_merge: true,
            enabled_measure: true,
        },
        ..Default::default()
    };

    map_config.normalize();

    // Station: radius clamped to 1.0, hit_area clamped to 1.0 then max'd to radius
    assert_eq!(map_config.station.radius, 1.0);
    assert_eq!(map_config.station.hit_area, 1.0); // max(1.0, 1.0)

    // Shot: width clamped to 20.0, hit_area clamped to 20.0 then max'd to width
    assert_eq!(map_config.shot.width, 20.0);
    assert_eq!(map_config.shot.hit_area, 20.0);

    // Snap: threshold clamped to 1.0
    assert_eq!(map_config.snap.threshold, 1.0);
}

/// Test: Config normalization handles NaN/inf values
#[test]
fn test_config_normalize_handles_nan_inf() {
    use stalagmight_lib::config::{MapConfig, MapSnapConfig, ShotConfig, StationConfig};

    let mut map_config = MapConfig {
        station: StationConfig {
            radius: f64::NAN,
            hit_area: f64::INFINITY,
            ..Default::default()
        },
        shot: ShotConfig {
            width: f64::NEG_INFINITY,
            hit_area: f64::NAN,
            ..Default::default()
        },
        snap: MapSnapConfig {
            threshold: f64::NAN,
            enabled_merge: true,
            enabled_measure: true,
        },
        ..Default::default()
    };

    map_config.normalize();

    // All should fall back to defaults
    assert_eq!(map_config.station.radius, 4.0);
    assert_eq!(map_config.station.hit_area, 10.0);
    assert_eq!(map_config.shot.width, 2.0);
    assert_eq!(map_config.shot.hit_area, 8.0);
    assert_eq!(map_config.snap.threshold, 10.0);
}

/// Test: hit_area must be >= radius/width after normalization
#[test]
fn test_config_normalize_hit_area_ge_visual_size() {
    use stalagmight_lib::config::{MapConfig, ShotConfig, StationConfig};

    let mut map_config = MapConfig {
        station: StationConfig {
            radius: 20.0,
            hit_area: 5.0,
            ..Default::default()
        }, // hit_area < radius
        shot: ShotConfig {
            width: 15.0,
            hit_area: 3.0,
            ..Default::default()
        }, // hit_area < width
        ..Default::default()
    };

    map_config.normalize();

    // hit_area should be raised to match radius/width
    assert_eq!(map_config.station.radius, 20.0);
    assert_eq!(map_config.station.hit_area, 20.0); // raised to radius

    assert_eq!(map_config.shot.width, 15.0);
    assert_eq!(map_config.shot.hit_area, 15.0); // raised to width
}

// ============================================================================
// Persistence Tests (8 tests)
// ============================================================================

/// Test 8: Reference shot flag - mark, save, reload, verify
#[test]
fn test_reference_shot_flag() {
    let mut project = create_test_project();

    let mut jump = create_shot_data(0.0, 10.0, 5.0, 5.0);
    jump.kind = ShotKind::Jump;
    let mut reference = create_shot_data(0.0, 10.0, 5.0, 5.0);
    reference.kind = ShotKind::Reference;

    let main = add_line(&mut project, "Main", vec![jump, reference]);

    let shots = &project.line(main).unwrap().shots;
    assert_eq!(shots[0].kind, ShotKind::Jump);
    assert_eq!(shots[1].kind, ShotKind::Reference);
    let reloaded = round_trip(&project);
    let shots = &reloaded.line(main).unwrap().shots;
    assert_eq!(shots[0].kind, ShotKind::Jump);
    assert_eq!(shots[1].kind, ShotKind::Reference);
}

/// Test 21: Shot LRUD persistence - verify all 4 values, through a save
#[test]
fn test_shot_lrud_persistence() {
    let data = ShotData {
        azimuth_in: 0.0,
        azimuth_out: 5.0,
        length: 10.0,
        depth_in: 5.0,
        depth_out: 5.0,
        left: Some(1.0),
        right: Some(2.0),
        up: Some(3.0),
        down: Some(4.0),
        ..Default::default()
    };
    let shot = Shot::new(0, data.clone());

    assert_eq!(shot.left, Some(1.0));
    assert_eq!(shot.right, Some(2.0));
    assert_eq!(shot.up, Some(3.0));
    assert_eq!(shot.down, Some(4.0));

    let mut project = create_test_project();
    let main = add_line(&mut project, "Main", vec![data]);
    let reloaded = round_trip(&project);
    let shot = &reloaded.line(main).unwrap().shots[0];
    assert_eq!(
        (shot.left, shot.right, shot.up, shot.down),
        (Some(1.0), Some(2.0), Some(3.0), Some(4.0))
    );
}

/// Test 22: Shot comment special chars - quotes, newlines, unicode
#[test]
fn test_shot_comment_special_chars() {
    let comment = "Test \"quotes\" and\nnewlines and émoji 🎉";

    let data = ShotData {
        azimuth_in: 0.0,
        azimuth_out: 5.0,
        length: 10.0,
        depth_in: 5.0,
        depth_out: 5.0,
        comment: Some(comment.to_string()),
        ..Default::default()
    };
    let shot = Shot::new(0, data.clone());

    assert_eq!(shot.comment, Some(comment.to_string()));

    // Verify JSON serialization handles special chars
    let json = serde_json::to_string(&shot).unwrap();
    let restored: Shot = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.comment, Some(comment.to_string()));

    // ...and the `.stalx` file does too.
    let mut project = create_test_project();
    let main = add_line(&mut project, "Main", vec![data]);
    let reloaded = round_trip(&project);
    assert_eq!(
        reloaded.line(main).unwrap().shots[0].comment.as_deref(),
        Some(comment)
    );
}

/// Test 29: New project creates file
#[test]
fn test_new_project_creates_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("new.stalx");

    let project = create_test_project();

    // Write it as a version 2 file
    let bytes = stalx_v2::encode(&project, &Extras::default(), Mode::Normal).unwrap();
    stalx_v2::write_atomic(&path, &bytes, stalx_v2::Guard::Absent).unwrap();

    assert!(path.exists());
    assert!(std::fs::read(&path)
        .unwrap()
        .starts_with(br#"{"format":"stalx""#));
}

/// Test 37: Empty project save/load
#[test]
fn test_empty_project_save_load() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("empty.stalx");

    let project = Project::new("Empty", None);
    assert!(project.lines().is_empty());

    write_v2(&project, &path);

    let loaded = read_v2(&path);
    assert_eq!(loaded.name, "Empty");
    assert!(loaded.lines().is_empty());
    assert_eq!(loaded.created_at, project.created_at);

    // And through the app's own create, save and open.
    let state = AppState::new().unwrap();
    assert_success(&state.execute(Command::CreateProject {
        name: "Empty".to_string(),
        location: None,
    }));
    let saved = dir.path().join("saved.stalx");
    assert_success(&state.execute(Command::SaveProject {
        path: Some(saved.clone()),
    }));
    let reopened = AppState::new().unwrap();
    assert_success(&reopened.execute(Command::OpenProject { path: saved }));
    let project = reopened.get_project_state().unwrap();
    assert_eq!(project.name, "Empty");
    assert!(project.lines().is_empty());
}

/// Test 38: Large survey performance (simplified - tests data structure)
#[test]
fn test_large_survey_data_structure() {
    let mut project = create_test_project();

    // Create line with 1000 shots
    let shots: Vec<_> = (0..1000)
        .map(|i| create_shot_data(f64::from(i) % 360.0, 1.0, 5.0, 5.0 + (f64::from(i) * 0.01)))
        .collect();

    add_line(&mut project, "Large", shots);
    assert_valid(&project);

    let raw = project.raw_positions();
    let stats = project.survey_stats(&raw, false);
    assert_eq!(stats.shot_count, 1000);
    assert!((stats.total_length - 1000.0).abs() < 0.001);
    assert_eq!(round_trip(&project).lines()[0].shots.len(), 1000);
}

/// Test 46: Shot order preserved
#[test]
fn test_shot_order_preserved() {
    let mut project = create_test_project();

    let shots = vec![
        create_shot_data(0.0, 1.0, 5.0, 5.0),
        create_shot_data(90.0, 2.0, 5.0, 5.0),
        create_shot_data(180.0, 3.0, 5.0, 5.0),
        create_shot_data(270.0, 4.0, 5.0, 5.0),
    ];

    let ordered = add_line(&mut project, "Ordered", shots);

    for project in [project.clone(), round_trip(&project)] {
        let line = project.line(ordered).unwrap();
        assert_eq!(line.shots.len(), 4);
        assert!((line.shots[0].length - 1.0).abs() < 0.001);
        assert!((line.shots[1].length - 2.0).abs() < 0.001);
        assert!((line.shots[2].length - 3.0).abs() < 0.001);
        assert!((line.shots[3].length - 4.0).abs() < 0.001);
    }
}

/// Test: Project JSON serialization round-trip
#[test]
fn test_project_json_roundtrip() {
    let mut project = create_test_project();
    add_line(
        &mut project,
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );

    let json = serde_json::to_string(&project).unwrap();
    let mut restored: Project = serde_json::from_str(&json).unwrap();
    restored.assign_session_shot_ids();

    assert_eq!(restored.name, project.name);
    assert_eq!(restored.lines().len(), 1);
    assert_valid(&restored);
    assert_eq!(restored, project);
}

// ============================================================================
// Zero Datum Tests (2 tests)
// ============================================================================

/// Test 44: Zero datum removal keeps position
#[test]
fn test_zero_datum_removal() {
    let mut project = create_test_project();

    let main = project
        .add_line(
            line_meta("Main"),
            StartAt::New,
            vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        )
        .unwrap();

    // Set and then remove zero datum
    set_datum(&mut project, main, 0, gps(45.0, -122.0, 5.0));
    assert_eq!(project.datums.len(), 1);
    let before = [slot_xyz(&project, main, 0), slot_xyz(&project, main, 1)];

    project
        .remove_datum_at(StationRef::new(main, 0))
        .unwrap()
        .expect("the slot had a datum");
    assert_eq!(project.datums.len(), 0);
    assert_valid(&project);

    // The piece stays where it was (it is placed there now).
    let after = [slot_xyz(&project, main, 0), slot_xyz(&project, main, 1)];
    for (was, now) in before.iter().zip(&after) {
        for axis in 0..3 {
            assert!((was[axis] - now[axis]).abs() < 1e-9, "{was:?} -> {now:?}");
        }
    }
}

/// Test: A second datum in a held piece becomes a GPS fix
#[test]
fn test_connected_zero_datum_becomes_gps_fix() {
    let mut project = create_test_project();

    let main = project
        .add_line(
            line_meta("Main"),
            StartAt::New,
            vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        )
        .unwrap();

    // Set zero datum at station 0
    set_datum(&mut project, main, 0, gps(45.0, -122.0, 5.0));

    let before = slot_xyz(&project, main, 1);

    // A second datum in the same piece is accepted as a GPS fix (#741)...
    set_datum(&mut project, main, 1, gps(45.0001, -122.0, 10.0));
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );

    // ...and does not move the Raw Data View.
    let after = slot_xyz(&project, main, 1);
    assert!((before[0] - after[0]).abs() < 1e-9 && (before[1] - after[1]).abs() < 1e-9);
}

// ============================================================================
// ID Boundary Tests (1 test)
// ============================================================================

/// Test 51: Shot ID overflow prevention
#[test]
fn test_shot_id_generation() {
    let mut project = create_test_project();

    // Add many shots to verify ID generation doesn't overflow
    let shots: Vec<_> = (0..1000)
        .map(|_| create_shot_data(0.0, 1.0, 5.0, 5.0))
        .collect();

    let large = add_line(&mut project, "Large", shots);

    let line = project.line(large).unwrap();

    // Verify all shots have unique IDs, uuids and end stations
    let ids: HashSet<_> = line.shots.iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), 1000);
    let uuids: HashSet<_> = line.shots.iter().map(|s| s.uuid.clone()).collect();
    assert_eq!(uuids.len(), 1000);
    let stations: HashSet<_> = line.stations().collect();
    assert_eq!(stations.len(), 1001);
    // A reload gives every shot its session id back the same way.
    let reloaded = round_trip(&project);
    let reloaded_ids: Vec<_> = reloaded
        .line(large)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();
    let original_ids: Vec<_> = line.shots.iter().map(|s| s.id).collect();
    assert_eq!(reloaded_ids, original_ids);
}

// ============================================================================
// Shot Update Tests
// ============================================================================

/// Test: Shot updates apply correctly
#[test]
fn test_shot_updates() {
    let mut shot = Shot::new(0, create_shot_data(0.0, 10.0, 5.0, 5.0));

    let updates = ShotUpdates {
        length: Some(20.0),
        comment: Some(Some("Updated".to_string())),
        kind: Some(ShotKind::Jump),
        ..Default::default()
    };

    shot.apply_updates(&updates);

    assert!((shot.length - 20.0).abs() < 0.001);
    assert_eq!(shot.comment, Some("Updated".to_string()));
    assert_eq!(shot.kind, ShotKind::Jump);
    // Unchanged fields
    assert!((shot.azimuth_in - 0.0).abs() < 0.001);
}

/// Test: Average azimuth calculation
#[test]
fn test_average_azimuth() {
    // Normal case - same in/out
    let shot1 = Shot::new(
        0,
        ShotData {
            azimuth_in: 10.0,
            azimuth_out: 10.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        },
    );
    assert!((shot1.average_azimuth_measured() - 10.0).abs() < 0.001);

    // Averaging with small difference
    let shot2 = Shot::new(
        0,
        ShotData {
            azimuth_in: 8.0,
            azimuth_out: 12.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        },
    );
    assert!((shot2.average_azimuth_measured() - 10.0).abs() < 0.001);

    // Wraparound case - averaging around 0/360
    let shot3 = Shot::new(
        0,
        ShotData {
            azimuth_in: 358.0,
            azimuth_out: 2.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        },
    );
    assert!((shot3.average_azimuth_measured() - 0.0).abs() < 0.001);
}

// ============================================================================
// Line Stats Tests
// ============================================================================

/// Test: Line statistics calculation
#[test]
fn test_line_stats() {
    let mut project = create_test_project();

    let mut shots = vec![
        create_shot_data(0.0, 10.0, 5.0, 8.0),
        create_shot_data(0.0, 15.0, 8.0, 12.0),
    ];
    shots[1].kind = ShotKind::Reference;

    add_line(&mut project, "Test", shots);

    let raw = project.raw_positions();
    let stats = project.survey_stats(&raw, false);

    assert_eq!(stats.shot_count, 2);
    // A Reference repeats passage already surveyed, so its length is left out
    // of the total (#689) while it still counts as a shot.
    assert!((stats.total_length - 10.0).abs() < 0.001);
    // The Recents card says the same.
    let card = recent_card(&project);
    assert!((card.total_length - 10.0).abs() < 0.001);
}

/// A jump counts separately from a reference, and unlike a reference its length
/// counts toward the total: it is surveyed passage (#27 q7, #689).
#[test]
fn test_line_stats_splits_jumps_from_references() {
    let mut project = create_test_project();

    let mut shots = vec![
        create_shot_data(0.0, 10.0, 5.0, 8.0),
        create_shot_data(0.0, 15.0, 8.0, 12.0),
    ];
    shots[1].kind = ShotKind::Jump;

    add_line(&mut project, "Test", shots);

    let raw = project.raw_positions();
    let stats = project.survey_stats(&raw, false);

    assert_eq!(stats.total_length, 25.0, "a jump's length counts");
    assert_eq!(stats.shot_count, 2);
    let card = recent_card(&project);
    assert_eq!(card.total_length, 25.0);
}

// ============================================================================
// DMP Parser Tests (already exist, but add one more)
// ============================================================================

/// Test 1: DMP round-trip (structure test): the import line the DMP wizard
/// sends over IPC round-trips through JSON
#[test]
fn test_dmp_section_data_roundtrip() {
    let line = stalagmight_lib::models::ImportLineConfig {
        cave_section: None,
        reverse: false,
        line_index: 0,
        name: Some("Test Line".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![
            create_shot_data(45.0, 10.0, 5.0, 6.0),
            create_shot_data(90.0, 15.0, 6.0, 8.0),
        ],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        station_external: Vec::new(),
        placement: stalagmight_lib::models::ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    };

    let json = serde_json::to_string(&line).unwrap();
    let restored: stalagmight_lib::models::ImportLineConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.name, Some("Test Line".to_string()));
    assert_eq!(restored.shots.len(), 2);
}

// ============================================================================
// Error Handling Tests (2 tests)
// ============================================================================

/// Test 39: Invalid station reference handling
#[test]
fn test_invalid_station_reference() {
    let mut project = create_test_project();

    // Check non-existent station
    assert!(project.station_at(StationRef::new(999, 0)).is_none());
    let main = add_line(
        &mut project,
        "Main",
        vec![create_shot_data(0.0, 1.0, 5.0, 5.0)],
    );
    assert!(project.station_at(StationRef::new(main, 2)).is_none());
    assert!(project.station_of_slot(StationRef::new(main, 2)).is_err());
}

/// Test 36: Recent projects (structure test)
#[test]
fn test_project_metadata() {
    let mut project = Project::new("Test Cave", Some("Location".to_string()));

    add_line(
        &mut project,
        "Main",
        vec![create_shot_data(0.0, 50.0, 5.0, 10.0)],
    );

    let metadata = recent_card(&project);

    assert_eq!(metadata.name, "Test Cave");
    assert_eq!(metadata.location, Some("Location".to_string()));
    assert_eq!(metadata.line_count, 1);
    assert_eq!(metadata.station_count, 2);
    assert!((metadata.total_length - 50.0).abs() < 0.001);
}

// ============================================================================
// NEW COVERAGE TESTS (15 tests for increased coverage)
// ============================================================================

/// Helper: Create default validation config
fn default_validation_config() -> ValidationConfig {
    ValidationConfig {
        warn_shot_length_min: 0.1,
        warn_shot_length_max: 50.0,
        warn_depth_change_per_shot_max: 10.0,
        warn_azimuth_deviation_max: 30.0,
        azimuth_diff_threshold: 20.0,
        pitch_diff_threshold: 5.0,
        inclination_depth_diff_threshold: 10.0,
    }
}

// ----------------------------------------------------------------------------
// Validation Tests (5 tests)
// ----------------------------------------------------------------------------

/// Coverage Test 1: Negative length shot returns NEGATIVE_LENGTH error
#[test]
fn test_negative_length_shot() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: -5.0, // Negative!
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let config = default_validation_config();
    let warnings = Validator::validate_shot(&shot, &config, String::new);

    assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LENGTH"));
    assert!(Validator::has_errors(&warnings));
}

/// Coverage Test 2: Short shot warning
#[test]
fn test_short_shot_warning() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: 0.05, // Below 0.1m threshold
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let config = ValidationConfig {
        warn_shot_length_min: 0.1,
        ..default_validation_config()
    };
    let warnings = Validator::validate_shot(&shot, &config, String::new);

    assert!(warnings.iter().any(|w| w.code == "SHORT_SHOT"));
    // SHORT_SHOT is a warning, not error
    let short_warning = warnings.iter().find(|w| w.code == "SHORT_SHOT").unwrap();
    assert_eq!(
        short_warning.severity,
        stalagmight_lib::core::WarningSeverity::Warning
    );
}

/// Coverage Test 3: Long shot warning
#[test]
fn test_long_shot_warning() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: 100.0, // Above 50m threshold
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let config = ValidationConfig {
        warn_shot_length_max: 50.0,
        ..default_validation_config()
    };
    let warnings = Validator::validate_shot(&shot, &config, String::new);

    assert!(warnings.iter().any(|w| w.code == "LONG_SHOT"));
}

/// Coverage Test 4: Large depth change warning
#[test]
fn test_large_depth_change_warning() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: 10.0,
        depth_in: 10.0,
        depth_out: 25.0, // 15m change, above 10m threshold
        ..Default::default()
    };
    let config = ValidationConfig {
        warn_depth_change_per_shot_max: 10.0,
        ..default_validation_config()
    };
    let warnings = Validator::validate_shot(&shot, &config, String::new);

    assert!(warnings.iter().any(|w| w.code == "LARGE_DEPTH_CHANGE"));
}

/// Coverage Test 5: Negative LRUD values return errors
#[test]
fn test_negative_lrud_errors() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: 10.0,
        depth_in: 10.0,
        depth_out: 10.0,
        left: Some(-1.0),
        right: Some(-2.0),
        up: Some(-3.0),
        down: Some(-4.0),
        ..Default::default()
    };
    let warnings = Validator::validate_shot(&shot, &default_validation_config(), String::new);

    let lrud_errors: Vec<_> = warnings
        .iter()
        .filter(|w| w.code == "NEGATIVE_LRUD")
        .collect();
    assert_eq!(lrud_errors.len(), 4); // All 4 LRUD values negative
    assert!(Validator::has_errors(&warnings));
}

// ----------------------------------------------------------------------------
// Persistence Tests (5 tests)
// ----------------------------------------------------------------------------

/// Coverage Test 6: Save/load with station merges (a junction)
#[test]
fn test_save_load_with_station_merges() {
    let mut project = Project::new("Test", None);
    let a = add_line(
        &mut project,
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let b = add_line_at(
        &mut project,
        "B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
        30.0,
        0.0,
    );

    // Add a merge
    project
        .merge_keeping_stationary(station_of(&project, a, 1), station_of(&project, b, 0))
        .unwrap();

    let loaded = round_trip(&project);

    assert_eq!(station_of(&loaded, a, 1), station_of(&loaded, b, 0));
    let merges = loaded.project_wire().station_merges;
    assert_eq!(merges.len(), 1);
    let ends = [merges[0].station_a, merges[0].station_b];
    assert!(ends.contains(&StationRef::new(a, 1)));
    assert!(ends.contains(&StationRef::new(b, 0)));
    assert_eq!(loaded, project);
}

/// Coverage Test 7: Save/load with zero datums
#[test]
fn test_save_load_with_zero_datums() {
    let mut project = Project::new("Test", None);
    let a = project
        .add_line(
            line_meta("A"),
            StartAt::New,
            vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        )
        .unwrap();

    set_datum(&mut project, a, 0, gps(45.123, -122.456, 5.0));

    let loaded = round_trip(&project);

    assert_eq!(loaded.datums.len(), 1);
    // Bit for bit, not within a tolerance.
    let at = loaded.datums[0].gps;
    assert_eq!(
        (at.latitude, at.longitude, at.depth_reference),
        (45.123, -122.456, 5.0)
    );
    assert_eq!(loaded.datums[0].station, station_of(&loaded, a, 0));
    assert_eq!(loaded.datums, project.datums);
}

/// Coverage Test 8: Save/load with a placement (what a line offset was)
#[test]
fn test_save_load_with_line_offsets() {
    let mut project = Project::new("Test", None);
    let a = add_line_at(
        &mut project,
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        15.5,
        -20.3,
    );

    let loaded = round_trip(&project);

    assert_eq!(loaded.placements, project.placements);
    let start = slot_xyz(&loaded, a, 0);
    assert_eq!((start[0], start[1]), (15.5, -20.3));
}

/// Coverage Test 9: Save/load with loop closures
#[test]
fn test_save_load_with_loop_closures() {
    let mut project = Project::new("Test", None);
    let a = add_line(
        &mut project,
        "A",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(90.0, 10.0, 5.0, 5.0),
            create_shot_data(200.0, 13.0, 5.0, 6.5),
        ],
    );

    project
        .create_closure_at(
            StationRef::new(a, 0),
            StationRef::new(a, 3),
            survey::ClosureStrategy::default(),
            None,
            &[],
        )
        .unwrap();

    let loaded = round_trip(&project);

    assert_eq!(loaded.closures, project.closures);
    // The figures are derived, so a reload gives exactly the same ones.
    let before = project.project_wire().loop_closures;
    let after = loaded.project_wire().loop_closures;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].path_length, before[0].path_length);
    assert!((after[0].path_length - 33.0).abs() < 1e-9);
    assert_eq!(after[0].error_percentage, before[0].error_percentage);
    assert_eq!(
        after[0].horizontal_misclosure,
        before[0].horizontal_misclosure
    );
    assert_eq!(after[0].vertical_misclosure, before[0].vertical_misclosure);
    assert!((after[0].vertical_misclosure.abs() - 1.5).abs() < 1e-9);
}

/// Coverage Test 10: Open nonexistent file fails
#[test]
fn test_open_nonexistent_file_fails() {
    let state = AppState::new().unwrap();
    let result = state.execute(Command::OpenProject {
        path: Path::new("/nonexistent/path/file.stalx").to_path_buf(),
    });
    assert!(!result.success);
    assert!(
        result.error.as_deref().unwrap_or("").contains("not found"),
        "{:?}",
        result.error
    );
    assert!(state.get_project_state().is_none());
}

// ----------------------------------------------------------------------------
// Model Tests (5 tests)
// ----------------------------------------------------------------------------

/// Coverage Test 11: Shot depth_change calculation
#[test]
fn test_shot_depth_change_method() {
    let shot = Shot::new(
        0,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 10.0,
            depth_in: 10.0,
            depth_out: 15.0,
            ..Default::default()
        },
    );
    assert!((shot.depth_change() - 5.0).abs() < 0.001);

    let shot2 = Shot::new(
        1,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 10.0,
            depth_in: 20.0,
            depth_out: 12.0,
            ..Default::default()
        },
    );
    assert!((shot2.depth_change() - (-8.0)).abs() < 0.001);
}

/// Coverage Test 12: Line insert shot after
#[test]
fn test_line_insert_shot_after() {
    let mut project = create_test_project();
    let line = add_line(
        &mut project,
        "Test",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(90.0, 10.0, 5.0, 5.0),
        ],
    );

    let first_shot_id = project.line(line).unwrap().shots[0].id;
    let new_shot = project.create_shot(
        line,
        create_shot_data(45.0, 5.0, 5.0, 5.0),
        survey::ShotPlace::After(first_shot_id),
    );

    assert!(new_shot.is_ok(), "{new_shot:?}");
    assert_valid(&project);
    let shots = &project.line(line).unwrap().shots;
    assert_eq!(shots.len(), 3);
    // New shot should be in position 1 (after first shot)
    assert!((shots[1].azimuth_in - 45.0).abs() < 0.001);
    assert!((shots[1].length - 5.0).abs() < 0.001);
}

/// Coverage Test 13: A line's data: the surveyor template goes onto the shots
#[test]
fn test_line_to_line_data() {
    let state = app();
    let result = state.execute(Command::CreateLine {
        name: Some("Original".to_string()),
        surveyor: Some("John".to_string()),
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![create_shot_data(0.0, 10.0, 5.0, 6.0)],
        cave_section_id: None,
    });
    assert_success(&result);
    let project = state.get_project_state().unwrap();
    let line = project.line(result.line_id.unwrap()).unwrap();

    assert_eq!(line.name, Some("Original".to_string()));
    // The template went onto the shot; the line keeps no legacy copy.
    assert_eq!(line.legacy_surveyor, None);
    assert_eq!(
        line.shots[0]
            .credit
            .as_ref()
            .and_then(|c| c.surveyor_names())
            .as_deref(),
        Some("John")
    );
    assert_eq!(line.color, "#ff0000");
    assert_eq!(line.shots.len(), 1);
    assert!((line.shots[0].length - 10.0).abs() < 0.001);
    // The shot reads back as the measurement record it was made from.
    let data = line.shots[0].to_shot_data();
    assert!((data.length - 10.0).abs() < 0.001);
    assert_eq!(data.uuid.as_deref(), Some(line.shots[0].uuid.as_str()));
}

/// Coverage Test 14: Project insert_shot_after (the command)
#[test]
fn test_project_insert_shot_after() {
    let state = app();
    let line = create_line_cmd(
        &state,
        "A",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(90.0, 10.0, 5.0, 5.0),
        ],
    );

    let first_shot_id = state.get_project_state().unwrap().line(line).unwrap().shots[0].id;
    let result = state.execute(Command::CreateShot {
        line_id: line,
        shot: create_shot_data(45.0, 5.0, 5.0, 5.0),
        after_shot_id: Some(first_shot_id),
        before_shot_id: None,
    });

    assert_success(&result);
    assert!(result.shot_id.is_some());
    let project = state.get_project_state().unwrap();
    let shots = &project.line(line).unwrap().shots;
    assert_eq!(shots.len(), 3);
    // Verify the new shot is in the middle
    assert!((shots[1].azimuth_in - 45.0).abs() < 0.001);
}

/// Coverage Test 15: Project update_shot
#[test]
fn test_project_update_shot() {
    let mut project = Project::new("Test", None);
    let line = add_line(
        &mut project,
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );

    let shot_id = project.line(line).unwrap().shots[0].id;
    let updates = ShotUpdates {
        length: Some(20.0),
        comment: Some(Some("Updated comment".to_string())),
        kind: Some(ShotKind::Jump),
        ..Default::default()
    };

    let result = project.update_shot(line, shot_id, &updates);
    assert!(result.is_ok(), "{result:?}");

    let shot = &project.line(line).unwrap().shots[0];
    assert_eq!(shot.id, shot_id);
    assert!((shot.length - 20.0).abs() < 0.001);
    assert_eq!(shot.comment, Some("Updated comment".to_string()));
    assert_eq!(shot.kind, ShotKind::Jump);
    // The station after it moved with the new length.
    assert!((slot_xyz(&project, line, 1)[1] - 20.0).abs() < 0.001);
}

// ============================================================================
// GPS Metadata Tests (5 tests)
// ============================================================================

/// Test GPS metadata with no zero datums returns None
#[test]
fn test_gps_metadata_no_datums() {
    let mut project = Project::new("Test Cave", None);
    add_line(
        &mut project,
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );

    let metadata = recent_card(&project);

    assert!(metadata.gps_center.is_none());
    assert!(metadata.gps_bounds.is_none());
}

/// Test GPS metadata with single zero datum
#[test]
fn test_gps_metadata_single_datum() {
    let mut project = Project::new("Test Cave", None);
    let main = project
        .add_line(
            line_meta("Main"),
            StartAt::New,
            vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        )
        .unwrap();

    set_datum(&mut project, main, 0, gps(45.5, -122.7, 0.0));

    let metadata = recent_card(&project);

    // Center should be from first (and only) datum (lng, lat order for MapLibre)
    let (lng, lat) = metadata.gps_center.unwrap();
    assert!((lat - 45.5).abs() < 0.0001);
    assert!((lng - (-122.7)).abs() < 0.0001);

    // Bounds should be a point (same as center for single datum)
    let bounds = metadata.gps_bounds.unwrap();
    assert!((bounds.north - 45.5).abs() < 0.0001);
    assert!((bounds.south - 45.5).abs() < 0.0001);
    assert!((bounds.east - (-122.7)).abs() < 0.0001);
    assert!((bounds.west - (-122.7)).abs() < 0.0001);
}

/// `n` disconnected lines, each one shot north, placed 50 m apart.
fn disconnected_lines(project: &mut Project, n: u32) -> Vec<LineId> {
    (0..n)
        .map(|i| {
            add_line_at(
                project,
                &format!("Line {i}"),
                vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
                f64::from(i) * 50.0,
                0.0,
            )
        })
        .collect()
}

/// Test GPS metadata with multiple zero datums
#[test]
fn test_gps_metadata_multiple_datums() {
    let mut project = Project::new("Test Cave", None);

    // Create two disconnected lines
    let lines = disconnected_lines(&mut project, 2);

    // Set zero datums on different lines (allowed because disconnected)
    set_datum(&mut project, lines[0], 0, gps(45.0, -122.0, 0.0));
    set_datum(&mut project, lines[1], 0, gps(46.0, -123.0, 0.0));

    let metadata = recent_card(&project);

    // Center should be from first datum (lng, lat order for MapLibre)
    let (lng, lat) = metadata.gps_center.unwrap();
    assert!((lat - 45.0).abs() < 0.0001);
    assert!((lng - (-122.0)).abs() < 0.0001);

    // Bounds should encompass both datums
    let bounds = metadata.gps_bounds.unwrap();
    assert!((bounds.north - 46.0).abs() < 0.0001);
    assert!((bounds.south - 45.0).abs() < 0.0001);
    // For non-dateline-crossing, west is smaller longitude
    assert!((bounds.west - (-123.0)).abs() < 0.0001);
    assert!((bounds.east - (-122.0)).abs() < 0.0001);
}

/// Test GPS bounds calculation handles dateline crossing
#[test]
fn test_gps_metadata_dateline_crossing() {
    let mut project = Project::new("Test Cave", None);

    // Create three disconnected lines
    let lines = disconnected_lines(&mut project, 3);

    // Set zero datums spanning the dateline (179 and -179 are close together)
    set_datum(&mut project, lines[0], 0, gps(50.0, 179.0, 0.0));
    set_datum(&mut project, lines[1], 0, gps(50.0, -179.0, 0.0));
    set_datum(&mut project, lines[2], 0, gps(50.0, 178.0, 0.0));

    let metadata = recent_card(&project);

    let bounds = metadata.gps_bounds.unwrap();

    // The bounds should span the small arc across the dateline (178 to -179)
    // not the large arc (178 to 179 to ... to -179)
    // west should be 178, east should be -179 (crossing dateline)
    assert!((bounds.west - 178.0).abs() < 0.0001);
    assert!((bounds.east - (-179.0)).abs() < 0.0001);
}

/// Test GPS metadata center is stable across updates to other datums
#[test]
fn test_gps_metadata_center_stability() {
    let mut project = Project::new("Test Cave", None);

    // Create two disconnected lines
    let lines = disconnected_lines(&mut project, 2);

    set_datum(&mut project, lines[0], 0, gps(45.0, -122.0, 0.0));

    // Get initial center
    let metadata = recent_card(&project);
    let (initial_lat, initial_lng) = metadata.gps_center.unwrap();

    // Add another zero datum
    set_datum(&mut project, lines[1], 0, gps(46.0, -123.0, 0.0));

    // Center should still be from first datum
    let metadata = recent_card(&project);
    let (final_lat, final_lng) = metadata.gps_center.unwrap();

    assert!((initial_lat - final_lat).abs() < 0.0001);
    assert!((initial_lng - final_lng).abs() < 0.0001);
}

/// Test that updating the first datum preserves center position (in-place update)
#[test]
fn test_gps_metadata_update_preserves_order() {
    let mut project = Project::new("Test Cave", None);

    // Create two disconnected lines
    let lines = disconnected_lines(&mut project, 2);

    // Set first datum, then the second
    set_datum(&mut project, lines[0], 0, gps(45.0, -122.0, 0.0));
    set_datum(&mut project, lines[1], 0, gps(46.0, -123.0, 0.0));

    // Update the first datum with new GPS (should update in-place, not reorder)
    set_datum(&mut project, lines[0], 0, gps(45.5, -122.5, 0.0));
    assert_eq!(project.datums.len(), 2);

    // Center should still be from first datum (now with updated values)
    // (lng, lat order for MapLibre)
    let metadata = recent_card(&project);
    let (center_lng, center_lat) = metadata.gps_center.unwrap();

    // Verify center is the updated first datum, not the second
    assert!(
        (center_lat - 45.5).abs() < 0.0001,
        "Center should be updated first datum lat"
    );
    assert!(
        (center_lng - (-122.5)).abs() < 0.0001,
        "Center should be updated first datum lng"
    );
}

/// Test that invalid GPS coordinates are rejected
#[test]
fn test_gps_invalid_coordinates_rejected() {
    let state = app();
    let line = create_line_cmd(
        &state,
        "Line A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let set = |at: GpsCoordinates| {
        state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(line, 0),
            coordinates: at,
            accuracy_m: None,
            entrance: None,
        })
    };

    // NaN latitude, infinite longitude and out of range values are rejected
    for bad in [
        gps(f64::NAN, -122.0, 0.0),
        gps(45.0, f64::INFINITY, 0.0),
        gps(91.0, -122.0, 0.0),
        gps(45.0, 181.0, 0.0),
    ] {
        assert!(!set(bad).success, "{bad:?} must be refused");
        assert!(state.get_project_state().unwrap().datums.is_empty());
        assert!(GpsCoordinates::new(bad.latitude, bad.longitude, 0.0).is_err());
    }

    // Valid coordinates should work
    assert_success(&set(gps(45.0, -122.0, 0.0)));
    assert_eq!(state.get_project_state().unwrap().datums.len(), 1);
}

// ============================================================================
// Merge Projects Tests
// ============================================================================

fn merge(sources: &[&Path], output: &Path, name: &str) -> Project {
    let state = AppState::new().unwrap();
    let result = state.execute(Command::MergeProjects {
        source_paths: sources.iter().map(|p| p.to_path_buf()).collect(),
        output_path: output.to_path_buf(),
        output_name: name.to_string(),
        archive_originals: false,
    });
    assert_success(&result);
    // The open project is never touched.
    assert!(state.get_project_state().is_none());
    read_v2(output)
}

/// Test: Merge two projects into a new project
#[test]
fn test_merge_projects_basic() {
    let dir = TempDir::new().unwrap();

    // Create first project with 2 lines
    let path1 = dir.path().join("project1.stalx");
    let mut project1 = Project::new("Cave A", Some("Location A".to_string()));
    add_line(
        &mut project1,
        "A-Main",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(45.0, 8.0, 5.0, 6.0),
        ],
    );
    add_line_at(
        &mut project1,
        "A-Side",
        vec![create_shot_data(90.0, 5.0, 6.0, 7.0)],
        30.0,
        0.0,
    );
    write_v2(&project1, &path1);

    // Create second project with 1 line
    let path2 = dir.path().join("project2.stalx");
    let mut project2 = Project::new("Cave B", Some("Location B".to_string()));
    add_line(
        &mut project2,
        "B-Main",
        vec![
            create_shot_data(180.0, 12.0, 4.0, 4.0),
            create_shot_data(270.0, 6.0, 4.0, 5.0),
        ],
    );
    write_v2(&project2, &path2);

    // Merge projects
    let output_path = dir.path().join("merged.stalx");
    let merged = merge(&[&path1, &path2], &output_path, "Merged Cave");

    assert_eq!(merged.name, "Merged Cave");
    let lines = merged.lines();
    assert_eq!(lines.len(), 3); // 2 from project1 + 1 from project2
    assert_eq!(lines[0].name, Some("A-Main".to_string()));
    assert_eq!(lines[1].name, Some("A-Side".to_string()));
    assert_eq!(lines[2].name, Some("B-Main".to_string()));

    // Verify line IDs are numbered in order (0, 1, 2)
    assert_eq!(lines[0].id, 0);
    assert_eq!(lines[1].id, 1);
    assert_eq!(lines[2].id, 2);

    // Verify shots are preserved
    assert_eq!(lines[0].shots.len(), 2);
    assert_eq!(lines[1].shots.len(), 1);
    assert_eq!(lines[2].shots.len(), 2);
    assert_eq!(lines[2].shots[0].uuid, project2.lines()[0].shots[0].uuid);

    // Verify total stats
    let raw = merged.raw_positions();
    let stats = merged.survey_stats(&raw, false);
    assert_eq!(stats.line_count, 3);
    assert_eq!(stats.shot_count, 5);
    assert_eq!(stats.station_count, 8);
}

/// Test: Merge projects preserves zero datums with remapped IDs
#[test]
fn test_merge_projects_preserves_zero_datums() {
    let dir = TempDir::new().unwrap();

    // Create first project with zero datum
    let path1 = dir.path().join("project_zd1.stalx");
    let mut project1 = Project::new("Cave ZD1", None);
    let l1 = project1
        .add_line(
            line_meta("ZD1-Main"),
            StartAt::New,
            vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
        )
        .unwrap();
    set_datum(&mut project1, l1, 0, gps(45.0, -122.0, 0.0));
    write_v2(&project1, &path1);

    // Create second project with zero datum
    let path2 = dir.path().join("project_zd2.stalx");
    let mut project2 = Project::new("Cave ZD2", None);
    let l2 = project2
        .add_line(
            line_meta("ZD2-Main"),
            StartAt::New,
            vec![create_shot_data(90.0, 8.0, 4.0, 4.0)],
        )
        .unwrap();
    // End of line
    set_datum(&mut project2, l2, 1, gps(46.0, -123.0, 5.0));
    write_v2(&project2, &path2);

    let output_path = dir.path().join("merged_zd.stalx");
    let merged = merge(&[&path1, &path2], &output_path, "Merged ZD");

    // Verify zero datums were preserved on the copied lines' stations
    assert_eq!(merged.datums.len(), 2);

    // First zero datum from project 1 (line 0, station 0)
    let slot0 = merged.datum_slot(&merged.datums[0]).unwrap();
    assert_eq!(slot0, StationRef::new(0, 0));
    assert!((merged.datums[0].gps.latitude - 45.0).abs() < 0.001);

    // Second zero datum from project 2 (line 1, station 1)
    let slot1 = merged.datum_slot(&merged.datums[1]).unwrap();
    assert_eq!(slot1, StationRef::new(1, 1));
    assert!((merged.datums[1].gps.latitude - 46.0).abs() < 0.001);
    assert_eq!(merged.datums[1].gps.depth_reference, 5.0);
}

/// Test: Merge projects with station merges preserves them with remapped IDs
#[test]
fn test_merge_projects_preserves_station_merges() {
    let dir = TempDir::new().unwrap();

    // Create project with 2 lines and a station merge
    let path1 = dir.path().join("project_merge1.stalx");
    let mut project1 = Project::new("Cave Merge", None);
    let a = add_line(
        &mut project1,
        "Line A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    );
    let b = add_line_at(
        &mut project1,
        "Line B",
        vec![create_shot_data(90.0, 8.0, 4.0, 4.0)],
        30.0,
        0.0,
    );

    // Merge last station of line 0 with first station of line 1
    project1
        .merge_keeping_stationary(station_of(&project1, a, 1), station_of(&project1, b, 0))
        .unwrap();
    write_v2(&project1, &path1);

    // Create second project
    let path2 = dir.path().join("project_merge2.stalx");
    let mut project2 = Project::new("Empty Cave", None);
    add_line(
        &mut project2,
        "Line C",
        vec![create_shot_data(180.0, 6.0, 3.0, 3.0)],
    );
    write_v2(&project2, &path2);

    let output_path = dir.path().join("merged_stations.stalx");
    let merged = merge(&[&path1, &path2], &output_path, "Merged Stations");

    // Verify the junction was preserved on the copied lines
    assert_eq!(station_of(&merged, 0, 1), station_of(&merged, 1, 0));
    let merges = merged.project_wire().station_merges;
    assert_eq!(merges.len(), 1);
    let ends = [merges[0].station_a, merges[0].station_b];
    assert!(ends.contains(&StationRef::new(0, 1)));
    assert!(ends.contains(&StationRef::new(1, 0)));
    // ...and no junction across the sources
    assert!(!merged.topology().is_junction(station_of(&merged, 2, 0)));

    // Verify 3 lines total
    assert_eq!(merged.lines().len(), 3);
}
