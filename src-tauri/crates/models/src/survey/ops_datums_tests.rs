//! Zero datums (plan 5.7, 4.1, 4.3), ported from today's
//! `multi_datum_tests`, `datum_ui_tests`, `datum_entrance_tests` (models) and
//! the model behaviour of core's `multi_datum_tests` /
//! `datum_entrance_tests`. Synthetic surveys only.

use std::collections::BTreeMap;

use crate::{gps_to_local, DatumRole, GpsCoordinates, StationId, StationRef, MAX_GPS_ACCURACY_M};

use super::m3_stubs::{datum_gap, datum_roles};
use super::ops_test_helpers::{
    assert_shifted, assert_still, assert_valid, branch, free_line, gps_at, gps_local, near, ok,
    origin, run, xy, xy_all, z_all,
};
use super::test_helpers::{at, project, shot};
use super::{DatumChange, LineId, OpError, Project, ZeroDatum};

const TOL: f64 = 1e-9;

fn roles(p: &Project) -> Vec<Option<DatumRole>> {
    datum_roles(p)
}

/// Two entrances joined by a 500 m traverse: A (250 m north from the first
/// entrance, the frame datum at A:0) and B (250 m more) starting on A's last
/// station. Returns `(project, a, b)`.
fn two_entrance_traverse() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 5, 50.0));
    let a5 = at(&p, a, 5);
    let b = branch(&mut p, a5, run(0.0, 5, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    (p, a, b)
}

/// The frame correction for a datum at `gps`: where the new frame puts it,
/// less where the old one did.
fn frame_correction(gps: &GpsCoordinates, old: &GpsCoordinates, new: &GpsCoordinates) -> [f64; 2] {
    let (nx, ny) = gps_to_local(gps, new);
    let (ox, oy) = gps_to_local(gps, old);
    [nx - ox, ny - oy]
}

fn on_line(p: &Project, line: LineId) -> Vec<StationId> {
    p.line_stations(line).unwrap().to_vec()
}

// ----- Set: role, fixes, accuracy ------------------------------------------

#[test]
fn a_second_datum_in_a_placed_survey_is_a_gps_fix_and_moves_nothing() {
    let (mut p, _a, b) = two_entrance_traverse();
    let second = at(&p, b, 5);
    near(xy(&p, second), [0.0, 500.0], 1e-9, "B:5");
    let before = xy_all(&p);
    let out = ok(p.set_datum_at(StationRef::new(b, 5), gps_at(40.0, 500.0), Some(3.0)));
    assert_valid(&p);
    assert_eq!(out.value.change, DatumChange::Added);
    assert_eq!(out.value.role, Some(DatumRole::GpsFix));
    assert_still(&before, &xy_all(&p), TOL);
    assert_eq!(
        roles(&p),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    assert_eq!(p.datums[1].accuracy_m, Some(3.0));
    assert_eq!(p.datums[1].via, Some(b));
    assert_eq!(
        p.frame_datum(),
        Some(0),
        "the frame stays on the first entrance"
    );

    let gap = datum_gap(&p, second, &p.datums[1].gps).unwrap();
    assert!((gap.gap_m - 40.0).abs() < 0.01, "gap {}", gap.gap_m);
    assert!(
        (gap.bearing_deg - 90.0).abs() < 0.1,
        "bearing {}",
        gap.bearing_deg
    );
    assert!((gap.survey_distance_m.unwrap() - 500.0).abs() < 1e-6);
    assert!((gap.threshold_m - 25.0).abs() < 1e-6);
    assert!(gap.exceeds_threshold);
}

#[test]
fn re_setting_a_fix_updates_it_in_place_and_moves_nothing() {
    let (mut p, _a, b) = two_entrance_traverse();
    let fix = StationRef::new(b, 5);
    ok(p.set_datum_at(fix, gps_at(40.0, 500.0), None));
    let before = xy_all(&p);
    let out = ok(p.set_datum_at(fix, gps_at(-10.0, 480.0), Some(0.0)));
    assert_eq!(out.value.change, DatumChange::GpsEdited);
    assert_still(&before, &xy_all(&p), TOL);
    assert_eq!(p.datums.len(), 2);
    assert_eq!(p.datums[1].accuracy_m, Some(0.0));
    assert_eq!(roles(&p)[1], Some(DatumRole::GpsFix));
    let gap = datum_gap(&p, at(&p, b, 5), &p.datums[1].gps).unwrap();
    assert!((gap.gap_m - 10.0_f64.hypot(20.0)).abs() < 0.01);
}

#[test]
fn an_invalid_accuracy_or_gps_is_refused_and_changes_nothing() {
    let (mut p, _a, b) = two_entrance_traverse();
    let fix = StationRef::new(b, 5);
    let before = p.clone();
    for bad in [-0.1, f64::NAN, f64::INFINITY, MAX_GPS_ACCURACY_M + 0.5] {
        assert!(matches!(
            p.set_datum_at(fix, gps_at(40.0, 500.0), Some(bad)),
            Err(OpError::InvalidAccuracy(_))
        ));
    }
    let off_the_globe = GpsCoordinates {
        latitude: 91.0,
        longitude: 0.0,
        depth_reference: 0.0,
    };
    assert!(matches!(
        p.set_datum_at(fix, off_the_globe, None),
        Err(OpError::InvalidGps(_))
    ));
    assert_eq!(p, before);
    ok(p.set_datum_at(fix, gps_at(40.0, 500.0), Some(MAX_GPS_ACCURACY_M)));
}

#[test]
fn a_placement_datum_stores_the_accuracy_it_is_sent() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 10.0));
    let datum = StationRef::new(a, 0);
    ok(p.set_datum_at(datum, origin(), Some(7.0)));
    assert_eq!(roles(&p)[0], Some(DatumRole::Placement));
    ok(p.set_datum_at(datum, gps_at(0.0, 100.0), Some(7.0)));
    assert_eq!(p.datums[0].accuracy_m, Some(7.0));
    ok(p.set_datum_at(datum, gps_at(0.0, 200.0), None));
    assert_eq!(p.datums[0].accuracy_m, None);
    assert_eq!(p.datums.len(), 1);
}

