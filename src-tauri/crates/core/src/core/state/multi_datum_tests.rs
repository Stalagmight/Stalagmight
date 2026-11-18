//! Several zero datums in one merge-connected survey (#741), through the
//! command layer: the accuracy argument and its undo, merging two placed
//! groups and its undo, the `datum_gps_gap` warning and the preview query.
//! Synthetic surveys only.

use std::collections::HashMap;

use super::AppState;
use crate::core::Command;
use crate::models::{
    gps_to_local, local_to_gps, ClosureStrategy, CombineJoinEnds, CombinePick,
    CombineSectionsOptions, DatumRole, GpsCoordinates, Project, SectionEnd, SectionId, ShotData,
    StationRef, SurveyDirection, DATUM_GPS_GAP, DATUM_GPS_GAP_START,
};

const ORIGIN: (f64, f64) = (20.5, -87.25);

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(ORIGIN.0, ORIGIN.1, 0.0).unwrap()
}

/// The GPS position `east`/`north` metres from the first entrance.
fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

fn north_shots(count: usize, length: f64) -> Vec<ShotData> {
    (0..count)
        .map(|_| ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length,
            ..Default::default()
        })
        .collect()
}

fn create_section(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    state
        .execute(Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: Some(SurveyDirection::In),
            shots,
        })
        .section_id
        .unwrap()
}

fn set_datum(state: &AppState, station_ref: StationRef, gps: GpsCoordinates, acc: Option<f64>) {
    let r = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
        accuracy_m: acc,
        entrance: None,
    });
    assert!(r.success, "{:?}", r.error);
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn raw_xy(project: &Project) -> HashMap<StationRef, (f64, f64)> {
    project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, (s.coordinates.x, s.coordinates.y)))
        .collect()
}

fn assert_same_xy(a: &HashMap<StationRef, (f64, f64)>, b: &HashMap<StationRef, (f64, f64)>) {
    assert_eq!(a.len(), b.len());
    for (sr, (ax, ay)) in a {
        let (bx, by) = b[sr];
        assert!(
            (ax - bx).abs() < 1e-6 && (ay - by).abs() < 1e-6,
            "{sr:?}: ({ax}, {ay}) vs ({bx}, {by})"
        );
    }
}

/// Two entrances joined by a 500 m traverse, first entrance placed at A:0.
fn two_entrances() -> (AppState, SectionId, SectionId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "TwoEntrances".to_string(),
        location: None,
    });
    let a = create_section(&state, "A", north_shots(5, 50.0));
    let b = create_section(&state, "B", north_shots(5, 50.0));
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 5),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    set_datum(&state, StationRef::new(a, 0), origin(), None);
    (state, a, b)
}

