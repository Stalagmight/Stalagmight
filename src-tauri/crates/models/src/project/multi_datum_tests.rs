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
fn unmerging_places_the_detached_piece_on_its_own_gps() {
    let (mut project, a, b) = two_placed_pieces();
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    let merged = raw_xy(&project);
    // The merge moved B 30 m west onto A:2; its datum is a fix 30 m off.
    assert_eq!(xy(&project, StationRef::new(b, 0)), (0.0, 100.0));

    let before = project.datum_layout();
    let (ok, _) = project.unmerge_stations(StationRef::new(a, 2), StationRef::new(b, 0));
    assert!(ok);
    assert!(project.settle_datum_layout(&before));
    project.resolve_offsets();

    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)],
        "the detached piece is placed by its own datum again"
    );
    // A stays; B goes back onto its GPS, 30 m east, rigidly.
    let after = raw_xy(&project);
    for (sr, (mx, my)) in &merged {
        let (ax, ay) = after[sr];
        let east = if sr.section_id == b { 30.0 } else { 0.0 };
        assert!(
            (ax - (mx + east)).abs() < 1e-6 && (ay - my).abs() < 1e-6,
            "{sr:?}: ({mx}, {my}) -> ({ax}, {ay})"
        );
    }
}

#[test]
fn removing_a_placement_datum_places_the_promoted_fix_on_its_gps() {
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

    let layout = project.datum_layout();
    assert!(project.remove_zero_datum(StationRef::new(a, 0)));
    project.settle_datum_layout(&layout);
    project.resolve_offsets();
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    // The frame did not change; the traverse moves 40 m east so the promoted
    // fix sits on its GPS, and the frame's piece stays.
    let after = raw_xy(&project);
    for (sr, (bx, by)) in &before {
        let (ax, ay) = after[sr];
        let east = if sr.section_id == elsewhere {
            0.0
        } else {
            40.0
        };
        assert!(
            (ax - (bx + east)).abs() < 1e-6 && (ay - by).abs() < 1e-6,
            "{sr:?}: ({bx}, {by}) -> ({ax}, {ay})"
        );
    }
    let (fx, fy) = xy(&project, StationRef::new(b, 5));
    assert!((fx - 40.0).abs() < 1e-6 && (fy - 500.0).abs() < 1e-6);

    // (b) The removed placement IS the frame and the survey is one piece: the
    // placement and the re-frame onto the promoted datum add up to one rigid
    // translation by (0, -500).
    let (mut project, a, b) = two_entrance_traverse();
    project
        .set_zero_datum(StationRef::new(b, 5), gps_at(40.0, 500.0))
        .unwrap();
    let before = raw_xy(&project);
    let layout = project.datum_layout();
    assert!(project.remove_zero_datum(StationRef::new(a, 0)));
    project.settle_datum_layout(&layout);
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
fn settling_twice_with_the_same_layout_moves_nothing_when_the_frame_is_kept() {
    // A placement only measures, so a repeat with the frame unchanged is inert.
    let (mut project, a, b) = two_entrance_traverse();
    let elsewhere = north_line(&mut project, 1, 10.0);
    project
        .set_zero_datum(StationRef::new(elsewhere, 0), gps_at(-200.0, 0.0))
        .unwrap();
    project
        .set_zero_datum(StationRef::new(b, 5), gps_at(40.0, 500.0))
        .unwrap();
    // Make the separate piece's datum the frame by moving it first.
    let elsewhere_datum = project.zero_datums.remove(1);
    project.zero_datums.insert(0, elsewhere_datum);
    project.renormalize_frame();
    let layout = project.datum_layout();
    assert!(project.remove_zero_datum(StationRef::new(a, 0)));
    assert!(project.settle_datum_layout(&layout));
    let settled = raw_xy(&project);
    assert!(!project.settle_datum_layout(&layout));
    assert_same_xy(&settled, &raw_xy(&project));
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
fn an_old_over_anchored_file_opens_with_nothing_moved() {
    // The open path re-solves offsets; left a merge, the torn B:2 = C:0 would
    // be healed by moving C (whose datum is the later one, so a GPS fix) 40 m
    // off its GPS. Opening a file moves nothing (#741 D6): the merge becomes
    // the loop closure it should have stayed, and each piece keeps its datum.
    let (mut project, a, b, c) = over_anchored_legacy_file();
    assert_eq!(
        project.check_merge_alignment().len(),
        1,
        "setup: torn on disk"
    );
    let before = raw_xy(&project);

    project.redrive_offsets_after_load();

    assert_same_xy(&before, &raw_xy(&project));
    assert_eq!(project.station_merges.len(), 1, "only the torn merge went");
    assert!(project.check_merge_alignment().is_empty());
    assert_eq!(project.loop_closures.len(), 1);
    let closure = &project.loop_closures[0];
    assert!(!closure.is_aligned, "a soft closure moves nothing");
    let ends = [
        project.resolve_key(&closure.station_a),
        project.resolve_key(&closure.station_b),
    ];
    assert!(ends.contains(&Some(StationRef::new(b, 2))));
    assert!(ends.contains(&Some(StationRef::new(c, 0))));
    assert!((closure.misclosure_distance - 40.0).abs() < 1e-6);
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)],
        "each piece is placed by its own datum again"
    );
    assert_eq!(xy(&project, StationRef::new(a, 0)), (0.0, 0.0));
    assert!(project
        .check_structural_integrity()
        .iter()
        .all(|v| !v.category.contains("datum")));

    // Opening again changes nothing more.
    let settled = raw_xy(&project);
    project.redrive_offsets_after_load();
    assert_same_xy(&settled, &raw_xy(&project));
    assert_eq!(project.loop_closures.len(), 1);
}