#[test]
fn preview_names_the_role_and_the_gap_before_anything_is_stored() {
    let (mut p, _a, b) = two_entrance_traverse();
    let lone = free_line(&mut p, [900.0, 0.0], run(0.0, 2, 10.0));
    let b5 = at(&p, b, 5);
    let preview = p
        .preview_zero_datum(b5, Some(b), &gps_at(40.0, 500.0))
        .unwrap();
    assert_eq!(preview.role, DatumRole::GpsFix);
    assert!((preview.gap.unwrap().gap_m - 40.0).abs() < 0.01);
    let preview = p
        .preview_zero_datum(at(&p, lone, 0), Some(lone), &gps_at(5.0, 5.0))
        .unwrap();
    assert_eq!(preview.role, DatumRole::Placement);
    assert!(preview.gap.is_none());
    assert!(p.preview_zero_datum(999, None, &origin()).is_none());
    assert_eq!(p.datums.len(), 1, "a preview stores nothing");
}

// ----- the first datum, free pieces, the last datum ------------------------

#[test]
fn the_first_datum_translates_the_whole_survey_onto_it() {
    let mut p = project();
    let a = free_line(&mut p, [100.0, 50.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [300.0, 0.0], run(0.0, 2, 10.0));
    let a1 = at(&p, a, 1);
    let before = xy_all(&p);
    let shift = [-before[&a1][0], -before[&a1][1]];
    let out = ok(p.set_datum_at(StationRef::new(a, 1), gps_at(1234.0, -567.0), None));
    assert_valid(&p);
    assert!(out.anchors.frame_changed);
    near(out.anchors.translation, shift, TOL, "translation");
    assert_eq!(xy(&p, a1), [0.0, 0.0], "the frame station is the origin");
    assert_shifted(&before, &xy_all(&p), 1e-9, |_| shift);
    assert_eq!(p.placements.len(), 1, "only the free piece keeps one");
    assert_eq!(p.placements[0].station, at(&p, b, 0));
}

#[test]
fn a_datum_on_a_free_piece_drops_its_placement_and_puts_it_on_its_gps() {
    let (mut p, a, _b) = two_entrance_traverse();
    let f = free_line(&mut p, [700.0, 700.0], run(90.0, 2, 10.0));
    let traverse = xy_all(&p);
    let target = gps_at(-80.0, 30.0);
    let out = ok(p.set_datum_at(StationRef::new(f, 2), target, None));
    assert_valid(&p);
    assert_eq!(out.value.role, Some(DatumRole::Placement));
    assert_eq!(out.anchors.placed_on_gps, vec![1]);
    assert!(p.placements.is_empty());
    near(
        xy(&p, at(&p, f, 2)),
        gps_local(&p, &target),
        1e-9,
        "F:2 on its GPS",
    );
    near(
        xy(&p, at(&p, f, 0)),
        [-100.0, 30.0],
        1e-6,
        "F:0 rides along",
    );
    let a_only: BTreeMap<_, _> = traverse
        .into_iter()
        .filter(|(s, _)| on_line(&p, a).contains(s))
        .collect();
    assert_still(&a_only, &xy_all(&p), TOL);
}

#[test]
fn removing_the_last_datum_moves_nothing_and_leaves_a_placement() {
    let (mut p, _a, _b) = two_entrance_traverse();
    let before = xy_all(&p);
    let out = ok(p.remove_datum(0));
    assert_valid(&p);
    assert!(!out.anchors.frame_changed);
    assert!(p.datums.is_empty());
    assert_eq!(p.placements.len(), 1);
    assert_still(&before, &xy_all(&p), TOL);
    assert_eq!(p.remove_datum(0).err(), Some(OpError::UnknownDatum(0)));
}

#[test]
fn removing_the_only_datum_of_a_non_frame_piece_moves_nothing() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    let before = xy_all(&p);
    let removed = p
        .remove_datum_at(StationRef::new(b, 0))
        .unwrap()
        .expect("B:0 holds a datum")
        .value;
    assert_valid(&p);
    assert_eq!(removed.station, at(&p, b, 0));
    assert_eq!(roles(&p), vec![Some(DatumRole::Placement)]);
    assert_eq!(p.placements.len(), 1);
    assert_still(&before, &xy_all(&p), TOL);
    assert!(p.remove_datum_at(StationRef::new(b, 1)).unwrap().is_none());
}