#[test]
fn a_gps_fix_through_the_command_stores_its_accuracy_and_undo_restores_both() {
    let (state, _a, b) = two_entrances();
    let fix = StationRef::new(b, 5);
    let before = raw_xy(&project(&state));

    set_datum(&state, fix, gps_at(40.0, 500.0), Some(3.0));
    let p = project(&state);
    assert_same_xy(&before, &raw_xy(&p));
    assert_eq!(
        p.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    assert_eq!(p.zero_datums[1].accuracy_m, Some(3.0));

    // Re-set it with new GPS and accuracy, then undo: both come back.
    set_datum(&state, fix, gps_at(10.0, 495.0), Some(0.0));
    assert_eq!(project(&state).zero_datums[1].accuracy_m, Some(0.0));
    assert!(state.undo().success);
    let p = project(&state);
    assert_eq!(p.zero_datums[1].accuracy_m, Some(3.0));
    let restored = p.zero_datums[1].gps;
    let expected = gps_at(40.0, 500.0);
    assert!((restored.latitude - expected.latitude).abs() < 1e-12);
    assert!((restored.longitude - expected.longitude).abs() < 1e-12);

    // Undoing the add removes the fix again; nothing ever moved.
    assert!(state.undo().success);
    let p = project(&state);
    assert_eq!(p.zero_datums.len(), 1);
    assert_same_xy(&before, &raw_xy(&p));

    // An invalid accuracy is refused and changes nothing.
    let r = state.execute(Command::SetZeroDatum {
        station_ref: fix,
        coordinates: gps_at(40.0, 500.0),
        accuracy_m: Some(-1.0),
        entrance: None,
    });
    assert!(!r.success);
    assert_eq!(project(&state).zero_datums.len(), 1);
}

#[test]
fn merging_two_placed_groups_and_undo_restores_both_placements() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "TwoPieces".to_string(),
        location: None,
    });
    let a = create_section(&state, "A", north_shots(2, 50.0));
    let b = create_section(&state, "B", north_shots(2, 50.0));
    set_datum(&state, StationRef::new(a, 0), origin(), None);
    set_datum(&state, StationRef::new(b, 0), gps_at(30.0, 100.0), None);
    let placed = project(&state);
    let before = raw_xy(&placed);
    let offsets_before = placed.section_offsets.clone();
    assert_eq!(
        placed.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );

    // B's station first: the list order decides, not the argument order.
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 0),
        station_b: StationRef::new(a, 2),
    });
    assert!(r.success, "{:?}", r.error);
    let merged = project(&state);
    assert_eq!(
        merged.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    assert!(merged.check_merge_alignment().is_empty());
    for i in 0..=2 {
        let sr = StationRef::new(a, i);
        assert_eq!(raw_xy(&merged)[&sr], before[&sr], "A never moves");
    }
    let gap = merged.gps_fix_gaps()[&StationRef::new(b, 0)];
    assert!((gap.gap_m - 30.0).abs() < 0.01, "gap {}", gap.gap_m);

    assert!(state.undo().success);
    let undone = project(&state);
    assert_same_xy(&before, &raw_xy(&undone));
    assert_eq!(undone.section_offsets, offsets_before);
    assert!(undone.station_merges.is_empty());
    assert_eq!(
        undone.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
}

#[test]
fn a_fix_past_the_threshold_gets_one_datum_gps_gap_warning() {
    // 500 m of survey between the datums: the threshold is 25 m.
    for (east, warned) in [(24.9, false), (25.1, true), (40.0, true)] {
        let (state, _a, b) = two_entrances();
        set_datum(&state, StationRef::new(b, 5), gps_at(east, 500.0), None);
        let p = project(&state);
        let warnings: Vec<_> = state
            .compute_project_warnings(&p, None)
            .into_iter()
            .filter(|w| w.code == DATUM_GPS_GAP)
            .collect();
        if !warned {
            assert!(warnings.is_empty(), "{east} m is under 25 m: {warnings:?}");
            continue;
        }
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let w = &warnings[0];
        assert_eq!(w.section_id, b);
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

#[test]
fn fixes_on_stations_0_and_1_of_one_section_are_ignored_independently() {
    // Both stations touch shot 0 of B; the codes keep their Ignores apart.
    let (state, _a, b) = two_entrances();
    let temp = tempfile::TempDir::new().unwrap();
    let saved = state.execute(Command::SaveProject {
        path: Some(temp.path().join("fixes.stalx")),
    });
    assert!(saved.success, "{:?}", saved.error);
    // B:0 is surveyed at (0, 250) and B:1 at (0, 300); each GPS is 40 m east,
    // over the 20 m and 21 m thresholds 250 m and 300 m of survey from the
    // placement datum A:0 give.
    set_datum(&state, StationRef::new(b, 0), gps_at(40.0, 250.0), None);
    set_datum(&state, StationRef::new(b, 1), gps_at(40.0, 300.0), None);

    let gap_rows = |state: &AppState| -> Vec<(Option<u32>, usize, String, bool)> {
        let p = project(state);
        let mut rows: Vec<_> = state
            .compute_project_warnings(&p, None)
            .into_iter()
            .filter(|w| w.code == DATUM_GPS_GAP || w.code == DATUM_GPS_GAP_START)
            .map(|w| (w.datum_station_index, w.shot_index, w.code, w.is_resolved))
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(
        gap_rows(&state),
        vec![
            (Some(0), 0, DATUM_GPS_GAP_START.to_string(), false),
            (Some(1), 0, DATUM_GPS_GAP.to_string(), false),
        ]
    );

    // Ignore station 1's row exactly as the frontend does, from the row.
    ignore_datum_row(&state, 1);
    assert_eq!(
        gap_rows(&state),
        vec![
            (Some(0), 0, DATUM_GPS_GAP_START.to_string(), false),
            (Some(1), 0, DATUM_GPS_GAP.to_string(), true),
        ]
    );
}

#[test]
fn the_preview_query_names_the_role_and_the_gap() {
    let (state, a, b) = two_entrances();
    let fix = state
        .preview_zero_datum(StationRef::new(b, 5), gps_at(40.0, 500.0))
        .unwrap()
        .unwrap();
    assert_eq!(fix.role, DatumRole::GpsFix);
    assert!((fix.gap.unwrap().gap_m - 40.0).abs() < 0.01);

    let placement = state
        .preview_zero_datum(StationRef::new(a, 0), origin())
        .unwrap()
        .unwrap();
    assert_eq!(placement.role, DatumRole::Placement);
    assert!(placement.gap.is_none());

    assert!(state
        .preview_zero_datum(StationRef::new(b, 99), origin())
        .unwrap()
        .is_none());
    assert_eq!(project(&state).zero_datums.len(), 1, "read-only");
}

/// Every station's raw (x, y, z), keyed by station.
fn raw_xyz(project: &Project) -> HashMap<StationRef, (f64, f64, f64)> {
    project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| {
            let c = s.coordinates;
            (s.station_ref, (c.x, c.y, c.z))
        })
        .collect()
}

fn xy_of(project: &Project, sr: StationRef) -> (f64, f64) {
    let c = project.compute_station_raw_coords(sr).unwrap();
    (c.x, c.y)
}

fn assert_near(actual: (f64, f64), expected: (f64, f64), tol: f64, what: &str) {
    assert!(
        (actual.0 - expected.0).abs() < tol && (actual.1 - expected.1).abs() < tol,
        "{what}: {actual:?}, expected {expected:?}"
    );
}

/// Undo puts every offset, datum and station back exactly; redo replays the
/// same settled state.
fn assert_undo_redo_round_trip(state: &AppState, before: &Project) {
    let after = project(state);
    assert!(state.undo().success);
    let undone = project(state);
    assert_eq!(undone.section_offsets, before.section_offsets);
    assert_eq!(undone.zero_datums.len(), before.zero_datums.len());
    assert_eq!(undone.datum_roles(), before.datum_roles());
    assert_eq!(raw_xyz(&undone), raw_xyz(before));
    assert!(state.redo().success);
    let redone = project(state);
    assert_eq!(redone.section_offsets, after.section_offsets);
    assert_eq!(raw_xyz(&redone), raw_xyz(&after));
}

#[test]
fn removing_the_frame_places_the_promoted_fix_and_keeps_every_piece_on_its_gps() {
    // Frame A:0 + fix B:5 (40 m east of where the survey puts it) in one piece,
    // plus a separately placed piece C with depths.
    let (state, a, b) = two_entrances();
    let fix_gps = gps_at(40.0, 500.0);
    set_datum(&state, StationRef::new(b, 5), fix_gps, None);
    let c_shots = north_shots(2, 20.0)
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            s.depth_in = 3.0 + i as f64;
            s.depth_out = 4.0 + i as f64;
            s
        })
        .collect();
    let c = create_section(&state, "C", c_shots);
    let c_gps = gps_at(-300.0, 120.0);
    set_datum(&state, StationRef::new(c, 0), c_gps, None);
    let before = project(&state);
    assert_eq!(
        before.datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
            Some(DatumRole::Placement)
        ]
    );
    assert_near(
        xy_of(&before, StationRef::new(c, 0)),
        (-300.0, 120.0),
        1e-6,
        "C placed",
    );

    let r = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(a, 0),
    });
    assert!(r.success, "{:?}", r.error);

    let p = project(&state);
    assert_eq!(
        p.frame_datum_key().and_then(|k| p.resolve_key(&k)),
        Some(StationRef::new(b, 5)),
        "the promoted fix is the frame"
    );
    assert_near(
        xy_of(&p, StationRef::new(b, 5)),
        (0.0, 0.0),
        1e-6,
        "F on the origin",
    );
    let c_expected = gps_to_local(&c_gps, &fix_gps);
    assert_near(
        xy_of(&p, StationRef::new(c, 0)),
        c_expected,
        0.01,
        "C on its GPS",
    );
    // The traverse moved rigidly by (40, 0) then (-40, -500); C by (-40, -500).
    let (old, new) = (raw_xyz(&before), raw_xyz(&p));
    for (sr, (bx, by, bz)) in &old {
        let (nx, ny, nz) = new[sr];
        assert!((nz - bz).abs() < 1e-12, "{sr:?}: depth changed");
        let (ex, ey) = if sr.section_id == c {
            (bx - 40.0, by - 500.0)
        } else {
            (*bx, by - 500.0)
        };
        assert_near((nx, ny), (ex, ey), 0.01, &format!("{sr:?}"));
    }

    assert_undo_redo_round_trip(&state, &before);
}

