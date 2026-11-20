//! Placements and the move tools (plan 4.4): Move Segment / Line, Move
//! Station (today's shot rewrite, generalised to every line through the
//! station), and the rigid / anchored / co-located / immovable sets.


use crate::StationRef;

use super::ops_test_helpers::{
    assert_shifted, assert_still, assert_valid, branch, free_line, gps_at, near, ok, origin, run,
    xy, xy_all, z_all,
};
use super::test_helpers::{at, flat, project};
use super::{ClosureStrategy, LineId, OpError, Project};

const TOL: f64 = 1e-9;

/// A free piece: A east 3 x 10 m from (0, 0) with branch B north from A:1
/// (2 x 5 m).
fn junction_piece() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let a1 = at(&p, a, 1);
    let b = branch(&mut p, a1, run(0.0, 2, 5.0));
    (p, a, b)
}

// ----- Move Segment / Move Line --------------------------------------------

#[test]
fn move_segment_translates_the_whole_free_piece() {
    let (mut p, a, b) = junction_piece();
    let other = free_line(&mut p, [500.0, 0.0], run(0.0, 1, 5.0));
    let before = xy_all(&p);
    let out = ok(p.move_segment(b, [3.0, 4.0, 0.0]));
    assert_valid(&p);
    assert_eq!(out.value.lines, vec![a, b]);
    assert_eq!(out.value.placements, vec![at(&p, a, 0)]);
    let others: Vec<_> = p.line_stations(other).unwrap().to_vec();
    assert_shifted(&before, &xy_all(&p), TOL, |s| {
        if others.contains(&s) {
            [0.0, 0.0]
        } else {
            [3.0, 4.0]
        }
    });
}

#[test]
fn move_segment_is_refused_when_the_piece_holds_a_datum() {
    let (mut p, a, b) = junction_piece();
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    let before = p.clone();
    assert_eq!(
        p.move_segment(b, [1.0, 0.0, 0.0]).err(),
        Some(OpError::SegmentHoldsDatum)
    );
    assert_eq!(
        OpError::SegmentHoldsDatum.to_string(),
        "Cannot move a segment whose rigid group holds a zero datum"
    );
    assert_eq!(
        p.move_segment(b, [0.0, 0.0, f64::NAN]).err(),
        Some(OpError::NonFiniteOffset)
    );
    assert_eq!(
        p.move_segment(77, [1.0, 0.0, 0.0]).err(),
        Some(OpError::UnknownLine(77))
    );
    assert_eq!(p, before);
    ok(p.move_segment(b, [0.0, 0.0, 0.0]));
    assert_eq!(p, before, "a zero move is allowed and moves nothing");
}

// ----- Move Station ----------------------------------------------------------

#[test]
fn move_station_moves_only_that_station_and_its_depth() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let a1 = at(&p, a, 1);
    let (before, depths) = (xy_all(&p), z_all(&p));
    let out = ok(p.move_station(a1, [0.0, 2.0, 0.5]));
    assert_valid(&p);
    assert_eq!(out.value.rewritten, vec![(a, 0), (a, 1)]);
    assert!(!out.value.placement_moved);
    assert_shifted(&before, &xy_all(&p), 1e-9, |s| {
        if s == a1 {
            [0.0, 2.0]
        } else {
            [0.0, 0.0]
        }
    });
    let line = p.line(a).unwrap();
    assert!((line.shots[0].length - (104.0_f64 + 0.25).sqrt()).abs() < 1e-12);
    assert_eq!(line.shots[0].depth_out, 0.5);
    assert_eq!(line.shots[1].depth_in, 0.5);
    assert_eq!(line.shots[0].azimuth_in, line.shots[0].azimuth_out);
    assert!(line.shots[0].pitch_in.is_some());
    let z = z_all(&p);
    assert!((z[&a1] - depths[&a1] - 0.5).abs() < 1e-12);
    assert_eq!(z[&at(&p, a, 2)], depths[&at(&p, a, 2)]);
}

#[test]
fn move_station_on_a_junction_rewrites_every_incident_shot_on_every_line() {
    let (mut p, a, b) = junction_piece();
    let a1 = at(&p, a, 1);
    let c = branch(&mut p, a1, run(180.0, 2, 5.0));
    let before = xy_all(&p);
    let out = ok(p.move_station(a1, [1.0, 1.0, 0.0]));
    assert_valid(&p);
    assert_eq!(out.value.rewritten, vec![(a, 0), (a, 1), (b, 0), (c, 0)]);
    assert_shifted(&before, &xy_all(&p), 1e-9, |s| {
        if s == a1 {
            [1.0, 1.0]
        } else {
            [0.0, 0.0]
        }
    });
}

#[test]
fn moving_the_placement_station_moves_the_placement_and_nothing_else() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let a0 = at(&p, a, 0);
    let before = xy_all(&p);
    let out = ok(p.move_station(a0, [5.0, -2.0, 0.0]));
    assert_valid(&p);
    assert!(out.value.placement_moved);
    assert_eq!(out.value.rewritten, vec![(a, 0)]);
    assert_eq!((p.placements[0].x, p.placements[0].y), (5.0, -2.0));
    assert_shifted(&before, &xy_all(&p), 1e-9, |s| {
        if s == a0 {
            [5.0, -2.0]
        } else {
            [0.0, 0.0]
        }
    });
}

#[test]
fn moving_a_line_start_that_is_not_the_placement_station_rewrites_its_shot() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let (a0, a3) = (at(&p, a, 0), at(&p, a, 3));
    p.set_placement(a3, 30.0, 0.0).unwrap();
    let before = xy_all(&p);
    let out = ok(p.move_station(a0, [0.0, 3.0, 0.0]));
    assert_valid(&p);
    assert!(!out.value.placement_moved);
    assert_shifted(&before, &xy_all(&p), 1e-9, |s| {
        if s == a0 {
            [0.0, 3.0]
        } else {
            [0.0, 0.0]
        }
    });
}