// ----- remove: promotion and frame change ----------------------------------

#[test]
fn removing_a_non_frame_placement_datum_places_the_promoted_fix_on_its_gps() {
    // E holds the frame; the traverse is placed by its own datum at A:0, with
    // a fix at B:5 that is 40 m east of where the survey puts it.
    let mut p = project();
    let e = free_line(&mut p, [0.0, 0.0], run(0.0, 1, 10.0));
    ok(p.set_datum_at(StationRef::new(e, 0), origin(), None));
    let a = free_line(&mut p, [999.0, 0.0], run(0.0, 5, 50.0));
    let a5 = at(&p, a, 5);
    let b = branch(&mut p, a5, run(0.0, 5, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), gps_at(200.0, 0.0), None));
    let fix_gps = gps_at(240.0, 500.0);
    ok(p.set_datum_at(StationRef::new(b, 5), fix_gps, None));
    near(xy(&p, at(&p, b, 5)), [200.0, 500.0], 1e-6, "fix station");
    let before = xy_all(&p);

    let out = ok(p.remove_datum(1));
    assert_valid(&p);
    assert!(!out.anchors.frame_changed);
    assert_eq!(
        roles(&p),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    near(
        xy(&p, at(&p, b, 5)),
        gps_local(&p, &fix_gps),
        1e-6,
        "the promoted fix sits on its GPS",
    );
    let e_stations = on_line(&p, e);
    assert_shifted(&before, &xy_all(&p), 1e-6, |s| {
        if e_stations.contains(&s) {
            [0.0, 0.0]
        } else {
            [40.0, 0.0]
        }
    });
}

#[test]
fn removing_the_frame_of_a_single_piece_is_one_rigid_translation() {
    // The promoted fix goes onto its GPS against the old frame (+40 east),
    // then the survey is re-framed onto it: (0, -500) in all.
    let (mut p, _a, b) = two_entrance_traverse();
    ok(p.set_datum_at(StationRef::new(b, 5), gps_at(40.0, 500.0), None));
    let before = xy_all(&p);
    let out = ok(p.remove_datum(0));
    assert_valid(&p);
    assert!(out.anchors.frame_changed);
    assert_eq!(roles(&p), vec![Some(DatumRole::Placement)]);
    assert_shifted(&before, &xy_all(&p), 1e-6, |_| [0.0, -500.0]);
    assert_eq!(p.datums[0].correction, None);
}

