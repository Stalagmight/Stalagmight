//! Station comments, Move Comment and the move tools (Move Line / Segment /
//! Station) through the execute pipeline on the station graph.
//!
//! Ported from the 0.6.0 `state/tests.rs` sections "Move Line Tests", "Move
//! Station Tests", "Move Segment Tests", "Comment system tests" and
//! "Coverage gap: move_station edge cases". 0.6.0 read a move off
//! `line_offsets`; that field is gone (plan 1.3), so the ports read the
//! derived raw positions and the free piece's `Placement` instead.
//!
//! Retired, with the reason:
//! - `test_move_segment_unaligned_closure_independent` and
//!   `test_move_segment_aligned_closure_connectivity`: two pieces joined
//!   only by a closure must both hold a datum (invariant 7), and then
//!   neither may move, so "the far piece moves (or not) with the closure"
//!   has no movable state left. What remains of them, a datum anywhere in
//!   the rigid group refusing the move, is
//!   [`move_segment_is_refused_when_its_group_holds_a_datum`].
//!
//! New: junctions (one station, one comment, every incident shot rewritten),
//! the batch of co-located slots moving a station once (plan 9.1), Move
//! Comment between shots and between two slots of one junction, the refusal
//! words, the recorded preview rows, no-op moves, and a move surviving a
//! save and reopen.
//!
//! Lines are built with the model's own `create_line` (what Create Line
//! does), outside the history; an "unrelated recorded edit" runs a model
//! operation through the real pipeline ([`run_model`]).

use std::path::Path;

use tempfile::TempDir;

use super::commands::Record;
use super::AppState;
use crate::core::{Command, CommandResult, CommentHolder};
use crate::models::survey::{ClosureStrategy, LineMeta, Project, StartAt};
use crate::models::{
    Coordinates, GpsCoordinates, LineId, ShotData, StationId, StationRef, LINE_GONE, SHOT_GONE,
};

// === Kit ===

/// 0.6.0's `test_shot_data`.
fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

/// A level shot on `azimuth`, `length` long, at depth 0.
fn level(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    }
}

fn ok(result: CommandResult) -> CommandResult {
    assert!(result.success, "{:?}", result.error);
    result
}

fn refused(result: CommandResult) -> String {
    assert!(!result.success, "the command was expected to be refused");
    result.error.unwrap_or_default()
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    ok(state.execute(Command::CreateProject {
        name: "Moves".to_string(),
        location: None,
    }));
    state
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// Change the open project directly: a fixture outside the history.
fn edit<R>(state: &AppState, f: impl FnOnce(&mut Project) -> R) -> R {
    let mut p = project(state);
    let out = f(&mut p);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    state.set_project_for_tests(p);
    out
}

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#ff0000".to_string(),
        ..LineMeta::default()
    }
}

/// A new free line whose first station sits at the local origin (what
/// Create Line makes).
fn add_line(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    edit(state, |p| {
        p.create_line(meta(name), StartAt::New, shots, None)
            .unwrap()
            .value
    })
}

/// A new line starting at the station of `at`: a junction there.
fn add_branch(state: &AppState, name: &str, at: StationRef, shots: Vec<ShotData>) -> LineId {
    edit(state, |p| {
        let start = p.station_at(at).unwrap();
        p.create_line(meta(name), StartAt::Station(start), shots, None)
            .unwrap()
            .value
    })
}

/// Run a model operation through the execute pipeline as one recorded
/// edit, as the command family that owns it does.
fn run_model(
    state: &AppState,
    kind: &str,
    f: impl FnOnce(&mut Project) -> Result<(), String>,
) -> CommandResult {
    let recorded = state.run_recorded(
        |_| Record {
            kind: kind.to_string(),
            label: kind.to_string(),
            ..Record::default()
        },
        |p, _| f(p),
    );
    match recorded {
        Ok(()) => CommandResult::success(),
        Err(failure) => failure,
    }
}