#[test]
fn hand_moves_at_the_station_are_folded_into_the_rewritten_shots() {
    let (mut p, a, b) = junction_piece();
    let a1 = at(&p, a, 1);
    p.line_mut(a)
        .unwrap()
        .hand_moves
        .insert(a1, [0.5, 0.0, 0.1]);
    p.line_mut(b)
        .unwrap()
        .hand_moves
        .insert(a1, [0.0, 0.3, 0.0]);
    assert_valid(&p);
    let before = xy_all(&p);
    let out = ok(p.move_station(a1, [1.0, 0.0, 0.0]));
    assert_valid(&p);
    assert_eq!(out.value.hand_moves_folded, vec![a, b]);
    assert!(p.line(a).unwrap().hand_moves.is_empty());
    assert!(p.line(b).unwrap().hand_moves.is_empty());
    assert_shifted(&before, &xy_all(&p), 1e-9, |s| {
        if s == a1 {
            [1.0, 0.0]
        } else {
            [0.0, 0.0]
        }
    });
}

#[test]
fn move_station_refuses_datum_stations_zero_length_shots_and_bad_offsets() {
    let (mut p, a, b) = junction_piece();
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 2), gps_at(10.0, 10.0), None));
    let (a0, a1, b1, b2) = (at(&p, a, 0), at(&p, a, 1), at(&p, b, 1), at(&p, b, 2));
    let before = p.clone();
    assert_eq!(
        p.move_station(a0, [1.0, 0.0, 0.0]).err(),
        Some(OpError::StationHoldsDatum)
    );
    assert_eq!(
        p.move_station(b2, [1.0, 0.0, 0.0]).err(),
        Some(OpError::StationHoldsDatum),
        "a GPS fix too"
    );
    // B runs due north (an exact vector): pulling B:1 back onto B:0 leaves
    // shot 0 with no length.
    assert_eq!(
        p.move_station(b1, [0.0, -5.0, 0.0]).err(),
        Some(OpError::ZeroLengthShot)
    );
    assert_eq!(
        p.move_station(a1, [f64::INFINITY, 0.0, 0.0]).err(),
        Some(OpError::NonFiniteOffset)
    );
    assert_eq!(
        p.move_station(999, [1.0, 0.0, 0.0]).err(),
        Some(OpError::UnknownStation(999))
    );
    assert_eq!(p, before);
    ok(p.move_station(a0, [0.0, 0.0, 0.0]));
    assert_still(&xy_all(&before), &xy_all(&p), 0.0);
}

// ----- rigid groups, anchored lines, co-located and immovable stations -----

/// Three pieces: A (datum at A:0), D (datum at D:0, tied to A:0 by a legacy
/// `Tie`), and B, a free loop whose end comes back within 5 mm of its start,
/// snapped there by a `Move` closure.
fn tied_survey() -> (Project, [LineId; 3]) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let d = free_line(&mut p, [300.0, 0.0], run(0.0, 2, 10.0));
    let b = free_line(
        &mut p,
        [100.0, 100.0],
        vec![
            flat(90.0, 10.0),
            flat(0.0, 10.0),
            flat(270.0, 10.0),
            flat(180.0, 9.995),
        ],
    );
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(d, 0), gps_at(300.0, 0.0), None));
    let (a0, d1) = (at(&p, a, 0), at(&p, d, 1));
    p.push_closure(a0, None, d1, None, ClosureStrategy::Tie);
    let (b0, b4) = (at(&p, b, 0), at(&p, b, 4));
    ok(p.create_closure(b0, None, b4, None, ClosureStrategy::Move));
    assert_valid(&p);
    (p, [a, d, b])
}

#[test]
fn rigid_groups_follow_aligned_closures_and_anchored_lines_every_datum() {
    let (p, [a, d, b]) = tied_survey();
    assert_eq!(p.rigid_group_lines(a), vec![a, d], "the Tie joins A and D");
    assert_eq!(p.rigid_group_lines(b), vec![b]);
    assert!(p.rigid_group_holds_datum(d));
    assert!(!p.rigid_group_holds_datum(b));
    assert!(p.rigidly_tied(at(&p, a, 2), at(&p, d, 2)));
    assert!(!p.rigidly_tied(at(&p, a, 2), at(&p, b, 2)));
    let aligned = p.closure_alignment(&p.raw_positions());
    assert_eq!(p.anchored_lines(&p.rigid_groups(&aligned)), vec![a, d]);
}

#[test]
fn colocated_and_immovable_stations_follow_aligned_closures() {
    let (mut p, [a, d, b]) = tied_survey();
    let (a0, d0, d1, b0, b4) = (
        at(&p, a, 0),
        at(&p, d, 0),
        at(&p, d, 1),
        at(&p, b, 0),
        at(&p, b, 4),
    );
    assert_eq!(p.colocated_stations(b0), vec![b0, b4]);
    assert_eq!(p.colocated_stations(at(&p, b, 2)), vec![at(&p, b, 2)]);
    let aligned = p.closure_alignment(&p.raw_positions());
    assert_eq!(
        p.immovable_stations(&p.tie_groups(&aligned)),
        vec![a0, d0, d1],
        "datum stations and what aligned closures tie to them"
    );
    // Tear the snap (move B:4 2 cm away): it is no longer co-located.
    ok(p.move_station(b4, [0.02, 0.0, 0.0]));
    assert_eq!(p.colocated_stations(b0), vec![b0]);
    near(xy(&p, b0), [100.0, 100.0], 1e-9, "B:0 stays");
}
