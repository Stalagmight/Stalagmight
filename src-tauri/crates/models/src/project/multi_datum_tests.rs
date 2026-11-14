//! Several zero datums in one merge-connected survey (#741), on the model:
//! the first datum of a merge component places it, later ones are GPS fixes
//! that never move raw geometry. Synthetic surveys only.

use std::collections::HashMap;

use chrono::Utc;

use crate::config::ClosureAdjustmentConfig;
use crate::{
    datum_gap_threshold_m, local_to_gps, ClosureStrategy, DatumRole, GpsCoordinates, Project,
    SectionData, SectionId, ShotData, StationMerge, StationRef, ZeroDatum, MAX_GPS_ACCURACY_M,
};

/// The first entrance's GPS, and the frame of every survey here.
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

/// A section of `count` shots of `length` metres due north.
fn north_line(project: &mut Project, count: usize, length: f64) -> SectionId {
    let shots = (0..count)
        .map(|_| ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length,
            ..Default::default()
        })
        .collect();
    project
        .add_section(SectionData {
            shots,
            ..Default::default()
        })
        .id
}

/// Every station's raw (x, y), keyed by station.
fn raw_xy(project: &Project) -> HashMap<StationRef, (f64, f64)> {
    project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, (s.coordinates.x, s.coordinates.y)))
        .collect()
}

fn assert_same_xy(
    before: &HashMap<StationRef, (f64, f64)>,
    after: &HashMap<StationRef, (f64, f64)>,
) {
    assert_eq!(before.len(), after.len());
    for (sr, (bx, by)) in before {
        let (ax, ay) = after[sr];
        assert!(
            (ax - bx).abs() < 1e-6 && (ay - by).abs() < 1e-6,
            "{sr:?} moved from ({bx}, {by}) to ({ax}, {ay})"
        );
    }
}

fn xy(project: &Project, sr: StationRef) -> (f64, f64) {
    let c = project.compute_station_raw_coords(sr).unwrap();
    (c.x, c.y)
}

fn merge(project: &mut Project, a: StationRef, b: StationRef) {
    project.station_merges.push(StationMerge {
        station_a: a.into(),
        station_b: b.into(),
        merged_at: Utc::now(),
    });
}

/// Two entrances joined by a 500 m traverse: section A (250 m north from the
/// first entrance) merged end-to-start onto section B (250 m more). The first
/// entrance is the frame datum at A:0. Returns `(project, a, b)`.
fn two_entrance_traverse() -> (Project, SectionId, SectionId) {
    let mut project = Project::new("TwoEntrances".to_string(), None);
    let a = north_line(&mut project, 5, 50.0);
    let b = north_line(&mut project, 5, 50.0);
    project.ensure_shot_uuids();
    project
        .merge_stations(StationRef::new(a, 5), StationRef::new(b, 0))
        .unwrap();
    project
        .set_zero_datum(StationRef::new(a, 0), origin())
        .unwrap();
    (project, a, b)
}

#[test]
fn a_second_datum_in_a_placed_survey_is_a_gps_fix_and_moves_nothing() {
    let (mut project, a, b) = two_entrance_traverse();
    let second = StationRef::new(b, 5);
    assert_eq!(xy(&project, second), (0.0, 500.0));
    let before = raw_xy(&project);

    // The GPS of the second entrance is 40 m east of where the survey puts it.
    project
        .set_zero_datum_with_accuracy(second, gps_at(40.0, 500.0), Some(3.0))
        .expect("a second datum in a placed survey is allowed");

    assert_same_xy(&before, &raw_xy(&project));
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    assert_eq!(project.zero_datums[1].accuracy_m, Some(3.0));
    assert_eq!(
        project.frame_datum_key(),
        Some(StationRef::new(a, 0).into()),
        "the frame stays on the first entrance"
    );

    let gaps = project.gps_fix_gaps();
    let gap = gaps[&second];
    assert!((gap.gap_m - 40.0).abs() < 0.01, "gap {}", gap.gap_m);
    assert!(
        (gap.bearing_deg - 90.0).abs() < 0.1,
        "bearing {}",
        gap.bearing_deg
    );
    assert!((gap.survey_distance_m.unwrap() - 500.0).abs() < 1e-6);
    assert!((gap.threshold_m - 25.0).abs() < 1e-6);
    assert!(gap.exceeds_threshold);

    // No shot is frozen by the fix: only placement datums pin raw geometry.
    assert!(project.compute_frozen_shots().is_empty());
}