fn at(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

fn station(state: &AppState, line: LineId, index: u32) -> StationId {
    project(state).station_at(at(line, index)).unwrap()
}

/// The raw (measured) position of a slot: x east, y north, z depth.
fn position(state: &AppState, line: LineId, index: u32) -> [f64; 3] {
    let p = project(state);
    p.raw_positions().slot(&p, at(line, index)).unwrap()
}

fn assert_close(actual: [f64; 3], expected: [f64; 3], what: &str) {
    for axis in 0..3 {
        assert!(
            (actual[axis] - expected[axis]).abs() < 1e-6,
            "{what}: axis {axis}: {actual:?} vs {expected:?}"
        );
    }
}

fn plus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// The project without what an undo or redo restamps (`modified_at`).
fn canonical(p: &Project) -> Project {
    let mut p = p.clone();
    p.modified_at = p.created_at;
    p
}

fn entries(state: &AppState) -> usize {
    state.get_undo_stack().len()
}

fn move_line(state: &AppState, line_id: LineId, x: f64, y: f64) -> CommandResult {
    state.execute(Command::MoveLine {
        line_id,
        offset: Coordinates::new(x, y, 0.0),
    })
}

fn move_segment(state: &AppState, line_id: LineId, x: f64, y: f64) -> CommandResult {
    state.execute(Command::MoveSegment {
        line_id,
        offset: Coordinates::new(x, y, 0.0),
    })
}

fn move_station(state: &AppState, slot: StationRef, offset: [f64; 3]) -> CommandResult {
    state.execute(Command::MoveStation {
        station_ref: slot,
        offset: Coordinates::new(offset[0], offset[1], offset[2]),
    })
}

fn set_comment(state: &AppState, slot: StationRef, comment: Option<&str>) -> CommandResult {
    state.execute(Command::SetStationComment {
        station_ref: slot,
        comment: comment.map(str::to_string),
    })
}

fn comment(state: &AppState, slot: StationRef) -> Option<String> {
    let p = project(state);
    let id = p.station_at(slot)?;
    p.stations.get(&id)?.comment.clone()
}

fn shot_comment(state: &AppState, line: LineId, index: usize) -> Option<String> {
    project(state).line(line)?.shots.get(index)?.comment.clone()
}

fn station_holder(slot: StationRef) -> CommentHolder {
    CommentHolder::Station { station_ref: slot }
}

fn shot_holder(state: &AppState, line: LineId, index: usize) -> CommentHolder {
    CommentHolder::Shot {
        line_id: line,
        shot_id: project(state).line(line).unwrap().shots[index].id,
    }
}

fn move_comment(
    state: &AppState,
    source: CommentHolder,
    target: CommentHolder,
    swap: bool,
) -> CommandResult {
    state.execute(Command::MoveComment {
        source,
        target,
        swap,
    })
}

fn gps(latitude: f64, longitude: f64) -> GpsCoordinates {
    GpsCoordinates::new(latitude, longitude, 0.0).unwrap()
}

/// The newest entry's preview rows as `(field, new value)`.
fn newest_preview(state: &AppState) -> Vec<(String, String)> {
    state
        .get_undo_stack()
        .first()
        .and_then(|item| item.preview_data.clone())
        .map(|preview| {
            preview
                .changes
                .into_iter()
                .map(|c| (c.field, c.new_value))
                .collect()
        })
        .unwrap_or_default()
}

fn save(state: &AppState, path: &Path) {
    ok(state.save_project(Some(path.to_path_buf())));
}

fn reopen(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    ok(state.open_project(path.to_path_buf()));
    state
}

// === Move Line ===

#[test]
fn move_line_translates_the_piece_and_accumulates() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    let end_before = position(&state, line, 1);

    ok(move_line(&state, line, 10.0, 20.0));
    assert_close(position(&state, line, 0), [10.0, 20.0, 10.0], "first move");
    assert_close(
        position(&state, line, 1),
        plus(end_before, [10.0, 20.0, 0.0]),
        "the whole line moves",
    );

    ok(move_line(&state, line, 5.0, -10.0));
    assert_close(position(&state, line, 0), [15.0, 10.0, 10.0], "accumulated");
    let p = project(&state);
    let start = p.line(line).unwrap().start;
    assert_eq!(p.placements.len(), 1, "one piece, one placement");
    assert_eq!(p.placements[0].station, start);
    assert!((p.placements[0].x - 15.0).abs() < 1e-9 && (p.placements[0].y - 10.0).abs() < 1e-9);
    assert_eq!(entries(&state), 2, "one entry per move");
}

#[test]
fn move_line_undo_redo() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    let before = project(&state);

    ok(move_line(&state, line, 10.0, 20.0));
    let after = project(&state);
    assert_close(position(&state, line, 0), [10.0, 20.0, 10.0], "moved");

    ok(state.undo());
    assert_eq!(canonical(&project(&state)), canonical(&before), "undo");
    assert_close(
        position(&state, line, 0),
        [0.0, 0.0, 10.0],
        "back at the origin",
    );

    ok(state.redo());
    assert_eq!(canonical(&project(&state)), canonical(&after), "redo");
}

#[test]
fn move_line_moves_the_whole_merged_group_and_sticks() {
    // A merged line is one piece with its partner: Move Line moves the whole
    // rigid group (0.6.0 finding #6), so the junction stays one point.
    let state = new_state();
    let a = add_line(&state, "A", vec![shot(5.0)]);
    let b = add_line(&state, "B", vec![shot(5.0)]);
    ok(state.execute(Command::MergeStations {
        station_a: at(a, 0),
        station_b: at(b, 0),
    }));
    assert_eq!(station(&state, a, 0), station(&state, b, 0), "one station");
    let (a1, b1) = (position(&state, a, 1), position(&state, b, 1));
    let before = project(&state);

    ok(move_line(&state, a, 10.0, 20.0));
    assert_close(
        position(&state, a, 1),
        plus(a1, [10.0, 20.0, 0.0]),
        "A moved",
    );
    assert_close(
        position(&state, b, 1),
        plus(b1, [10.0, 20.0, 0.0]),
        "B moved with it",
    );
    assert_close(
        position(&state, a, 0),
        position(&state, b, 0),
        "the junction is one point",
    );

    ok(state.undo());
    assert_eq!(canonical(&project(&state)), canonical(&before));
    ok(state.redo());
    assert_close(position(&state, b, 1), plus(b1, [10.0, 20.0, 0.0]), "redo");
}