#[test]
fn a_batch_that_deletes_the_frame_section_settles_the_promotion_once() {
    // `execute_batch` settles inside its snapshot pair; a second settle in
    // `execute()`'s epilogue would place the new frame against the old one.
    let (state, a, b) = two_entrances();
    let fix_gps = gps_at(40.0, 500.0);
    set_datum(&state, StationRef::new(b, 5), fix_gps, None);
    let c = create_section(&state, "C", north_shots(2, 20.0));
    let c_gps = gps_at(-300.0, 120.0);
    set_datum(&state, StationRef::new(c, 0), c_gps, None);
    let before = project(&state);

    let r = state.execute(Command::Batch {
        commands: vec![Command::DeleteSection { section_id: a }],
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_near(
        xy_of(&p, StationRef::new(b, 5)),
        (0.0, 0.0),
        1e-6,
        "F on the origin",
    );
    assert_near(
        xy_of(&p, StationRef::new(c, 0)),
        gps_to_local(&c_gps, &fix_gps),
        0.01,
        "C on its GPS",
    );

    assert!(state.undo().success);
    let undone = project(&state);
    assert_eq!(undone.section_offsets, before.section_offsets);
    assert_eq!(raw_xyz(&undone), raw_xyz(&before));
}

#[test]
fn unmerging_moves_the_piece_whose_fix_is_promoted_onto_its_gps() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "TwoPieces".to_string(),
        location: None,
    });
    let a = create_section(&state, "A", north_shots(2, 50.0));
    let b = create_section(&state, "B", north_shots(2, 50.0));
    set_datum(&state, StationRef::new(a, 0), origin(), None);
    set_datum(&state, StationRef::new(b, 0), gps_at(30.0, 100.0), None);
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    let merged = project(&state);
    assert_near(
        xy_of(&merged, StationRef::new(b, 0)),
        (0.0, 100.0),
        1e-6,
        "merged",
    );

    let r = state.execute(Command::UnmergeStationPair {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_eq!(
        p.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    assert_near(
        xy_of(&p, StationRef::new(b, 0)),
        (30.0, 100.0),
        1e-6,
        "B on its GPS",
    );
    for i in 0..=2 {
        let sr = StationRef::new(a, i);
        assert_eq!(xy_of(&p, sr), xy_of(&merged, sr), "A never moves");
    }

    assert_undo_redo_round_trip(&state, &merged);
}

#[test]
fn a_promotion_under_an_unchanged_frame_places_only_its_own_piece() {
    // E holds the frame; the traverse is placed by A:0 200 m east of it, with a
    // fix at B:5 that is 40 m east of where the survey puts it.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "KeptFrame".to_string(),
        location: None,
    });
    let e = create_section(&state, "E", north_shots(1, 10.0));
    set_datum(&state, StationRef::new(e, 0), origin(), None);
    let a = create_section(&state, "A", north_shots(5, 50.0));
    let b = create_section(&state, "B", north_shots(5, 50.0));
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 5),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    set_datum(&state, StationRef::new(a, 0), gps_at(200.0, 0.0), None);
    let fix_gps = gps_at(240.0, 500.0);
    set_datum(&state, StationRef::new(b, 5), fix_gps, None);
    let before = project(&state);
    assert_near(
        xy_of(&before, StationRef::new(b, 5)),
        (200.0, 500.0),
        1e-6,
        "fix",
    );

    let r = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(a, 0),
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_eq!(
        p.frame_datum_key().and_then(|k| p.resolve_key(&k)),
        Some(StationRef::new(e, 0))
    );
    assert_near(
        xy_of(&p, StationRef::new(b, 5)),
        gps_to_local(&fix_gps, &origin()),
        1e-6,
        "the promoted fix sits on its GPS",
    );
    let (old, new) = (raw_xyz(&before), raw_xyz(&p));
    for (sr, (bx, by, _)) in &old {
        let (nx, ny, _) = new[sr];
        let east = if sr.section_id == e { 0.0 } else { 40.0 };
        assert_near((nx, ny), (bx + east, *by), 1e-6, &format!("{sr:?}"));
    }

    assert_undo_redo_round_trip(&state, &before);
}