#[test]
fn re_setting_a_fix_updates_it_in_place_and_moves_nothing() {
    let (mut project, _a, b) = two_entrance_traverse();
    let second = StationRef::new(b, 5);
    project.set_zero_datum(second, gps_at(40.0, 500.0)).unwrap();
    let before = raw_xy(&project);

    project
        .set_zero_datum_with_accuracy(second, gps_at(-10.0, 480.0), Some(0.0))
        .unwrap();

    assert_same_xy(&before, &raw_xy(&project));
    assert_eq!(project.zero_datums.len(), 2);
    assert_eq!(project.zero_datums[1].accuracy_m, Some(0.0));
    assert_eq!(project.datum_roles()[1], Some(DatumRole::GpsFix));
    let gap = project.gps_fix_gaps()[&second];
    assert!((gap.gap_m - 10.0_f64.hypot(20.0)).abs() < 0.01);
}

#[test]
fn an_invalid_accuracy_is_refused_and_changes_nothing() {
    let (mut project, _a, b) = two_entrance_traverse();
    let second = StationRef::new(b, 5);
    for bad in [-0.1, f64::NAN, f64::INFINITY, MAX_GPS_ACCURACY_M + 0.5] {
        assert!(project
            .set_zero_datum_with_accuracy(second, gps_at(40.0, 500.0), Some(bad))
            .is_err());
        assert!(project
            .store_zero_datum_without_repositioning(second, gps_at(40.0, 500.0), Some(bad))
            .is_err());
    }
    assert_eq!(project.zero_datums.len(), 1);
    assert!(project
        .set_zero_datum_with_accuracy(second, gps_at(40.0, 500.0), Some(MAX_GPS_ACCURACY_M))
        .is_ok());
}

#[test]
fn preview_names_the_role_and_the_gap_before_anything_is_stored() {
    let (mut project, _a, b) = two_entrance_traverse();
    let lone = north_line(&mut project, 2, 10.0);

    let preview = project
        .preview_zero_datum(StationRef::new(b, 5), &gps_at(40.0, 500.0))
        .unwrap();
    assert_eq!(preview.role, DatumRole::GpsFix);
    assert!((preview.gap.unwrap().gap_m - 40.0).abs() < 0.01);

    let preview = project
        .preview_zero_datum(StationRef::new(lone, 0), &gps_at(5.0, 5.0))
        .unwrap();
    assert_eq!(preview.role, DatumRole::Placement);
    assert!(preview.gap.is_none());
    assert_eq!(project.zero_datums.len(), 1, "a preview stores nothing");
}

/// Two separately placed pieces, 30 m apart where they are about to be merged:
/// A (frame, datum A:0) runs 0..100 m north; B is placed by its own GPS so that
/// B:0 sits 30 m east of A:2.
fn two_placed_pieces() -> (Project, SectionId, SectionId) {
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
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    let (bx, by) = xy(&project, StationRef::new(b, 0));
    assert!((bx - 30.0).abs() < 0.01 && (by - 100.0).abs() < 0.01);
    (project, a, b)
}