#[test]
fn move_line_of_a_missing_line_is_refused() {
    let state = new_state();
    add_line(&state, "A", vec![shot(5.0)]);
    assert_eq!(
        refused(move_line(&state, 999, 10.0, 20.0)),
        "Line not found"
    );
    assert_eq!(entries(&state), 0);
}

// === Move Station ===

#[test]
fn move_station_updates_shot_data() {
    let state = new_state();
    let line = add_line(&state, "A", vec![level(90.0, 10.0)]);
    let end = at(line, 1);

    // Extend eastwards: the shot (10, 0, 0) becomes (15, 0, 0).
    ok(move_station(&state, end, [5.0, 0.0, 0.0]));
    let p = project(&state);
    let s = &p.line(line).unwrap().shots[0];
    assert!((s.length - 15.0).abs() < 1e-6, "length {}", s.length);
    assert!(
        (s.azimuth_in - 90.0).abs() < 1e-6,
        "azimuth {}",
        s.azimuth_in
    );
    assert!(p.line(line).unwrap().hand_moves.is_empty(), "no hand move");

    // (0, 10, 3) more: the shot is (15, 10, 3).
    ok(move_station(&state, end, [0.0, 10.0, 3.0]));
    let p = project(&state);
    let s = &p.line(line).unwrap().shots[0];
    let expected_length = (15.0_f64.powi(2) + 10.0_f64.powi(2) + 3.0_f64.powi(2)).sqrt();
    assert!(
        (s.length - expected_length).abs() < 1e-6,
        "length {}",
        s.length
    );
    let expected_azimuth = 15.0_f64.atan2(10.0).to_degrees();
    assert!(
        (s.azimuth_in - expected_azimuth).abs() < 1e-6,
        "azimuth {}",
        s.azimuth_in
    );
    assert!(
        (s.depth_out - 3.0).abs() < 1e-6,
        "depth_out {}",
        s.depth_out
    );
}

#[test]
fn moving_station_zero_moves_the_placement_and_keeps_the_rest() {
    let state = new_state();
    let line = add_line(&state, "A", vec![level(90.0, 10.0)]);
    let end_before = position(&state, line, 1);

    ok(move_station(&state, at(line, 0), [5.0, 0.0, 0.0]));
    assert_close(
        position(&state, line, 0),
        [5.0, 0.0, 0.0],
        "station 0 moved",
    );
    assert_close(position(&state, line, 1), end_before, "station 1 stays");
    // 0.6.0 moved the line offset; the free piece's placement does it now.
    let p = project(&state);
    assert_eq!(p.placements[0].station, p.line(line).unwrap().start);
    assert!((p.placements[0].x - 5.0).abs() < 1e-6 && p.placements[0].y.abs() < 1e-6);
    assert!((p.line(line).unwrap().shots[0].length - 5.0).abs() < 1e-6);
}

#[test]
fn move_station_undo() {
    let state = new_state();
    let line = add_line(&state, "A", vec![level(90.0, 10.0)]);
    let before = project(&state);

    // Station 0 by (5, 0, 0): the following shot becomes (5, 0, 0).
    ok(move_station(&state, at(line, 0), [5.0, 0.0, 0.0]));
    assert!((project(&state).line(line).unwrap().shots[0].length - 5.0).abs() < 1e-6);

    ok(state.undo());
    assert_eq!(
        canonical(&project(&state)),
        canonical(&before),
        "undo restores the shot and the placement exactly"
    );
}

#[test]
fn move_station_redo() {
    let state = new_state();
    let line = add_line(&state, "A", vec![level(90.0, 10.0)]);

    ok(move_station(&state, at(line, 1), [5.0, 0.0, 0.0]));
    let moved = project(&state);
    assert!((moved.line(line).unwrap().shots[0].length - 15.0).abs() < 1e-6);
    ok(state.undo());
    assert!((project(&state).line(line).unwrap().shots[0].length - 10.0).abs() < 1e-6);
    ok(state.redo());
    assert_eq!(canonical(&project(&state)), canonical(&moved), "redo");
    let redone = project(&state);
    assert!((redone.line(line).unwrap().shots[0].azimuth_in - 90.0).abs() < 1e-6);
}

#[test]
fn a_station_move_shows_in_its_coordinates() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    let before = position(&state, line, 1);
    ok(move_station(&state, at(line, 1), [100.0, 200.0, 50.0]));
    assert_close(
        position(&state, line, 1),
        plus(before, [100.0, 200.0, 50.0]),
        "the station moved by exactly the offset",
    );
}

