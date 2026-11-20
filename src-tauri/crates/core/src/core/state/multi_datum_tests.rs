//! Several zero datums in one survey (#741), through the command layer, on
//! the station-graph model: the accuracy argument and its undo, merging two
//! placed pieces and its undo, promotions when the frame or a placement
//! datum goes (remove, delete line, unmerge), frame GPS edits, accuracy- and
//! entrance-only edits, the preview query, and the `datum_gps_gap` warning.
//! Synthetic surveys only.
//!
//! Where 0.6.0 checked line offsets, these check the raw layout and the
//! canonical snapshot: an undo lands on the state before to the bit. A
//! placement datum an older file left off its GPS (0.6.0's line offset) is a
//! datum `correction` now (plan 4.3).
//!
//! Fates of the legacy tests:
//! - `an_undo_entry_with_an_accuracy_the_app_would_refuse_restores_the_default`
//!   tampered with `UndoData::ZeroDatumInfo`; the changeset engine records
//!   the datum list itself, a step checks its preconditions, and a decoded
//!   entry is validated after its step (C1), so there is no per-arm sanitiser
//!   left to pin; retired;
//! - the Repair case of `no_placement_datum_is_left_off_its_gps_by_a_promotion`
//!   injected a dangling merge, which the station graph cannot hold (invalid
//!   files open through salvage); retired;
//! - `a_frame_gps_copied_from_a_fix_on_its_spot_moves_every_piece_once` put a
//!   fix on the junction slot of a second line; Set on a junction slot edits
//!   the station's datum (E1), so the fix sits on that line's far station;
//! - the warning tests wait for the project warnings (package C10), the
//!   first-shot deletes for Delete Shot (C2) and the combines for Combine
//!   Lines (C4): ported, ignored with the reason.

use super::topology_kit::{
    assert_near, assert_round_trip, assert_same_xy, deep, empty, gps_at, line_at, merge, north,
    open, origin, project, raw_xy, raw_xyz, run, set_datum, slot, snapshot, st, xy, Opened,
};
use super::AppState;
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{
    gps_to_local, ClosureStrategy, CombineJoinEnds, CombineLinesOptions, CombinePick, DatumRole,
    GpsCoordinates, LineEnd, LineId, ShotData, StationRef, DATUM_GPS_GAP, DATUM_GPS_GAP_START,
};

/// `gps_to_local` as a point.
fn local(gps: &GpsCoordinates, frame: &GpsCoordinates) -> [f64; 2] {
    let (x, y) = gps_to_local(gps, frame);
    [x, y]
}

/// The datum `at`'s slot has (E1: via its line, else its station's first).
fn datum_at(p: &Project, at: StationRef) -> usize {
    let station = p.topology().station_at(at).unwrap();
    p.datum_index_for(station, Some(at.line_id)).unwrap()
}

/// Two entrances joined by a 500 m traverse (A:5 = B:0), first entrance
/// placed at A:0, plus `extra` lines, each a free piece of its own.
fn two_entrances(extra: Vec<(&str, Vec<ShotData>)>) -> (Opened, LineId, LineId, Vec<LineId>) {
    let mut p = empty("TwoEntrances");
    let a = line_at(&mut p, "A", [0.0, 0.0], north(5, 50.0));
    let b = line_at(&mut p, "B", [0.0, 0.0], north(5, 50.0));
    let extra = extra
        .into_iter()
        .map(|(name, shots)| line_at(&mut p, name, [0.0, 0.0], shots))
        .collect();
    let opened = open(&p);
    merge(&opened.state, slot(a, 5), slot(b, 0));
    set_datum(&opened.state, slot(a, 0), origin(), None);
    (opened, a, b, extra)
}

/// The open survey changed outside the command path (as an older file can
/// hold it), written as a new file and opened in a new `AppState`.
fn reopen_with(state: &AppState, change: impl FnOnce(&mut Project)) -> Opened {
    let mut p = project(state);
    change(&mut p);
    open(&p)
}

