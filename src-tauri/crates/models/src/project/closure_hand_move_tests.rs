//! The Closures Computed View and a station moved by hand (#759): a manual
//! station offset (`station_offsets`), as an older or hand-edited file
//! carries it. Move Station folds its moves into the shots; Split Section
//! Here, Append Project and Undo carry an existing one on.
//!
//! The view adjusts the measured shots, never a hand move: every station is
//! shown at its adjusted chain position plus its own hand move, every merge
//! and every closure the solve takes holds its stations on one point as
//! shown (as merges are in the Raw Data View, and as a closure's misclosure
//! is measured), and the station the view holds the survey at stays where
//! the Raw Data View shows it. [`assert_view_is_the_solve`] checks exactly
//! that, without solving.

use std::collections::{HashMap, HashSet};

use super::combine_closure_tests::{
    add, dist, every_station, network, passage_network, shot, side_by_side_network, xy,
};
use crate::config::ClosureAdjustmentConfig;
use crate::{ClosureStrategy, LoopClosure, Project, SectionId, StationRef};

/// Every station's closure-adjusted position (the Plan and Export geometry).
fn view(project: &Project) -> HashMap<StationRef, (f64, f64)> {
    project
        .compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default())
        .into_iter()
        .map(|s| (s.station_ref, (s.coordinates.x, s.coordinates.y)))
        .collect()
}

/// The Closures Computed View and a station moved by hand (a manual station
/// offset, as an older or hand-edited file carries it): A runs 10 m east from
/// the origin and its start, the station the view holds the survey at (the
/// lowest section id's), was moved 0.1 m east and 0.05 m north by hand. B
/// (two shots) and C run north from A's end, merged there, B 10 m and C
/// 9.5 m, their ends tied by a loop closure, which the view solves. A's end
/// is on no loop and nothing was moved there: the view must show it where it
/// is measured, (10, 0).
#[test]
fn a_hand_move_on_the_held_station_does_not_shift_the_closures_computed_view() {
    let mut p = Project::new("View".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 10.0)]);
    let b = add(&mut p, "B", vec![shot(0.0, 5.0), shot(0.0, 5.0)]);
    let c = add(&mut p, "C", vec![shot(0.0, 9.5)]);
    let junction = StationRef::new(a, 1);
    for id in [b, c] {
        p.set_section_offset(id, (10.0, 0.0));
        p.merge_stations(junction, StationRef::new(id, 0)).unwrap();
    }
    p.create_loop_closure(
        StationRef::new(b, 2),
        StationRef::new(c, 1),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let key = p.key_for_ref(StationRef::new(a, 0)).unwrap();
    p.station_offsets.insert(key, (0.1, 0.05, 0.0));
    let view = view(&p);
    let (shown, measured) = (view[&junction], xy(&p, junction));
    assert!(
        dist(shown, measured) < 1e-9,
        "A:1 measured at {measured:?} is at {shown:?} in the Closures Computed View, {:.6} m off",
        dist(shown, measured)
    );
}

