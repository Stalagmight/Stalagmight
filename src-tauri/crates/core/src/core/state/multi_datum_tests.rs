//! Several zero datums in one merge-connected survey (#741), through the
//! command layer: the accuracy argument and its undo, merging two placed
//! groups and its undo, the `datum_gps_gap` warning and the preview query.
//! Synthetic surveys only.

use std::collections::HashMap;

use super::AppState;
use crate::core::Command;
use crate::models::{
    gps_to_local, local_to_gps, DatumRole, GpsCoordinates, Project, SectionId, ShotData,
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
    // over the 16 m threshold 50 m of survey from the other fix gives.
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
    let p = project(&state);
    let row = state
        .compute_project_warnings(&p, None)
        .into_iter()
        .find(|w| w.datum_station_index == Some(1))
        .unwrap();
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: row.section_id,
            shot_id: row.shot_id,
            warning_code: row.code.clone(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: row.shot_uuid.clone(),
        })
        .unwrap();
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
            match &mut entry.undo_data {
                crate::core::UndoData::ZeroDatumInfo {
                    previous_accuracy_m,
                    ..
                } => *previous_accuracy_m = stored,
                other => panic!("unexpected undo data {other:?}"),
            }
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
