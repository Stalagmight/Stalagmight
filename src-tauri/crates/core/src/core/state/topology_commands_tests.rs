//! The station topology commands through `AppState::execute` on the
//! station-graph model: Merge Stations, Unmerge (station and pair), Create
//! and Remove Loop Closure, Set and Remove Zero Datum, and the read-only
//! queries the Merge, closure and Move tools ask first.
//!
//! Ported from 0.6.0's `state/tests.rs` sections "Station Merge Tests",
//! "Zero Datum Tests", "CreateLoopClosure undo/redo tests", "Zero datum merge
//! validation tests", "Full pipeline: transitive closures through AppState",
//! "Merge chain tests" and "Group 3: Multi-line merge chain operations". A
//! merge is one shared station now (plan 1.2): what the legacy tests checked
//! on `station_merges` and line offsets they check here on the derived merge
//! pairs of the wire and on the raw layout, and every undo is checked to
//! land on the state before to the bit (`topology_kit::assert_same`).
//!
//! Fates of the legacy tests:
//! - `test_undo_create_loop_closure_has_closure_info` read the old
//!   `UndoData::ClosureInfo`: now the entry's kind, label and refs, and the
//!   closure as stored;
//! - `test_undo_unmerge_restores_removed_closures` injected a closure between
//!   the two merged stations, which is one station now (invariant 7): it is a
//!   closure the detach hands to the detached line and the sweep promotes;
//! - `test_delete_middle_line_removes_both_merge_edges` and
//!   `test_delete_middle_line_in_merge_chain` are one test;
//! - `test_insert_before_pos0_shifts_merge_refs_in_chain` is ported and
//!   ignored until Create Shot lands (package C2);
//! - every test that injected a closure into the project state behind the
//!   command path creates it through Create Loop Closure, which now accepts
//!   a closure between two placed pieces.
//!
//! All data is synthetic.

use super::topology_kit::{
    apart, assert_near, assert_round_trip, assert_same, deep, empty, gps_at, history, line_at,
    line_from, merge, open, origin, project, raw_xy, refused, run, save_close_reopen, set_datum,
    shot, slot, snapshot, st, warnings, xy,
};
use super::AppState;
use crate::core::Command;
use crate::models::survey::{ClosureStrategy, Project};
use crate::models::{
    ClosureStrategy as WireStrategy, GpsCoordinates, LineId, ShotKind, StationRef,
    CLOSURE_REMOVED_MERGED_ENDS,
};

/// 0.6.0's `test_shot_data`: a 45 degree shot from 10 m to 11 m deep.
fn sloped(length: f64) -> crate::models::ShotData {
    deep(45.0, length, 10.0, 11.0)
}

/// The derived merge pairs of the wire (plan 9.2), each pair sorted.
fn merge_pairs(state: &AppState) -> Vec<(StationRef, StationRef)> {
    let mut pairs: Vec<_> = project(state)
        .merge_pairs()
        .into_iter()
        .map(|(a, b)| if a <= b { (a, b) } else { (b, a) })
        .collect();
    pairs.sort();
    pairs
}

fn pair(a: StationRef, b: StationRef) -> (StationRef, StationRef) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The lines of `line`'s piece (shots and junctions; closures do not join).
fn piece_of(p: &Project, line: LineId) -> Vec<LineId> {
    p.piece_lines(p.line(line).unwrap().start)
}

fn closure(state: &AppState, a: StationRef, b: StationRef) -> crate::core::CommandResult {
    run(
        state,
        Command::CreateLoopClosure {
            station_a: a,
            station_b: b,
            closure_strategy: WireStrategy::Batch,
        },
    )
}

/// Three free lines A (2 shots), B and C (1 shot each), all at the origin as
/// 0.6.0's Create Line put them.
fn three_lines(a_shots: usize) -> (Project, LineId, LineId, LineId) {
    let mut p = empty("Chain");
    let a = line_at(
        &mut p,
        "A",
        [0.0, 0.0],
        (0..a_shots).map(|_| sloped(5.0)).collect(),
    );
    let b = line_at(&mut p, "B", [0.0, 0.0], vec![sloped(4.0)]);
    let c = line_at(&mut p, "C", [0.0, 0.0], vec![sloped(6.0)]);
    (p, a, b, c)
}

// === Station Merge Tests ===

#[test]
fn a_merge_on_one_line_is_refused_in_0_6_0_words() {
    let mut p = empty("Test");
    let line = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(5.0), sloped(7.0)]);
    let opened = open(&p);
    let error = refused(
        &opened.state,
        Command::MergeStations {
            station_a: slot(line, 0),
            station_b: slot(line, 1),
        },
    );
    assert_eq!(error, "Cannot merge stations from the same line");
}

#[test]
fn a_merge_makes_one_station_records_one_entry_and_undoes_exactly() {
    let (p, a, b, _) = three_lines(1);
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);
    let r = merge(state, slot(a, 1), slot(b, 0));
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let merged = project(state);
    assert_eq!(st(&merged, a, 1), st(&merged, b, 0), "one station");
    assert_eq!(merge_pairs(state), [pair(slot(a, 1), slot(b, 0))]);
    assert_eq!(
        history(state)[0],
        (
            "merge_stations".to_string(),
            "Merge stations A / 1 and B / 0".to_string()
        )
    );
    assert_round_trip(state, &before, "merge");
}

#[test]
fn a_merge_of_connected_stations_routes_to_the_closure_dialog() {
    let (p, a, b, c) = three_lines(2);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 2), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    let error = refused(
        state,
        Command::MergeStations {
            station_a: slot(c, 1),
            station_b: slot(a, 0),
        },
    );
    assert_eq!(
        error,
        "Stations are already connected. Use Create Loop Closure instead."
    );
    // A slot that names no station is refused too, in 0.6.0's words.
    assert_eq!(
        refused(
            state,
            Command::MergeStations {
                station_a: slot(c, 9),
                station_b: slot(a, 0),
            },
        ),
        "One or both stations do not exist"
    );
}

// === Zero Datum Tests ===