#[test]
fn a_station_move_folds_a_legacy_hand_move_into_the_shots() {
    // 0.6.0 cleared the station offset it folded into the shots ("Station
    // offset should be cleared"); a migrated file keeps one per line as a
    // hand move, and Move Station folds it the same way.
    let state = new_state();
    let line = add_line(&state, "A", vec![level(90.0, 10.0), level(90.0, 10.0)]);
    edit(&state, |p| {
        let s = p.station_at(at(line, 1)).unwrap();
        p.line_mut(line)
            .unwrap()
            .hand_moves
            .insert(s, [0.5, 1.0, 0.25]);
    });
    let shown = position(&state, line, 1);
    let before = project(&state);
    ok(move_station(&state, at(line, 1), [1.0, 0.0, 0.0]));
    assert!(project(&state).line(line).unwrap().hand_moves.is_empty());
    assert_close(
        position(&state, line, 1),
        plus(shown, [1.0, 0.0, 0.0]),
        "where it was shown, plus the offset",
    );
    assert_close(
        position(&state, line, 2),
        [20.0, 0.0, 0.0],
        "its neighbour stays",
    );
    ok(state.undo());
    assert_eq!(
        canonical(&project(&state)),
        canonical(&before),
        "undo puts the hand move back"
    );
}

// === Move Station edge cases ===

#[test]
fn move_station_refuses_a_zero_length_result() {
    let state = new_state();
    let line = add_line(&state, "A", vec![level(0.0, 1.0)]);
    let before = project(&state);
    let error = refused(move_station(&state, at(line, 1), [0.0, -1.0, 0.0]));
    assert!(error.contains("zero-length"), "{error}");
    assert_eq!(error, "Move would create zero-length shot");
    assert_eq!(canonical(&project(&state)), canonical(&before));
}

#[test]
fn moving_the_last_station_rewrites_only_the_shot_into_it() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0), shot(3.0)]);
    let before = project(&state);
    ok(move_station(&state, at(line, 2), [1.0, 0.0, 0.0]));
    let after = project(&state);
    let (old, new) = (before.line(line).unwrap(), after.line(line).unwrap());
    assert_eq!(new.shots.len(), 2);
    assert_eq!(new.shots[0], old.shots[0], "the first shot is untouched");
    assert_ne!(new.shots[1], old.shots[1], "the last shot is rewritten");
}

#[test]
fn move_station_refuses_a_missing_slot_in_0_6_0_words() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    assert_eq!(
        refused(move_station(&state, at(line, 999), [1.0, 0.0, 0.0])),
        "Invalid station index"
    );
    assert_eq!(
        refused(move_station(&state, at(999, 0), [1.0, 0.0, 0.0])),
        "Line not found"
    );
}

#[test]
fn non_finite_offsets_are_refused_with_0_6_0_words() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    let before = project(&state);
    let nan = Coordinates::new(f64::NAN, 0.0, 0.0);
    for cmd in [
        Command::MoveLine {
            line_id: line,
            offset: nan,
        },
        Command::MoveSegment {
            line_id: line,
            offset: nan,
        },
    ] {
        assert_eq!(
            refused(state.execute(cmd)),
            "Move offset must be finite (no NaN/inf)"
        );
    }
    assert_eq!(
        refused(move_station(&state, at(line, 1), [f64::INFINITY, 0.0, 0.0])),
        "Move offset has a non-finite (NaN/inf) value"
    );
    // Before the slot is even looked at, as 0.6.0 checked it.
    assert_eq!(
        refused(move_station(&state, at(999, 0), [0.0, f64::NAN, 0.0])),
        "Move offset has a non-finite (NaN/inf) value"
    );
    assert_eq!(canonical(&project(&state)), canonical(&before));
    assert_eq!(entries(&state), 0);
    ok(move_segment(&state, line, 1.0, 2.0));
}

#[test]
fn move_station_is_refused_on_a_datum_station_on_any_of_its_lines() {
    let state = new_state();
    let a = add_line(&state, "A", vec![level(90.0, 10.0), level(90.0, 10.0)]);
    let b = add_branch(&state, "B", at(a, 1), vec![level(0.0, 5.0)]);
    edit(&state, |p| {
        p.set_datum_at(at(a, 1), gps(24.55, -81.78), None).unwrap();
    });
    let before = project(&state);
    for slot in [at(a, 1), at(b, 0)] {
        assert_eq!(
            refused(move_station(&state, slot, [1.0, 0.0, 0.0])),
            "Cannot move a zero datum station",
            "{slot:?}"
        );
    }
    assert_eq!(canonical(&project(&state)), canonical(&before));
    // Its neighbour is not a datum station.
    ok(move_station(&state, at(a, 2), [1.0, 0.0, 0.0]));
}

