//! Loop closures (plan 5.5): creation and its refusals in today's order,
//! removal, and the orphan-closure sweep (today's `closure_sweep_tests`
//! rules on the station graph).

use crate::{DatumRole, StationRef};

use super::ops_test_helpers::{
    assert_shifted, assert_valid, free_line, gps_at, ok, origin, run, xy, xy_all,
};
use super::test_helpers::{at, project};
use super::{ClosureStrategy, LineId, LoopClosure, OpError, Project, Violation};

const TOL: f64 = 1e-9;

/// One line east, 5 x 10 m, free at the origin.
fn one_line() -> (Project, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 5, 10.0));
    (p, a)
}

#[test]
fn a_closure_stores_its_ends_hints_and_strategy_and_derives_its_figures() {
    let (mut p, a) = one_line();
    let (a0, a3) = (at(&p, a, 0), at(&p, a, 3));
    let before = xy_all(&p);
    let out = ok(p.create_closure(a0, Some(a), a3, Some(a), ClosureStrategy::Batch));
    assert_valid(&p);
    assert_eq!(
        out.value.closure,
        LoopClosure {
            id: 1,
            a: a0,
            a_via: Some(a),
            b: a3,
            b_via: Some(a),
            strategy: ClosureStrategy::Batch,
        }
    );
    assert_eq!(p.closures, vec![out.value.closure]);
    assert_eq!(p.counters.next_closure_id, 2);
    let metrics = out.value.metrics.unwrap();
    assert!((metrics.path_length - 30.0).abs() < TOL);
    assert!((metrics.misclosure_distance - 30.0).abs() < TOL);
    assert_eq!(xy_all(&p), before, "a closure moves nothing");

    let preview = p.closure_preview(a0, a3).unwrap();
    assert!((preview.path_length - 30.0).abs() < TOL);

    // Not next to the knot {a0, a3}: a closure from a1 or a4 would squeeze
    // the shot from that knot (#737).
    let slot = ok(p.create_closure_at(
        StationRef::new(a, 1),
        StationRef::new(a, 5),
        ClosureStrategy::Move,
    ))
    .value
    .closure;
    assert_eq!((slot.a_via, slot.b_via), (Some(a), Some(a)));
    assert_eq!(slot.strategy, ClosureStrategy::Move);
}

#[test]
fn creation_refuses_in_todays_order_with_todays_words() {
    let (mut p, a) = one_line();
    let b = free_line(&mut p, [0.0, 100.0], run(0.0, 1, 5.0));
    let (a0, a1, a2, a3, b0) = (
        at(&p, a, 0),
        at(&p, a, 1),
        at(&p, a, 2),
        at(&p, a, 3),
        at(&p, b, 0),
    );
    let batch = ClosureStrategy::Batch;
    assert_eq!(
        p.create_closure(a0, None, 999, None, batch).err(),
        Some(OpError::UnknownStation(999))
    );
    let same = p.create_closure(a0, None, a0, None, batch).err().unwrap();
    assert_eq!(
        same.to_string(),
        "These stations are already merged. Closures are for unmerged station pairs."
    );
    let apart = p.create_closure(a0, None, b0, None, batch).err().unwrap();
    assert_eq!(apart, OpError::NotConnected);
    assert_eq!(
        apart.to_string(),
        "Stations are not connected. They must be in the same connected group to create a \
         loop closure."
    );
    let adjacent = p.create_closure(a0, None, a1, None, batch).err().unwrap();
    assert!(matches!(
        adjacent,
        OpError::AcrossOneShot { line, index: 0, .. } if line == a
    ));
    assert_eq!(
        adjacent.to_string(),
        "This closure would join the two ends of one shot (L0 / 0 to L0 / 1) and squeeze that \
         shot to zero length. Pick two stations that are not the ends of a single shot."
    );
    assert_eq!(
        p.create_closure(a0, None, a2, None, ClosureStrategy::ProportionalByLength)
            .err(),
        Some(OpError::DeferredStrategy)
    );
    assert_eq!(
        p.create_closure(a0, None, a2, None, ClosureStrategy::Tie)
            .err(),
        Some(OpError::TieNotCreatable)
    );
    assert_eq!(
        p.create_closure(a0, Some(b), a2, None, batch).err(),
        Some(OpError::StationNotOnLine {
            station: a0,
            line: b
        })
    );
    assert!(p.closures.is_empty());

    ok(p.create_closure(a0, None, a2, None, batch));
    assert_eq!(
        p.create_closure(a2, None, a0, None, batch).err(),
        Some(OpError::DuplicateClosure),
        "either way round"
    );
    // a0 and a2 are one knot now: a3 to a0 would squeeze shot 2 (a2 -> a3).
    let knotted = p.create_closure(a3, None, a0, None, batch).err().unwrap();
    assert!(
        matches!(knotted, OpError::AcrossOneShot { index: 2, .. }),
        "{knotted:?}"
    );
    assert!(knotted.to_string().contains("(L0 / 2 to L0 / 3)"));
}