#[test]
fn an_old_aligned_group_with_two_datums_opens_unchanged_with_a_fix() {
    // An older file whose merged group holds two datums but no torn merge (the
    // second piece already sat on the first): nothing moves on open, nothing is
    // demoted, and the later datum derives as a GPS fix that reports its gap.
    let (mut project, _a, b, c) = over_anchored_legacy_file();
    project.set_section_offset(c, (0.0, 40.0));
    assert!(project.check_merge_alignment().is_empty(), "setup: aligned");
    let before = raw_xy(&project);

    project.redrive_offsets_after_load();

    assert_same_xy(&before, &raw_xy(&project));
    assert_eq!(project.station_merges.len(), 2);
    assert!(project.loop_closures.is_empty());
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    let gaps = project.gps_fix_gaps();
    let gap = gaps[&StationRef::new(c, 0)];
    assert_eq!(xy(&project, StationRef::new(b, 2)), (0.0, 40.0));
    assert!((gap.gap_m - 38.0).abs() < 1e-3, "{gap:?}");
}

#[test]
fn a_torn_merge_whose_far_half_holds_no_datum_still_heals_on_open() {
    // The demotion is for two datum-placed halves only: an ordinary piece
    // with drift (no datum of its own) is pulled back onto its merge, as it
    // always was.
    let (mut project, _a, b, c) = over_anchored_legacy_file();
    project.zero_datums.truncate(1);
    project.redrive_offsets_after_load();
    assert_eq!(project.station_merges.len(), 2, "nothing demoted");
    assert!(project.loop_closures.is_empty());
    assert!(project.check_merge_alignment().is_empty());
    assert_eq!(
        xy(&project, StationRef::new(c, 0)),
        xy(&project, StationRef::new(b, 2))
    );
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

    let anchors = project.adjustment_anchors_for_tests();
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
    assert!(plain.adjustment_anchors_for_tests().is_empty());
}

#[test]
fn the_distribution_fallback_walks_to_a_placement_datum_never_a_fix() {
    // `shortest_path_to_any_zero_datum` feeds the destructive closure
    // distribution, which only placement datums anchor (#741). From B:5 the fix
    // at B:4 is one shot away and the placement datum at A:0 is nine: the walk
    // must end at A:0.
    let (mut project, a, b) = two_entrance_traverse();
    project
        .set_zero_datum(StationRef::new(b, 4), gps_at(40.0, 450.0))
        .unwrap();
    assert_eq!(project.datum_roles()[1], Some(DatumRole::GpsFix));

    let (found, _path) = project
        .shortest_path_to_any_zero_datum(StationRef::new(b, 5))
        .expect("the placement datum is reachable");
    assert_eq!(found, StationRef::new(a, 0));
}

/// Store a GPS fix on `sr` directly, as a load does: no placement, no checks.
fn push_fix(project: &mut Project, sr: StationRef, gps: GpsCoordinates) {
    let station_ref = project.key_for_ref(sr).unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref,
        gps,
        accuracy_m: None,
        entrance: None,
    });
}

