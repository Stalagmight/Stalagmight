//! A snapped loop closure in the Closures Computed View (#736): a `Move`
//! closure the Raw Data View holds rigid (`is_aligned`, see
//! `Project::closure_alignment_stamp`) is one point in the least-squares
//! solve too, as a merge is, so the view (and Plan and Export) never pulls a
//! snapped pair apart.

use std::collections::{HashMap, HashSet};

use super::combine_closure_tests::{add, dist, shot, xy};
use crate::config::ClosureAdjustmentConfig;
use crate::{
    local_to_gps, ClosureStrategy, GpsCoordinates, LoopClosure, Project, SectionId, ShotData,
    Station, StationRef, ZeroDatum,
};

/// The parts of `project` the Closures Computed View solves without failing:
/// every section of each.
fn solved_sections(project: &Project, failed: &HashSet<SectionId>) -> HashSet<SectionId> {
    let mut solved = HashSet::new();
    for anchor in project.adjustment_anchors_for_tests() {
        let component = project.get_connected_section_ids(anchor.section_id);
        if !component.iter().any(|id| failed.contains(id)) {
            solved.extend(component);
        }
    }
    solved
}

/// Every snap the solve ties (an aligned closure it does not take as an
/// eligible one, and does not skip as across one shot) in a part the view
/// solves without failing is shown on ONE point in the Closures Computed
/// View, to 1e-6 m: the tolerance the solve's own section realignment holds
/// the stations of one group to ("inconsistent section translation"), far
/// above the float dust the shot update leaves. Returns how many snapped
/// pairs it checked.
pub(super) fn assert_snaps_held(project: &Project, what: &str) -> usize {
    let adjusted = project.compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    let view: HashMap<StationRef, (f64, f64)> = adjusted
        .stations
        .iter()
        .map(|s| (s.station_ref, (s.coordinates.x, s.coordinates.y)))
        .collect();
    let failed: HashSet<SectionId> = adjusted
        .failures
        .iter()
        .flat_map(|f| f.section_ids.iter().copied())
        .collect();
    let solved = solved_sections(project, &failed);
    let skipped: HashSet<u64> = project
        .closures_across_one_shot()
        .into_iter()
        .map(|c| c.closure_id)
        .collect();
    let mut held = 0;
    for closure in &project.loop_closures {
        if !Project::closure_is_snap(closure.strategy, closure.is_aligned)
            || skipped.contains(&closure.id)
        {
            continue;
        }
        let (Some(a), Some(b)) = (
            project.resolve_key(&closure.station_a),
            project.resolve_key(&closure.station_b),
        ) else {
            continue;
        };
        if !(solved.contains(&a.section_id) && solved.contains(&b.section_id)) {
            continue;
        }
        let apart = dist(view[&a], view[&b]);
        assert!(
            apart < 1e-6,
            "{what}: the snapped pair {a:?} {b:?} (closure {}) is {apart:.6} m apart in the \
             Closures Computed View",
            closure.id
        );
        held += 1;
    }
    held
}

/// The horizontal distance between `a` and `b` in `stations`.
fn gap_in(stations: &[Station], a: StationRef, b: StationRef) -> f64 {
    let at = |r: StationRef| {
        let s = stations.iter().find(|s| s.station_ref == r).unwrap();
        (s.coordinates.x, s.coordinates.y)
    };
    let (p, q) = (at(a), at(b));
    (p.0 - q.0).hypot(p.1 - q.1)
}

/// The issue's survey: a square A, B, C, D of one 10 m shot each, merged end
/// to start, D's end snapped onto A's start (a `Move` closure, aligned), and
/// a diagonal E from A's end that misses C's end by 0.5 m, closed there by a
/// `Batch` closure.
#[test]
fn adjusted_view_keeps_a_snapped_pair_together() {
    let mut p = Project::new("F1".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 10.0)]);
    let b = add(&mut p, "B", vec![shot(0.0, 10.0)]);
    let c = add(&mut p, "C", vec![shot(270.0, 10.0)]);
    let d = add(&mut p, "D", vec![shot(180.0, 10.0)]);
    let e = add(&mut p, "E", vec![shot(315.0, 200f64.sqrt() + 0.5)]);
    for (x, y) in [
        ((a, 1), (b, 0)),
        ((b, 1), (c, 0)),
        ((c, 1), (d, 0)),
        ((a, 1), (e, 0)),
    ] {
        p.merge_stations(StationRef::new(x.0, x.1), StationRef::new(y.0, y.1))
            .unwrap();
    }
    let (d1, a0) = (StationRef::new(d, 1), StationRef::new(a, 0));
    let snapped = p
        .create_loop_closure(d1, a0, ClosureStrategy::Move)
        .unwrap();
    assert!(snapped.is_aligned);
    p.create_loop_closure(
        StationRef::new(e, 1),
        StationRef::new(c, 1),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let measured = gap_in(&p.compute_station_coordinates(), d1, a0);
    let adjusted = gap_in(
        &p.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default()),
        d1,
        a0,
    );
    assert!(measured < 1e-6);
    assert!(
        adjusted < 0.01,
        "the closure-adjusted view pulled the snapped pair {adjusted:.3} m apart"
    );
}