#[test]
fn moving_a_junction_rewrites_every_incident_shot_and_its_neighbours_stay() {
    // A junction of three lines: A passes it (two shots), B and C start
    // there. One Move Station rewrites all four shots.
    let state = new_state();
    let a = add_line(&state, "A", vec![level(90.0, 10.0), level(90.0, 10.0)]);
    let b = add_branch(&state, "B", at(a, 1), vec![level(0.0, 5.0)]);
    let c = add_branch(&state, "C", at(a, 1), vec![level(180.0, 5.0)]);
    let neighbours = [(a, 0), (a, 2), (b, 1), (c, 1)];
    let held: Vec<[f64; 3]> = neighbours
        .iter()
        .map(|&(l, i)| position(&state, l, i))
        .collect();
    let junction = position(&state, a, 1);

    ok(move_station(&state, at(b, 0), [2.0, 3.0, 0.0]));
    for (slot, was) in neighbours.iter().zip(&held) {
        assert_close(position(&state, slot.0, slot.1), *was, "a neighbour stays");
    }
    let moved = plus(junction, [2.0, 3.0, 0.0]);
    for slot in [(a, 1), (b, 0), (c, 0)] {
        assert_close(position(&state, slot.0, slot.1), moved, "every slot moves");
    }
    assert_eq!(entries(&state), 1);
    ok(state.undo());
    assert_close(position(&state, c, 0), junction, "undo puts it back");
}

#[test]
fn a_batch_moving_every_slot_of_one_station_moves_it_once() {
    // The map's co-located drag sends one Move Station per slot; a junction
    // has a slot on each of its lines but is one station (plan 9.1).
    let state = new_state();
    let a = add_line(&state, "A", vec![level(90.0, 10.0), level(90.0, 10.0)]);
    let b = add_branch(&state, "B", at(a, 1), vec![level(0.0, 5.0)]);
    let c = add_branch(&state, "C", at(a, 1), vec![level(180.0, 5.0)]);
    let junction = position(&state, a, 1);
    let offset = Coordinates::new(2.0, 3.0, 0.0);
    let before = project(&state);

    let batch = |slots: &[StationRef]| Command::Batch {
        commands: slots
            .iter()
            .map(|&slot| Command::MoveStation {
                station_ref: slot,
                offset,
            })
            .collect(),
        label: None,
    };
    ok(state.execute(batch(&[at(a, 1), at(b, 0), at(c, 0)])));
    let once = plus(junction, [2.0, 3.0, 0.0]);
    assert_close(position(&state, b, 0), once, "moved once, not three times");
    assert_eq!(entries(&state), 1, "one entry for the drag");
    let moved = project(&state);

    // Exactly what one Move Station does.
    ok(state.undo());
    assert_eq!(canonical(&project(&state)), canonical(&before));
    ok(move_station(&state, at(c, 0), [2.0, 3.0, 0.0]));
    assert_eq!(canonical(&project(&state)), canonical(&moved));
}

#[test]
fn a_batch_moving_both_ends_of_a_shot_moves_both() {
    // The move-shot drag: both ends (different stations) and their
    // co-located slots, one offset. Every distinct station moves; the shot
    // itself keeps its vector.
    let state = new_state();
    let a = add_line(
        &state,
        "A",
        vec![level(90.0, 10.0), level(90.0, 10.0), level(90.0, 10.0)],
    );
    let b = add_branch(&state, "B", at(a, 1), vec![level(0.0, 5.0)]);
    let offset = Coordinates::new(0.0, 4.0, 0.0);
    let (from, to) = (position(&state, a, 1), position(&state, a, 2));
    let middle = project(&state).line(a).unwrap().shots[1].clone();
    ok(state.execute(Command::Batch {
        commands: [at(a, 1), at(b, 0), at(a, 2)]
            .into_iter()
            .map(|slot| Command::MoveStation {
                station_ref: slot,
                offset,
            })
            .collect(),
        label: None,
    }));
    assert_close(position(&state, a, 1), plus(from, [0.0, 4.0, 0.0]), "from");
    assert_close(position(&state, a, 2), plus(to, [0.0, 4.0, 0.0]), "to");
    let moved = project(&state).line(a).unwrap().shots[1].clone();
    assert!((moved.length - middle.length).abs() < 1e-9);
    assert!((moved.azimuth_in - middle.azimuth_in).abs() < 1e-9);
}

#[test]
fn a_repeated_station_move_after_another_command_is_its_own_move() {
    // Only an unbroken run of Move Station collapses: any other command
    // between two moves of one station ends it.
    let state = new_state();
    let a = add_line(&state, "A", vec![level(90.0, 10.0), level(90.0, 10.0)]);
    let b = add_line(&state, "B", vec![level(0.0, 5.0)]);
    let start = position(&state, a, 2);
    let offset = Coordinates::new(1.0, 0.0, 0.0);
    let step = Command::MoveStation {
        station_ref: at(a, 2),
        offset,
    };
    ok(state.execute(Command::Batch {
        commands: vec![
            step.clone(),
            Command::MoveLine {
                line_id: b,
                offset: Coordinates::new(0.0, 1.0, 0.0),
            },
            step,
        ],
        label: None,
    }));
    assert_close(
        position(&state, a, 2),
        plus(start, [2.0, 0.0, 0.0]),
        "twice",
    );
}