#[test]
fn an_undo_entry_with_an_accuracy_the_app_would_refuse_restores_the_default() {
    // The undo history is persisted, so a crafted file can carry any value.
    let (state, _a, b) = two_entrances();
    let fix = StationRef::new(b, 5);
    set_datum(&state, fix, gps_at(40.0, 500.0), Some(3.0));
    set_datum(&state, fix, gps_at(10.0, 495.0), Some(1.0));
    for (stored, restored) in [
        (Some(-1.0), None),
        (Some(crate::models::MAX_GPS_ACCURACY_M + 1.0), None),
        (Some(f64::NAN), None),
        (Some(7.0), Some(7.0)),
    ] {
        {
            let mut stack = state.undo_stack.lock().unwrap();
            let mut entry = stack.pop_undo().unwrap();
            let mut tampered = false;
            if let crate::core::UndoData::ZeroDatumInfo {
                previous_accuracy_m,
                ..
            } = &mut entry.undo_data
            {
                *previous_accuracy_m = stored;
                tampered = true;
            }
            assert!(tampered, "a SetZeroDatum records ZeroDatumInfo");
            stack.push_undo(entry);
        }
        assert!(state.undo().success);
        assert_eq!(project(&state).zero_datums[1].accuracy_m, restored);
        assert!(state.redo().success);
    }
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
    let (state, _a, b) = two_entrances();
    let fix = gps_at(40.0, 500.0);
    let json = serde_json::json!({
        "type": "batch",
        "commands": [{
            "type": "set_zero_datum",
            "station_ref": { "section_id": b, "station_index": 5 },
            "coordinates": {
                "latitude": fix.latitude,
                "longitude": fix.longitude,
                "depth_reference": 0.0
            },
            "accuracy_m": 2.5
        }]
    });
    let batch: Command = serde_json::from_value(json).unwrap();
    let r = state.execute(batch);
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_eq!(p.zero_datums[1].accuracy_m, Some(2.5));
    assert_eq!(p.datum_roles()[1], Some(DatumRole::GpsFix));
}

/// How far each PLACEMENT datum's station is from its own GPS, in the
/// project's current frame.
fn placement_gaps(p: &Project) -> Vec<(StationRef, f64)> {
    let frame = p.frame_datum().unwrap().gps;
    p.placement_datums()
        .into_iter()
        .map(|zd| {
            let sr = p.resolve_key(&zd.station_ref).unwrap();
            let (gx, gy) = gps_to_local(&zd.gps, &frame);
            let (x, y) = xy_of(p, sr);
            (sr, (gx - x).hypot(gy - y))
        })
        .collect()
}

fn assert_placements_on_their_gps(state: &AppState, what: &str) {
    for (sr, gap) in placement_gaps(&project(state)) {
        assert!(
            gap < 0.01,
            "{what}: placement datum {sr:?} is {gap} m off its GPS"
        );
    }
}

/// Frame A:0, a fix on B:5 40 m east of the survey, and a separate piece C
/// placed by its own GPS: the shape where a promoted fix used to leave a
/// placement datum off its GPS (#741 D4/D5).
fn frame_fix_and_separate_piece() -> (AppState, SectionId, SectionId, SectionId) {
    let (state, a, b) = two_entrances();
    set_datum(&state, StationRef::new(b, 5), gps_at(40.0, 500.0), None);
    let c = create_section(&state, "C", north_shots(2, 20.0));
    set_datum(&state, StationRef::new(c, 0), gps_at(200.0, 0.0), None);
    assert_placements_on_their_gps(&state, "setup");
    (state, a, b, c)
}