#[test]
fn set_and_remove_zero_datum() {
    let mut p = empty("Test");
    let line = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(5.0)]);
    let opened = open(&p);
    let state = &opened.state;
    let at = slot(line, 0);
    let before = snapshot(state);

    set_datum(
        state,
        at,
        GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
        None,
    );
    let p = project(state);
    assert_eq!(p.datums.len(), 1);
    assert_eq!(p.datums[0].gps.latitude, 45.0);
    assert_eq!(p.datums[0].via, Some(line), "attached via the slot's line");
    assert!(p.placements.is_empty(), "the datum places the piece");
    assert_eq!(
        history(state)[0],
        (
            "set_zero_datum".to_string(),
            "Set zero datum at A / 0".to_string()
        )
    );
    let set = snapshot(state);

    let r = run(state, Command::RemoveZeroDatum { station_ref: at });
    assert!(r.warnings.is_empty());
    let p = project(state);
    assert!(p.datums.is_empty());
    assert_eq!(p.placements.len(), 1, "the piece keeps its place");
    assert_eq!(
        history(state)[0],
        (
            "remove_zero_datum".to_string(),
            "Remove zero datum from A / 0".to_string()
        )
    );
    assert_round_trip(state, &set, "remove");
    assert!(state.undo().success);
    assert_same(&snapshot(state), &set, "remove undone");
    assert!(state.undo().success);
    assert_same(&snapshot(state), &before, "set undone");
}

#[test]
fn removing_a_datum_a_station_does_not_hold_is_refused() {
    let mut p = empty("Test");
    let line = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(5.0)]);
    let opened = open(&p);
    let error = refused(
        &opened.state,
        Command::RemoveZeroDatum {
            station_ref: slot(line, 0),
        },
    );
    assert_eq!(error, "Station is not a zero datum");
    // As 0.6.0, a slot that names no station reads the same.
    let error = refused(
        &opened.state,
        Command::RemoveZeroDatum {
            station_ref: slot(line, 9),
        },
    );
    assert_eq!(error, "Station is not a zero datum");
}

#[test]
fn an_invalid_gps_or_accuracy_is_refused_and_changes_nothing() {
    let mut p = empty("Test");
    let line = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(5.0)]);
    let opened = open(&p);
    let state = &opened.state;
    refused(
        state,
        Command::SetZeroDatum {
            station_ref: slot(line, 0),
            coordinates: GpsCoordinates {
                latitude: 95.0,
                longitude: 0.0,
                depth_reference: 0.0,
            },
            accuracy_m: None,
            entrance: None,
        },
    );
    refused(
        state,
        Command::SetZeroDatum {
            station_ref: slot(line, 0),
            coordinates: origin(),
            accuracy_m: Some(-1.0),
            entrance: None,
        },
    );
    assert_eq!(
        refused(
            state,
            Command::SetZeroDatum {
                station_ref: slot(line, 7),
                coordinates: origin(),
                accuracy_m: None,
                entrance: None,
            },
        ),
        "Station does not exist"
    );
}

// === CreateLoopClosure undo/redo tests ===

/// A (1 shot) and B (1 shot) merged A.1 = B.0, so A.0 and B.1 are two shots
/// apart. The junction is one station named `1` on both lines (plan 5.3),
/// so B's far station is `1a`.
fn merged_pair() -> (Project, LineId, LineId) {
    let mut p = empty("Test");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(10.0)]);
    let a1 = st(&p, a, 1);
    let b = line_from(&mut p, "B", a1, vec![sloped(5.0)]);
    (p, a, b)
}

#[test]
fn undo_create_loop_closure() {
    let (p, a, b) = merged_pair();
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);
    let r = closure(state, slot(a, 0), slot(b, 1));
    assert!(r.warnings.is_empty());
    assert_eq!(project(state).closures.len(), 1);
    let created = snapshot(state);

    assert!(state.undo().success);
    assert!(project(state).closures.is_empty());
    assert_same(&snapshot(state), &before, "undo");
    assert!(state.redo().success);
    assert_eq!(project(state).closures.len(), 1);
    assert_same(&snapshot(state), &created, "redo");
}

#[test]
fn a_created_closure_is_stored_on_its_slots_and_named_in_the_history() {
    let (p, a, b) = merged_pair();
    let opened = open(&p);
    let state = &opened.state;
    closure(state, slot(a, 0), slot(b, 1));
    let p = project(state);
    let c = p.closures[0];
    assert_eq!((c.a, c.a_via), (st(&p, a, 0), Some(a)));
    assert_eq!((c.b, c.b_via), (st(&p, b, 1), Some(b)));
    assert_eq!(c.strategy, ClosureStrategy::Batch);
    let wire = state.project_state_wire().unwrap().project.loop_closures;
    assert_eq!(wire.len(), 1);
    assert_eq!(
        (wire[0].station_a, wire[0].station_b),
        (slot(a, 0), slot(b, 1))
    );
    assert!(wire[0].path_length > 0.0, "{:?}", wire[0]);
    assert_eq!(
        history(state)[0],
        (
            "create_loop_closure".to_string(),
            "Create loop closure between A / 0 and B / 1a".to_string()
        )
    );
}

#[test]
fn create_loop_closure_deferred_strategy_is_rejected() {
    let mut p = empty("DistUndo");
    let line = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(10.0), sloped(6.0)]);
    let opened = open(&p);
    let error = refused(
        &opened.state,
        Command::CreateLoopClosure {
            station_a: slot(line, 0),
            station_b: slot(line, 2),
            closure_strategy: WireStrategy::ProportionalByLength,
        },
    );
    assert_eq!(
        error,
        "Closure distribution strategies are deferred. Use Batch."
    );
}