fn deep(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        depth_in,
        depth_out,
        ..shot(azimuth, length)
    }
}

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(20.5, -87.3, 0.0).unwrap()
}

/// Plant a closure as a file can carry it, past `create_loop_closure`'s
/// measurement and refusals.
fn plant(
    project: &mut Project,
    a: StationRef,
    b: StationRef,
    strategy: ClosureStrategy,
    is_aligned: bool,
) -> u64 {
    let (station_a, station_b) = (
        project.key_for_ref(a).unwrap(),
        project.key_for_ref(b).unwrap(),
    );
    project.normalize_next_closure_id();
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a,
        station_b,
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned,
    });
    id
}

/// A survey with no snap (no aligned closure the solve does not already
/// take), but every neighbour of one: a placement datum at A's start (the
/// frame) and a soft GPS fix on a spur's end, a `Batch` loop and a
/// `LeastSquaresWeighted` one, a spur merged off the second, a `Move`
/// closure a later edit tore 0.3 m apart (so not aligned), a legacy aligned
/// `Batch` closure (the destructive adjust before 0.5.0 wrote those), a
/// closure across one shot (skipped), a hand move on a loop station, and a
/// second, datum-less part with a loop of its own. Depths vary.
fn snap_free_survey() -> (Project, [SectionId; 6]) {
    let mut p = Project::new("Snap-free".to_string(), None);
    let a = add(
        &mut p,
        "A",
        vec![
            deep(10.0, 8.0, 2.0, 3.1),
            deep(80.0, 7.5, 3.1, 4.4),
            deep(170.0, 9.0, 4.4, 5.0),
            deep(250.0, 6.2, 5.0, 3.6),
            deep(330.0, 4.1, 3.6, 2.2),
        ],
    );
    p.set_section_offset(a, (12.0, -7.5));
    p.zero_datums.push(ZeroDatum::new(
        p.key_for_ref(StationRef::new(a, 0)).unwrap(),
        origin(),
    ));
    p.create_loop_closure(
        StationRef::new(a, 5),
        StationRef::new(a, 0),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let b = add(
        &mut p,
        "B",
        vec![
            deep(120.0, 6.5, 5.0, 6.2),
            deep(200.0, 5.5, 6.2, 6.0),
            deep(280.0, 7.7, 6.0, 4.0),
        ],
    );
    let at = xy(&p, StationRef::new(a, 2));
    p.set_section_offset(b, at);
    p.merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(b, 3),
        StationRef::new(a, 4),
        ClosureStrategy::LeastSquaresWeighted,
    )
    .unwrap();
    let c = add(
        &mut p,
        "C",
        vec![deep(60.0, 3.3, 6.2, 7.0), deep(20.0, 4.8, 7.0, 7.7)],
    );
    let at = xy(&p, StationRef::new(b, 1));
    p.set_section_offset(c, at);
    p.merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();
    // D from A:3 towards A:0, its end 0.3 m off: the Move closure measures
    // the gap and is not aligned.
    let (from, to) = (xy(&p, StationRef::new(a, 3)), xy(&p, StationRef::new(a, 0)));
    let bearing = (to.0 - from.0)
        .atan2(to.1 - from.1)
        .to_degrees()
        .rem_euclid(360.0);
    let length = (to.0 - from.0).hypot(to.1 - from.1);
    let d = add(&mut p, "D", vec![deep(bearing + 2.0, length, 3.6, 2.0)]);
    p.set_section_offset(d, from);
    p.merge_stations(StationRef::new(a, 3), StationRef::new(d, 0))
        .unwrap();
    let torn = p
        .create_loop_closure(
            StationRef::new(d, 1),
            StationRef::new(a, 0),
            ClosureStrategy::Move,
        )
        .unwrap();
    assert!(!torn.is_aligned, "setup: D's end is 0.3 m off A's start");
    // A hand move on a loop station, settled as opening the file does.
    let key = p.key_for_ref(StationRef::new(b, 2)).unwrap();
    p.station_offsets.insert(key, (0.04, -0.03, 0.0));
    p.redrive_offsets_after_load();
    // A second part, without a datum, with a loop and a torn Move closure.
    let g = add(
        &mut p,
        "G",
        vec![
            deep(0.0, 11.0, 1.0, 1.5),
            deep(95.0, 9.4, 1.5, 2.5),
            deep(185.0, 10.3, 2.5, 1.9),
            deep(272.0, 8.8, 1.9, 1.2),
        ],
    );
    p.set_section_offset(g, (140.0, 90.0));
    p.create_loop_closure(
        StationRef::new(g, 4),
        StationRef::new(g, 0),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let h = add(&mut p, "H", vec![deep(135.0, 9.0, 1.5, 2.0)]);
    let at = xy(&p, StationRef::new(g, 1));
    p.set_section_offset(h, at);
    p.merge_stations(StationRef::new(g, 1), StationRef::new(h, 0))
        .unwrap();
    // Legacy and odd closures, as a file carries them.
    plant(
        &mut p,
        StationRef::new(c, 2),
        StationRef::new(a, 1),
        ClosureStrategy::Batch,
        true,
    );
    plant(
        &mut p,
        StationRef::new(c, 1),
        StationRef::new(c, 2),
        ClosureStrategy::Batch,
        false,
    );
    plant(
        &mut p,
        StationRef::new(h, 1),
        StationRef::new(g, 3),
        ClosureStrategy::Move,
        false,
    );
    // A soft GPS fix on C's end, 1.4 m off where it is shown.
    let shown = xy(&p, StationRef::new(c, 2));
    let (lat, lon) = local_to_gps(shown.0 + 1.2, shown.1 - 0.7, &origin()).unwrap();
    p.zero_datums.push(ZeroDatum {
        station_ref: p.key_for_ref(StationRef::new(c, 2)).unwrap(),
        gps: GpsCoordinates::new(lat, lon, 0.0).unwrap(),
        accuracy_m: Some(1.5),
        entrance: None,
    });
    assert!(
        !p.loop_closures
            .iter()
            .any(|c| c.is_aligned && !Project::closure_is_eligible(c.strategy)),
        "setup: no snap"
    );
    assert_eq!(p.gps_fix_datums().len(), 1, "setup: one GPS fix");
    assert_eq!(p.closures_across_one_shot().len(), 1, "setup: one skipped");
    (p, [a, b, c, d, g, h])
}

/// Every station's closure-adjusted `x, y, z` as raw bits, in station order.
fn view_bits(project: &Project) -> Vec<(u32, u32, [u64; 3])> {
    let mut out: Vec<(u32, u32, [u64; 3])> = project
        .compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default())
        .into_iter()
        .map(|s| {
            (
                s.station_ref.section_id,
                s.station_ref.station_index,
                [
                    s.coordinates.x.to_bits(),
                    s.coordinates.y.to_bits(),
                    s.coordinates.z.to_bits(),
                ],
            )
        })
        .collect();
    out.sort_unstable_by_key(|(s, i, _)| (*s, *i));
    out
}

