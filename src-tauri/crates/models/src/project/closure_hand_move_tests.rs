//! The Closures Computed View and a station moved by hand (#759): a manual
//! station offset (`station_offsets`), as an older or hand-edited file
//! carries it. Move Station folds its moves into the shots; Split Section
//! Here, Append Project and Undo carry an existing one on.

use std::collections::HashMap;

use super::combine_closure_tests::{add, dist, shot, xy};
use crate::{ClosureStrategy, LoopClosure, Project, StationRef};

/// Every station's closure-adjusted position (the Plan and Export geometry).
fn view(project: &Project) -> HashMap<StationRef, (f64, f64)> {
    project
        .compute_adjusted_station_coordinates(&crate::config::ClosureAdjustmentConfig::default())
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
fn pinned_survey() -> (Project, [crate::SectionId; 4]) {
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
        .compute_adjusted_station_coordinates(&crate::config::ClosureAdjustmentConfig::default())
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
        [0xc05c519ce0ae287e, 0xc05edb399dfc8d32, 0x4009333333333333],
    ),
    (
        0,
        1,
        [0xc05c1ad482ddc13d, 0xc05d1fdc06213303, 0x4010888888888889],
    ),
    (
        0,
        2,
        [0xc05b2d9330d1c962, 0xc05d1f37ed2a68e1, 0x4017bbbbbbbbbbbc],
    ),
    (
        0,
        3,
        [0xc05b18af6e9c9b22, 0xc05ea71a2eee64b8, 0x401199999999999a],
    ),
    (
        0,
        4,
        [0xc05c519ce0ae287e, 0xc05edb399dfc8d32, 0x4009333333333333],
    ),
    (
        1,
        0,
        [0xc05b2d9330d1c962, 0xc05d1f37ed2a68e1, 0x4017bbbbbbbbbbbc],
    ),
    (
        1,
        1,
        [0xc05b02cbc3c5aad0, 0xc05b8a08b0a153cf, 0x401ddddddddddddd],
    ),
    (
        1,
        2,
        [0xc05ab04d87501d0c, 0xc05c124037481428, 0x4020333333333333],
    ),
    (
        1,
        3,
        [0xc05c1ad482ddc13d, 0xc05d1fdc06213303, 0x4010888888888889],
    ),
    (
        2,
        0,
        [0xc05b02cbc3c5aad0, 0xc05b8a08b0a153cf, 0x401ddddddddddddd],
    ),
    (
        2,
        1,
        [0xc05be5f932080218, 0xc05b06df7b5026c5, 0x4022000000000000],
    ),
    (
        2,
        2,
        [0xc05c4dbe62756c09, 0xc05aafcca42972f7, 0x4023333333333333],
    ),
    (
        3,
        0,
        [0xbfe9b449f6ba2117, 0xc023cd87cffdfa5a, 0x3ff0cccccccccccd],
    ),
    (
        3,
        1,
        [0x3cfb800000000000, 0x3d00000000000000, 0x4000000000000000],
    ),
    (
        3,
        2,
        [0x40244953deb8326c, 0xbff02bf9667f41f9, 0x4004000000000000],
    ),
    (
        3,
        3,
        [0x40227ace7f40f3dd, 0xc025517dac61d617, 0x3ffccccccccccccd],
    ),
    (
        3,
        4,
        [0xbfe9b449f6ba2110, 0xc023cd87cffdfa5e, 0x3ff0cccccccccccd],
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