#[test]
fn a_zero_offset_segment_move_records_nothing_and_keeps_redo() {
    // (A zero Move Station still rewrites its shots from their vectors, as
    // 0.6.0's did; the map never sends one below its 1 mm threshold.)
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    ok(move_line(&state, line, 1.0, 0.0));
    ok(state.undo());
    assert!(state.can_redo());
    for cmd in [
        Command::MoveLine {
            line_id: line,
            offset: Coordinates::new(0.0, 0.0, 0.0),
        },
        Command::MoveSegment {
            line_id: line,
            offset: Coordinates::new(0.0, 0.0, 0.0),
        },
    ] {
        ok(state.execute(cmd));
    }
    assert_eq!(entries(&state), 0, "no entry");
    assert!(state.can_redo(), "the redo branch survives");
}

// === Move Segment ===

/// A and B (A's end is B's start): one piece.
fn two_joined_lines(state: &AppState) -> (LineId, LineId) {
    let a = add_line(state, "A", vec![shot(5.0)]);
    let b = add_line(state, "B", vec![shot(3.0)]);
    ok(state.execute(Command::MergeStations {
        station_a: at(a, 1),
        station_b: at(b, 0),
    }));
    (a, b)
}

#[test]
fn move_segment_moves_every_line_of_the_piece() {
    let state = new_state();
    let (a, b) = two_joined_lines(&state);
    let (a0, b1) = (position(&state, a, 0), position(&state, b, 1));
    ok(move_segment(&state, a, 10.0, 20.0));
    assert_close(position(&state, a, 0), plus(a0, [10.0, 20.0, 0.0]), "A");
    assert_close(position(&state, b, 1), plus(b1, [10.0, 20.0, 0.0]), "B");
}

#[test]
fn move_segment_undo_redo() {
    let state = new_state();
    let (a, b) = two_joined_lines(&state);
    let before = project(&state);
    ok(move_segment(&state, a, 10.0, 20.0));
    let after = project(&state);
    ok(state.undo());
    assert_eq!(canonical(&project(&state)), canonical(&before), "undo");
    ok(state.redo());
    assert_eq!(canonical(&project(&state)), canonical(&after), "redo");
    assert_close(
        position(&state, b, 1),
        plus(
            before.raw_positions().slot(&before, at(b, 1)).unwrap(),
            [10.0, 20.0, 0.0],
        ),
        "redo re-applies the offset",
    );
}

#[test]
fn move_segment_of_a_missing_line_is_refused() {
    let state = new_state();
    assert_eq!(
        refused(move_segment(&state, 999, 10.0, 20.0)),
        "Line not found"
    );
}

#[test]
fn move_segment_is_refused_when_its_group_holds_a_datum() {
    // A holds the datum; B is joined to A at a junction, so dragging B would
    // drag the datum's piece along (0.6.0's widened guard: the group, not the
    // line). Two pieces joined only by an aligned closure both hold a datum
    // on the station graph (invariant 7), so the closure-only case of 0.6.0's
    // synthetic test is this same refusal.
    let state = new_state();
    let (a, b) = two_joined_lines(&state);
    edit(&state, |p| {
        p.set_datum_at(at(a, 0), gps(24.55, -81.78), None).unwrap();
    });
    let before = project(&state);
    for line in [a, b] {
        assert_eq!(
            refused(move_segment(&state, line, 5.0, 15.0)),
            "Cannot move a segment whose rigid group holds a zero datum"
        );
        assert_eq!(
            refused(move_line(&state, line, 5.0, 15.0)),
            "Cannot move a segment whose rigid group holds a zero datum"
        );
    }
    assert_eq!(canonical(&project(&state)), canonical(&before));

    // A snapped (aligned) closure between two datum pieces: still refused.
    let c = add_line(&state, "C", vec![shot(4.0)]);
    edit(&state, |p| {
        p.set_datum_at(at(c, 0), gps(24.5501, -81.7801), None)
            .unwrap();
        let (bs, cs) = (
            p.station_at(at(b, 1)).unwrap(),
            p.station_at(at(c, 1)).unwrap(),
        );
        p.push_closure(bs, Some(b), cs, Some(c), ClosureStrategy::Move);
    });
    assert_eq!(
        refused(move_segment(&state, c, 1.0, 0.0)),
        "Cannot move a segment whose rigid group holds a zero datum"
    );
}

#[test]
fn move_segment_leaves_an_unconnected_line_alone() {
    let state = new_state();
    let a = add_line(&state, "A", vec![shot(5.0)]);
    let b = add_line(&state, "B", vec![shot(3.0)]);
    let (a1, b1) = (position(&state, a, 1), position(&state, b, 1));
    ok(move_segment(&state, a, 10.0, 20.0));
    assert_close(
        position(&state, a, 1),
        plus(a1, [10.0, 20.0, 0.0]),
        "A moved",
    );
    assert_close(position(&state, b, 1), b1, "B did not");
}