#[test]
fn no_placement_datum_is_left_off_its_gps_by_a_promotion() {
    // Remove Zero Datum on the frame.
    let (state, a, _b, _c) = frame_fix_and_separate_piece();
    let r = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(a, 0),
    });
    assert!(r.success, "{:?}", r.error);
    assert_placements_on_their_gps(&state, "remove");

    // Delete Section on the frame's section.
    let (state, a, _b, _c) = frame_fix_and_separate_piece();
    let r = state.execute(Command::DeleteSection { section_id: a });
    assert!(r.success, "{:?}", r.error);
    assert_placements_on_their_gps(&state, "delete section");

    // Unmerge: the fix's piece becomes a piece of its own.
    let (state, a, b, _c) = frame_fix_and_separate_piece();
    let r = state.execute(Command::UnmergeStationPair {
        station_a: StationRef::new(a, 5),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(project(&state).placement_datums().len(), 3);
    assert_placements_on_their_gps(&state, "unmerge");

    // Repair. Its `repair_structural_integrity` drops only dangling datums,
    // which hold no role, so it promotes nothing; a damaged merge for it to
    // drop proves it ran, and the pieces stay on their GPS.
    let (state, a, _b, _c) = frame_fix_and_separate_piece();
    {
        let mut guard = state.project.lock().unwrap();
        let p = guard.as_mut().unwrap();
        p.station_merges.push(crate::models::StationMerge {
            station_a: crate::models::StationKey::new(
                a,
                crate::models::StationAnchor::AfterShot("gone".to_string()),
            ),
            station_b: StationRef::new(a, 0).into(),
            merged_at: chrono::Utc::now(),
        });
    }
    let result = state.repair_open_project_integrity();
    assert!(result.changed && result.fixed >= 1, "{result:?}");
    assert_eq!(project(&state).placement_datums().len(), 2);
    assert_placements_on_their_gps(&state, "repair");
}

#[test]
fn an_accuracy_only_edit_never_moves_a_piece() {
    let (state, a, b, c) = frame_fix_and_separate_piece();
    let fix = StationRef::new(b, 5);

    // Every datum, placement or fix, re-set with its stored GPS and a new
    // accuracy, as the Edit Station dialog sends it.
    for sr in [StationRef::new(a, 0), fix, StationRef::new(c, 0)] {
        let p = project(&state);
        let before = raw_xy(&p);
        let gps = p
            .zero_datums
            .iter()
            .find(|zd| p.resolve_key(&zd.station_ref) == Some(sr))
            .unwrap()
            .gps;
        set_datum(&state, sr, gps, Some(7.5));
        let after = project(&state);
        assert_same_xy(&before, &raw_xy(&after));
        let stored = after
            .zero_datums
            .iter()
            .find(|zd| after.resolve_key(&zd.station_ref) == Some(sr))
            .unwrap();
        assert_eq!(stored.accuracy_m, Some(7.5), "{sr:?}");
    }

    // A placement datum an older file left 30 m off its GPS: changing only
    // its accuracy still moves nothing (re-placing it is a GPS edit's job).
    {
        let mut guard = state.project.lock().unwrap();
        let p = guard.as_mut().unwrap();
        p.set_section_offset(c, (230.0, 0.0));
    }
    let p = project(&state);
    let before = raw_xy(&p);
    let c_gps = gps_at(200.0, 0.0);
    assert!((xy_of(&p, StationRef::new(c, 0)).0 - 230.0).abs() < 1e-9);
    set_datum(&state, StationRef::new(c, 0), c_gps, Some(2.0));
    assert_same_xy(&before, &raw_xy(&project(&state)));

    // A GPS edit on that placement datum does re-place its piece.
    set_datum(&state, StationRef::new(c, 0), gps_at(210.0, 0.0), Some(2.0));
    assert_near(
        xy_of(&project(&state), StationRef::new(c, 0)),
        (210.0, 0.0),
        0.01,
        "a GPS change places the piece",
    );
}

/// Ignore the datum gap row of `station` through the user-facing resolve path,
/// from the row as the frontend has it.
fn ignore_datum_row(state: &AppState, station: u32) {
    let p = project(state);
    let row = state
        .compute_project_warnings(&p, None)
        .into_iter()
        .find(|w| w.datum_station_index == Some(station))
        .unwrap();
    state
        .save_resolved_warning_gated(&crate::persistence::ResolvedWarning {
            section_id: row.section_id,
            shot_id: row.shot_id,
            warning_code: row.code.clone(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: row.shot_uuid.clone(),
        })
        .unwrap();
}

#[test]
fn an_ignored_gap_comes_back_when_the_datum_gps_changes() {
    // An Ignore says "this GPS reading is far off, and I know". A new reading
    // is not covered by it; undoing the change brings the Ignore back (#741 U2).
    let (state, _a, b) = two_entrances();
    let temp = tempfile::TempDir::new().unwrap();
    let saved = state.execute(Command::SaveProject {
        path: Some(temp.path().join("ignore.stalx")),
    });
    assert!(saved.success, "{:?}", saved.error);
    let fix = StationRef::new(b, 5);
    set_datum(&state, fix, gps_at(40.0, 500.0), None);
    let ignored = |state: &AppState| -> Vec<bool> {
        let p = project(state);
        state
            .compute_project_warnings(&p, None)
            .into_iter()
            .filter(|w| w.code == DATUM_GPS_GAP)
            .map(|w| w.is_resolved)
            .collect()
    };
    assert_eq!(ignored(&state), vec![false]);
    ignore_datum_row(&state, 5);
    assert_eq!(ignored(&state), vec![true]);

    // An accuracy-only edit keeps the reading, so the Ignore holds.
    set_datum(&state, fix, gps_at(40.0, 500.0), Some(3.0));
    assert_eq!(ignored(&state), vec![true]);

    // A new GPS reading, 150 m off: the warning is back.
    set_datum(&state, fix, gps_at(150.0, 500.0), Some(3.0));
    assert_eq!(ignored(&state), vec![false]);
    assert!(state.undo().success);
    assert_eq!(
        ignored(&state),
        vec![true],
        "undo restores the ignored reading"
    );
    assert!(state.redo().success);
    assert_eq!(ignored(&state), vec![false]);

    // Un-ignoring the reading it was ignored for clears it for good.
    assert!(state.undo().success);
    let p = project(&state);
    let row = state
        .compute_project_warnings(&p, None)
        .into_iter()
        .find(|w| w.code == DATUM_GPS_GAP)
        .unwrap();
    state
        .delete_resolved_warning_gated(
            row.section_id,
            row.shot_id,
            &row.code,
            row.shot_uuid.as_deref(),
        )
        .unwrap();
    assert_eq!(ignored(&state), vec![false]);
}

/// A separately placed piece C, 300 m west and 120 m north of the first
/// entrance's GPS.
fn placed_piece(state: &AppState) -> (SectionId, GpsCoordinates) {
    let c = create_section(state, "C", north_shots(2, 20.0));
    let c_gps = gps_at(-300.0, 120.0);
    set_datum(state, StationRef::new(c, 0), c_gps, None);
    (c, c_gps)
}

#[test]
fn deleting_the_first_shot_promotes_the_next_stations_fix_onto_its_gps() {
    // Frame A:0 and a fix on A:1, 30 m east of where the survey puts A:1.
    // Deleting A's first shot drops the frame and re-anchors the fix onto
    // A's start, the key the frame had: it must still count as promoted.
    let (state, a, _b) = two_entrances();
    let fix_gps = gps_at(30.0, 50.0);
    set_datum(&state, StationRef::new(a, 1), fix_gps, None);
    let (c, c_gps) = placed_piece(&state);
    let before = project(&state);
    let first = before.get_section(a).unwrap().shots[0].id;

    let r = state.execute(Command::DeleteShot {
        section_id: a,
        shot_id: first,
    });
    assert!(r.success, "{:?}", r.error);

    let p = project(&state);
    assert_eq!(p.zero_datums.len(), 2);
    let frame_gps = p
        .frame_datum()
        .map(|zd| (zd.gps.latitude, zd.gps.longitude));
    assert_eq!(
        frame_gps,
        Some((fix_gps.latitude, fix_gps.longitude)),
        "the fix is the frame now"
    );
    assert_near(
        xy_of(&p, StationRef::new(a, 0)),
        (0.0, 0.0),
        1e-6,
        "the promoted fix on the origin",
    );
    assert_near(
        xy_of(&p, StationRef::new(c, 0)),
        gps_to_local(&c_gps, &fix_gps),
        0.01,
        "C on its GPS",
    );

    assert_undo_redo_round_trip(&state, &before);
}

#[test]
fn removing_the_frame_promotes_a_fix_that_shares_its_gps() {
    // The fix on B:5 was given the entrance's exact GPS (copied coordinates).
    // It is a different datum all the same: removing the frame promotes it,
    // its piece moves onto that GPS and C stays on its own.
    let (state, a, b) = two_entrances();
    set_datum(&state, StationRef::new(b, 5), origin(), None);
    let (c, c_gps) = placed_piece(&state);
    let before = project(&state);
    assert_eq!(before.datum_roles()[1], Some(DatumRole::GpsFix));

    let r = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(a, 0),
    });
    assert!(r.success, "{:?}", r.error);

    let p = project(&state);
    assert_near(
        xy_of(&p, StationRef::new(b, 5)),
        (0.0, 0.0),
        1e-6,
        "the promoted fix on the origin",
    );
    assert_near(
        xy_of(&p, StationRef::new(c, 0)),
        gps_to_local(&c_gps, &origin()),
        0.01,
        "C on its GPS",
    );

    assert_undo_redo_round_trip(&state, &before);
}