#[test]
fn each_fix_measures_its_survey_distance_from_its_placement_datum() {
    // A 400 m line L (40 shots of 10 m) placed at L:0, with fixes at L:10,
    // L:30 and L:31, and a 40 m branch merged at L:22 with a fix at its end.
    // Every distance runs back to L:0, the datum the raw positions are
    // carried from, however close another fix is (#741 U5).
    let mut project = Project::new("PlacementDistance".to_string(), None);
    let line = north_line(&mut project, 40, 10.0);
    let branch = north_line(&mut project, 4, 10.0);
    project.ensure_shot_uuids();
    merge(
        &mut project,
        StationRef::new(line, 22),
        StationRef::new(branch, 0),
    );
    project
        .set_zero_datum(StationRef::new(line, 0), origin())
        .unwrap();
    let fixes = [
        (StationRef::new(line, 10), 100.0),
        (StationRef::new(line, 30), 300.0),
        (StationRef::new(line, 31), 310.0),
        (StationRef::new(branch, 4), 260.0),
    ];
    for (i, (sr, _)) in fixes.iter().enumerate() {
        push_fix(&mut project, *sr, gps_at(i as f64, 100.0 * i as f64));
    }

    let gaps = project.gps_fix_gaps();
    assert_eq!(gaps.len(), fixes.len());
    for (sr, expected) in fixes {
        let got = gaps[&sr].survey_distance_m.unwrap();
        assert!((got - expected).abs() < 1e-9, "{sr:?}: {got} vs {expected}");
        // The one-pass answer agrees with the per-station preview path.
        let zd = project
            .zero_datums
            .iter()
            .find(|zd| project.resolve_key(&zd.station_ref) == Some(sr))
            .unwrap();
        assert_eq!(project.datum_gap(sr, &zd.gps), Some(gaps[&sr]));
    }
}

#[test]
fn gap_check_cost_does_not_multiply_with_the_number_of_fixes() {
    // 80 sections of 50 shots chained end to start, a placement datum at the
    // start and a GPS fix on every other station: about 4000 fixes. One Dijkstra
    // per fix took tens of seconds here; one shared pass takes a fraction of one.
    let mut project = Project::new("ManyFixes".to_string(), None);
    let sections: Vec<SectionId> = (0..80).map(|_| north_line(&mut project, 50, 2.0)).collect();
    project.ensure_shot_uuids();
    for pair in sections.windows(2) {
        merge(
            &mut project,
            StationRef::new(pair[0], 50),
            StationRef::new(pair[1], 0),
        );
    }
    project
        .set_zero_datum(StationRef::new(sections[0], 0), origin())
        .unwrap();
    for (s, &section) in sections.iter().enumerate() {
        for i in 1..=50u32 {
            let north = (s as f64 * 50.0 + f64::from(i)) * 2.0;
            push_fix(
                &mut project,
                StationRef::new(section, i),
                gps_at(1.0, north),
            );
        }
    }
    assert_eq!(project.gps_fix_datums().len(), 4000);

    let started = std::time::Instant::now();
    let gaps = project.gps_fix_gaps();
    let elapsed = started.elapsed();
    assert_eq!(gaps.len(), 4000);
    // Each fix is measured back along the chain to the placement datum.
    for (sr, g) in &gaps {
        let s = sections.iter().position(|id| *id == sr.section_id).unwrap();
        let expected = (s as f64 * 50.0 + f64::from(sr.station_index)) * 2.0;
        assert!((g.survey_distance_m.unwrap() - expected).abs() < 1e-6);
    }
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "gps_fix_gaps over 4000 fixes took {elapsed:?}"
    );
}

