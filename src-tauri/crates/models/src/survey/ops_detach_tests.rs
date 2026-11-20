//! Detach / unmerge (plan 5.2): one line gets its own copy of a junction, and
//! what its member of the old merge owned (`via` hints) goes with it.

use crate::{DatumRole, StationRef};

use super::m3_stubs::datum_roles;
use super::ops_test_helpers::{
    assert_shifted, assert_still, assert_valid, branch, free_line, gps_at, ok, origin, run, xy,
    xy_all,
};
use super::test_helpers::{at, project};
use super::{AnchorReport, ClosureEnd, OpError};

const TOL: f64 = 1e-9;

#[test]
fn detaching_after_a_merge_gives_two_pieces_where_they_were() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_stations(a2, b0));
    let merged = xy_all(&p);

    let out = ok(p.unmerge_station_pair(StationRef::new(a, 2), StationRef::new(b, 0)));
    assert_valid(&p);
    let report = out.value;
    assert_eq!((report.line, report.station), (b, b0));
    let fresh = report.new_station;
    assert_eq!(at(&p, b, 0), fresh, "b's slot got the new station");
    assert_eq!(at(&p, a, 2), b0, "a keeps the junction's id");
    assert_eq!(p.stations[&fresh].name, p.stations[&b0].name);
    assert!(p.stations[&fresh].comment.is_none() && p.stations[&fresh].aliases.is_empty());
    assert!(!p.topology().connected(b0, fresh));
    assert_eq!(p.placements.len(), 2);
    let after = xy_all(&p);
    assert_still(&merged, &after, TOL);
    assert_eq!(
        after[&fresh], merged[&b0],
        "the new station sits where the junction was"
    );
}

#[test]
fn the_pair_form_needs_one_station_on_two_lines() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let a1 = at(&p, a, 1);
    let b = branch(&mut p, a1, run(0.0, 1, 5.0));
    let before = p.clone();
    assert_eq!(
        p.unmerge_station_pair(StationRef::new(a, 0), StationRef::new(b, 0)),
        Err(OpError::NotDirectlyMerged)
    );
    assert_eq!(
        p.unmerge_station_pair(StationRef::new(a, 1), StationRef::new(a, 1)),
        Err(OpError::NotDirectlyMerged)
    );
    assert_eq!(
        p.unmerge_station_pair(StationRef::new(a, 9), StationRef::new(b, 0)),
        Err(OpError::UnknownSlot(StationRef::new(a, 9)))
    );
    assert_eq!(
        p.detach_line(at(&p, a, 0), a),
        Err(OpError::NotDirectlyMerged)
    );
    assert_eq!(p, before);
}

#[test]
fn the_station_form_detaches_the_slots_line_and_is_a_no_op_off_a_junction() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let a1 = at(&p, a, 1);
    let b = branch(&mut p, a1, run(0.0, 2, 5.0));
    let before = p.clone();
    let out = ok(p.unmerge_station(StationRef::new(a, 0)));
    assert_eq!(out.value, None);
    assert_eq!(out.anchors, AnchorReport::default());
    assert_eq!(p, before);

    let junction = at(&p, a, 1);
    let out = ok(p.unmerge_station(StationRef::new(b, 0)));
    assert_valid(&p);
    let report = out.value.unwrap();
    assert_eq!((report.line, report.station), (b, junction));
    assert_eq!(at(&p, a, 1), junction);
    assert_ne!(at(&p, b, 0), junction);
}

#[test]
fn a_hub_of_three_lines_detaches_the_chosen_line_only() {
    // Declared change (plan 5.2): the other two lines stay joined.
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let hub = at(&p, a, 1);
    let b = branch(&mut p, hub, run(0.0, 2, 5.0));
    let c = branch(&mut p, hub, run(180.0, 2, 5.0));
    let before = xy_all(&p);
    ok(p.detach_line(hub, b));
    assert_valid(&p);
    assert_eq!(p.topology().components().len(), 2);
    assert!(p.topology().connected(at(&p, a, 0), at(&p, c, 2)));
    assert!(!p.topology().connected(at(&p, a, 0), at(&p, b, 2)));
    assert_eq!(p.topology().lines_of(hub), vec![a, c]);
    assert_still(&before, &xy_all(&p), TOL);
}