#[test]
fn deleting_the_first_shot_promotes_a_next_station_fix_with_the_frames_gps() {
    // The entrance GPS copied onto A:1 as well: once A's first shot is
    // deleted, the fix sits on A's start with the very key and GPS the frame
    // had. It is still the fix (its station did not move), so it places A on
    // that GPS: A:1, now A:0, moves 50 m south onto the origin.
    let (state, a, _b) = two_entrances();
    set_datum(&state, StationRef::new(a, 1), origin(), None);
    let (c, c_gps) = placed_piece(&state);
    let before = project(&state);
    let first = before.get_section(a).unwrap().shots[0].id;

    let r = state.execute(Command::DeleteShot {
        section_id: a,
        shot_id: first,
    });
    assert!(r.success, "{:?}", r.error);

    let p = project(&state);
    assert_near(
        xy_of(&p, StationRef::new(a, 0)),
        (0.0, 0.0),
        1e-6,
        "the promoted fix on the origin",
    );
    assert_near(
        xy_of(&p, StationRef::new(c, 0)),
        gps_to_local(&c_gps, &origin()),
        0.01,
        "C on its GPS",
    );

    assert_undo_redo_round_trip(&state, &before);
}

/// A survey with two datums on one GPS (copied coordinates): X hangs off A
/// with a GPS fix on X:0 at `fix_acc`, 30 m east of where X:0 is, and Y is a
/// separate piece placed at Y:`twin_at` by the very same GPS at `twin_acc`. A
/// closure ties X:`closure_at` to Y:0. Returns the state, X and Y.
fn fix_and_twin(
    fix_acc: Option<f64>,
    twin_acc: Option<f64>,
    twin_at: u32,
    closure_at: u32,
) -> (AppState, SectionId, SectionId) {
    let (state, a, _b) = two_entrances();
    let x = create_section(&state, "X", north_shots(2, 20.0));
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(x, 2),
        station_b: StationRef::new(a, 2),
    });
    assert!(r.success, "{:?}", r.error);
    let twin = gps_at(30.0, 60.0);
    set_datum(&state, StationRef::new(x, 0), twin, fix_acc);
    let y = create_section(&state, "Y", north_shots(1, 20.0));
    set_datum(&state, StationRef::new(y, twin_at), twin, twin_acc);
    let r = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(x, closure_at),
        station_b: StationRef::new(y, 0),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        project(&state).datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
            Some(DatumRole::Placement)
        ]
    );
    (state, x, y)
}