#[test]
fn two_placed_pieces_may_be_tied_by_a_closure_and_a_free_one_may_not() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    let c = free_line(&mut p, [900.0, 0.0], run(0.0, 1, 10.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    let (a2, b0, c0) = (at(&p, a, 2), at(&p, b, 0), at(&p, c, 0));
    ok(p.create_closure(a2, Some(a), b0, Some(b), ClosureStrategy::Batch));
    assert_valid(&p);
    assert_eq!(
        p.create_closure(a2, None, c0, None, ClosureStrategy::Batch)
            .err(),
        Some(OpError::NotConnected)
    );
}

#[test]
fn a_closure_is_removed_by_id() {
    let (mut p, a) = one_line();
    let (a0, a2, a4) = (at(&p, a, 0), at(&p, a, 2), at(&p, a, 4));
    let first = ok(p.create_closure(a0, None, a2, None, ClosureStrategy::Batch))
        .value
        .closure;
    let second = ok(p.create_closure(a0, None, a4, None, ClosureStrategy::Batch))
        .value
        .closure;
    let out = ok(p.remove_closure(first.id));
    assert_eq!(out.value, first);
    assert_eq!(p.closures, vec![second]);
    let missing = p.remove_closure(first.id).err().unwrap();
    assert_eq!(missing.to_string(), "Loop closure not found");
    assert_eq!(p.counters.next_closure_id, 3, "ids are never reused");
}

// ----- the orphan-closure sweep -------------------------------------------

/// Two free pieces: A east from (0, 0), 2 x 10 m; B north from (23, 4),
/// 2 x 5 m.
fn two_free_pieces() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [23.0, 4.0], run(0.0, 2, 5.0));
    (p, a, b)
}

#[test]
fn a_closure_from_a_station_to_itself_goes() {
    let (mut p, a) = one_line();
    let a1 = at(&p, a, 1);
    let planted = p.push_closure(a1, None, a1, None, ClosureStrategy::Batch);
    let report = p.sweep_orphan_closures();
    assert_eq!(report.removed.len(), 1);
    assert_eq!(report.removed[0].id, planted);
    assert!(report.promoted.is_empty());
    assert_valid(&p);
}

#[test]
fn a_sole_bridge_is_promoted_and_the_piece_of_end_a_moves_onto_end_b() {
    let (mut p, a, b) = two_free_pieces();
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let id = p.push_closure(a2, Some(a), b0, Some(b), ClosureStrategy::Batch);
    assert!(matches!(
        p.validate().first(),
        Some(Violation::ClosureAcrossUnplacedPieces { .. })
    ));
    let before = xy_all(&p);
    let report = p.sweep_orphan_closures();
    assert_valid(&p);
    assert!(p.closures.is_empty());
    assert_eq!(report.promoted.len(), 1);
    let promotion = &report.promoted[0];
    assert_eq!(promotion.closure.id, id);
    assert_eq!((promotion.survivor, promotion.retired), (b0, a2));
    // A moves by the misclosure, (23 - 20, 4 - 0); B stays.
    let a_stations: Vec<_> = p.line_stations(a).unwrap().to_vec();
    assert_shifted(&before, &xy_all(&p), TOL, |s| {
        if a_stations.contains(&s) && s != b0 {
            [3.0, 4.0]
        } else {
            [0.0, 0.0]
        }
    });
    assert!(p.topology().is_junction(b0));
}