/// The same hand move where the solved part is a tree (C tied to B's end by
/// a loop closure only; the app keeps such a closure only between two
/// datum-anchored groups, and the combine sweep's oracle builds one): the
/// view shifts B, merged at A's end, by the hand move and leaves A, so the
/// merge comes apart (in a debug build the merge-alignment assertion fires).
#[test]
fn a_hand_move_on_the_held_station_keeps_merges_closed_in_the_closures_computed_view() {
    let mut p = Project::new("View".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 10.0)]);
    let b = add(&mut p, "B", vec![shot(0.0, 10.0)]);
    let c = add(&mut p, "C", vec![shot(90.0, 9.5)]);
    p.set_section_offset(b, (10.0, 0.0));
    p.merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    p.set_section_offset(c, (0.0, 10.0));
    p.loop_closures.push(LoopClosure {
        id: 1,
        station_a: p.key_for_ref(StationRef::new(c, 1)).unwrap(),
        station_b: p.key_for_ref(StationRef::new(b, 1)).unwrap(),
        path_length: 0.0,
        misclosure_distance: 0.5,
        error_percentage: 0.0,
        horizontal_misclosure: 0.5,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    p.next_closure_id = 2;
    let key = p.key_for_ref(StationRef::new(a, 0)).unwrap();
    p.station_offsets.insert(key, (0.1, 0.05, 0.0));
    let view = view(&p);
    let (at_a, at_b) = (view[&StationRef::new(a, 1)], view[&StationRef::new(b, 0)]);
    assert!(
        dist(at_a, at_b) < 1e-9,
        "merged A:1 {at_a:?} and B:0 {at_b:?} are {:.6} m apart in the Closures Computed View",
        dist(at_a, at_b)
    );
}

fn deep_shot(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> crate::ShotData {
    crate::ShotData {
        depth_in,
        depth_out,
        ..shot(azimuth, length)
    }
}

/// A survey with no hand move: a datum-less part with two loops (one closure
/// of each strategy the solve takes), a branch merged on a loop and a spur
/// merged off it, and a second part held by a zero datum with a loop of its
/// own. Depths vary, so the view's depth is pinned too.
fn pinned_survey() -> (Project, [SectionId; 4]) {
    let mut p = Project::new("Pinned".to_string(), None);
    let a = add(
        &mut p,
        "A",
        vec![
            deep_shot(12.5, 7.31, 3.0, 4.2),
            deep_shot(87.2, 5.12, 4.2, 5.9),
            deep_shot(171.9, 6.27, 5.9, 4.4),
            deep_shot(263.4, 4.93, 4.4, 3.3),
        ],
    );
    p.set_section_offset(a, (3.7, -2.1));
    let b = add(
        &mut p,
        "B",
        vec![
            deep_shot(44.0, 8.8, 6.0, 7.5),
            deep_shot(131.5, 3.3, 7.5, 8.1),
            deep_shot(200.2, 6.1, 8.1, 4.0),
        ],
    );
    p.merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    let c = add(
        &mut p,
        "C",
        vec![
            deep_shot(300.0, 4.4, 7.4, 9.0),
            deep_shot(310.0, 2.2, 9.0, 9.6),
        ],
    );
    p.merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(a, 4),
        StationRef::new(a, 0),
        ClosureStrategy::Batch,
    )
    .unwrap();
    p.create_loop_closure(
        StationRef::new(b, 3),
        StationRef::new(a, 1),
        ClosureStrategy::LeastSquaresWeighted,
    )
    .unwrap();
    let d = add(
        &mut p,
        "D",
        vec![
            deep_shot(5.0, 10.0, 1.0, 2.0),
            deep_shot(95.0, 10.2, 2.0, 2.5),
            deep_shot(185.0, 9.7, 2.5, 1.8),
            deep_shot(275.0, 10.1, 1.8, 1.1),
        ],
    );
    p.set_section_offset(d, (100.0, 100.0));
    p.set_zero_datum(
        StationRef::new(d, 1),
        crate::GpsCoordinates {
            latitude: 20.0,
            longitude: -87.0,
            depth_reference: 0.0,
        },
    )
    .unwrap();
    p.create_loop_closure(
        StationRef::new(d, 4),
        StationRef::new(d, 0),
        ClosureStrategy::Batch,
    )
    .unwrap();
    (p, [a, b, c, d])
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

/// The view of [`pinned_survey`] as 63050ac1c computed it, bit for bit (`x`,
/// `y`, `z` per station): a survey without a hand move solves exactly as
/// before #759.
const PINNED_VIEW: [(u32, u32, [u64; 3]); 17] = [
    (
        0,
        0,
        [
            0xc05c_519c_e0ae_287e,
            0xc05e_db39_9dfc_8d32,
            0x4009_3333_3333_3333,
        ],
    ),
    (
        0,
        1,
        [
            0xc05c_1ad4_82dd_c13d,
            0xc05d_1fdc_0621_3303,
            0x4010_8888_8888_8889,
        ],
    ),
    (
        0,
        2,
        [
            0xc05b_2d93_30d1_c962,
            0xc05d_1f37_ed2a_68e1,
            0x4017_bbbb_bbbb_bbbc,
        ],
    ),
    (
        0,
        3,
        [
            0xc05b_18af_6e9c_9b22,
            0xc05e_a71a_2eee_64b8,
            0x4011_9999_9999_999a,
        ],
    ),
    (
        0,
        4,
        [
            0xc05c_519c_e0ae_287e,
            0xc05e_db39_9dfc_8d32,
            0x4009_3333_3333_3333,
        ],
    ),
    (
        1,
        0,
        [
            0xc05b_2d93_30d1_c962,
            0xc05d_1f37_ed2a_68e1,
            0x4017_bbbb_bbbb_bbbc,
        ],
    ),
    (
        1,
        1,
        [
            0xc05b_02cb_c3c5_aad0,
            0xc05b_8a08_b0a1_53cf,
            0x401d_dddd_dddd_dddd,
        ],
    ),
    (
        1,
        2,
        [
            0xc05a_b04d_8750_1d0c,
            0xc05c_1240_3748_1428,
            0x4020_3333_3333_3333,
        ],
    ),
    (
        1,
        3,
        [
            0xc05c_1ad4_82dd_c13d,
            0xc05d_1fdc_0621_3303,
            0x4010_8888_8888_8889,
        ],
    ),
    (
        2,
        0,
        [
            0xc05b_02cb_c3c5_aad0,
            0xc05b_8a08_b0a1_53cf,
            0x401d_dddd_dddd_dddd,
        ],
    ),
    (
        2,
        1,
        [
            0xc05b_e5f9_3208_0218,
            0xc05b_06df_7b50_26c5,
            0x4022_0000_0000_0000,
        ],
    ),
    (
        2,
        2,
        [
            0xc05c_4dbe_6275_6c09,
            0xc05a_afcc_a429_72f7,
            0x4023_3333_3333_3333,
        ],
    ),
    (
        3,
        0,
        [
            0xbfe9_b449_f6ba_2117,
            0xc023_cd87_cffd_fa5a,
            0x3ff0_cccc_cccc_cccd,
        ],
    ),
    (
        3,
        1,
        [
            0x3cfb_8000_0000_0000,
            0x3d00_0000_0000_0000,
            0x4000_0000_0000_0000,
        ],
    ),
    (
        3,
        2,
        [
            0x4024_4953_deb8_326c,
            0xbff0_2bf9_667f_41f9,
            0x4004_0000_0000_0000,
        ],
    ),
    (
        3,
        3,
        [
            0x4022_7ace_7f40_f3dd,
            0xc025_517d_ac61_d617,
            0x3ffc_cccc_cccc_cccd,
        ],
    ),
    (
        3,
        4,
        [
            0xbfe9_b449_f6ba_2110,
            0xc023_cd87_cffd_fa5e,
            0x3ff0_cccc_cccc_cccd,
        ],
    ),
];

/// No hand move, no change: the fix for #759 only acts where a station
/// carries a manual offset, so a survey without one keeps the Closures
/// Computed View it had, to the last bit.
#[test]
fn a_survey_without_a_hand_move_keeps_its_closures_computed_view_bit_for_bit() {
    let (p, _) = pinned_survey();
    assert!(p.station_offsets.is_empty());
    let now = view_bits(&p);
    assert_eq!(now.len(), PINNED_VIEW.len());
    for (got, want) in now.iter().zip(PINNED_VIEW.iter()) {
        assert_eq!(
            got,
            want,
            "{}:{} is at ({}, {}, {}) in the Closures Computed View, was ({}, {}, {})",
            got.0,
            got.1,
            f64::from_bits(got.2[0]),
            f64::from_bits(got.2[1]),
            f64::from_bits(got.2[2]),
            f64::from_bits(want.2[0]),
            f64::from_bits(want.2[1]),
            f64::from_bits(want.2[2]),
        );
    }
}

/// Union-find root of `x` in `parent`.
fn root(parent: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
    let up = *parent.get(&x).unwrap_or(&x);
    if up == x {
        return x;
    }
    let r = root(parent, up);
    parent.insert(x, r);
    r
}

/// The Closures Computed View of `project` is the closure solve with every
/// station's hand move kept on it, checked without solving:
///
/// - each tie group (stations merged, or tied by a closure the solve takes)
///   is shown on one point;
/// - the stations the view holds a solved survey at (its zero datums, else
///   its gauge station) are where the Raw Data View shows them;
/// - the adjusted chain positions (shown, less each station's own hand move)
///   fit the measured shots by weighted least squares: for every group the
///   solve may move, the weighted residuals of the shots pulling on it
///   cancel, axis by axis, with the solve's own weights;
/// - a survey the view does not solve, or fails to, is shown as measured.
fn assert_view_is_the_solve(project: &Project, what: &str) {
    let config = ClosureAdjustmentConfig::default();
    let adjusted = project.compute_adjusted_stations(&config);
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
    let hand = |at: StationRef| {
        project
            .station_offset_for_ref(at)
            .map_or((0.0, 0.0), |h| (h.0, h.1))
    };
    let skipped: HashSet<u64> = project
        .closures_across_one_shot()
        .into_iter()
        .map(|c| c.closure_id)
        .collect();
    let mut solved: HashSet<SectionId> = HashSet::new();
    for anchor in project.adjustment_anchors_for_tests() {
        let component = project.get_connected_section_ids(anchor.section_id);
        if component.iter().any(|id| failed.contains(id)) {
            continue;
        }
        solved.extend(component.iter().copied());
        let inside = |at: &StationRef| component.contains(&at.section_id);
        let mut parent: HashMap<StationRef, StationRef> = HashMap::new();
        let ties = project
            .station_merges
            .iter()
            .map(|m| (&m.station_a, &m.station_b))
            .chain(
                project
                    .loop_closures
                    .iter()
                    .filter(|c| Project::closure_is_eligible(c.strategy))
                    .filter(|c| !skipped.contains(&c.id))
                    .map(|c| (&c.station_a, &c.station_b)),
            );
        for (a, b) in ties {
            if let (Some(a), Some(b)) = (project.resolve_key(a), project.resolve_key(b)) {
                if inside(&a) && inside(&b) {
                    let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                    parent.insert(ra, rb);
                }
            }
        }
        let datums: Vec<StationRef> = project
            .zero_datums
            .iter()
            .filter_map(|z| project.resolve_key(&z.station_ref))
            .filter(|at| inside(at))
            .collect();
        let held = if datums.is_empty() {
            vec![anchor]
        } else {
            datums
        };
        let mut fixed: HashSet<StationRef> = HashSet::new();
        for at in &held {
            let (shown, measured) = (view[at], xy(project, *at));
            assert!(
                dist(shown, measured) < 1e-6,
                "{what}: the view holds the survey at {at:?}, shown at {measured:?}, but shows \
                 it at {shown:?}"
            );
            fixed.insert(root(&mut parent, *at));
        }
        let mut point: HashMap<StationRef, (StationRef, (f64, f64))> = HashMap::new();
        let mut pull: HashMap<StationRef, ([f64; 2], [f64; 2])> = HashMap::new();
        for section in project
            .sections
            .iter()
            .filter(|s| component.contains(&s.id))
        {
            for i in 0..=section.shots.len() as u32 {
                let at = StationRef::new(section.id, i);
                let r = root(&mut parent, at);
                let (first, p) = *point.entry(r).or_insert((at, view[&at]));
                assert!(
                    dist(p, view[&at]) < 1e-6,
                    "{what}: {at:?} and {first:?} are tied but {:.6} m apart in the view",
                    dist(p, view[&at])
                );
            }
            let quality = if section.quality.is_finite() && section.quality > 0.0 {
                section.quality
            } else {
                1.0
            };
            let sigma_len = config.base_sigma_length_m * quality;
            let sigma_az = config.base_sigma_azimuth_deg.to_radians() * quality;
            let reference = project.azimuth_reference_for(section);
            for (k, s) in section.shots.iter().enumerate() {
                let (i, j) = (
                    StationRef::new(section.id, k as u32),
                    StationRef::new(section.id, k as u32 + 1),
                );
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                if ri == rj {
                    continue;
                }
                let v = Project::shot_vector_along(s, true, reference);
                let az = s.average_azimuth_true_rad(reference);
                let sx = ((az.sin() * sigma_len).powi(2)
                    + (s.length * az.cos() * sigma_az).powi(2))
                .sqrt();
                let sy = ((az.cos() * sigma_len).powi(2)
                    + (s.length * az.sin() * sigma_az).powi(2))
                .sqrt();
                let w = [
                    if sx > 0.0 { 1.0 / (sx * sx) } else { 0.0 },
                    if sy > 0.0 { 1.0 / (sy * sy) } else { 0.0 },
                ];
                let (pi, pj, hi, hj) = (view[&i], view[&j], hand(i), hand(j));
                let residual = [
                    (pj.0 - hj.0) - (pi.0 - hi.0) - v.x,
                    (pj.1 - hj.1) - (pi.1 - hi.1) - v.y,
                ];
                for axis in 0..2 {
                    let e = pull.entry(rj).or_default();
                    e.0[axis] += w[axis] * residual[axis];
                    e.1[axis] += w[axis];
                    let e = pull.entry(ri).or_default();
                    e.0[axis] -= w[axis] * residual[axis];
                    e.1[axis] += w[axis];
                }
            }
        }
        for (group, (sum, weight)) in &pull {
            if fixed.contains(group) {
                continue;
            }
            for axis in 0..2 {
                // The weighted mean of the residuals pulling on the group.
                let imbalance = if weight[axis] > 0.0 {
                    (sum[axis] / weight[axis]).abs()
                } else {
                    0.0
                };
                assert!(
                    imbalance < 1e-7,
                    "{what}: the shots pulling on {group:?}'s point are not balanced in the \
                     view (axis {axis}: {imbalance:.3e} m): it is not the least-squares fit \
                     of the shots with each station's hand move kept on it"
                );
            }
        }
    }
    for at in every_station(project) {
        if solved.contains(&at.section_id) {
            continue;
        }
        assert!(
            dist(view[&at], xy(project, at)) < 1e-9,
            "{what}: {at:?} is in a survey the view does not adjust, but not shown as measured"
        );
    }
}

/// Plant `hand` on `at`, as an older or hand-edited file carries it, and
/// settle the offsets as opening the file does: what is merged at `at`
/// follows it as shown.
fn plant(project: &mut Project, at: StationRef, hand: (f64, f64, f64)) {
    let key = project.key_for_ref(at).unwrap();
    project.station_offsets.insert(key, hand);
    project.redrive_offsets_after_load();
}

/// `with` and `without` differ only by hand moves on `moved` (each by its
/// own): the Closures Computed View shows every station of `with` where
/// `without` shows it, plus `moved`'s hand move on that station.
fn assert_only_nudged(
    with: &Project,
    without: &Project,
    moved: &HashMap<StationRef, (f64, f64)>,
    what: &str,
) {
    let (a, b) = (view(with), view(without));
    for (at, p) in &b {
        let h = moved.get(at).copied().unwrap_or((0.0, 0.0));
        let want = (p.0 + h.0, p.1 + h.1);
        assert!(
            dist(a[at], want) < 1e-9,
            "{what}: {at:?} is at {:?} in the Closures Computed View, {:.6} m off {want:?}",
            a[at],
            dist(a[at], want)
        );
    }
}

/// A hand move on a station of a loop that nothing is merged or tied to:
/// the view shows it on that station alone, on its adjusted position, as
/// the Raw Data View shows it on its measured one. (It used to drop it: the
/// shots beside the station absorbed it.)
#[test]
fn a_hand_move_on_a_loop_station_shows_on_that_station_alone() {
    let (without, [a, ..]) = pinned_survey();
    let at = StationRef::new(a, 3);
    let mut with = without.clone();
    plant(&mut with, at, (0.12, -0.07, 0.0));
    assert_only_nudged(
        &with,
        &without,
        &HashMap::from([(at, (0.12, -0.07))]),
        "loop station",
    );
    assert_view_is_the_solve(&with, "loop station");
}

/// A hand move on a spur station (no loop through it, nothing merged
/// there): the same nudge of that station alone.
#[test]
fn a_hand_move_off_a_loop_shows_on_that_station_alone() {
    let (without, [_, _, c, _]) = pinned_survey();
    let at = StationRef::new(c, 1);
    let mut with = without.clone();
    plant(&mut with, at, (-0.3, 0.2, 0.0));
    assert_only_nudged(
        &with,
        &without,
        &HashMap::from([(at, (-0.3, 0.2))]),
        "spur station",
    );
    assert_view_is_the_solve(&with, "spur station");
}

/// A hand move on a merged station where the merge is off any loop: C, a
/// spur merged at B:1, follows its hand-moved start as shown in the Raw Data
/// View, and the Closures Computed View moves exactly as the Raw Data View
/// does: C by the hand move, B's loop not at all, the merge closed.
#[test]
fn a_hand_move_at_a_merge_off_a_loop_moves_what_hangs_there_in_both_views() {
    let (without, [a, b, c, d]) = pinned_survey();
    for (at, hand) in [
        // The spur's own end of the merge: the spur follows it.
        (StationRef::new(c, 0), (0.25, 0.1)),
        // The loop's end of it: shown off its loop, the spur with it.
        (StationRef::new(b, 1), (-0.15, 0.05)),
    ] {
        let mut with = without.clone();
        plant(&mut with, at, (hand.0, hand.1, 0.0));
        let (view_with, view_without) = (view(&with), view(&without));
        for id in [a, b, c, d] {
            let n = with.get_section(id).unwrap().shots.len() as u32;
            for i in 0..=n {
                let s = StationRef::new(id, i);
                let (raw_with, raw_without) = (xy(&with, s), xy(&without, s));
                let measured = (raw_with.0 - raw_without.0, raw_with.1 - raw_without.1);
                let shown = (
                    view_with[&s].0 - view_without[&s].0,
                    view_with[&s].1 - view_without[&s].1,
                );
                assert!(
                    dist(measured, shown) < 1e-9,
                    "{at:?} moved by hand: {s:?} moved {measured:?} in the Raw Data View and \
                     {shown:?} in the Closures Computed View"
                );
            }
        }
        assert!(
            dist(
                view_with[&StationRef::new(b, 1)],
                view_with[&StationRef::new(c, 0)]
            ) < 1e-9,
            "{at:?} moved by hand: the merge B:1-C:0 came apart"
        );
        assert_view_is_the_solve(&with, "merge off a loop");
    }
}

/// A hand move on a merged station where the merge is on a loop: the Raw
/// Data View lines the merged section up on the station as shown, so the
/// loop through the merge carries the hand move, and the view closes it with
/// the rest of the loop's misclosure, holding the merge on one point as
/// shown and the held station where it is shown.
#[test]
fn a_hand_move_at_a_merge_on_a_loop_keeps_the_merge_closed_in_the_view() {
    let (without, [a, b, ..]) = pinned_survey();
    for at in [StationRef::new(b, 0), StationRef::new(a, 2)] {
        let mut with = without.clone();
        plant(&mut with, at, (0.2, -0.15, 0.0));
        let v = view(&with);
        assert!(
            dist(v[&StationRef::new(a, 2)], v[&StationRef::new(b, 0)]) < 1e-9,
            "{at:?} moved by hand: the merge A:2-B:0 came apart in the view"
        );
        assert_view_is_the_solve(&with, "merge on a loop");
    }
}

/// A hand move on a closure's end: the closure's misclosure is measured as
/// shown, so the view closes it, hand move included, and shows the two ends
/// on one point.
#[test]
fn a_hand_move_on_a_closure_end_is_closed_with_the_loop() {
    let (mut with, [a, ..]) = pinned_survey();
    plant(&mut with, StationRef::new(a, 4), (0.3, 0.1, 0.0));
    let v = view(&with);
    assert!(
        dist(v[&StationRef::new(a, 4)], v[&StationRef::new(a, 0)]) < 1e-9,
        "the closure A:4-A:0 is open in the view"
    );
    assert_view_is_the_solve(&with, "closure end");
}

/// A hand move on the station the view holds the survey at (the lowest
/// section id's station 0, here also a closure's end): it stays where the
/// Raw Data View shows it, and the rest is solved around it.
#[test]
fn a_hand_move_on_the_held_station_holds_the_survey_where_it_is_shown() {
    let (mut with, [a, ..]) = pinned_survey();
    plant(&mut with, StationRef::new(a, 0), (-0.2, 0.35, 0.0));
    assert_view_is_the_solve(&with, "held station");
}

/// A hand move on a zero datum, the station the view holds its survey at:
/// the datum stays where the Raw Data View shows it, hand move included, and
/// nothing else of its survey moves.
#[test]
fn a_hand_move_on_a_zero_datum_shows_on_the_datum_alone() {
    let (without, [.., d]) = pinned_survey();
    let at = StationRef::new(d, 1);
    let mut with = without.clone();
    plant(&mut with, at, (0.4, -0.25, 0.0));
    assert_only_nudged(
        &with,
        &without,
        &HashMap::from([(at, (0.4, -0.25))]),
        "zero datum",
    );
    assert_view_is_the_solve(&with, "zero datum");
}

/// A hand move in depth: the view's depth of that station moves by it,
/// exactly as the Raw Data View's does, and nothing else moves, in depth or
/// horizontally, on a loop, a spur, a closure's end or a datum.
#[test]
fn a_hand_move_in_depth_shows_on_that_station_alone() {
    let (without, [a, _, c, d]) = pinned_survey();
    let depth = |p: &Project| -> HashMap<StationRef, (f64, f64, f64)> {
        p.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default())
            .into_iter()
            .map(|s| {
                (
                    s.station_ref,
                    (s.coordinates.x, s.coordinates.y, s.coordinates.z),
                )
            })
            .collect()
    };
    for at in [
        StationRef::new(a, 3),
        StationRef::new(c, 2),
        StationRef::new(a, 4),
        StationRef::new(d, 1),
    ] {
        let mut with = without.clone();
        plant(&mut with, at, (0.0, 0.0, 0.8));
        let (now, was) = (depth(&with), depth(&without));
        for (s, p) in &was {
            let dz = if *s == at { 0.8 } else { 0.0 };
            let q = now[s];
            assert!(
                (q.0 - p.0).abs() < 1e-12 && (q.1 - p.1).abs() < 1e-12,
                "{at:?} moved in depth: {s:?} moved horizontally"
            );
            assert!(
                (q.2 - p.2 - dz).abs() < 1e-12,
                "{at:?} moved 0.8 m in depth: {s:?} is {:.6} m deeper in the view",
                q.2 - p.2
            );
            let raw = with.compute_station_raw_coords(*s).unwrap().z
                - without.compute_station_raw_coords(*s).unwrap().z;
            assert!((raw - dz).abs() < 1e-12, "{s:?} in the Raw Data View");
        }
    }
}

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A random hand move of 0.02 mm to 0.5 m, in depth one time in three.
fn random_hand(rng: &mut Rng) -> (f64, f64, f64) {
    let size = match rng.below(3) {
        0 => rng.range(0.02e-3, 0.95e-3),
        1 => rng.range(0.002, 0.05),
        _ => rng.range(0.1, 0.5),
    };
    let bearing = rng.range(0.0, std::f64::consts::TAU);
    let depth = if rng.below(3) == 0 {
        rng.range(-size, size)
    } else {
        0.0
    };
    (size * bearing.sin(), size * bearing.cos(), depth)
}

/// Every station a merge or a closure (of any strategy) names.
fn tied_stations(project: &Project) -> HashSet<StationRef> {
    project
        .station_merges
        .iter()
        .map(|m| (&m.station_a, &m.station_b))
        .chain(
            project
                .loop_closures
                .iter()
                .map(|c| (&c.station_a, &c.station_b)),
        )
        .flat_map(|(a, b)| [project.resolve_key(a), project.resolve_key(b)])
        .flatten()
        .collect()
}

/// Hand moves anywhere in random surveys (the combine sweeps' networks:
/// merged branches, snapped and recorded loops, zero datums, side-by-side
/// ends, legacy closures): one to four stations moved by hand, on a loop, a
/// spur, a merge, a closure's end, the held station or a datum, then settled
/// as a file opens. The Closures Computed View is the solve with every hand
/// move kept on its station ([`assert_view_is_the_solve`]; in a debug build
/// the merge-alignment assertion after the solve runs too). And a hand move
/// on a station that nothing ties is a nudge of that station alone: the view
/// is the one without it, plus the hand move there.
///
/// `STALAGMIGHT_HAND_VIEW_SEEDS` (default 150) seeds from
/// `STALAGMIGHT_HAND_VIEW_FIRST_SEED` (default 0), each over the three
/// network kinds.
#[test]
fn hand_moves_anywhere_keep_the_closures_computed_view_the_solve() {
    let env = |name: &str, default: u64| -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    };
    let seeds = env("STALAGMIGHT_HAND_VIEW_SEEDS", 150);
    let first = env("STALAGMIGHT_HAND_VIEW_FIRST_SEED", 0);
    let (mut surveys, mut solved, mut nudged, mut tied_moved) = (0usize, 0usize, 0usize, 0usize);
    for seed in first..first + seeds {
        for (kind, make) in [
            ("network", network as fn(u64) -> Project),
            ("passage", passage_network),
            ("side by side", side_by_side_network),
        ] {
            let base = make(seed);
            let mut rng = Rng(seed.wrapping_mul(0xA076_1D64_78BD_642F) | 1);
            let stations = every_station(&base);
            let tied = tied_stations(&base);
            let held: Vec<StationRef> = base.adjustment_anchors_for_tests();
            let what = format!("{kind} seed {seed}");
            surveys += 1;
            solved += usize::from(!held.is_empty());

            // Anywhere, the held stations included.
            let mut with = base.clone();
            for _ in 0..=rng.below(4) {
                let at = if !held.is_empty() && rng.below(4) == 0 {
                    held[rng.below(held.len())]
                } else {
                    stations[rng.below(stations.len())]
                };
                tied_moved += usize::from(tied.contains(&at));
                plant(&mut with, at, random_hand(&mut rng));
            }
            assert_view_is_the_solve(&with, &what);

            // On stations nothing ties: a nudge of each alone.
            let free: Vec<StationRef> = stations
                .iter()
                .copied()
                .filter(|at| !tied.contains(at))
                .collect();
            if free.is_empty() {
                continue;
            }
            let mut with = base.clone();
            let mut moved: HashMap<StationRef, (f64, f64)> = HashMap::new();
            for _ in 0..=rng.below(3) {
                let at = free[rng.below(free.len())];
                let hand = random_hand(&mut rng);
                let key = with.key_for_ref(at).unwrap();
                with.station_offsets.insert(key, hand);
                moved.insert(at, (hand.0, hand.1));
            }
            nudged += moved.len();
            assert_only_nudged(&with, &base, &moved, &what);
            assert_view_is_the_solve(&with, &what);
        }
    }
    eprintln!(
        "hand-moved views seeds {first}..{}: {surveys} surveys ({solved} the view solves), \
         {tied_moved} hand moves on a merged or tied station, {nudged} on a free one",
        first + seeds
    );
    if seeds >= 20 {
        assert!(solved > 0 && tied_moved > 0 && nudged > 0);
    }
}
