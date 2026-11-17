//! What the UI is told about GPS fixes (#741): the gap on the wire datum, the
//! merge preview's datum shift, and the fixes the computed view is bent to.
//! Synthetic surveys only.

use crate::{
    local_to_gps, DatumRole, GpsCoordinates, Project, LineData, LineId, ShotData, StationRef,
};

const ORIGIN_LAT: f64 = 20.5;
const ORIGIN_LON: f64 = -87.25;

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(ORIGIN_LAT, ORIGIN_LON, 0.0).unwrap()
}

/// The GPS position `east`/`north` metres from the origin entrance.
fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

/// A line of `count` shots of `length` metres due north.
fn north_line(project: &mut Project, count: usize, length: f64) -> LineId {
    let shots = (0..count)
        .map(|_| ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length,
            ..Default::default()
        })
        .collect();
    project
        .add_line(LineData {
            shots,
            ..Default::default()
        })
        .id
}

/// A 500 m traverse (A then B, merged end to start), placed at A:0, with a
/// GPS fix at B:5 whose GPS is `fix_east` metres east of where the survey
/// puts it.
fn traverse_with_fix(fix_east: f64) -> (Project, StationRef) {
    let mut project = Project::new("Traverse".to_string(), None);
    let a = north_line(&mut project, 5, 50.0);
    let b = north_line(&mut project, 5, 50.0);
    project.ensure_shot_uuids();
    project
        .merge_stations(StationRef::new(a, 5), StationRef::new(b, 0))
        .unwrap();
    project
        .set_zero_datum(StationRef::new(a, 0), origin())
        .unwrap();
    let fix = StationRef::new(b, 5);
    project
        .set_zero_datum(fix, gps_at(fix_east, 500.0))
        .unwrap();
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    (project, fix)
}

#[test]
fn the_wire_datum_carries_the_backend_gap_for_a_fix_only() {
    let (project, fix) = traverse_with_fix(40.0);
    let wire = project.zero_datums_wire();
    assert_eq!(wire.len(), 2);
    assert!(wire[0].gap.is_none(), "a placement datum has no gap");
    let gap = wire[1].gap.expect("a fix carries its gap");
    assert_eq!(gap, project.gps_fix_gaps()[&fix]);
    assert!((gap.gap_m - 40.0).abs() < 0.01, "gap {}", gap.gap_m);
    assert!(
        (gap.bearing_deg - 90.0).abs() < 0.5,
        "bearing {}",
        gap.bearing_deg
    );

    // Off the wire when absent, so the generated type reads `gap?: DatumGap`.
    let json = serde_json::to_value(&wire).unwrap();
    assert!(json[0].get("gap").is_none(), "{json}");
    assert!(json[1]["gap"]["gap_m"].is_number(), "{json}");
    let decl = <crate::ZeroDatumWire as ts_rs::TS>::decl();
    assert!(decl.contains("gap?: DatumGap"), "{decl}");
}

/// Two separately placed pieces: A (the frame, datum A:0) runs 0..100 m
/// north; B is placed by its own GPS so that B:0 sits 30 m east of A:2.
fn two_placed_pieces() -> (Project, LineId, LineId) {
    let mut project = Project::new("TwoPieces".to_string(), None);
    let a = north_line(&mut project, 2, 50.0);
    let b = north_line(&mut project, 2, 50.0);
    project.ensure_shot_uuids();
    project
        .set_zero_datum(StationRef::new(a, 0), origin())
        .unwrap();
    project
        .set_zero_datum(StationRef::new(b, 0), gps_at(30.0, 100.0))
        .unwrap();
    (project, a, b)
}

fn raw_xy(project: &Project, sr: StationRef) -> (f64, f64) {
    let c = project.compute_station_raw_coords(sr).unwrap();
    (c.x, c.y)
}

#[test]
fn the_merge_preview_names_the_piece_that_moves_and_matches_the_real_merge() {
    // Both pick orders: the list order of the datums decides, never the order
    // the user picked the stations in.
    for b_first in [false, true] {
        let (mut project, a, b) = two_placed_pieces();
        let (end_a, start_b) = (StationRef::new(a, 2), StationRef::new(b, 0));
        let (first, second) = if b_first {
            (start_b, end_a)
        } else {
            (end_a, start_b)
        };
        let shift = project
            .merge_datum_shift(first, second)
            .expect("two placed pieces");
        assert_eq!(shift.moving_line_ids, vec![b]);
        assert_eq!(shift.datum_station, start_b);
        assert!(
            (shift.shift_m - 30.0).abs() < 0.01,
            "shift {}",
            shift.shift_m
        );

        let before = raw_xy(&project, start_b);
        project.merge_stations(first, second).unwrap();
        let after = raw_xy(&project, start_b);
        let moved = (after.0 - before.0).hypot(after.1 - before.1);
        assert!(
            (moved - shift.shift_m).abs() < 1e-6,
            "the merge moved B by {moved}, the preview said {}",
            shift.shift_m
        );
        assert_eq!(
            project.datum_roles()[1],
            Some(DatumRole::GpsFix),
            "the datum the preview named is a fix after the merge"
        );
    }
}

#[test]
fn the_merge_preview_is_silent_unless_both_pieces_are_placed() {
    let mut project = Project::new("OnePlaced".to_string(), None);
    let a = north_line(&mut project, 2, 50.0);
    let b = north_line(&mut project, 2, 50.0);
    let c = north_line(&mut project, 2, 50.0);
    project.ensure_shot_uuids();
    let (end_a, start_b, start_c) = (
        StationRef::new(a, 2),
        StationRef::new(b, 0),
        StationRef::new(c, 0),
    );
    assert_eq!(
        project.merge_datum_shift(end_a, start_b),
        None,
        "nothing placed"
    );
    project
        .set_zero_datum(StationRef::new(a, 0), origin())
        .unwrap();
    assert_eq!(
        project.merge_datum_shift(end_a, start_b),
        None,
        "one side placed"
    );
    assert_eq!(
        project.merge_datum_shift(start_c, end_a),
        None,
        "one side placed"
    );
}

#[test]
fn far_gps_fixes_lists_only_the_fixes_that_fail_the_gap_check() {
    // 500 m of survey to the placement datum: the threshold is 25 m.
    let (project, fix) = traverse_with_fix(40.0);
    let far = project.far_gps_fixes();
    assert_eq!(far.len(), 1, "{far:?}");
    assert_eq!(far[0].station, fix);
    let gap = project.gps_fix_gaps()[&fix];
    assert_eq!(
        (far[0].gap_m, far[0].threshold_m),
        (gap.gap_m, gap.threshold_m)
    );

    let (near, _) = traverse_with_fix(5.0);
    assert!(near.far_gps_fixes().is_empty(), "a 5 m gap is within 25 m");
}