#[test]
fn move_segment_accumulates() {
    let state = new_state();
    let a = add_line(&state, "A", vec![shot(5.0)]);
    let start = position(&state, a, 0);
    ok(move_segment(&state, a, 10.0, 5.0));
    assert_close(
        position(&state, a, 0),
        plus(start, [10.0, 5.0, 0.0]),
        "first",
    );
    ok(move_segment(&state, a, 3.0, 7.0));
    assert_close(
        position(&state, a, 0),
        plus(start, [13.0, 12.0, 0.0]),
        "second",
    );
    ok(move_segment(&state, a, -5.0, -2.0));
    assert_close(
        position(&state, a, 0),
        plus(start, [8.0, 10.0, 0.0]),
        "third",
    );
}

#[test]
fn move_line_of_a_branch_moves_the_piece_whose_placement_is_elsewhere() {
    // B's piece is placed at A's first station; moving B moves the placement.
    let state = new_state();
    let a = add_line(&state, "A", vec![level(90.0, 10.0)]);
    let b = add_branch(&state, "B", at(a, 1), vec![level(0.0, 5.0)]);
    let a0 = position(&state, a, 0);
    ok(move_line(&state, b, 0.0, -3.0));
    assert_close(
        position(&state, a, 0),
        plus(a0, [0.0, -3.0, 0.0]),
        "A moved",
    );
    let p = project(&state);
    assert_eq!(p.placements.len(), 1);
    assert_eq!(p.placements[0].station, p.line(a).unwrap().start);
}

#[test]
fn a_moved_line_stays_moved_after_save_and_reopen_and_undoes_there() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("moves.stalx");
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    let before = project(&state);
    ok(move_line(&state, line, 7.0, -2.0));
    save(&state, &path);

    let reopened = reopen(&path);
    assert_close(position(&reopened, line, 0), [7.0, -2.0, 10.0], "reopened");
    ok(reopened.undo());
    let back = project(&reopened);
    assert_eq!(back.placements, before.placements, "undo through the file");
}

#[test]
fn a_lone_move_records_0_6_0_preview_rows() {
    let state = new_state();
    let (a, _) = two_joined_lines(&state);
    ok(move_line(&state, a, 0.0, 5.0));
    assert_eq!(
        newest_preview(&state),
        [
            ("Line".to_string(), "A".to_string()),
            ("Offset".to_string(), "5.0m N".to_string())
        ]
    );
    ok(move_segment(&state, a, 5.0, 0.0));
    assert_eq!(
        newest_preview(&state),
        [
            ("Offset".to_string(), "5.0m E".to_string()),
            ("Affected lines".to_string(), "2".to_string())
        ]
    );
    ok(move_station(&state, at(a, 0), [0.0, -2.0, 1.0]));
    assert_eq!(
        newest_preview(&state),
        [("Offset".to_string(), "2.0m S, 1.0m down".to_string())]
    );
}

// === Comments ===

#[test]
fn set_station_comment_sets_updates_and_removes() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    let slot = at(line, 1);
    ok(set_comment(&state, slot, Some("marker A")));
    assert_eq!(comment(&state, slot).as_deref(), Some("marker A"));
    ok(set_comment(&state, slot, Some("marker B")));
    assert_eq!(comment(&state, slot).as_deref(), Some("marker B"));
    ok(set_comment(&state, slot, None));
    assert_eq!(comment(&state, slot), None, "None removes the comment");
    ok(set_comment(&state, slot, Some("temp")));
    ok(set_comment(&state, slot, Some("")));
    assert_eq!(comment(&state, slot), None, "an empty string removes it");
    // Stored as typed, not trimmed (0.6.0).
    ok(set_comment(&state, slot, Some("  padded ")));
    assert_eq!(comment(&state, slot).as_deref(), Some("  padded "));
}

#[test]
fn set_station_comment_refuses_a_missing_slot_in_0_6_0_words() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(3.0)]);
    assert_eq!(
        refused(set_comment(&state, at(line, 3), Some("oob"))),
        "Station index 3 out of range (line has 2 stations)"
    );
    assert_eq!(
        refused(set_comment(&state, at(9999, 0), Some("ghost"))),
        LINE_GONE
    );
    assert_eq!(entries(&state), 0);
}

#[test]
fn a_station_comment_survives_an_unrelated_undo_and_redo() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0), shot(3.0)]);
    let slot = at(line, 1);
    ok(set_comment(&state, slot, Some("keep me")));
    ok(run_model(&state, "create_line", |p| {
        p.create_line(meta("B"), StartAt::New, vec![shot(2.0)], Some([50.0, 0.0]))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }));
    ok(state.undo());
    assert_eq!(comment(&state, slot).as_deref(), Some("keep me"), "undo");
    ok(state.redo());
    assert_eq!(comment(&state, slot).as_deref(), Some("keep me"), "redo");
}