#[test]
fn create_loop_closure_refuses_in_0_6_0_order_and_words() {
    let (p, a, b) = merged_pair();
    let mut p = p;
    let free = line_at(&mut p, "Free", [50.0, 0.0], vec![sloped(3.0)]);
    let opened = open(&p);
    let state = &opened.state;
    let create = |x: StationRef, y: StationRef| Command::CreateLoopClosure {
        station_a: x,
        station_b: y,
        closure_strategy: WireStrategy::Batch,
    };
    assert_eq!(
        refused(state, create(slot(a, 0), slot(b, 5))),
        "One or both stations do not exist"
    );
    assert_eq!(
        refused(state, create(slot(a, 1), slot(b, 0))),
        "These stations are already merged. Closures are for unmerged station pairs."
    );
    assert_eq!(
        refused(state, create(slot(a, 0), slot(free, 1))),
        "Stations are not connected. They must be in the same connected group to create a \
         loop closure."
    );
    assert!(
        refused(state, create(slot(a, 0), slot(b, 0)))
            .starts_with("This closure would join the two ends of one shot (A / 0 to A / 1)"),
        "#737"
    );
    closure(state, slot(a, 0), slot(b, 1));
    assert_eq!(
        refused(state, create(slot(b, 1), slot(a, 0))),
        "A loop closure already exists between these stations."
    );
}

#[test]
fn remove_loop_closure_undoes_exactly_and_refuses_an_unknown_id() {
    let (p, a, b) = merged_pair();
    let opened = open(&p);
    let state = &opened.state;
    closure(state, slot(a, 0), slot(b, 1));
    let id = project(state).closures[0].id;
    let before = snapshot(state);
    run(state, Command::RemoveLoopClosure { closure_id: id });
    assert!(project(state).closures.is_empty());
    assert_eq!(history(state)[0].0, "remove_loop_closure");
    assert_round_trip(state, &before, "remove closure");
    assert_eq!(
        refused(state, Command::RemoveLoopClosure { closure_id: id + 7 }),
        "Loop closure not found"
    );
}

/// 0.6.0's `test_undo_unmerge_restores_removed_closures`, on the station
/// graph: the closure that belonged to B's slot of the junction follows B
/// when B is detached (plan 5.2), is then the only connection between the
/// two pieces, and the sweep promotes it. One undo restores the junction and
/// the closure.
#[test]
fn an_unmerge_that_orphans_a_closure_promotes_it_and_one_undo_restores_both() {
    let mut p = empty("Test");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(10.0), sloped(8.0)]);
    let a2 = st(&p, a, 2);
    let b = line_from(&mut p, "B", a2, vec![sloped(5.0)]);
    let opened = open(&p);
    let state = &opened.state;
    closure(state, slot(b, 0), slot(a, 0));
    let before = snapshot(state);

    let r = run(
        state,
        Command::UnmergeStationPair {
            station_a: slot(a, 2),
            station_b: slot(b, 0),
        },
    );
    assert!(r.warnings.is_empty(), "a promotion is logged, not reported");
    let p = project(state);
    assert!(p.closures.is_empty(), "promoted");
    assert_eq!(st(&p, b, 0), st(&p, a, 0), "B now hangs off A:0");
    assert_ne!(st(&p, b, 0), st(&p, a, 2));
    assert_round_trip(state, &before, "unmerge pair");
}

// === Unmerge ===

#[test]
fn unmerge_on_a_station_off_a_junction_is_a_no_op_that_keeps_redo() {
    let (p, a, b) = merged_pair();
    let opened = open(&p);
    let state = &opened.state;
    closure(state, slot(a, 0), slot(b, 1));
    assert!(state.undo().success);
    let before = snapshot(state);
    let r = run(
        state,
        Command::UnmergeStations {
            station_ref: slot(a, 0),
        },
    );
    assert!(r.warnings.is_empty());
    assert_same(&snapshot(state), &before, "nothing changed");
    assert!(state.can_redo(), "a provable no-op keeps the redo branch");
    assert!(state.get_undo_stack().is_empty(), "and records nothing");
    // As 0.6.0, a slot that names no station is the same no-op.
    run(
        state,
        Command::UnmergeStations {
            station_ref: slot(a, 9),
        },
    );
    assert_same(&snapshot(state), &before, "nothing changed");
    assert!(state.can_redo());
}

#[test]
fn unmerge_pair_needs_two_slots_of_one_station() {
    let (p, a, b) = merged_pair();
    let opened = open(&p);
    let state = &opened.state;
    let pair = |x: StationRef, y: StationRef| Command::UnmergeStationPair {
        station_a: x,
        station_b: y,
    };
    for (x, y) in [
        (slot(a, 0), slot(b, 1)),
        (slot(a, 1), slot(a, 1)),
        (slot(a, 1), slot(b, 9)),
    ] {
        assert_eq!(
            refused(state, pair(x, y)),
            "Stations are not directly merged"
        );
    }
    let before = snapshot(state);
    run(state, pair(slot(a, 1), slot(b, 0)));
    let p = project(state);
    assert_ne!(st(&p, a, 1), st(&p, b, 0));
    assert!(merge_pairs(state).is_empty());
    assert_eq!(
        history(state)[0],
        (
            "unmerge_station_pair".to_string(),
            "Detach line \"B\" from station 1".to_string()
        )
    );
    assert_round_trip(state, &before, "unmerge pair");
}

/// Declared change (plan 5.2): Unmerge on a hub of three lines detaches the
/// chosen slot's line only; the other two stay joined. Nothing moves.
#[test]
fn unmerge_on_a_hub_of_three_lines_detaches_only_that_line() {
    let mut p = empty("Hub");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let hub = st(&p, a, 1);
    let b = line_from(&mut p, "B", hub, vec![shot(0.0, 5.0)]);
    let c = line_from(&mut p, "C", hub, vec![shot(180.0, 4.0)]);
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);
    let layout = raw_xy(&project(state));

    run(
        state,
        Command::UnmergeStations {
            station_ref: slot(b, 0),
        },
    );
    let p = project(state);
    assert_eq!(st(&p, a, 1), hub, "A keeps the hub");
    assert_eq!(st(&p, c, 0), hub, "C stays on it");
    assert_ne!(st(&p, b, 0), hub, "B has its own station now");
    assert_eq!(
        p.stations[&st(&p, b, 0)].name,
        p.stations[&hub].name,
        "named as the hub"
    );
    assert_eq!(merge_pairs(state), [pair(slot(a, 1), slot(c, 0))]);
    super::topology_kit::assert_same_xy(&layout, &raw_xy(&p));
    assert_round_trip(state, &before, "detach one line");
}