/// The view of [`snap_free_survey`] as 3a4f62b0c (before #736) computed it
/// on Linux, as raw bits (`x`, `y`, `z` per station).
const SNAP_FREE_VIEW: [(u32, u32, [u64; 3]); 22] = [
    (
        0,
        0,
        [
            0x4028_0000_0000_0000,
            0xc01e_0000_0000_0000,
            0x4000_cccc_cccc_cccd,
        ],
    ),
    (
        0,
        1,
        [
            0x402e_a839_0b96_34fb,
            0x3fcb_f2a6_6e52_6f20,
            0x4012_8888_8888_8889,
        ],
    ),
    (
        0,
        2,
        [
            0x4033_bf9c_e153_44c7,
            0xbfdc_f24b_e17a_449a,
            0x4012_6666_6666_6667,
        ],
    ),
    (
        0,
        3,
        [
            0x4033_73c7_743b_b692,
            0xc022_4ec0_8b89_ca56,
            0x4012_2222_2222_2222,
        ],
    ),
    (
        0,
        4,
        [
            0x402b_0010_0eb0_d02a,
            0xc025_7c6a_c8d4_eaba,
            0x400d_dddd_dddd_dddd,
        ],
    ),
    (
        0,
        5,
        [
            0x4028_0000_0000_0000,
            0xc01e_0000_0000_0000,
            0x4000_cccc_cccc_cccd,
        ],
    ),
    (
        1,
        0,
        [
            0x4033_bf9c_e153_44c7,
            0xbfdc_f24b_e17a_4498,
            0x4012_6666_6666_6667,
        ],
    ),
    (
        1,
        1,
        [
            0x4034_bb8a_5403_96f8,
            0xc015_3d17_b8dd_4104,
            0x4018_cccc_cccc_cccd,
        ],
    ),
    (
        1,
        2,
        [
            0x4034_5983_b00d_c9dd,
            0xc025_7fff_bd3b_4331,
            0x4018_0000_0000_0000,
        ],
    ),
    (
        1,
        3,
        [
            0x402b_0010_0eb0_d02a,
            0xc025_7c6a_c8d4_eaba,
            0x400d_dddd_dddd_dddd,
        ],
    ),
    (
        2,
        0,
        [
            0x4034_bb8a_5403_96f8,
            0xc015_3d17_b8dd_4103,
            0x4018_cccc_cccc_cccd,
        ],
    ),
    (
        2,
        1,
        [
            0x4034_9f59_6440_429b,
            0xc00f_d73d_562a_2716,
            0x401c_0000_0000_0000,
        ],
    ),
    (
        2,
        2,
        [
            0x402e_a839_0b96_34fa,
            0x3fcb_f2a6_6e52_6ee0,
            0x4012_8888_8888_8889,
        ],
    ),
    (
        3,
        0,
        [
            0x4033_73c7_743b_b692,
            0xc022_4ec0_8b89_ca55,
            0x4012_2222_2222_2222,
        ],
    ),
    (
        3,
        1,
        [
            0x4022_bba7_b27c_1009,
            0xc022_153d_a5b0_8d64,
            0x4000_0000_0000_0000,
        ],
    ),
    (
        4,
        0,
        [
            0x4061_8000_0000_0000,
            0x4056_8000_0000_0000,
            0x3ff1_9999_9999_999a,
        ],
    ),
    (
        4,
        1,
        [
            0x4061_8598_9d8f_58f1,
            0x4059_3e97_0ad2_34d3,
            0x3ff8_0000_0000_0000,
        ],
    ),
    (
        4,
        2,
        [
            0x4062_aff5_7201_01c9,
            0x4059_0328_ae53_0be5,
            0x4004_0000_0000_0000,
        ],
    ),
    (
        4,
        3,
        [
            0x4062_9827_38c0_1f3e,
            0x4056_72d6_9d92_1dd1,
            0x3ffe_6666_6666_6666,
        ],
    ),
    (
        4,
        4,
        [
            0x4061_8000_0000_0000,
            0x4056_8000_0000_0000,
            0x3ff1_9999_9999_999a,
        ],
    ),
    (
        5,
        0,
        [
            0x4061_8598_9d8f_58f1,
            0x4059_3e97_0ad2_34d3,
            0x3ff8_0000_0000_0000,
        ],
    ),
    (
        5,
        1,
        [
            0x4062_50ed_ab44_e055,
            0x4057_a7ec_ef67_260c,
            0x4000_0000_0000_0000,
        ],
    ),
];