#[test]
fn merging_two_placed_pieces_moves_the_later_one_and_makes_its_datum_a_fix() {
    // Both argument orders: the list order decides, never which station the
    // user picked first.
    for b_first in [false, true] {
        let (mut project, a, b) = two_placed_pieces();
        let a_before: Vec<_> = (0..=2)
            .map(|i| xy(&project, StationRef::new(a, i)))
            .collect();
        let (end_a, start_b) = (StationRef::new(a, 2), StationRef::new(b, 0));
        let result = if b_first {
            project.merge_stations(start_b, end_a)
        } else {
            project.merge_stations(end_a, start_b)
        };
        result.expect("merging two placed pieces is allowed");

        let a_after: Vec<_> = (0..=2)
            .map(|i| xy(&project, StationRef::new(a, i)))
            .collect();
        assert_eq!(a_before, a_after, "the earlier placement holds its piece");
        let (ex, ey) = xy(&project, end_a);
        let (sx, sy) = xy(&project, start_b);
        assert!((ex - sx).abs() < 1e-9 && (ey - sy).abs() < 1e-9);
        assert_eq!(
            project.datum_roles(),
            vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
        );
        // The fix now shows the 30 m the merge moved it by.
        let gap = project.gps_fix_gaps()[&start_b];
        assert!((gap.gap_m - 30.0).abs() < 0.01, "gap {}", gap.gap_m);

        // Nothing tears afterwards: the fix's section is not authoritative.
        project.resolve_offsets();
        assert!(project.check_merge_alignment().is_empty());
        assert_eq!(project.datum_roles()[1], Some(DatumRole::GpsFix));
    }
}

#[test]
fn unmerging_returns_the_fix_to_placement_and_moves_nothing() {
    let (mut project, a, b) = two_placed_pieces();
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    let merged = raw_xy(&project);

    let (ok, _) = project.unmerge_stations(StationRef::new(a, 2), StationRef::new(b, 0));
    assert!(ok);
    project.resolve_offsets();

    assert_same_xy(&merged, &raw_xy(&project));
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)],
        "the detached piece is placed by its own datum again, where it is"
    );
}

#[test]
fn removing_a_placement_datum_promotes_the_fix_without_moving_raw_geometry() {
    // (a) The removed placement is not the frame: an unrelated piece holds it.
    let (mut project, a, b) = two_entrance_traverse();
    let elsewhere = north_line(&mut project, 1, 10.0);
    project
        .set_zero_datum(StationRef::new(b, 5), gps_at(40.0, 500.0))
        .unwrap();
    // Make the unrelated piece the frame by putting its datum first.
    project
        .store_zero_datum_without_repositioning(StationRef::new(elsewhere, 0), origin(), None)
        .unwrap();
    let frame = project.zero_datums.pop().unwrap();
    project.zero_datums.insert(0, frame);
    project.renormalize_frame();
    project.resolve_offsets();
    let before = raw_xy(&project);
    assert_eq!(
        project.datum_roles(),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix)
        ]
    );

    let frame_before = project.frame_datum_key();
    assert!(project.remove_zero_datum(StationRef::new(a, 0)));
    project.renormalize_frame_if_changed(frame_before);
    project.resolve_offsets();
    assert_same_xy(&before, &raw_xy(&project));
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );

    // (b) The removed placement IS the frame: the only movement is the frame's
    // rigid re-translation onto the promoted datum's station.
    let (mut project, a, b) = two_entrance_traverse();
    project
        .set_zero_datum(StationRef::new(b, 5), gps_at(40.0, 500.0))
        .unwrap();
    let before = raw_xy(&project);
    let frame_before = project.frame_datum_key();
    assert!(project.remove_zero_datum(StationRef::new(a, 0)));
    project.renormalize_frame_if_changed(frame_before);
    project.resolve_offsets();
    let after = raw_xy(&project);
    for (sr, (bx, by)) in &before {
        let (ax, ay) = after[sr];
        assert!(
            (ax - bx).abs() < 1e-6 && (ay - (by - 500.0)).abs() < 1e-6,
            "{sr:?}: one rigid translation by (0, -500) only"
        );
    }
    assert_eq!(project.datum_roles(), vec![Some(DatumRole::Placement)]);
}

#[test]
fn gap_warning_boundary_is_15_m_plus_2_percent() {
    // 500 m of survey between the two datums: the threshold is 25 m.
    assert!((datum_gap_threshold_m(500.0) - 25.0).abs() < 1e-12);
    for (east, exceeds) in [(24.9, false), (25.1, true)] {
        let (mut project, _a, b) = two_entrance_traverse();
        project
            .set_zero_datum(StationRef::new(b, 5), gps_at(east, 500.0))
            .unwrap();
        let gap = project.gps_fix_gaps()[&StationRef::new(b, 5)];
        assert!((gap.threshold_m - 25.0).abs() < 1e-6);
        assert_eq!(gap.exceeds_threshold, exceeds, "gap {}", gap.gap_m);
    }
}