// === Sweep warnings (#738) ===

#[test]
fn merging_two_placed_pieces_reports_the_closure_it_folds() {
    // Two pieces each placed by its own datum, tied by a closure between
    // A:1 and B:1; merging those two stations makes the closure one point.
    let mut p = empty("TwoPlaced");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(0.0, 10.0)]);
    let b = line_at(&mut p, "B", [30.0, 0.0], vec![shot(0.0, 10.0)]);
    let opened = open(&p);
    let state = &opened.state;
    set_datum(state, slot(a, 0), origin(), None);
    set_datum(state, slot(b, 0), gps_at(30.0, 0.0), None);
    closure(state, slot(a, 1), slot(b, 1));
    let before = snapshot(state);

    let r = merge(state, slot(a, 1), slot(b, 1));
    assert_eq!(
        warnings(&r),
        [(
            CLOSURE_REMOVED_MERGED_ENDS.to_string(),
            "Removed loop closure A / 1 to B / 1: the merge made its two stations one point."
                .to_string()
        )]
    );
    let p = project(state);
    assert!(p.closures.is_empty());
    assert_eq!(st(&p, a, 1), st(&p, b, 1));
    assert_round_trip(state, &before, "merge of two placed pieces");
}

#[test]
fn removing_a_zero_datum_reports_the_closure_its_sweep_removes() {
    // A placed at A:0 with a branch C off A:1; B placed on its own. A closure
    // B:1-A:1 bridges the two placed pieces and a second one C:0-B:1 ties the
    // same two stations' pieces. Removing B's datum leaves B unplaced: the
    // first closure is now B's sole bridge and is promoted (B:1 onto A:1),
    // which makes the second one's two ends one station.
    // The second closure joins the same two stations, which Create Loop
    // Closure refuses; it is planted as an old file can hold it.
    let mut p = empty("Datums");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let a1 = st(&p, a, 1);
    let c = line_from(&mut p, "C", a1, vec![shot(0.0, 3.0)]);
    let b = line_at(&mut p, "B", [0.0, -20.0], vec![shot(0.0, 10.0)]);
    p.set_datum_at(slot(a, 0), origin(), None).unwrap();
    p.set_datum_at(slot(b, 0), gps_at(0.0, -20.0), None)
        .unwrap();
    p.create_closure_at(slot(b, 1), slot(a, 1), ClosureStrategy::Batch)
        .unwrap();
    let b1 = st(&p, b, 1);
    p.push_closure(a1, Some(c), b1, Some(b), ClosureStrategy::Batch);
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let r = run(
        state,
        Command::RemoveZeroDatum {
            station_ref: slot(b, 0),
        },
    );
    assert_eq!(
        warnings(&r),
        [(
            CLOSURE_REMOVED_MERGED_ENDS.to_string(),
            "Removed loop closure C / 1 to B / 1: another loop closure became a merge, which \
             made its two stations one point."
                .to_string()
        )]
    );
    let p = project(state);
    assert!(p.closures.is_empty());
    assert_eq!(st(&p, b, 1), a1, "B hangs off A:1");
    assert_round_trip(state, &before, "remove datum");
}

// === Zero datum merge validation tests ===

/// A (1 shot) and B (1 shot), free, optionally each placed by its own datum.
fn two_free_lines(datum_a: bool, datum_b: bool) -> (AppState, tempfile::TempDir, LineId, LineId) {
    let mut p = empty("Merge check");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(10.0)]);
    let b = line_at(&mut p, "B", [0.0, 0.0], vec![sloped(5.0)]);
    let opened = open(&p);
    if datum_a {
        set_datum(
            &opened.state,
            slot(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
            None,
        );
    }
    if datum_b {
        set_datum(
            &opened.state,
            slot(b, 0),
            GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
            None,
        );
    }
    (opened.state, opened.dir, a, b)
}

#[test]
fn check_merge_connectivity_allows_two_placed_lines() {
    // Since #741 the later datum becomes a GPS fix and its line moves.
    let (state, _dir, a, b) = two_free_lines(true, true);
    let result = state
        .check_merge_connectivity(slot(a, 1), slot(b, 0))
        .unwrap();
    assert!(result.can_merge);
    assert!(!result.connected);
    assert_eq!(result.reason, None);
    let shift = result.datum_shift.expect("two placed pieces");
    assert_eq!(shift.moving_line_ids, [b]);
    assert_eq!(shift.datum_station, slot(b, 0));
}

#[test]
fn check_merge_connectivity_allows_one_or_no_placed_line() {
    for datum_a in [true, false] {
        let (state, _dir, a, b) = two_free_lines(datum_a, false);
        let result = state
            .check_merge_connectivity(slot(a, 1), slot(b, 0))
            .unwrap();
        assert!(result.can_merge, "datum on A: {datum_a}");
        assert!(!result.connected);
        assert!(result.reason.is_none());
        assert!(result.datum_shift.is_none());
        assert!(result.preview.is_none());
    }
}

#[test]
fn check_merge_connectivity_of_connected_stations_creates_a_loop() {
    let (state, _dir, a, b) = two_free_lines(false, false);
    merge(&state, slot(a, 1), slot(b, 0));
    let result = state
        .check_merge_connectivity(slot(a, 0), slot(b, 1))
        .unwrap();
    assert!(!result.can_merge);
    assert!(result.connected);
    assert_eq!(result.reason.as_deref(), Some("Creates survey loop"));
    let preview = result.preview.expect("the closure dialog's figures");
    assert!(preview.path_length > 0.0);
    assert!(result.closure_refusal.is_none(), "two shots apart");

    // One shot apart: the closure dialog can say why before Create (#737).
    let result = state
        .check_merge_connectivity(slot(a, 0), slot(a, 1))
        .unwrap();
    assert!(result.closure_refusal.is_some());

    // One station, or a pair a closure joins, is refused in 0.6.0's words.
    assert_eq!(
        state
            .check_merge_connectivity(slot(a, 1), slot(b, 0))
            .unwrap_err(),
        "These stations are already merged."
    );
    closure(&state, slot(a, 0), slot(b, 1));
    assert_eq!(
        state
            .check_merge_connectivity(slot(a, 0), slot(b, 1))
            .unwrap_err(),
        "A loop closure already exists between these stations."
    );
    assert_eq!(
        state
            .check_merge_connectivity(slot(a, 9), slot(b, 1))
            .unwrap_err(),
        "Station A does not exist."
    );
}