#[test]
fn the_lines_hand_move_goes_to_the_new_station() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let hub = at(&p, a, 1);
    let b = branch(&mut p, hub, run(0.0, 2, 5.0));
    p.line_mut(b)
        .unwrap()
        .hand_moves
        .insert(hub, [0.5, 0.0, 0.0]);
    p.line_mut(a)
        .unwrap()
        .hand_moves
        .insert(hub, [0.0, 0.25, 0.0]);
    let before = xy_all(&p);
    let report = ok(p.detach_line(hub, b)).value;
    assert_valid(&p);
    assert!(report.moved_hand_move);
    let fresh = report.new_station;
    assert_eq!(
        p.line(b).unwrap().hand_moves.get(&fresh),
        Some(&[0.5, 0.0, 0.0])
    );
    assert_eq!(
        p.line(a).unwrap().hand_moves.get(&hub),
        Some(&[0.0, 0.25, 0.0])
    );
    assert_still(&before, &xy_all(&p), TOL);
}

#[test]
fn unmerging_places_the_detached_piece_on_its_own_gps() {
    // Today's #741 case: A placed (frame), B placed by its own GPS 30 m east,
    // merged (B moves west, its datum a fix), then unmerged: B's fix is
    // promoted and B goes back onto its GPS; A never moves.
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_stations(a2, b0));
    let merged = xy_all(&p);
    let b_stations: Vec<_> = p.line_stations(b).unwrap()[1..].to_vec();
    assert!((xy(&p, b_stations[0])[0]).abs() < 1e-6, "B moved onto A");

    let out = ok(p.unmerge_station_pair(StationRef::new(a, 2), StationRef::new(b, 0)));
    assert_valid(&p);
    assert_eq!(out.value.moved_datums, vec![1]);
    assert_eq!(
        datum_roles(&p),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    assert_eq!(out.anchors.placed_on_gps, vec![1]);
    let after = xy_all(&p);
    assert_shifted(&merged, &after, 1e-6, |s| {
        if b_stations.contains(&s) {
            [30.0, 0.0]
        } else {
            [0.0, 0.0]
        }
    });
    let b0_now = at(&p, b, 0);
    let pos = xy(&p, b0_now);
    assert!((pos[0] - 30.0).abs() < 0.01 && (pos[1] - 100.0).abs() < 0.01);
}

#[test]
fn a_placement_datum_attached_via_the_detached_line_goes_with_it() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let hub = at(&p, a, 1);
    let b = branch(&mut p, hub, run(0.0, 2, 5.0));
    ok(p.set_datum_at(StationRef::new(b, 0), origin(), None));
    let before = xy_all(&p);
    let out = ok(p.detach_line(hub, b));
    assert_valid(&p);
    let fresh = out.value.new_station;
    assert_eq!(p.datums[0].station, fresh);
    assert_eq!(p.datums[0].via, Some(b));
    assert!(
        !out.anchors.frame_changed,
        "the frame datum is the same datum"
    );
    assert_eq!(
        p.placements.len(),
        1,
        "a's piece gets a placement where it is"
    );
    assert_still(&before, &xy_all(&p), TOL);
}

#[test]
fn closure_ends_attached_via_the_line_move_and_others_stay() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let hub = at(&p, a, 1);
    let b = branch(&mut p, hub, run(0.0, 2, 5.0));
    let (a3, b2) = (at(&p, a, 3), at(&p, b, 2));
    // Two closures from the hub: one attached via a, one via b. Each closes a
    // loop inside the piece now.
    let via_a = ok(p.create_closure(hub, Some(a), b2, Some(b), super::ClosureStrategy::Batch))
        .value
        .closure
        .id;
    let via_b = ok(p.create_closure(hub, Some(b), a3, Some(a), super::ClosureStrategy::Batch))
        .value
        .closure
        .id;
    let out = ok(p.detach_line(hub, b));
    assert_valid(&p);
    assert_eq!(out.value.moved_closure_ends, vec![(via_b, ClosureEnd::A)]);
    // via_a now joins the hub (a's piece) to b2 (b's piece): a sole bridge,
    // promoted by the sweep, which merges the hub onto b2 (neither piece is
    // placed, so end a's piece moves). via_b then closes a loop and stays.
    assert_eq!(out.sweep.promoted.len(), 1);
    assert_eq!(out.sweep.promoted[0].closure.id, via_a);
    assert!(p.closures.iter().any(|c| c.id == via_b));
    assert!(p.topology().connected(at(&p, a, 0), at(&p, b, 0)));
}