#[test]
fn a_gps_fix_through_the_command_stores_its_accuracy_and_undo_restores_both() {
    let (opened, _a, b, _) = two_entrances(vec![]);
    let state = &opened.state;
    let fix = slot(b, 5);
    let before = raw_xy(&project(state));

    set_datum(state, fix, gps_at(40.0, 500.0), Some(3.0));
    let p = project(state);
    assert_same_xy(&before, &raw_xy(&p));
    assert_eq!(
        p.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    assert_eq!(p.datums[1].accuracy_m, Some(3.0));

    // Re-set it with new GPS and accuracy, then undo: both come back.
    set_datum(state, fix, gps_at(10.0, 495.0), Some(0.0));
    assert_eq!(project(state).datums[1].accuracy_m, Some(0.0));
    assert!(state.undo().success);
    let p = project(state);
    assert_eq!(p.datums[1].accuracy_m, Some(3.0));
    let expected = gps_at(40.0, 500.0);
    assert_eq!(
        p.datums[1].gps.latitude.to_bits(),
        expected.latitude.to_bits()
    );
    assert_eq!(
        p.datums[1].gps.longitude.to_bits(),
        expected.longitude.to_bits()
    );

    // Undoing the add removes the fix again; nothing ever moved.
    assert!(state.undo().success);
    let p = project(state);
    assert_eq!(p.datums.len(), 1);
    assert_same_xy(&before, &raw_xy(&p));

    // An invalid accuracy is refused and changes nothing.
    let r = state.execute(Command::SetZeroDatum {
        station_ref: fix,
        coordinates: gps_at(40.0, 500.0),
        accuracy_m: Some(-1.0),
        entrance: None,
    });
    assert!(!r.success);
    assert_eq!(project(state).datums.len(), 1);
}

#[test]
fn merging_two_placed_pieces_and_undo_restores_both_placements() {
    let mut p = empty("TwoPieces");
    let a = line_at(&mut p, "A", [0.0, 0.0], north(2, 50.0));
    let b = line_at(&mut p, "B", [0.0, 0.0], north(2, 50.0));
    let opened = open(&p);
    let state = &opened.state;
    set_datum(state, slot(a, 0), origin(), None);
    set_datum(state, slot(b, 0), gps_at(30.0, 100.0), None);
    let placed = project(state);
    let before = snapshot(state);
    assert_eq!(
        placed.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );

    // B's station first: the list order decides, not the argument order.
    merge(state, slot(b, 0), slot(a, 2));
    let merged = project(state);
    assert_eq!(
        merged.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    assert_eq!(st(&merged, b, 0), st(&merged, a, 2), "one station");
    for i in 0..=2 {
        let at = slot(a, i);
        assert_eq!(xy(&merged, at), xy(&placed, at), "A never moves");
    }
    let gaps = merged.gps_fix_gaps();
    let (_, gap) = gaps.iter().find(|(i, _)| *i == 1).unwrap();
    assert!((gap.gap_m - 30.0).abs() < 0.01, "gap {}", gap.gap_m);

    assert_round_trip(state, &before, "merge of two placed pieces");
    assert!(state.undo().success);
    let undone = project(state);
    assert_ne!(st(&undone, b, 0), st(&undone, a, 2));
    assert_eq!(
        undone.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
}

#[test]
#[ignore = "C10: the project warnings are not ported on this branch"]
fn a_fix_past_the_threshold_gets_one_datum_gps_gap_warning() {
    // 500 m of survey between the datums: the threshold is 25 m.
    for (east, warned) in [(24.9, false), (25.1, true), (40.0, true)] {
        let (opened, _a, b, _) = two_entrances(vec![]);
        let state = &opened.state;
        set_datum(state, slot(b, 5), gps_at(east, 500.0), None);
        let p = project(state);
        let warnings: Vec<_> = state
            .compute_project_warnings(&p)
            .into_iter()
            .filter(|w| w.code == DATUM_GPS_GAP)
            .collect();
        if !warned {
            assert!(warnings.is_empty(), "{east} m is under 25 m: {warnings:?}");
            continue;
        }
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let w = &warnings[0];
        assert_eq!(w.line_id, b);
        assert_eq!(w.datum_station_index, Some(5));
        assert_eq!(w.shot_index, 4, "station 5 is the end of shot 4");
        assert!(w.datum_refs.is_none());
        assert_eq!(w.severity, crate::core::WarningSeverity::Warning);
        assert!(!w.is_resolved);
        assert!(
            w.message.contains(&format!("{east:.1} m"))
                && w.message.contains("90 degrees")
                && w.message.contains("25.0 m"),
            "{}",
            w.message
        );
    }
}

/// Ignore the datum gap row of `station` through the user-facing resolve
/// path, from the row as the frontend has it.
fn ignore_datum_row(state: &AppState, station: u32) {
    let p = project(state);
    let row = state
        .compute_project_warnings(&p)
        .into_iter()
        .find(|w| w.datum_station_index == Some(station))
        .unwrap();
    state
        .save_resolved_warning_gated(&crate::persistence::ResolvedWarning {
            line_id: row.line_id,
            shot_id: row.shot_id,
            warning_code: row.code,
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: row.shot_uuid,
        })
        .unwrap();
}

#[test]
#[ignore = "C10: the project warnings are not ported on this branch"]
fn fixes_on_stations_0_and_1_of_one_line_are_ignored_independently() {
    // Both stations touch shot 0 of B; the codes keep their Ignores apart.
    // B:0 is the junction with A:5, so its fix is the junction's: the
    // station's datum (E1), its row on B's slot.
    let (opened, _a, b, _) = two_entrances(vec![]);
    let state = &opened.state;
    set_datum(state, slot(b, 0), gps_at(40.0, 250.0), None);
    set_datum(state, slot(b, 1), gps_at(40.0, 300.0), None);

    let gap_rows = |state: &AppState| -> Vec<(Option<u32>, usize, String, bool)> {
        let p = project(state);
        let mut rows: Vec<_> = state
            .compute_project_warnings(&p)
            .into_iter()
            .filter(|w| w.code == DATUM_GPS_GAP || w.code == DATUM_GPS_GAP_START)
            .map(|w| (w.datum_station_index, w.shot_index, w.code, w.is_resolved))
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(
        gap_rows(state),
        vec![
            (Some(0), 0, DATUM_GPS_GAP_START.to_string(), false),
            (Some(1), 0, DATUM_GPS_GAP.to_string(), false),
        ]
    );
    ignore_datum_row(state, 1);
    assert_eq!(
        gap_rows(state),
        vec![
            (Some(0), 0, DATUM_GPS_GAP_START.to_string(), false),
            (Some(1), 0, DATUM_GPS_GAP.to_string(), true),
        ]
    );
}

#[test]
fn the_preview_query_names_the_role_and_the_gap() {
    let (opened, a, b, _) = two_entrances(vec![]);
    let state = &opened.state;
    let fix = state
        .preview_zero_datum(slot(b, 5), gps_at(40.0, 500.0))
        .unwrap()
        .unwrap();
    assert_eq!(fix.role, DatumRole::GpsFix);
    assert!((fix.gap.unwrap().gap_m - 40.0).abs() < 0.01);

    let placement = state
        .preview_zero_datum(slot(a, 0), origin())
        .unwrap()
        .unwrap();
    assert_eq!(placement.role, DatumRole::Placement);
    assert!(placement.gap.is_none());

    assert!(state
        .preview_zero_datum(slot(b, 99), origin())
        .unwrap()
        .is_none());
    assert_eq!(project(state).datums.len(), 1, "read-only");
}

/// C: two 20 m shots north, with depths.
fn c_shots() -> Vec<ShotData> {
    (0..2)
        .map(|i| deep(0.0, 20.0, 3.0 + f64::from(i), 4.0 + f64::from(i)))
        .collect()
}

#[test]
fn removing_the_frame_places_the_promoted_fix_and_keeps_every_piece_on_its_gps() {
    // Frame A:0 + fix B:5 (40 m east of where the survey puts it) in one piece,
    // plus a separately placed piece C with depths.
    let (opened, a, b, extra) = two_entrances(vec![("C", c_shots())]);
    let (state, c) = (&opened.state, extra[0]);
    let fix_gps = gps_at(40.0, 500.0);
    set_datum(state, slot(b, 5), fix_gps, None);
    let c_gps = gps_at(-300.0, 120.0);
    set_datum(state, slot(c, 0), c_gps, None);
    let before = project(state);
    let snap = snapshot(state);
    assert_eq!(
        before.datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
            Some(DatumRole::Placement)
        ]
    );
    assert_near(xy(&before, slot(c, 0)), [-300.0, 120.0], 1e-6, "C placed");

    run(
        state,
        Command::RemoveZeroDatum {
            station_ref: slot(a, 0),
        },
    );
    let p = project(state);
    let frame = p.frame_datum().unwrap();
    assert_eq!(
        p.datums[frame].station,
        st(&p, b, 5),
        "the promoted fix is the frame"
    );
    assert_near(xy(&p, slot(b, 5)), [0.0, 0.0], 1e-6, "F on the origin");
    assert_near(
        xy(&p, slot(c, 0)),
        local(&c_gps, &fix_gps),
        0.01,
        "C on its GPS",
    );
    // The traverse moved rigidly by (40, 0) then (-40, -500); C by (-40, -500).
    let (old, new) = (raw_xyz(&before), raw_xyz(&p));
    for (at, [bx, by, bz]) in &old {
        let [nx, ny, nz] = new[at];
        assert!((nz - bz).abs() < 1e-12, "{at:?}: depth changed");
        let expected = if at.line_id == c {
            [bx - 40.0, by - 500.0]
        } else {
            [*bx, by - 500.0]
        };
        assert_near([nx, ny], expected, 0.01, &format!("{at:?}"));
    }
    assert_round_trip(state, &snap, "remove the frame");
}

#[test]
fn a_batch_that_deletes_the_frame_line_settles_the_promotion_once() {
    let (opened, a, b, extra) = two_entrances(vec![("C", north(2, 20.0))]);
    let (state, c) = (&opened.state, extra[0]);
    let fix_gps = gps_at(40.0, 500.0);
    set_datum(state, slot(b, 5), fix_gps, None);
    let c_gps = gps_at(-300.0, 120.0);
    set_datum(state, slot(c, 0), c_gps, None);
    let before = snapshot(state);

    run(
        state,
        Command::Batch {
            commands: vec![Command::DeleteLine { line_id: a }],
            label: None,
        },
    );
    let p = project(state);
    assert_near(xy(&p, slot(b, 5)), [0.0, 0.0], 1e-6, "F on the origin");
    assert_near(
        xy(&p, slot(c, 0)),
        local(&c_gps, &fix_gps),
        0.01,
        "C on its GPS",
    );
    assert_round_trip(state, &before, "batched delete line");
}

#[test]
fn unmerging_moves_the_piece_whose_fix_is_promoted_onto_its_gps() {
    let mut p = empty("TwoPieces");
    let a = line_at(&mut p, "A", [0.0, 0.0], north(2, 50.0));
    let b = line_at(&mut p, "B", [0.0, 0.0], north(2, 50.0));
    let opened = open(&p);
    let state = &opened.state;
    set_datum(state, slot(a, 0), origin(), None);
    set_datum(state, slot(b, 0), gps_at(30.0, 100.0), None);
    merge(state, slot(a, 2), slot(b, 0));
    let merged = project(state);
    let before = snapshot(state);
    assert_near(xy(&merged, slot(b, 0)), [0.0, 100.0], 1e-6, "merged");

    run(
        state,
        Command::UnmergeStationPair {
            station_a: slot(a, 2),
            station_b: slot(b, 0),
        },
    );
    let p = project(state);
    assert_eq!(
        p.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    assert_near(xy(&p, slot(b, 0)), [30.0, 100.0], 1e-6, "B on its GPS");
    for i in 0..=2 {
        let at = slot(a, i);
        assert_eq!(xy(&p, at), xy(&merged, at), "A never moves");
    }
    assert_round_trip(state, &before, "unmerge");
}

#[test]
fn a_promotion_under_an_unchanged_frame_places_only_its_own_piece() {
    // E holds the frame; the traverse is placed by A:0 200 m east of it, with a
    // fix at B:5 that is 40 m east of where the survey puts it.
    let mut p = empty("KeptFrame");
    let e = line_at(&mut p, "E", [0.0, 0.0], north(1, 10.0));
    let a = line_at(&mut p, "A", [0.0, 0.0], north(5, 50.0));
    let b = line_at(&mut p, "B", [0.0, 0.0], north(5, 50.0));
    let opened = open(&p);
    let state = &opened.state;
    set_datum(state, slot(e, 0), origin(), None);
    merge(state, slot(a, 5), slot(b, 0));
    set_datum(state, slot(a, 0), gps_at(200.0, 0.0), None);
    let fix_gps = gps_at(240.0, 500.0);
    set_datum(state, slot(b, 5), fix_gps, None);
    let before = project(state);
    let snap = snapshot(state);
    assert_near(xy(&before, slot(b, 5)), [200.0, 500.0], 1e-6, "fix");

    run(
        state,
        Command::RemoveZeroDatum {
            station_ref: slot(a, 0),
        },
    );
    let p = project(state);
    let frame = p.frame_datum().unwrap();
    assert_eq!(p.datums[frame].station, st(&p, e, 0));
    assert_near(
        xy(&p, slot(b, 5)),
        local(&fix_gps, &origin()),
        1e-6,
        "the promoted fix sits on its GPS",
    );
    let (old, new) = (raw_xyz(&before), raw_xyz(&p));
    for (at, [bx, by, _]) in &old {
        let [nx, ny, _] = new[at];
        let east = if at.line_id == e { 0.0 } else { 40.0 };
        assert_near([nx, ny], [bx + east, *by], 1e-6, &format!("{at:?}"));
    }
    assert_round_trip(state, &snap, "promotion under a kept frame");
}

#[test]
fn a_set_zero_datum_command_persisted_before_accuracy_still_loads() {
    let cmd = Command::SetZeroDatum {
        station_ref: StationRef::new(3, 2),
        coordinates: origin(),
        accuracy_m: Some(7.5),
        entrance: None,
    };
    let mut json: serde_json::Value = serde_json::to_value(&cmd).unwrap();
    assert_eq!(json["accuracy_m"], 7.5);
    json.as_object_mut().unwrap().remove("accuracy_m");
    let legacy: Command = serde_json::from_value(json).unwrap();
    assert!(matches!(
        legacy,
        Command::SetZeroDatum {
            accuracy_m: None,
            ..
        }
    ));
}

#[test]
fn accuracy_survives_the_batch_command_path() {
    // The frontend's `execute_batch` sends nested commands as snake_case JSON,
    // so a set_zero_datum inside a batch carries `accuracy_m` verbatim.
    let (opened, _a, b, _) = two_entrances(vec![]);
    let state = &opened.state;
    let fix = gps_at(40.0, 500.0);
    let json = serde_json::json!({
        "type": "batch",
        "commands": [{
            "type": "set_zero_datum",
            "station_ref": { "line_id": b, "station_index": 5 },
            "coordinates": {
                "latitude": fix.latitude,
                "longitude": fix.longitude,
                "depth_reference": 0.0
            },
            "accuracy_m": 2.5
        }]
    });
    let batch: Command = serde_json::from_value(json).unwrap();
    run(state, batch);
    let p = project(state);
    assert_eq!(p.datums[1].accuracy_m, Some(2.5));
    assert_eq!(p.datum_roles()[1], Some(DatumRole::GpsFix));
}

/// How far each PLACEMENT datum's station is from its own GPS, in the
/// project's current frame.
fn placement_gaps(p: &Project) -> Vec<(usize, f64)> {
    let frame = p.datums[p.frame_datum().unwrap()].gps;
    let raw = p.raw_positions();
    p.placement_datums()
        .into_iter()
        .flatten()
        .map(|i| {
            let d = &p.datums[i];
            let [gx, gy] = local(&d.gps, &frame);
            let [x, y] = raw.xy(d.station).unwrap();
            (i, (gx - x).hypot(gy - y))
        })
        .collect()
}

fn assert_placements_on_their_gps(state: &AppState, what: &str) {
    for (datum, gap) in placement_gaps(&project(state)) {
        assert!(
            gap < 0.01,
            "{what}: placement datum {datum} is {gap} m off its GPS"
        );
    }
}

/// Frame A:0, a fix on B:5 40 m east of the survey, and a separate piece C
/// placed by its own GPS: the shape where a promoted fix used to leave a
/// placement datum off its GPS (#741 D4/D5).
fn frame_fix_and_separate_piece() -> (Opened, LineId, LineId, LineId) {
    let (opened, a, b, extra) = two_entrances(vec![("C", north(2, 20.0))]);
    set_datum(&opened.state, slot(b, 5), gps_at(40.0, 500.0), None);
    set_datum(&opened.state, slot(extra[0], 0), gps_at(200.0, 0.0), None);
    assert_placements_on_their_gps(&opened.state, "setup");
    (opened, a, b, extra[0])
}

#[test]
fn no_placement_datum_is_left_off_its_gps_by_a_promotion() {
    // Remove Zero Datum on the frame.
    let (opened, a, _b, _c) = frame_fix_and_separate_piece();
    run(
        &opened.state,
        Command::RemoveZeroDatum {
            station_ref: slot(a, 0),
        },
    );
    assert_placements_on_their_gps(&opened.state, "remove");

    // Delete Line on the frame's line.
    let (opened, a, _b, _c) = frame_fix_and_separate_piece();
    run(&opened.state, Command::DeleteLine { line_id: a });
    assert_placements_on_their_gps(&opened.state, "delete line");

    // Unmerge: the fix's piece becomes a piece of its own.
    let (opened, a, b, _c) = frame_fix_and_separate_piece();
    run(
        &opened.state,
        Command::UnmergeStationPair {
            station_a: slot(a, 5),
            station_b: slot(b, 0),
        },
    );
    let p = project(&opened.state);
    assert_eq!(p.placement_datums().iter().flatten().count(), 3);
    assert_placements_on_their_gps(&opened.state, "unmerge");
}

#[test]
fn an_accuracy_only_edit_never_moves_a_piece() {
    let (opened, a, b, c) = frame_fix_and_separate_piece();
    let state = &opened.state;

    // Every datum, placement or fix, re-set with its stored GPS and a new
    // accuracy, as the Edit Station dialog sends it.
    for at in [slot(a, 0), slot(b, 5), slot(c, 0)] {
        let p = project(state);
        let before = raw_xy(&p);
        let gps = p.datums[datum_at(&p, at)].gps;
        set_datum(state, at, gps, Some(7.5));
        let after = project(state);
        assert_same_xy(&before, &raw_xy(&after));
        assert_eq!(
            after.datums[datum_at(&after, at)].accuracy_m,
            Some(7.5),
            "{at:?}"
        );
    }

    // A placement datum an older file left 30 m off its GPS (a stored
    // correction): changing only its accuracy still moves nothing
    // (re-placing it is a GPS edit's job).
    let opened = reopen_with(state, |p| {
        let i = datum_at(p, slot(c, 0));
        p.datums[i].correction = Some([30.0, 0.0]);
    });
    let state = &opened.state;
    let p = project(state);
    let before = raw_xy(&p);
    let c_gps = gps_at(200.0, 0.0);
    assert!((xy(&p, slot(c, 0))[0] - 230.0).abs() < 1e-6);
    set_datum(state, slot(c, 0), c_gps, Some(2.0));
    assert_same_xy(&before, &raw_xy(&project(state)));

    // A GPS edit on that placement datum does re-place its piece.
    set_datum(state, slot(c, 0), gps_at(210.0, 0.0), Some(2.0));
    assert_near(
        xy(&project(state), slot(c, 0)),
        [210.0, 0.0],
        0.01,
        "a GPS change places the piece",
    );
}

#[test]
#[ignore = "C10: the project warnings are not ported on this branch"]
fn an_ignored_gap_comes_back_when_the_datum_gps_changes() {
    // An Ignore says "this GPS reading is far off, and I know". A new reading
    // is not covered by it; undoing the change brings the Ignore back (#741 U2).
    let (opened, _a, b, _) = two_entrances(vec![]);
    let state = &opened.state;
    let fix = slot(b, 5);
    set_datum(state, fix, gps_at(40.0, 500.0), None);
    let ignored = |state: &AppState| -> Vec<bool> {
        let p = project(state);
        state
            .compute_project_warnings(&p)
            .into_iter()
            .filter(|w| w.code == DATUM_GPS_GAP)
            .map(|w| w.is_resolved)
            .collect()
    };
    assert_eq!(ignored(state), vec![false]);
    ignore_datum_row(state, 5);
    assert_eq!(ignored(state), vec![true]);

    // An accuracy-only edit keeps the reading, so the Ignore holds.
    set_datum(state, fix, gps_at(40.0, 500.0), Some(3.0));
    assert_eq!(ignored(state), vec![true]);

    // A new GPS reading, 150 m off: the warning is back.
    set_datum(state, fix, gps_at(150.0, 500.0), Some(3.0));
    assert_eq!(ignored(state), vec![false]);
    assert!(state.undo().success);
    assert_eq!(
        ignored(state),
        vec![true],
        "undo restores the ignored reading"
    );
    assert!(state.redo().success);
    assert_eq!(ignored(state), vec![false]);

    // Un-ignoring the reading it was ignored for clears it for good.
    assert!(state.undo().success);
    let p = project(state);
    let row = state
        .compute_project_warnings(&p)
        .into_iter()
        .find(|w| w.code == DATUM_GPS_GAP)
        .unwrap();
    state
        .delete_resolved_warning_gated(
            row.line_id,
            row.shot_id,
            &row.code,
            row.shot_uuid.as_deref(),
        )
        .unwrap();
    assert_eq!(ignored(state), vec![false]);
}

#[test]
#[ignore = "C2: Delete Shot is not ported on this branch"]
fn deleting_the_first_shot_promotes_the_next_stations_fix_onto_its_gps() {
    // Frame A:0 and a fix on A:1, 30 m east of where the survey puts A:1.
    // Deleting A's first shot removes A:0 with the frame: the fix is the
    // frame now, on the origin.
    let (opened, a, _b, extra) = two_entrances(vec![("C", north(2, 20.0))]);
    let (state, c) = (&opened.state, extra[0]);
    let fix_gps = gps_at(30.0, 50.0);
    set_datum(state, slot(a, 1), fix_gps, None);
    let c_gps = gps_at(-300.0, 120.0);
    set_datum(state, slot(c, 0), c_gps, None);
    let before = snapshot(state);
    let first = project(state).line(a).unwrap().shots[0].id;

    run(
        state,
        Command::DeleteShot {
            line_id: a,
            shot_id: first,
        },
    );
    let p = project(state);
    assert_eq!(p.datums.len(), 2);
    let frame = p.datums[p.frame_datum().unwrap()].gps;
    assert_eq!(
        (frame.latitude, frame.longitude),
        (fix_gps.latitude, fix_gps.longitude),
        "the fix is the frame now"
    );
    assert_near(
        xy(&p, slot(a, 0)),
        [0.0, 0.0],
        1e-6,
        "the promoted fix on the origin",
    );
    assert_near(
        xy(&p, slot(c, 0)),
        local(&c_gps, &fix_gps),
        0.01,
        "C on its GPS",
    );
    assert_round_trip(state, &before, "delete the first shot");
}

#[test]
fn removing_the_frame_promotes_a_fix_that_shares_its_gps() {
    // The fix on B:5 was given the entrance's exact GPS (copied coordinates).
    // It is a different datum all the same: removing the frame promotes it,
    // its piece moves onto that GPS and C stays on its own.
    let (opened, a, b, extra) = two_entrances(vec![("C", north(2, 20.0))]);
    let (state, c) = (&opened.state, extra[0]);
    set_datum(state, slot(b, 5), origin(), None);
    let c_gps = gps_at(-300.0, 120.0);
    set_datum(state, slot(c, 0), c_gps, None);
    let before = snapshot(state);
    assert_eq!(project(state).datum_roles()[1], Some(DatumRole::GpsFix));

    run(
        state,
        Command::RemoveZeroDatum {
            station_ref: slot(a, 0),
        },
    );
    let p = project(state);
    assert_near(
        xy(&p, slot(b, 5)),
        [0.0, 0.0],
        1e-6,
        "the promoted fix on the origin",
    );
    assert_near(
        xy(&p, slot(c, 0)),
        local(&c_gps, &origin()),
        0.01,
        "C on its GPS",
    );
    assert_round_trip(state, &before, "remove the frame");
}

#[test]
#[ignore = "C2: Delete Shot is not ported on this branch"]
fn deleting_the_first_shot_promotes_a_next_station_fix_with_the_frames_gps() {
    // The entrance GPS copied onto A:1 as well: once A's first shot is
    // deleted, the fix places A on that GPS: A:1, now A:0, moves 50 m south
    // onto the origin.
    let (opened, a, _b, extra) = two_entrances(vec![("C", north(2, 20.0))]);
    let (state, c) = (&opened.state, extra[0]);
    set_datum(state, slot(a, 1), origin(), None);
    let c_gps = gps_at(-300.0, 120.0);
    set_datum(state, slot(c, 0), c_gps, None);
    let before = snapshot(state);
    let first = project(state).line(a).unwrap().shots[0].id;

    run(
        state,
        Command::DeleteShot {
            line_id: a,
            shot_id: first,
        },
    );
    let p = project(state);
    assert_near(
        xy(&p, slot(a, 0)),
        [0.0, 0.0],
        1e-6,
        "the promoted fix on the origin",
    );
    assert_near(
        xy(&p, slot(c, 0)),
        local(&c_gps, &origin()),
        0.01,
        "C on its GPS",
    );
    assert_round_trip(state, &before, "delete the first shot");
}

/// A survey with two datums on one GPS (copied coordinates): X hangs off A
/// (X:2 = A:2) with a GPS fix on X:0 at `fix_acc`, 30 m east of where X:0
/// is, and Y is a separate piece placed at Y:`twin_at` by the very same GPS
/// at `twin_acc`. A closure ties X:`closure_at` to Y:0.
fn fix_and_twin(
    fix_acc: Option<f64>,
    twin_acc: Option<f64>,
    twin_at: u32,
    closure_at: u32,
) -> (Opened, LineId, LineId) {
    let (opened, a, _b, extra) = two_entrances(vec![("X", north(2, 20.0)), ("Y", north(1, 20.0))]);
    let (x, y) = (extra[0], extra[1]);
    let state = &opened.state;
    merge(state, slot(x, 2), slot(a, 2));
    let twin = gps_at(30.0, 60.0);
    set_datum(state, slot(x, 0), twin, fix_acc);
    set_datum(state, slot(y, twin_at), twin, twin_acc);
    run(
        state,
        Command::CreateLoopClosure {
            station_a: slot(x, closure_at),
            station_b: slot(y, 0),
            closure_strategy: ClosureStrategy::Batch,
        },
    );
    assert_eq!(
        project(state).datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
            Some(DatumRole::Placement)
        ]
    );
    (opened, x, y)
}

fn combine_options(name: &str, join: CombineJoinEnds) -> CombineLinesOptions {
    CombineLinesOptions {
        name: name.to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: Some(join),
    }
}

/// After a twin combine: the fix (still second in the list) places the
/// combined piece, the twin (third) is its fix, every placement datum sits
/// on its GPS, and undo/redo round-trips.
fn assert_the_fix_places_the_combined_piece(
    state: &AppState,
    before: &super::topology_kit::Snapshot,
    fix_acc: Option<f64>,
    twin_acc: Option<f64>,
) {
    let p = project(state);
    let roles: Vec<(Option<f64>, Option<DatumRole>)> = p
        .datums
        .iter()
        .map(|d| d.accuracy_m)
        .zip(p.datum_roles())
        .collect();
    assert_eq!(
        roles[1..],
        [
            (fix_acc, Some(DatumRole::Placement)),
            (twin_acc, Some(DatumRole::GpsFix))
        ]
    );
    assert_placements_on_their_gps(state, "after the combine");
    assert_round_trip(state, before, "combine");
}

/// Combining X:Start with Y:Start moves X onto Y (its junction with A tears
/// for the frame) and puts Y's stations first.
fn combine_the_twin_onto_the_fix(fix_acc: Option<f64>, twin_acc: Option<f64>) {
    let (opened, x, y) = fix_and_twin(fix_acc, twin_acc, 1, 0);
    let state = &opened.state;
    let before = snapshot(state);
    let join = CombineJoinEnds {
        kept: LineEnd::Start,
        other: LineEnd::Start,
    };
    let r = state.execute_combine_lines(x, y, combine_options("XY", join));
    assert!(r.success, "{:?}", r.error);
    assert_the_fix_places_the_combined_piece(state, &before, fix_acc, twin_acc);
}

/// The mirror: joining Y:Start to X:End keeps Y and puts X's stations
/// first, so the fix's X:0 becomes the combined start.
fn combine_the_fix_onto_its_twin(fix_acc: Option<f64>, twin_acc: Option<f64>) {
    let (opened, x, y) = fix_and_twin(fix_acc, twin_acc, 0, 2);
    let state = &opened.state;
    let before = snapshot(state);
    let join = CombineJoinEnds {
        kept: LineEnd::Start,
        other: LineEnd::End,
    };
    let r = state.execute_combine_lines(y, x, combine_options("YX", join));
    assert!(r.success, "{:?}", r.error);
    assert_the_fix_places_the_combined_piece(state, &before, fix_acc, twin_acc);
}

#[test]
#[ignore = "C4: Combine Lines is not ported on this branch"]
fn a_combine_that_moves_a_twin_onto_the_fix_still_promotes_the_fix() {
    combine_the_twin_onto_the_fix(Some(5.0), Some(0.5));
}

#[test]
#[ignore = "C4: Combine Lines is not ported on this branch"]
fn a_combine_that_moves_the_fix_onto_its_twin_still_promotes_the_fix() {
    combine_the_fix_onto_its_twin(Some(5.0), Some(0.5));
}

/// #778: twins with one stored value (no accuracy typed on either), so
/// nothing stored tells them apart; only the list order does.
#[test]
#[ignore = "C4: Combine Lines is not ported on this branch"]
fn twins_with_one_stored_value_a_combine_moving_the_twin_still_promotes_the_fix() {
    combine_the_twin_onto_the_fix(None, None);
    combine_the_twin_onto_the_fix(Some(2.0), Some(2.0));
}

#[test]
#[ignore = "C4: Combine Lines is not ported on this branch"]
fn twins_with_one_stored_value_a_combine_moving_the_fix_still_promotes_the_fix() {
    combine_the_fix_onto_its_twin(None, None);
    combine_the_fix_onto_its_twin(Some(2.0), Some(2.0));
}

#[test]
fn a_new_frame_gps_re_places_the_other_piece_and_undo_restores_it() {
    let (opened, a, b, c) = frame_fix_and_separate_piece();
    let state = &opened.state;
    let before = project(state);
    let snap = snapshot(state);
    let c_gps = gps_at(200.0, 0.0);
    let fix_before = xy(&before, slot(b, 5));

    // The frame keeps its station and moves 100 m north (#769).
    let new_frame = gps_at(0.0, 100.0);
    set_datum(state, slot(a, 0), new_frame, None);
    let after = project(state);
    assert_near(
        xy(&after, slot(a, 0)),
        [0.0, 0.0],
        1e-9,
        "the frame stays on the origin",
    );
    assert_near(
        xy(&after, slot(c, 0)),
        local(&c_gps, &new_frame),
        1e-6,
        "C:0 sits on its own GPS in the new frame",
    );
    assert_near(
        xy(&after, slot(b, 5)),
        fix_before,
        1e-9,
        "a GPS fix is no placement",
    );
    assert_placements_on_their_gps(state, "frame GPS edit");

    assert_round_trip(state, &snap, "frame GPS edit");
    assert_eq!(project(state).datums[0].gps.latitude, new_frame.latitude);
}

#[test]
fn a_frame_gps_copied_from_a_fix_moves_every_other_piece_once() {
    // A is the frame on the first entrance. P is a piece of its own, placed
    // at P:0 by a copy of the frame's GPS, so P:0 sits on the origin as well;
    // Q hangs off P:0 with a fix on Q:2, 30 m east of where the survey puts
    // it. The frame then takes the fix's GPS: the frame correction alone
    // moves P and Q, once.
    let mut p = empty("FrameCopiesAFix");
    let a = line_at(&mut p, "A", [0.0, 0.0], north(2, 20.0));
    let pl = line_at(&mut p, "P", [0.0, 0.0], north(2, 20.0));
    let q = line_at(&mut p, "Q", [0.0, 0.0], north(2, 20.0));
    let opened = open(&p);
    let state = &opened.state;
    set_datum(state, slot(a, 0), origin(), None);
    set_datum(state, slot(pl, 0), origin(), None);
    merge(state, slot(q, 0), slot(pl, 0));
    let fix = gps_at(30.0, 40.0);
    set_datum(state, slot(q, 2), fix, None);
    let before = snapshot(state);
    assert_eq!(
        project(state).datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix)
        ]
    );
    assert_near(
        xy(&project(state), slot(q, 0)),
        [0.0, 0.0],
        1e-9,
        "Q:0 on the origin",
    );

    set_datum(state, slot(a, 0), fix, None);
    let after = project(state);
    assert_near(
        xy(&after, slot(a, 0)),
        [0.0, 0.0],
        1e-9,
        "the frame stays on the origin",
    );
    assert_near(
        xy(&after, slot(pl, 0)),
        local(&origin(), &fix),
        1e-6,
        "P:0 on its own GPS in the new frame, moved once",
    );
    assert_placements_on_their_gps(state, "frame GPS copied from a fix");
    assert_round_trip(state, &before, "frame GPS copied from a fix");
}

#[test]
fn an_entrance_only_edit_of_the_frame_moves_nothing() {
    let (opened, a, _b, c) = frame_fix_and_separate_piece();
    // C left 30 m off its GPS, as an older file can.
    let opened = reopen_with(&opened.state, |p| {
        let i = datum_at(p, slot(c, 0));
        p.datums[i].correction = Some([30.0, 0.0]);
    });
    let state = &opened.state;
    let before = raw_xy(&project(state));
    run(
        state,
        Command::SetZeroDatum {
            station_ref: slot(a, 0),
            coordinates: origin(),
            accuracy_m: None,
            entrance: Some(true),
        },
    );
    assert_same_xy(&before, &raw_xy(&project(state)));
    assert_eq!(project(state).datums[0].entrance, Some(true));
}

#[test]
fn a_batch_that_edits_a_fix_reverses_its_line_and_promotes_it_still_places_it() {
    // E is the frame piece, A+B a second piece placed at A:0 with a fix at
    // B's end 40 m east of where B:2 is. Re-setting the fix's accuracy,
    // reversing B (B:2 becomes B:0) and deleting A leaves the fix as B's
    // placement datum, on its GPS.
    let mut p = empty("BatchFix");
    let e = line_at(&mut p, "E", [0.0, 0.0], north(1, 20.0));
    let a = line_at(&mut p, "A", [0.0, 0.0], north(2, 50.0));
    let b = line_at(&mut p, "B", [0.0, 0.0], north(2, 50.0));
    let opened = open(&p);
    let state = &opened.state;
    set_datum(state, slot(e, 0), origin(), None);
    merge(state, slot(a, 2), slot(b, 0));
    set_datum(state, slot(a, 0), gps_at(300.0, 0.0), None);
    let fix_gps = gps_at(340.0, 200.0);
    set_datum(state, slot(b, 2), fix_gps, Some(3.0));
    let before = snapshot(state);
    assert_eq!(
        project(state).datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix)
        ]
    );

    run(
        state,
        Command::Batch {
            commands: vec![
                Command::SetZeroDatum {
                    station_ref: slot(b, 2),
                    coordinates: fix_gps,
                    accuracy_m: Some(1.0),
                    entrance: None,
                },
                Command::ReverseLine {
                    line_id: b,
                    swap_left_right: false,
                },
                Command::DeleteLine { line_id: a },
            ],
            label: None,
        },
    );
    let p = project(state);
    assert_eq!(
        p.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    assert_eq!(p.datums[1].accuracy_m, Some(1.0));
    assert_eq!(p.datums[1].station, st(&p, b, 0));
    assert_placements_on_their_gps(state, "after the batch");
    assert_round_trip(state, &before, "the batch");
}