#[test]
fn check_merge_connectivity_of_two_placed_lines_joined_by_a_closure() {
    // A (placed) with C hanging off A:1; B placed on its own; a closure
    // C:1-B:0 joins the two placed pieces (a closure the command accepts
    // between two placed pieces).
    let mut p = empty("BothPlacedConnected");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(10.0)]);
    let b = line_at(&mut p, "B", [0.0, 0.0], vec![sloped(5.0)]);
    let c = line_at(&mut p, "C", [0.0, 0.0], vec![sloped(3.0)]);
    let opened = open(&p);
    let state = &opened.state;
    set_datum(
        state,
        slot(a, 0),
        GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        None,
    );
    set_datum(
        state,
        slot(b, 0),
        GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
        None,
    );
    merge(state, slot(a, 1), slot(c, 0));
    closure(state, slot(c, 1), slot(b, 0));

    let result = state
        .check_merge_connectivity(slot(a, 0), slot(b, 1))
        .unwrap();
    assert!(result.connected, "joined through the closure");
    assert!(result.preview.is_some());
    assert!(!result.can_merge);
    assert_eq!(result.reason.as_deref(), Some("Creates survey loop"));
}

// === Full pipeline: transitive closures through AppState ===

#[test]
fn closures_with_lists_direct_and_transitive_closures_on_junction_slots() {
    // A (2 shots), B, C; A:2 = B:0 and B:1 = C:0; a closure A:2-C:1.
    let (p, a, b, c) = three_lines(2);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 2), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    closure(state, slot(a, 2), slot(c, 1));

    let stations = state.project_state_wire().unwrap().stations;
    let closures_of = |at: StationRef| {
        stations
            .iter()
            .find(|s| s.station_ref == at)
            .unwrap()
            .closures()
            .to_vec()
    };
    let has = |at: StationRef, to: StationRef, transitive: bool, via: Option<StationRef>| {
        closures_of(at)
            .iter()
            .any(|r| r.station_ref == to && r.is_transitive == transitive && r.via_station == via)
    };
    assert!(
        has(slot(a, 2), slot(c, 1), false, None),
        "{:?}",
        closures_of(slot(a, 2))
    );
    assert!(
        has(slot(b, 0), slot(c, 1), true, Some(slot(a, 2))),
        "{:?}",
        closures_of(slot(b, 0))
    );
    assert!(
        has(slot(c, 1), slot(a, 2), false, None),
        "{:?}",
        closures_of(slot(c, 1))
    );
    assert!(
        has(slot(c, 1), slot(b, 0), true, Some(slot(a, 2))),
        "{:?}",
        closures_of(slot(c, 1))
    );
    for quiet in [slot(a, 0), slot(a, 1), slot(b, 1), slot(c, 0)] {
        assert!(
            closures_of(quiet).is_empty(),
            "{quiet:?}: {:?}",
            closures_of(quiet)
        );
    }
}

#[test]
fn closures_with_survives_serialization_and_is_absent_when_empty() {
    use crate::models::Station;

    let (p, a, b, c) = three_lines(1);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 1), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    closure(state, slot(a, 1), slot(c, 1));
    let stations = state.project_state_wire().unwrap().stations;
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == slot(b, 0))
        .unwrap();
    assert!(!b0.closures().is_empty(), "B:0 is on A:1's station");

    let json = serde_json::to_string(&stations).unwrap();
    let back: Vec<Station> = serde_json::from_str(&json).unwrap();
    let find = |at: StationRef| back.iter().find(|s| s.station_ref == at).unwrap();
    assert_eq!(find(slot(b, 0)).closures().len(), b0.closures().len());
    assert!(find(slot(b, 0)).closures().iter().any(|r| r.is_transitive));
    assert!(find(slot(a, 1)).closures().iter().any(|r| !r.is_transitive));
    assert!(find(slot(c, 1)).closures().iter().any(|r| !r.is_transitive));
    assert!(find(slot(c, 1)).closures().iter().any(|r| r.is_transitive));

    // #441: a station with no closures omits the key, and the generated
    // type declares it optional.
    let wire: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    let (station, closure_free) = stations
        .iter()
        .zip(&wire)
        .find(|(s, _)| s.closures().is_empty())
        .expect("this survey has stations with no closures");
    assert_eq!(
        closure_free.get("station_ref"),
        Some(&serde_json::to_value(station.station_ref).unwrap())
    );
    assert!(!closure_free
        .as_object()
        .unwrap()
        .contains_key("closures_with"));
    assert!(<Station as ts_rs::TS>::decl().contains("closures_with?:"));
}

// === Merge chain tests ===

/// 0.6.0's test 2a: in a chain A-B-C of free lines, both junctions are one
/// station and every line keeps its own geometry. Plan 4.2: the piece of
/// the first argument moves onto the second, so B stays for the first merge
/// and C for the second.
#[test]
fn a_merge_chain_lays_every_line_out_from_its_junctions() {
    let mut p = empty("Chain");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let b = line_at(&mut p, "B", [0.0, 0.0], vec![shot(0.0, 5.0)]);
    let c = line_at(&mut p, "C", [0.0, 0.0], vec![shot(90.0, 3.0)]);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 1), slot(b, 0));
    let p = project(state);
    assert_near(xy(&p, slot(b, 0)), [0.0, 0.0], 1e-9, "B stays");
    assert_near(xy(&p, slot(a, 0)), [-10.0, 0.0], 1e-9, "A moved onto B");
    merge(state, slot(b, 1), slot(c, 0));
    let p = project(state);
    let expected = [
        (slot(c, 0), [0.0, 0.0]),
        (slot(c, 1), [3.0, 0.0]),
        (slot(b, 1), [0.0, 0.0]),
        (slot(b, 0), [0.0, -5.0]),
        (slot(a, 1), [0.0, -5.0]),
        (slot(a, 0), [-10.0, -5.0]),
    ];
    for (at, want) in expected {
        assert_near(xy(&p, at), want, 1e-9, &format!("{at:?}"));
    }
    assert_eq!(st(&p, a, 1), st(&p, b, 0));
    assert_eq!(st(&p, b, 1), st(&p, c, 0));
}