#[test]
fn a_gps_fix_still_pulls_a_component_whose_only_closure_spans_one_shot() {
    // The component's only closure ties the two ends of shot A:0-A:1, so the
    // solve skips it (#737) - which on its own would leave the component at its
    // measured geometry. The hard GPS fix on B:5, 40 m east of where the survey
    // puts it, is an observation of its own (#741): the component is still
    // solved, and the fix station lands on its GPS.
    let (mut project, a, b) = two_entrance_traverse();
    let fix = StationRef::new(b, 5);
    project.set_zero_datum(fix, gps_at(40.0, 500.0)).unwrap();
    let (key_a, key_b) = (
        project.key_for_ref(StationRef::new(a, 0)).unwrap(),
        project.key_for_ref(StationRef::new(a, 1)).unwrap(),
    );
    project.normalize_next_closure_id();
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    project.loop_closures.push(crate::LoopClosure {
        id,
        station_a: key_a,
        station_b: key_b,
        path_length: 50.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    let skipped: Vec<u64> = project
        .closures_across_one_shot()
        .into_iter()
        .map(|c| c.closure_id)
        .collect();
    assert_eq!(skipped, [id]);
    assert_eq!(xy(&project, fix), (0.0, 500.0));

    let solved = project.compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    assert!(solved.failures.is_empty(), "{:?}", solved.failures);
    let at = |sr: StationRef| {
        let s = solved
            .stations
            .iter()
            .find(|s| s.station_ref == sr)
            .unwrap();
        (s.coordinates.x, s.coordinates.y)
    };
    let (x, y) = at(fix);
    assert!(
        (x - 40.0).abs() < 1e-6 && (y - 500.0).abs() < 1e-6,
        "the fix station is at ({x}, {y}), not on its GPS"
    );
    let (x0, y0) = at(StationRef::new(a, 0));
    assert!(
        x0.abs() < 1e-6 && y0.abs() < 1e-6,
        "the placement datum moved"
    );
}

#[test]
fn a_second_consistent_fix_does_not_shrink_the_first_fixs_threshold() {
    // 2010 m north, placed at A:0. A fix at A:200 (2000 m along) 40 m off its
    // survey is inside 15 m + 2% of 2000 m = 55 m. A second fix 10 m further on,
    // off by the same 40 m, agrees with it: neither threshold may fall to
    // 15 m + 2% of the 10 m between them (#741 U5).
    let mut project = Project::new("TwoFixes".to_string(), None);
    let a = north_line(&mut project, 201, 10.0);
    project.ensure_shot_uuids();
    project
        .set_zero_datum(StationRef::new(a, 0), origin())
        .unwrap();
    let first = StationRef::new(a, 200);
    project.set_zero_datum(first, gps_at(40.0, 2000.0)).unwrap();
    let one = project.gps_fix_gaps()[&first];
    assert!((one.threshold_m - 55.0).abs() < 1e-6, "{one:?}");
    assert!(!one.exceeds_threshold);

    project
        .set_zero_datum(StationRef::new(a, 201), gps_at(40.0, 2010.0))
        .unwrap();
    let gaps = project.gps_fix_gaps();
    assert_eq!(gaps.len(), 2);
    assert_eq!(gaps[&first], one, "the first fix's gap is unchanged");
    let second = gaps[&StationRef::new(a, 201)];
    assert!((second.threshold_m - 55.2).abs() < 1e-6, "{second:?}");
    assert!(!second.exceeds_threshold);
}

#[test]
fn a_loop_closure_is_no_shortcut_for_the_gap_threshold() {
    // A 300 m line placed at A:0 whose far end is closed back onto A:0 by a
    // loop closure: the raw position of A:30 is still carried 300 m along the
    // shots, so its threshold is 15 m + 2% of 300 m, not the 15 m a zero-cost
    // closure edge back to the datum would give.
    let mut project = Project::new("Loop".to_string(), None);
    let a = north_line(&mut project, 30, 10.0);
    project.ensure_shot_uuids();
    project
        .set_zero_datum(StationRef::new(a, 0), origin())
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(a, 30),
            ClosureStrategy::Batch,
        )
        .unwrap();
    let gap = project
        .datum_gap(StationRef::new(a, 30), &gps_at(20.0, 300.0))
        .unwrap();
    assert!(
        (gap.survey_distance_m.unwrap() - 300.0).abs() < 1e-6,
        "{gap:?}"
    );
    assert!((gap.threshold_m - 21.0).abs() < 1e-6);
    assert!(!gap.exceeds_threshold);
}

#[test]
fn a_station_holds_at_most_one_datum() {
    // Re-setting the datum on its own station updates it in place; a second
    // datum stored on that station under another key is refused, so two
    // datums never share a station (#741 D2).
    let (mut project, a, b) = two_entrance_traverse();
    let fix = StationRef::new(b, 5);
    project.set_zero_datum(fix, gps_at(40.0, 500.0)).unwrap();
    project
        .set_zero_datum_with_accuracy(fix, gps_at(41.0, 500.0), Some(2.0))
        .unwrap();
    assert_eq!(project.zero_datums.len(), 2, "updated in place");
    assert_eq!(project.zero_datums[1].accuracy_m, Some(2.0));

    // A datum held under a positional key (as a load or an append stores it
    // before re-anchoring) is the same station as the stable key a new set
    // would mint: the set is refused rather than adding a second datum.
    project.zero_datums[1].station_ref = StationRef::new(b, 5).into();
    assert_ne!(
        project.zero_datums[1].station_ref,
        project.key_for_ref(fix).unwrap()
    );
    let err = project
        .set_zero_datum(fix, gps_at(42.0, 500.0))
        .unwrap_err();
    assert!(err.contains("already holds a zero datum"), "{err}");
    let err = project
        .store_zero_datum_without_repositioning(fix, gps_at(42.0, 500.0), None)
        .unwrap_err();
    assert!(err.contains("already holds a zero datum"), "{err}");
    assert_eq!(project.zero_datums.len(), 2);
    assert!(project.station_exists(StationRef::new(a, 0)));
}