#[test]
fn a_promotion_moves_the_free_piece_onto_a_placed_one() {
    let (mut p, a, b) = two_free_pieces();
    ok(p.set_datum_at(StationRef::new(b, 0), origin(), None));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let before = xy_all(&p);
    p.push_closure(a2, None, b0, None, ClosureStrategy::Batch);
    let report = p.sweep_orphan_closures();
    assert_valid(&p);
    assert_eq!(
        (report.promoted[0].survivor, report.promoted[0].retired),
        (b0, a2)
    );
    let b_stations: Vec<_> = p.line_stations(b).unwrap().to_vec();
    let delta = [
        before[&b0][0] - before[&a2][0],
        before[&b0][1] - before[&a2][1],
    ];
    assert_shifted(&before, &xy_all(&p), TOL, |s| {
        if b_stations.contains(&s) {
            [0.0, 0.0]
        } else {
            delta
        }
    });
    // Placed end a: the other piece moves instead.
    let (mut p, a, b) = two_free_pieces();
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let a2_at = xy(&p, a2);
    p.push_closure(a2, None, b0, None, ClosureStrategy::Batch);
    let report = p.sweep_orphan_closures();
    assert_eq!(
        (report.promoted[0].survivor, report.promoted[0].retired),
        (a2, b0)
    );
    assert_eq!(xy(&p, a2), a2_at);
}

#[test]
fn a_sole_bridge_between_two_placed_pieces_is_kept() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let id = ok(p.create_closure(a2, None, b0, None, ClosureStrategy::Batch))
        .value
        .closure
        .id;
    let before = p.clone();
    let report = p.sweep_orphan_closures();
    assert_eq!(report.kept_between_placed, vec![id]);
    assert!(report.is_empty());
    assert_eq!(p, before);
}

#[test]
fn two_closures_on_one_pair_promote_once_and_the_second_goes() {
    let (mut p, a, b) = two_free_pieces();
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let first = p.push_closure(a2, None, b0, None, ClosureStrategy::Batch);
    let second = p.push_closure(b0, None, a2, None, ClosureStrategy::Batch);
    let report = p.sweep_orphan_closures();
    assert_valid(&p);
    assert_eq!(report.promoted.len(), 1);
    assert_eq!(report.promoted[0].closure.id, first);
    assert_eq!(report.removed.len(), 1);
    assert_eq!(report.removed[0].id, second);
    assert_eq!(report.removed_positions, vec![1]);
    assert!(p.closures.is_empty());
}

#[test]
fn a_later_closure_between_the_same_pieces_becomes_a_loop_and_stays() {
    let (mut p, a, b) = two_free_pieces();
    let (a0, a2, b0, b2) = (at(&p, a, 0), at(&p, a, 2), at(&p, b, 0), at(&p, b, 2));
    let first = p.push_closure(a2, None, b0, None, ClosureStrategy::Batch);
    let second = p.push_closure(a0, None, b2, None, ClosureStrategy::Batch);
    let report = p.sweep_orphan_closures();
    assert_valid(&p);
    assert_eq!(report.promoted.len(), 1);
    assert_eq!(report.promoted[0].closure.id, first);
    assert!(report.removed.is_empty());
    assert_eq!(p.closures.len(), 1);
    assert_eq!(p.closures[0].id, second);
}

#[test]
fn removing_a_datum_unblocks_a_promotion() {
    // Two placed pieces tied by a closure; removing B's datum leaves the
    // closure a sole bridge to a free piece, so the command's sweep promotes
    // it (today's RemoveZeroDatum sweep) and B moves onto A.
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    let (a2, b0, b1) = (at(&p, a, 2), at(&p, b, 0), at(&p, b, 1));
    ok(p.create_closure(a2, None, b0, None, ClosureStrategy::Batch));
    let out = ok(p.remove_datum(1));
    assert_valid(&p);
    assert_eq!(out.sweep.promoted.len(), 1);
    assert_eq!(
        (
            out.sweep.promoted[0].survivor,
            out.sweep.promoted[0].retired
        ),
        (a2, b0)
    );
    assert!(p.closures.is_empty());
    assert_eq!(p.datum_roles(), vec![Some(DatumRole::Placement)]);
    assert_eq!(at(&p, b, 0), a2, "B starts on the junction");
    let b1_at = xy(&p, b1);
    assert!(
        b1_at[0].abs() < 1e-6 && (b1_at[1] - 150.0).abs() < 1e-6,
        "B moved 30 m west onto A: {b1_at:?}"
    );
}