/// 0.6.0's test 2b: a prepend on the middle line of a chain. The junctions
/// are stations, so they stay where they were: A:1 is B:1 and B:2 is C:0
/// afterwards, with nothing to shift.
#[test]
fn a_prepend_on_the_middle_line_of_a_chain_keeps_both_junctions() {
    let mut p = empty("ShiftChain");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let b = line_at(&mut p, "B", [0.0, 0.0], vec![shot(0.0, 5.0)]);
    let c = line_at(&mut p, "C", [0.0, 0.0], vec![shot(90.0, 3.0)]);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 1), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    let layout = raw_xy(&project(state));
    let first = project(state).line(b).unwrap().shots[0].id;
    run(
        state,
        Command::CreateShot {
            line_id: b,
            shot: shot(270.0, 2.0),
            after_shot_id: None,
            before_shot_id: Some(first),
        },
    );
    assert_eq!(
        merge_pairs(state),
        [pair(slot(a, 1), slot(b, 1)), pair(slot(b, 2), slot(c, 0))]
    );
    let p = project(state);
    for (at, was) in [
        (slot(a, 1), slot(a, 1)),
        (slot(b, 1), slot(b, 0)),
        (slot(b, 2), slot(b, 1)),
        (slot(c, 0), slot(c, 0)),
    ] {
        assert_near(xy(&p, at), layout[&was], 1e-9, &format!("{at:?}"));
    }
}

/// 0.6.0's test 2c and Group 3's `test_delete_middle_line_in_merge_chain`:
/// deleting the middle line of a chain leaves both junction stations on
/// their other lines, which are now apart, with no reference to the line.
#[test]
fn deleting_the_middle_line_of_a_chain_leaves_its_ends_apart() {
    let (p, a, b, c) = three_lines(1);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 1), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    assert_eq!(merge_pairs(state).len(), 2);
    let layout = raw_xy(&project(state));
    let before = snapshot(state);

    run(state, Command::DeleteLine { line_id: b });
    let p = project(state);
    assert!(p.line(b).is_none());
    assert_eq!(p.lines().len(), 2);
    assert!(merge_pairs(state).is_empty());
    assert_eq!(piece_of(&p, a), [a]);
    assert_eq!(piece_of(&p, c), [c]);
    let wire = state.project_state_wire().unwrap().project;
    assert!(wire
        .station_merges
        .iter()
        .all(|m| m.station_a.line_id != b && m.station_b.line_id != b));
    assert!(wire
        .loop_closures
        .iter()
        .all(|l| l.station_a.line_id != b && l.station_b.line_id != b));
    assert!(wire.zero_datums.iter().all(|d| d.station_ref.line_id != b));
    for at in [slot(a, 0), slot(a, 1), slot(c, 0), slot(c, 1)] {
        assert_near(xy(&p, at), layout[&at], 1e-9, &format!("{at:?} stays"));
    }
    assert_round_trip(state, &before, "delete the middle line");
}

/// 0.6.0's test 2d: unmerging one junction of a chain leaves the other one
/// one station, A on its own, and nothing moves.
#[test]
fn unmerging_one_junction_of_a_chain_moves_nothing() {
    let mut p = empty("UnmergeChain");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let b = line_at(&mut p, "B", [0.0, 0.0], vec![shot(0.0, 5.0)]);
    let c = line_at(&mut p, "C", [0.0, 0.0], vec![shot(90.0, 3.0)]);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 1), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    let layout = raw_xy(&project(state));

    run(
        state,
        Command::UnmergeStations {
            station_ref: slot(a, 1),
        },
    );
    let p = project(state);
    assert_eq!(merge_pairs(state), [pair(slot(b, 1), slot(c, 0))]);
    assert_eq!(piece_of(&p, a), [a]);
    assert_eq!(st(&p, b, 1), st(&p, c, 0));
    super::topology_kit::assert_same_xy(&layout, &raw_xy(&p));
}

/// 0.6.0's test 2e: merging a datum-placed line with a free one moves the
/// free one; the placed line stays and the merged piece needs no placement.
#[test]
fn a_merge_moves_the_free_piece_onto_the_placed_one() {
    let mut p = empty("DatumAware");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let b = line_at(&mut p, "B", [40.0, 40.0], vec![shot(0.0, 5.0)]);
    let opened = open(&p);
    let state = &opened.state;
    set_datum(
        state,
        slot(a, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        None,
    );
    let before = project(state);
    let (a0, a1) = (xy(&before, slot(a, 0)), xy(&before, slot(a, 1)));

    merge(state, slot(a, 1), slot(b, 0));
    let p = project(state);
    assert_eq!(xy(&p, slot(a, 0)), a0, "A does not move");
    assert_eq!(xy(&p, slot(a, 1)), a1, "A does not move");
    assert!(apart(xy(&p, slot(b, 0)), a1) < 1e-12, "B:0 is A:1");
    assert!(p.placements.is_empty(), "the datum places the merged piece");
}

/// 0.6.0's test 2f: a jump shot keeps its kind through a merge and its undo.
#[test]
fn a_jump_shot_keeps_its_kind_through_a_merge_and_its_undo() {
    let mut p = empty("RefShot");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(5.0), sloped(3.0)]);
    let b = line_at(&mut p, "B", [0.0, 0.0], vec![sloped(4.0)]);
    let opened = open(&p);
    let state = &opened.state;
    let first = project(state).line(a).unwrap().shots[0].id;
    run(
        state,
        Command::SetShotKind {
            line_id: a,
            shot_id: first,
            kind: ShotKind::Jump,
        },
    );
    let kind = |state: &AppState| {
        project(state)
            .line(a)
            .unwrap()
            .shots
            .iter()
            .find(|s| s.id == first)
            .unwrap()
            .kind
    };
    assert!(kind(state).is_virtual());
    let before = snapshot(state);
    merge(state, slot(a, 2), slot(b, 0));
    assert!(kind(state).is_virtual(), "after the merge");
    assert!(state.undo().success);
    assert!(kind(state).is_virtual(), "after its undo");
    assert!(merge_pairs(state).is_empty());
    assert_same(&snapshot(state), &before, "undo");
}