/// What an older build could leave in a file: two pieces each anchored by its
/// own datum (A+B by A:0, C by C:0) and a merge B:2 = C:0 between them whose
/// endpoints sit 40 m apart, because the old cascade held both datums.
///   A:0 (0,0)  A:2 (0,20) = B:0   B:2 (0,40)
///   C:0 (0,0)  C:2 (0,20)
fn over_anchored_legacy_file() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("Legacy".to_string(), None);
    let a = north_line(&mut project, 2, 10.0);
    let b = north_line(&mut project, 2, 10.0);
    let c = north_line(&mut project, 2, 10.0);
    project.ensure_shot_uuids();
    merge(&mut project, StationRef::new(a, 2), StationRef::new(b, 0));
    project.set_section_offset(b, (0.0, 20.0));
    project
        .zero_datums
        .push(ZeroDatum::new(StationRef::new(a, 0).into(), origin()));
    project.zero_datums.push(ZeroDatum::new(
        StationRef::new(c, 0).into(),
        gps_at(0.0, 2.0),
    ));
    merge(&mut project, StationRef::new(b, 2), StationRef::new(c, 0));
    (project, a, b, c)
}

#[test]
fn an_old_over_anchored_file_loads_with_a_fix_and_no_torn_merge() {
    let (mut project, a, b, c) = over_anchored_legacy_file();
    assert_eq!(
        project.check_merge_alignment().len(),
        1,
        "setup: torn on disk"
    );
    let a_before: Vec<_> = (0..=2)
        .map(|i| xy(&project, StationRef::new(a, i)))
        .collect();

    project.redrive_offsets_after_load();

    assert!(
        project.check_merge_alignment().is_empty(),
        "the later datum is a fix, so the load re-solve closes the merge"
    );
    assert_eq!(project.station_merges.len(), 2, "no merge was cut");
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    let a_after: Vec<_> = (0..=2)
        .map(|i| xy(&project, StationRef::new(a, i)))
        .collect();
    assert_eq!(
        a_before, a_after,
        "the placement datum's piece does not move"
    );
    let (cx, cy) = xy(&project, StationRef::new(c, 0));
    let (bx, by) = xy(&project, StationRef::new(b, 2));
    assert!((cx - bx).abs() < 1e-9 && (cy - by).abs() < 1e-9);
    assert!(project
        .check_structural_integrity()
        .iter()
        .all(|v| !v.category.contains("datum")));
}

#[test]
fn an_automatic_promotion_still_refuses_to_fuse_two_placed_pieces() {
    // A sole-bridge closure between two placed pieces stays a closure: promoting
    // it would translate a GPS-placed piece by the misclosure.
    let (mut project, a, b) = two_placed_pieces();
    project
        .create_loop_closure(
            StationRef::new(a, 2),
            StationRef::new(b, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    let before = raw_xy(&project);
    assert_eq!(project.convert_orphan_closures_to_merges(), 0);
    assert!(project.station_merges.is_empty());
    assert_same_xy(&before, &raw_xy(&project));
}

#[test]
fn a_component_with_only_a_gps_fix_reaches_the_computed_view_solve() {
    // No closure at all: before #741 the solve skipped the component. The fix is
    // an observation of its own, so the gate lets it through and the fixed-datum
    // check reads placement datums only.
    let (mut project, a, b) = two_entrance_traverse();
    project
        .set_zero_datum(StationRef::new(b, 5), gps_at(40.0, 500.0))
        .unwrap();
    assert!(project.loop_closures.is_empty());

    let anchors = project.plan_adjustment_anchors();
    assert_eq!(anchors, vec![StationRef::new(a, 0)]);
    let mut working = project.clone();
    working
        .adjust_loop_closures(
            StationRef::new(a, 0),
            ClosureStrategy::LeastSquaresWeighted,
            &ClosureAdjustmentConfig::default(),
        )
        .expect("a fix-only component is solvable");

    // A project with no fix and no closure still takes the fast path.
    let (plain, _, _) = two_entrance_traverse();
    assert!(plain.plan_adjustment_anchors().is_empty());
}