#[test]
fn removing_the_frame_places_the_promoted_fix_and_keeps_every_piece_on_its_gps() {
    // Core's test: frame A:0 + fix B:5 (40 m east of the survey) in one
    // piece, plus a separately placed piece C with depths.
    let (mut p, _a, b) = two_entrance_traverse();
    let fix_gps = gps_at(40.0, 500.0);
    ok(p.set_datum_at(StationRef::new(b, 5), fix_gps, None));
    let c_shots = (0..2)
        .map(|i| shot(0.0, 20.0, 3.0 + f64::from(i), 4.0 + f64::from(i)))
        .collect();
    let c = free_line(&mut p, [-1000.0, 0.0], c_shots);
    let c_gps = gps_at(-300.0, 120.0);
    ok(p.set_datum_at(StationRef::new(c, 0), c_gps, None));
    assert_eq!(
        roles(&p),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix),
            Some(DatumRole::Placement)
        ]
    );
    near(xy(&p, at(&p, c, 0)), [-300.0, 120.0], 1e-6, "C placed");
    let (before, depths) = (xy_all(&p), z_all(&p));

    ok(p.remove_datum(0));
    assert_valid(&p);
    assert_eq!(p.frame_datum(), Some(0));
    assert_eq!(
        p.datums[0].station,
        at(&p, b, 5),
        "the promoted fix is the frame"
    );
    near(xy(&p, at(&p, b, 5)), [0.0, 0.0], 1e-6, "F on the origin");
    let (ex, ey) = gps_to_local(&c_gps, &fix_gps);
    near(xy(&p, at(&p, c, 0)), [ex, ey], 0.01, "C on its GPS");
    let c_stations = on_line(&p, c);
    assert_shifted(&before, &xy_all(&p), 0.01, |s| {
        if c_stations.contains(&s) {
            [-40.0, -500.0]
        } else {
            [0.0, -500.0]
        }
    });
    for (s, z) in z_all(&p) {
        assert!((z - depths[&s]).abs() < 1e-12, "station {s}: depth changed");
    }
}

/// Frame A:0, a fix on B:5 40 m east of the survey, and a separate piece C
/// placed by its own GPS (core's `frame_fix_and_separate_piece`).
fn frame_fix_and_separate_piece() -> (Project, LineId, LineId, LineId) {
    let (mut p, a, b) = two_entrance_traverse();
    ok(p.set_datum_at(StationRef::new(b, 5), gps_at(40.0, 500.0), None));
    let c = free_line(&mut p, [-777.0, 0.0], run(0.0, 2, 20.0));
    ok(p.set_datum_at(StationRef::new(c, 0), gps_at(200.0, 0.0), None));
    assert_placements_on_their_gps(&p, "setup");
    (p, a, b, c)
}

fn assert_placements_on_their_gps(p: &Project, what: &str) {
    for datum in p.placement_datums().into_iter().flatten() {
        let d = p.datums[datum];
        let gap = {
            let [gx, gy] = gps_local(p, &d.gps);
            let [x, y] = xy(p, d.station);
            (gx - x).hypot(gy - y)
        };
        assert!(
            gap < 0.01,
            "{what}: placement datum {datum} is {gap} m off its GPS"
        );
    }
}

#[test]
fn no_placement_datum_is_left_off_its_gps_by_a_promotion() {
    let (mut p, _a, _b, _c) = frame_fix_and_separate_piece();
    ok(p.remove_datum(0));
    assert_valid(&p);
    assert_placements_on_their_gps(&p, "remove");

    // Unmerge: the fix's piece becomes a piece of its own.
    let (mut p, a, b, _c) = frame_fix_and_separate_piece();
    ok(p.unmerge_station_pair(StationRef::new(a, 5), StationRef::new(b, 0)));
    assert_valid(&p);
    assert_eq!(p.placement_datums().into_iter().flatten().count(), 3);
    assert_placements_on_their_gps(&p, "unmerge");
}

// ----- the frame's own GPS and corrections ----------------------------------

/// Two unconnected placed pieces (#769): A (frame, datum A:0) runs 200 m
/// east, C is placed by its own GPS 300 m east and 100 m north of A:0, and
/// C:3 holds a GPS fix.
fn frame_and_placed_piece() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 4, 50.0));
    let c = free_line(&mut p, [1000.0, 0.0], run(0.0, 3, 40.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(c, 0), gps_at(300.0, 100.0), None));
    ok(p.set_datum_at(StationRef::new(c, 3), gps_at(300.0, 225.0), None));
    assert_eq!(
        roles(&p),
        vec![
            Some(DatumRole::Placement),
            Some(DatumRole::Placement),
            Some(DatumRole::GpsFix)
        ]
    );
    (p, a, c)
}