// === Group 3: Multi-line merge chain operations ===

/// A datum on a chain places every line through its junctions; a new GPS on
/// the frame keeps the frame at the origin and moves nothing else here (one
/// piece), so every junction stays one point.
#[test]
fn a_datum_on_a_chain_places_every_line_and_a_new_gps_keeps_the_layout() {
    let (p, a, b, c) = three_lines(1);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 1), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    let free = raw_xy(&project(state));
    set_datum(
        state,
        slot(a, 0),
        GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
        None,
    );
    let p = project(state);
    assert_near(xy(&p, slot(a, 0)), [0.0, 0.0], 1e-9, "the frame");
    let shift = {
        let [x, y] = free[&slot(a, 0)];
        [-x, -y]
    };
    for (at, [x, y]) in &free {
        assert_near(
            xy(&p, *at),
            [x + shift[0], y + shift[1]],
            1e-9,
            &format!("{at:?} moved rigidly"),
        );
    }
    let placed = raw_xy(&p);

    set_datum(
        state,
        slot(a, 0),
        GpsCoordinates::new(46.0, -74.0, 200.0).unwrap(),
        None,
    );
    super::topology_kit::assert_same_xy(&placed, &raw_xy(&project(state)));
}

#[test]
fn undoing_the_second_merge_of_a_chain_keeps_the_first() {
    let (p, a, b, c) = three_lines(1);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 1), slot(b, 0));
    let one = snapshot(state);
    merge(state, slot(b, 1), slot(c, 0));
    let two = snapshot(state);
    assert_eq!(merge_pairs(state).len(), 2);

    assert!(state.undo().success);
    assert_same(&snapshot(state), &one, "undo");
    assert_eq!(merge_pairs(state), [pair(slot(a, 1), slot(b, 0))]);
    assert_eq!(piece_of(&project(state), c), [c]);
    assert!(state.redo().success);
    assert_same(&snapshot(state), &two, "redo");
    assert_eq!(merge_pairs(state).len(), 2);
}

#[test]
fn a_merge_chain_survives_save_and_reopen() {
    let (p, a, b, c) = three_lines(1);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 1), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    set_datum(
        state,
        slot(a, 0),
        GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
        None,
    );
    let before = snapshot(state);
    let c1 = xy(&project(state), slot(c, 1));

    let saved = opened.dir.path().join("chain.stalx");
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(saved.clone())
            })
            .success
    );
    let other = AppState::new().unwrap();
    assert!(other.execute(Command::OpenProject { path: saved }).success);
    let p = project(&other);
    assert_eq!(merge_pairs(&other).len(), 2);
    assert_eq!(p.datums.len(), 1);
    assert_eq!(p.lines().len(), 3);
    assert_eq!(xy(&p, slot(c, 1)), c1, "bit for bit");
    assert_eq!(snapshot(&other).0, before.0, "the same survey");

    // And the history rides the file: the reopened project undoes the datum.
    save_close_reopen(state, &opened.dir.path().join("chain.stalx"));
    assert_eq!(history(state).len(), 3);
    assert!(state.undo().success);
    assert!(project(state).datums.is_empty());
}

#[test]
fn a_loop_closure_closes_a_triangle_of_merged_lines() {
    let mut p = empty("Triangle");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![sloped(5.0), sloped(3.0)]);
    let b = line_at(&mut p, "B", [0.0, 0.0], vec![sloped(4.0), sloped(6.0)]);
    let c = line_at(&mut p, "C", [0.0, 0.0], vec![sloped(7.0), sloped(2.0)]);
    let opened = open(&p);
    let state = &opened.state;
    merge(state, slot(a, 2), slot(b, 0));
    merge(state, slot(b, 2), slot(c, 0));
    assert_eq!(
        refused(
            state,
            Command::MergeStations {
                station_a: slot(c, 2),
                station_b: slot(a, 0),
            }
        ),
        "Stations are already connected. Use Create Loop Closure instead."
    );
    closure(state, slot(c, 2), slot(a, 0));
    let p = project(state);
    assert_eq!(p.lines().len(), 3);
    assert_eq!(merge_pairs(state).len(), 2);
    assert_eq!(p.closures.len(), 1);
    assert!(p.validate().is_empty());
    let wire = state.project_state_wire().unwrap().project.loop_closures;
    assert_eq!(
        (wire[0].station_a, wire[0].station_b),
        (slot(c, 2), slot(a, 0))
    );
}

// === Read-only queries ===

#[test]
fn rigid_and_anchored_lines_follow_junctions_and_aligned_closures_only() {
    // A-B joined at a junction; C a separate piece tied to B by a Batch
    // closure once both are placed (not rigid: unaligned); D free.
    let mut p = empty("Rigid");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let a1 = st(&p, a, 1);
    let b = line_from(&mut p, "B", a1, vec![shot(0.0, 10.0), shot(0.0, 10.0)]);
    let c = line_at(&mut p, "C", [50.0, 0.0], vec![shot(0.0, 10.0)]);
    let d = line_at(&mut p, "D", [100.0, 0.0], vec![shot(0.0, 10.0)]);
    let opened = open(&p);
    let state = &opened.state;
    assert_eq!(state.get_rigid_connected_line_ids(a).unwrap(), [a, b]);
    assert_eq!(state.get_rigid_connected_line_ids(b).unwrap(), [a, b]);
    assert_eq!(state.get_rigid_connected_line_ids(d).unwrap(), [d]);
    assert_eq!(
        state.get_rigid_connected_line_ids(999).unwrap_err(),
        crate::models::LINE_GONE
    );
    assert!(state.get_anchored_line_ids().unwrap().is_empty());
    set_datum(state, slot(a, 0), origin(), None);
    assert_eq!(state.get_anchored_line_ids().unwrap(), [a, b]);
    set_datum(state, slot(c, 0), gps_at(50.0, 0.0), None);
    closure(state, slot(b, 2), slot(c, 1));
    assert_eq!(state.get_rigid_connected_line_ids(a).unwrap(), [a, b]);
    assert_eq!(state.get_rigid_connected_line_ids(c).unwrap(), [c]);
    assert_eq!(state.get_anchored_line_ids().unwrap(), [a, b, c]);
    set_datum(state, slot(d, 1), gps_at(100.0, 10.0), None);
    assert_eq!(state.get_anchored_line_ids().unwrap(), [a, b, c, d]);
}