fn combine_options(name: &str, join: CombineJoinEnds) -> CombineSectionsOptions {
    CombineSectionsOptions {
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
    before: &Project,
    fix_acc: Option<f64>,
    twin_acc: Option<f64>,
) {
    let p = project(state);
    let roles: Vec<(Option<f64>, Option<DatumRole>)> = p
        .zero_datums
        .iter()
        .map(|zd| zd.accuracy_m)
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
    assert_undo_redo_round_trip(state, before);
}

/// Combining X:Start with Y:Start moves X onto Y (its merge to A tears for
/// the frame) and puts Y's stations first, so Y:1's datum takes X's start
/// key, which the fix had, with the fix's GPS. The fix comes first in the
/// list, so it is the combined piece's placement datum now and must sit on
/// its GPS.
fn combine_re_keying_the_twin_onto_the_fixs_key(fix_acc: Option<f64>, twin_acc: Option<f64>) {
    let (state, x, y) = fix_and_twin(fix_acc, twin_acc, 1, 0);
    let before = project(&state);
    let join = CombineJoinEnds {
        kept: SectionEnd::Start,
        other: SectionEnd::Start,
    };
    let r = state.execute_combine_sections(x, y, combine_options("XY", join));
    assert!(r.success, "{:?}", r.error);
    assert_the_fix_places_the_combined_piece(&state, &before, fix_acc, twin_acc);
}

/// The mirror: here the FIX takes the key its twin placement datum had.
/// Joining Y:Start to X:End keeps Y and puts X's stations first, so X:0 (the
/// fix) becomes the combined start, Y's old start key, and Y:0's datum moves
/// on. The fix comes first in the list, so it is the combined piece's
/// placement datum now and must sit on its GPS.
fn combine_re_keying_the_fix_onto_its_twins_key(fix_acc: Option<f64>, twin_acc: Option<f64>) {
    let (state, x, y) = fix_and_twin(fix_acc, twin_acc, 0, 2);
    let before = project(&state);
    let twin_key = before.zero_datums[2].station_ref.clone();
    let join = CombineJoinEnds {
        kept: SectionEnd::Start,
        other: SectionEnd::End,
    };
    let r = state.execute_combine_sections(y, x, combine_options("YX", join));
    assert!(r.success, "{:?}", r.error);
    assert_eq!(project(&state).zero_datums[1].station_ref, twin_key);
    assert_the_fix_places_the_combined_piece(&state, &before, fix_acc, twin_acc);
}

#[test]
fn a_combine_that_re_keys_a_twin_onto_the_fixs_key_still_promotes_the_fix() {
    combine_re_keying_the_twin_onto_the_fixs_key(Some(5.0), Some(0.5));
}

#[test]
fn a_new_frame_gps_re_places_the_other_piece_and_undo_restores_it() {
    let (state, a, b, c) = frame_fix_and_separate_piece();
    let before = project(&state);
    let c_gps = gps_at(200.0, 0.0);
    let fix_before = xy_of(&before, StationRef::new(b, 5));

    // The frame keeps its station and moves 100 m north (#769).
    let new_frame = gps_at(0.0, 100.0);
    set_datum(&state, StationRef::new(a, 0), new_frame, None);

    let after = project(&state);
    assert_near(
        xy_of(&after, StationRef::new(a, 0)),
        (0.0, 0.0),
        1e-9,
        "the frame stays on the origin",
    );
    assert_near(
        xy_of(&after, StationRef::new(c, 0)),
        gps_to_local(&c_gps, &new_frame),
        1e-6,
        "C:0 sits on its own GPS in the new frame",
    );
    assert_near(
        xy_of(&after, StationRef::new(b, 5)),
        fix_before,
        1e-9,
        "a GPS fix is no placement",
    );
    assert_placements_on_their_gps(&state, "frame GPS edit");

    assert_undo_redo_round_trip(&state, &before);
    assert_eq!(
        project(&state).zero_datums[0].gps.latitude,
        new_frame.latitude
    );
}

#[test]
fn a_frame_gps_copied_from_a_fix_on_its_spot_moves_every_piece_once() {
    // A is the frame on the first entrance. P is a piece of its own, placed
    // at P:0 by a copy of the frame's GPS, so P:0 sits on the origin as well,
    // and Q hangs off P:0 with a fix 30 m east of the survey: on the origin
    // too. The frame then takes the fix's GPS, with no accuracy typed on
    // either, so the frame datum looks like Q's fix re-keyed where it
    // stands. The frame correction alone moves P and Q 30 m west.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "FrameCopiesAFix".to_string(),
        location: None,
    });
    let a = create_section(&state, "A", north_shots(2, 20.0));
    set_datum(&state, StationRef::new(a, 0), origin(), None);
    let p = create_section(&state, "P", north_shots(2, 20.0));
    set_datum(&state, StationRef::new(p, 0), origin(), None);
    let q = create_section(&state, "Q", north_shots(2, 20.0));
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(p, 0),
        station_b: StationRef::new(q, 0),
    });
    assert!(r.success, "{:?}", r.error);
    let fix = gps_at(30.0, 0.0);
    set_datum(&state, StationRef::new(q, 0), fix, None);
    let before = project(&state);
    assert_eq!(
        before.datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix)
        ]
    );
    assert_near(
        xy_of(&before, StationRef::new(q, 0)),
        (0.0, 0.0),
        1e-9,
        "Q:0 on the origin",
    );

    set_datum(&state, StationRef::new(a, 0), fix, None);

    let after = project(&state);
    assert_near(
        xy_of(&after, StationRef::new(a, 0)),
        (0.0, 0.0),
        1e-9,
        "the frame stays on the origin",
    );
    assert_near(
        xy_of(&after, StationRef::new(p, 0)),
        gps_to_local(&origin(), &fix),
        1e-6,
        "P:0 on its own GPS in the new frame, moved once",
    );
    assert_placements_on_their_gps(&state, "frame GPS copied from a fix");
    assert_undo_redo_round_trip(&state, &before);
}