#[test]
fn a_station_comment_persists_across_save_and_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("comments.stalx");
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    let slot = at(line, 1);
    ok(set_comment(&state, slot, Some("persisted note")));
    save(&state, &path);
    let reopened = reopen(&path);
    assert_eq!(comment(&reopened, slot).as_deref(), Some("persisted note"));
    ok(reopened.undo());
    assert_eq!(comment(&reopened, slot), None, "undone through the file");
}

#[test]
fn a_junction_has_one_comment_on_every_slot() {
    let state = new_state();
    let a = add_line(&state, "A", vec![shot(5.0), shot(5.0)]);
    let b = add_branch(&state, "B", at(a, 1), vec![shot(3.0)]);
    ok(set_comment(&state, at(b, 0), Some("junction")));
    assert_eq!(comment(&state, at(a, 1)).as_deref(), Some("junction"));
    ok(set_comment(&state, at(a, 1), None));
    assert_eq!(comment(&state, at(b, 0)), None);
}

#[test]
fn move_comment_transfers_between_stations() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0), shot(3.0)]);
    let (source, target) = (at(line, 0), at(line, 2));
    ok(set_comment(&state, source, Some("source text")));
    ok(move_comment(
        &state,
        station_holder(source),
        station_holder(target),
        false,
    ));
    assert_eq!(comment(&state, source), None, "the source is cleared");
    assert_eq!(comment(&state, target).as_deref(), Some("source text"));
    assert_eq!(entries(&state), 2, "one entry for the move");
    ok(state.undo());
    assert_eq!(comment(&state, source).as_deref(), Some("source text"));
    assert_eq!(comment(&state, target), None);
}

#[test]
fn move_comment_swap_exchanges_comments() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0), shot(3.0)]);
    let (s1, s2) = (at(line, 0), at(line, 2));
    ok(set_comment(&state, s1, Some("alpha")));
    ok(set_comment(&state, s2, Some("beta")));
    ok(move_comment(
        &state,
        station_holder(s1),
        station_holder(s2),
        true,
    ));
    assert_eq!(comment(&state, s1).as_deref(), Some("beta"));
    assert_eq!(comment(&state, s2).as_deref(), Some("alpha"));
}

#[test]
fn move_comment_between_a_station_and_a_shot() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0), shot(3.0)]);
    ok(set_comment(&state, at(line, 1), Some("on the station")));
    ok(move_comment(
        &state,
        station_holder(at(line, 1)),
        shot_holder(&state, line, 1),
        false,
    ));
    assert_eq!(comment(&state, at(line, 1)), None);
    assert_eq!(
        shot_comment(&state, line, 1).as_deref(),
        Some("on the station")
    );

    // A swap with a station that has none leaves the shot without one.
    ok(move_comment(
        &state,
        shot_holder(&state, line, 1),
        station_holder(at(line, 2)),
        true,
    ));
    assert_eq!(shot_comment(&state, line, 1), None);
    assert_eq!(
        comment(&state, at(line, 2)).as_deref(),
        Some("on the station")
    );
}

#[test]
fn move_comment_refusals_in_0_6_0_words() {
    let state = new_state();
    let line = add_line(&state, "A", vec![shot(5.0)]);
    let empty = station_holder(at(line, 0));
    let other = station_holder(at(line, 1));
    assert_eq!(
        refused(move_comment(&state, empty, other.clone(), false)),
        "Source has no comment to move"
    );
    assert_eq!(
        refused(move_comment(
            &state,
            station_holder(at(line, 7)),
            other.clone(),
            false
        )),
        "Station index 7 out of range"
    );
    assert_eq!(
        refused(move_comment(
            &state,
            other.clone(),
            CommentHolder::Shot {
                line_id: 999,
                shot_id: 0
            },
            false
        )),
        LINE_GONE
    );
    assert_eq!(
        refused(move_comment(
            &state,
            other,
            CommentHolder::Shot {
                line_id: line,
                shot_id: u32::MAX
            },
            false
        )),
        SHOT_GONE
    );
    assert_eq!(entries(&state), 0);
}

#[test]
fn moving_a_comment_onto_another_slot_of_its_junction_changes_nothing() {
    // Two slots of one junction are one station, so the comment is already
    // there; 0.6.0's two writes (target, then clear the source) would have
    // deleted it.
    let state = new_state();
    let a = add_line(&state, "A", vec![shot(5.0), shot(5.0)]);
    let b = add_branch(&state, "B", at(a, 1), vec![shot(3.0)]);
    ok(set_comment(&state, at(a, 1), Some("junction")));
    let history = entries(&state);
    for swap in [false, true] {
        ok(move_comment(
            &state,
            station_holder(at(a, 1)),
            station_holder(at(b, 0)),
            swap,
        ));
        assert_eq!(comment(&state, at(b, 0)).as_deref(), Some("junction"));
    }
    assert_eq!(entries(&state), history, "no entry");
}