#[test]
fn the_segment_geometry_spans_closures_and_is_relative_to_the_line_start() {
    // A placed at (5, 5); B hangs off A:1; C is a separate placed piece tied
    // to B by a closure; D is apart.
    let mut p = empty("Segment");
    let a = line_at(&mut p, "A", [5.0, 5.0], vec![shot(90.0, 10.0)]);
    let a1 = st(&p, a, 1);
    let b = line_from(&mut p, "B", a1, vec![shot(0.0, 10.0)]);
    let c = line_at(&mut p, "C", [40.0, 40.0], vec![shot(0.0, 4.0)]);
    let d = line_at(&mut p, "D", [-80.0, 0.0], vec![shot(0.0, 4.0)]);
    let opened = open(&p);
    let state = &opened.state;
    set_datum(state, slot(a, 0), origin(), None);
    set_datum(state, slot(c, 0), gps_at(40.0, 40.0), None);
    closure(state, slot(b, 1), slot(c, 1));

    let geometry = state.get_segment_geometry(a).unwrap();
    let ids: Vec<LineId> = geometry.lines.iter().map(|l| l.line_id).collect();
    assert_eq!(ids, [a, b, c], "D is in no segment of A's");
    let line_a = &geometry.lines[0];
    assert_eq!(line_a.shot_lines.len(), 1);
    assert_near(line_a.shot_lines[0][0], [0.0, 0.0], 1e-9, "relative to A:0");
    assert_near(line_a.shot_lines[0][1], [10.0, 0.0], 1e-9, "A:1");
    let line_c = &geometry.lines[2];
    assert_near(
        line_c.shot_lines[0][0],
        [40.0, 40.0],
        1e-6,
        "C:0 relative to A:0",
    );
    assert_eq!(
        state.get_segment_geometry(999).unwrap_err(),
        crate::models::LINE_GONE
    );
    assert_eq!(state.get_segment_geometry(d).unwrap().lines.len(), 1);
}

#[test]
fn the_datum_preview_validates_the_gps_and_answers_none_for_no_station() {
    let mut p = empty("Preview");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(0.0, 10.0)]);
    let opened = open(&p);
    let state = &opened.state;
    assert!(state
        .preview_zero_datum(
            slot(a, 0),
            GpsCoordinates {
                latitude: 95.0,
                longitude: 0.0,
                depth_reference: 0.0
            }
        )
        .is_err());
    assert!(state
        .preview_zero_datum(slot(a, 5), origin())
        .unwrap()
        .is_none());
    let preview = state
        .preview_zero_datum(slot(a, 0), origin())
        .unwrap()
        .unwrap();
    assert_eq!(preview.role, crate::models::DatumRole::Placement);
    assert!(project(state).datums.is_empty(), "read-only");
}

#[test]
fn the_queries_answer_without_an_open_project() {
    let state = AppState::new().unwrap();
    assert_eq!(
        state
            .check_merge_connectivity(slot(1, 0), slot(2, 0))
            .unwrap_err(),
        "No project open"
    );
    assert!(state.get_anchored_line_ids().is_err());
    assert!(state.get_rigid_connected_line_ids(1).is_err());
    assert!(state.get_segment_geometry(1).is_err());
    assert!(state.preview_zero_datum(slot(1, 0), origin()).is_err());
}

#[test]
fn a_batch_of_topology_commands_is_one_entry_with_every_warning() {
    let mut p = empty("Batch");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(0.0, 10.0)]);
    let b = line_at(&mut p, "B", [30.0, 0.0], vec![shot(0.0, 10.0)]);
    let opened = open(&p);
    let state = &opened.state;
    set_datum(state, slot(a, 0), origin(), None);
    set_datum(state, slot(b, 0), gps_at(30.0, 0.0), None);
    let before = snapshot(state);
    let r = run(
        state,
        Command::Batch {
            commands: vec![
                Command::CreateLoopClosure {
                    station_a: slot(a, 1),
                    station_b: slot(b, 1),
                    closure_strategy: WireStrategy::Batch,
                },
                Command::MergeStations {
                    station_a: slot(a, 1),
                    station_b: slot(b, 1),
                },
            ],
            label: None,
        },
    );
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert_eq!(state.get_undo_stack().len(), 3);
    assert!(state.undo().success);
    assert_same(&snapshot(state), &before, "one undo for the batch");
}

#[test]
fn a_topology_history_undoes_through_the_file_after_a_reopen() {
    // Merge, closure, datum, detach: saved, closed and reopened, the
    // persisted entries (no session ids) step back to the opened survey.
    let (p, a, b, c) = three_lines(2);
    let opened = open(&p);
    let state = &opened.state;
    let original = snapshot(state);
    merge(state, slot(a, 2), slot(b, 0));
    merge(state, slot(b, 1), slot(c, 0));
    closure(state, slot(a, 0), slot(c, 1));
    set_datum(state, slot(a, 0), origin(), None);
    run(
        state,
        Command::UnmergeStations {
            station_ref: slot(c, 0),
        },
    );
    let edited = snapshot(state);
    save_close_reopen(state, &opened.path);
    assert_same(&snapshot(state), &edited, "the file holds the edits");
    assert_eq!(history(state).len(), 5);
    for _ in 0..5 {
        let r = state.undo();
        assert!(r.success, "{:?}", r.error);
    }
    assert_same(&snapshot(state), &original, "five persisted undos");
    for _ in 0..5 {
        let r = state.redo();
        assert!(r.success, "{:?}", r.error);
    }
    assert_same(&snapshot(state), &edited, "and five redos");
}