fn gps_bits(p: &Project) -> Vec<(StationId, u64, u64)> {
    p.datums
        .iter()
        .map(|d| {
            (
                d.station,
                d.gps.latitude.to_bits(),
                d.gps.longitude.to_bits(),
            )
        })
        .collect()
}

#[test]
fn a_new_frame_gps_re_places_every_other_piece_on_its_own_gps() {
    let (mut p, a, c) = frame_and_placed_piece();
    let a_before: Vec<_> = (0..=4).map(|i| xy(&p, at(&p, a, i))).collect();
    let c_before: Vec<_> = (0..=3).map(|i| xy(&p, at(&p, c, i))).collect();
    let c_gps = p.datums[1].gps;
    let rows_before = gps_bits(&p);

    let new_frame = gps_at(0.0, 100.0);
    let out = ok(p.set_datum_at(StationRef::new(a, 0), new_frame, None));
    assert_valid(&p);
    assert_eq!(out.value.change, DatumChange::GpsEdited);
    assert!(!out.anchors.frame_changed, "same frame datum, new GPS");

    let a_after: Vec<_> = (0..=4).map(|i| xy(&p, at(&p, a, i))).collect();
    assert_eq!(a_before, a_after, "the frame's piece stays on the origin");
    let (ex, ey) = gps_to_local(&c_gps, &new_frame);
    assert!((ex - 300.0).abs() < 0.01 && ey.abs() < 0.01, "({ex}, {ey})");
    near(xy(&p, at(&p, c, 0)), [ex, ey], 1e-6, "C:0 on its GPS");
    let c0 = xy(&p, at(&p, c, 0));
    let d = [c0[0] - c_before[0][0], c0[1] - c_before[0][1]];
    for (i, b) in (0..).zip(&c_before) {
        near(
            xy(&p, at(&p, c, i)),
            [b[0] + d[0], b[1] + d[1]],
            1e-9,
            &format!("C:{i} rigid"),
        );
    }
    assert_eq!(
        gps_bits(&p)[1..],
        rows_before[1..],
        "no other datum changed"
    );
    assert_eq!(p.datums[0].gps.latitude, new_frame.latitude);
}

#[test]
fn an_accuracy_or_entrance_edit_of_the_frame_moves_nothing() {
    let (mut p, a, _c) = frame_and_placed_piece();
    // C left 5 m off its GPS, as an older file can.
    p.datums[1].correction = Some([5.0, 0.0]);
    let before = xy_all(&p);
    let out = ok(p.set_datum_at(StationRef::new(a, 0), origin(), Some(3.0)));
    assert_eq!(out.value.change, DatumChange::AccuracyOnly);
    p.set_datum_entrance(0, Some(true)).unwrap();
    assert_still(&before, &xy_all(&p), 0.0);
    assert_eq!(p.datums[0].accuracy_m, Some(3.0));
    assert_eq!(p.datums[1].correction, Some([5.0, 0.0]));
}

#[test]
fn a_new_frame_gps_moves_a_piece_off_its_gps_by_the_correction_and_keeps_its_gap() {
    let (mut p, a, c) = frame_and_placed_piece();
    p.datums[1].correction = Some([5.0, 0.0]);
    let c_gps = p.datums[1].gps;
    let c0 = at(&p, c, 0);
    let b = xy(&p, c0);
    let new_frame = gps_at(0.0, 2.0);
    ok(p.set_datum_at(StationRef::new(a, 0), new_frame, None));
    let d = frame_correction(&c_gps, &origin(), &new_frame);
    near(
        xy(&p, c0),
        [b[0] + d[0], b[1] + d[1]],
        1e-9,
        "moved by the correction",
    );
    let (gx, gy) = gps_to_local(&c_gps, &new_frame);
    let now = xy(&p, c0);
    let gap = (now[0] - gx).hypot(now[1] - gy);
    assert!((gap - 5.0).abs() < 1e-6, "C:0 is {gap} m off its GPS");
}