/// No snap, no change: a survey whose closures include no aligned one the
/// solve does not already take is adjusted exactly as before #736, the
/// torn `Move` closure, the legacy aligned `Batch` one, the closure across
/// one shot, the hand move and the GPS fix included. On the Linux the values
/// were pinned on it is the same to the last bit; the comparison allows
/// 1e-12 m, a few ulps at these magnitudes, because `sin` and `cos` come from
/// the platform's libm and may round the last bit differently on macOS or
/// Windows. Any real change to the solve moves a station by far more.
#[test]
fn a_survey_without_a_snap_keeps_its_closures_computed_view() {
    let (p, _) = snap_free_survey();
    assert!(p
        .compute_adjusted_stations(&ClosureAdjustmentConfig::default())
        .failures
        .is_empty());
    let now = view_bits(&p);
    assert_eq!(now.len(), SNAP_FREE_VIEW.len());
    for (got, want) in now.iter().zip(SNAP_FREE_VIEW.iter()) {
        assert_eq!((got.0, got.1), (want.0, want.1));
        let (g, w) = (got.2.map(f64::from_bits), want.2.map(f64::from_bits));
        let off = (0..3).map(|k| (g[k] - w[k]).abs()).fold(0.0, f64::max);
        assert!(
            off <= 1e-12,
            "{}:{} is at {g:?} in the Closures Computed View, was {w:?} ({off:.3e} m off)",
            got.0,
            got.1,
        );
    }
}