#[test]
fn an_entrance_only_edit_of_the_frame_moves_nothing() {
    let (state, a, _b, c) = frame_fix_and_separate_piece();
    // C left 30 m off its GPS, as an older file can.
    {
        let mut guard = state.project.lock().unwrap();
        guard.as_mut().unwrap().set_section_offset(c, (230.0, 0.0));
    }
    let before = raw_xy(&project(&state));
    let r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: origin(),
        accuracy_m: None,
        entrance: Some(true),
    });
    assert!(r.success, "{:?}", r.error);
    assert_same_xy(&before, &raw_xy(&project(&state)));
}

#[test]
fn a_combine_that_re_keys_the_fix_onto_its_twins_key_still_promotes_the_fix() {
    combine_re_keying_the_fix_onto_its_twins_key(Some(5.0), Some(0.5));
}

/// #778: twins with one stored value (no accuracy typed on either), so
/// nothing stored tells them apart; only the list order does.
#[test]
fn twins_with_one_stored_value_a_combine_re_keying_the_twin_still_promotes_the_fix() {
    combine_re_keying_the_twin_onto_the_fixs_key(None, None);
    combine_re_keying_the_twin_onto_the_fixs_key(Some(2.0), Some(2.0));
}

#[test]
fn twins_with_one_stored_value_a_combine_re_keying_the_fix_still_promotes_the_fix() {
    combine_re_keying_the_fix_onto_its_twins_key(None, None);
    combine_re_keying_the_fix_onto_its_twins_key(Some(2.0), Some(2.0));
}

#[test]
fn a_batch_that_edits_a_fix_re_keys_it_and_promotes_it_still_places_it() {
    // A Batch settles once for all its commands, so one window can both edit
    // a fix's stored accuracy and re-key it: E is the frame piece, A+B a
    // second piece placed at A:0 with a fix at B's end 40 m east of where B:2
    // is. Re-setting the fix's accuracy, reversing B (B:2 becomes B:0) and
    // deleting A leaves the fix as B's placement datum, on its GPS.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "BatchFix".to_string(),
        location: None,
    });
    let e = create_section(&state, "E", north_shots(1, 20.0));
    set_datum(&state, StationRef::new(e, 0), origin(), None);
    let a = create_section(&state, "A", north_shots(2, 50.0));
    let b = create_section(&state, "B", north_shots(2, 50.0));
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    set_datum(&state, StationRef::new(a, 0), gps_at(300.0, 0.0), None);
    let fix_gps = gps_at(340.0, 200.0);
    set_datum(&state, StationRef::new(b, 2), fix_gps, Some(3.0));
    let before = project(&state);
    assert_eq!(
        before.datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix)
        ]
    );

    let r = state.execute(Command::Batch {
        commands: vec![
            Command::SetZeroDatum {
                station_ref: StationRef::new(b, 2),
                coordinates: fix_gps,
                accuracy_m: Some(1.0),
                entrance: None,
            },
            Command::ReverseSection {
                section_id: b,
                swap_left_right: false,
            },
            Command::DeleteSection { section_id: a },
        ],
    });
    assert!(r.success, "{:?}", r.error);

    let p = project(&state);
    assert_eq!(
        p.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    assert_eq!(p.zero_datums[1].accuracy_m, Some(1.0));
    assert_eq!(
        p.resolve_key(&p.zero_datums[1].station_ref),
        Some(StationRef::new(b, 0))
    );
    assert_placements_on_their_gps(&state, "after the batch");

    assert_undo_redo_round_trip(&state, &before);
}