#[test]
fn a_new_frame_gps_moves_a_merged_piece_as_one() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let c = free_line(&mut p, [400.0, 0.0], run(0.0, 3, 40.0));
    let c3 = at(&p, c, 3);
    branch(&mut p, c3, run(0.0, 2, 30.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    let c_gps = gps_at(-200.0, 50.0);
    ok(p.set_datum_at(StationRef::new(c, 0), c_gps, None));
    let before = xy_all(&p);
    let new_frame = gps_at(30.0, -40.0);
    ok(p.set_datum_at(StationRef::new(a, 0), new_frame, None));
    assert_valid(&p);
    let shift = frame_correction(&c_gps, &origin(), &new_frame);
    let a_stations = on_line(&p, a);
    assert_shifted(&before, &xy_all(&p), 1e-9, |s| {
        if a_stations.contains(&s) {
            [0.0, 0.0]
        } else {
            shift
        }
    });
}

#[test]
fn an_accuracy_only_edit_never_moves_a_piece_and_a_gps_edit_re_places_it() {
    let (mut p, a, b, c) = frame_fix_and_separate_piece();
    for slot in [
        StationRef::new(a, 0),
        StationRef::new(b, 5),
        StationRef::new(c, 0),
    ] {
        let before = xy_all(&p);
        let station = p.station_of_slot(slot).unwrap();
        let index = p.datum_index_for(station, Some(slot.line_id)).unwrap();
        let gps = p.datums[index].gps;
        let out = ok(p.set_datum_at(slot, gps, Some(7.5)));
        assert_eq!(out.value.change, DatumChange::AccuracyOnly);
        assert_still(&before, &xy_all(&p), 0.0);
        assert_eq!(p.datums[index].accuracy_m, Some(7.5), "{slot:?}");
    }

    // A placement datum an older file left 30 m off its GPS: an accuracy-only
    // edit still moves nothing.
    p.datums[2].correction = Some([30.0, 0.0]);
    let c0 = at(&p, c, 0);
    assert!((xy(&p, c0)[0] - 230.0).abs() < 1e-6);
    let before = xy_all(&p);
    ok(p.set_datum_at(StationRef::new(c, 0), gps_at(200.0, 0.0), Some(2.0)));
    assert_still(&before, &xy_all(&p), 0.0);
    assert_eq!(p.datums[2].correction, Some([30.0, 0.0]));

    // A GPS edit does re-place it, and clears the correction.
    let out = ok(p.set_datum_at(StationRef::new(c, 0), gps_at(210.0, 0.0), Some(2.0)));
    assert_eq!(out.anchors.placed_on_gps, vec![2]);
    assert_eq!(p.datums[2].correction, None);
    near(
        xy(&p, c0),
        [210.0, 0.0],
        0.01,
        "a GPS change places the piece",
    );
}

// ----- which datum a slot has ----------------------------------------------

#[test]
fn a_slot_has_the_datum_attached_via_its_line_else_the_first_on_its_station() {
    let (mut p, a, b) = two_entrance_traverse();
    let junction = at(&p, a, 5);
    ok(p.set_datum_at(StationRef::new(a, 5), gps_at(3.0, 250.0), None));
    // A second datum on the junction (as merging two placed pieces leaves).
    p.datums.push(ZeroDatum {
        via: Some(b),
        ..ZeroDatum::new(junction, gps_at(4.0, 250.0))
    });
    assert_valid(&p);
    assert_eq!(p.datum_index_for(junction, Some(b)), Some(2));
    assert_eq!(p.datum_index_for(junction, Some(a)), Some(1));
    assert_eq!(p.datum_index_for(junction, None), Some(1));

    // Set on B's slot edits B's datum; set on a slot of a line with none on
    // the station edits the first one (plan 5.7: the dialog edits the first).
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(5.0, 250.0), None));
    assert_eq!(p.datums.len(), 3);
    assert_eq!(p.datums[2].gps.longitude, gps_at(5.0, 250.0).longitude);
    let c = branch(&mut p, junction, run(90.0, 1, 5.0));
    ok(p.set_datum_at(StationRef::new(c, 0), gps_at(6.0, 250.0), None));
    assert_eq!(p.datums.len(), 3);
    assert_eq!(p.datums[1].gps.longitude, gps_at(6.0, 250.0).longitude);
}

// ----- entrances (plan 3.8) ------------------------------------------------

/// One line of six 50 m shots due north (stations 0..=6), placed by a datum
/// on station 0.
fn line() -> (Project, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 6, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    (p, a)
}

/// A fix on station `i` of `line` on its surveyed position; returns its index.
fn fix(p: &mut Project, line: LineId, i: u32, entrance: Option<bool>) -> usize {
    let index = ok(p.set_datum_at(
        StationRef::new(line, i),
        gps_at(0.0, 50.0 * f64::from(i)),
        None,
    ))
    .value
    .index;
    if entrance.is_some() {
        p.set_datum_entrance(index, entrance).unwrap();
    }
    index
}

fn entrances(p: &Project, line: LineId) -> Vec<(usize, bool)> {
    let chain = on_line(p, line);
    (0..p.datums.len())
        .map(|i| {
            let index = chain
                .iter()
                .position(|&s| s == p.datums[i].station)
                .unwrap();
            (index, p.datum_entrance(i).unwrap())
        })
        .collect()
}

#[test]
fn a_mid_passage_fix_is_not_an_entrance_and_a_fix_at_a_line_end_is() {
    let (mut p, a) = line();
    fix(&mut p, a, 3, None);
    fix(&mut p, a, 6, None);
    assert_eq!(entrances(&p, a), vec![(0, true), (3, false), (6, true)]);
}

#[test]
fn the_user_answer_overrides_the_default_both_ways() {
    let (mut p, a) = line();
    let mid = fix(&mut p, a, 3, Some(true));
    let end = fix(&mut p, a, 6, Some(false));
    assert_eq!(entrances(&p, a), vec![(0, true), (3, true), (6, false)]);
    p.set_datum_entrance(mid, None).unwrap();
    p.set_datum_entrance(end, None).unwrap();
    assert_eq!(entrances(&p, a), vec![(0, true), (3, false), (6, true)]);
    assert_eq!(
        p.set_datum_entrance(9, Some(true)),
        Err(OpError::UnknownDatum(9))
    );
}

#[test]
fn a_placement_datum_is_an_entrance_whatever_its_stored_answer() {
    let (mut p, a) = line();
    p.set_datum_entrance(0, Some(false)).unwrap();
    assert_eq!(entrances(&p, a), vec![(0, true)]);
}

#[test]
fn re_setting_a_fix_gps_keeps_its_answer() {
    let (mut p, a) = line();
    fix(&mut p, a, 3, Some(true));
    ok(p.set_datum_at(StationRef::new(a, 3), gps_at(0.0, 160.0), None));
    assert_eq!(entrances(&p, a), vec![(0, true), (3, true)]);
}

#[test]
fn the_preview_reports_the_entrance_a_new_or_stored_datum_would_have() {
    let (mut p, a) = line();
    let preview = |p: &Project, i: u32| {
        let station = at(p, a, i as usize);
        p.preview_zero_datum(station, Some(a), &gps_at(0.0, 50.0 * f64::from(i)))
            .unwrap()
            .is_entrance
    };
    assert!(preview(&p, 0), "the placement datum");
    assert!(!preview(&p, 3), "a new mid-passage fix");
    assert!(preview(&p, 6), "a new fix at the line's end");
    fix(&mut p, a, 3, Some(true));
    assert!(preview(&p, 3), "a stored fix keeps its answer");
}

#[test]
fn a_fix_on_a_junction_follows_its_via_line_for_the_default() {
    // Mid-way along A, and the start of branch B: an entrance by B's ends,
    // not by A's (today's per-member rule, plan 3.8 / R1 F9).
    let (mut p, a) = line();
    let junction = at(&p, a, 3);
    let b = branch(&mut p, junction, run(90.0, 2, 10.0));
    let via_a = fix(&mut p, a, 3, None);
    assert_eq!(p.datum_entrance(via_a), Some(false));
    p.datums[via_a].via = Some(b);
    assert_eq!(p.datum_entrance(via_a), Some(true));
    p.datums[via_a].via = None;
    assert_eq!(
        p.datum_entrance(via_a),
        Some(false),
        "no via: the first line, A"
    );
}

#[test]
fn the_entrance_answer_round_trips_through_serde_and_is_absent_by_default() {
    let (mut p, a) = line();
    fix(&mut p, a, 3, Some(true));
    let json = serde_json::to_value(&p.datums).unwrap();
    assert!(json[0].get("entrance").is_none(), "None is not written");
    assert_eq!(json[1]["entrance"], serde_json::Value::Bool(true));
    let back: Vec<ZeroDatum> = serde_json::from_value(json).unwrap();
    assert_eq!(back[1].entrance, Some(true));
    assert_eq!(back[0].entrance, None);
}
