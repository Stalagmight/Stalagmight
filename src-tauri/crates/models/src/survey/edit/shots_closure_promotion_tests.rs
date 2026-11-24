//! Delete Shot's closure decisions when an earlier promotion of the same
//! delete changed the topology (#776, ported from `models::project`'s
//! `shots_closure_promotion_tests`): a promotion joins exactly the two pieces
//! it touches, so a later closure whose ends it connects stays a loop, one
//! whose ends it makes one station closes nothing and is removed (#738), and
//! a closure whose ends it leaves apart is judged on its own. The preview the
//! dialog shows is the delete run on a copy, so it promises what the delete
//! does. Closures are planted directly, as an older or imported file can carry
//! them. Synthetic data only.
//!
//! 0.6.0's version had no datums; on the station graph a closure between two
//! pieces needs a datum in each (invariant 7), so A, B and D are placed here.
//! 0.6.0's "promoted too" for a closure still apart after the first promotion
//! becomes "kept between two placed pieces": 0.6.0 itself kept such a closure
//! when promoting it would double-anchor (`would_double_anchor_after_removal`).

use crate::{local_to_gps, GpsCoordinates, LineId, ShotId, StationId};

use super::super::{ClosureId, ClosureStrategy, Project};
use super::edit_test_helpers::{
    add_datum, assert_valid, empty, line, line_at, line_from, merge, near, run, shot_id, st, xy,
};

const TOL: f64 = 1e-6;

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(20.5, -87.25, 0.0).unwrap()
}

/// The GPS position `east`/`north` metres from the origin.
fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

/// A runs 3 x 20 m east from its datum at the origin; B runs 2 x 10 m north
/// from its datum at (20, -30); C hangs one 10 m shot west off B's end (C:1
/// is the junction B:2). Returns the project and A, B, C.
fn survey() -> (Project, LineId, LineId, LineId) {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 20.0, 3));
    let b = line_at(&mut p, "B", [20.0, -30.0], run("b", 0.0, 10.0, 2));
    let c = line_at(&mut p, "C", [30.0, -10.0], run("c", 270.0, 10.0, 1));
    let (a0, b0) = (st(&p, a, 0), st(&p, b, 0));
    add_datum(&mut p, a0, origin());
    add_datum(&mut p, b0, gps_at(20.0, -30.0));
    let (c1, b2) = (st(&p, c, 1), st(&p, b, 2));
    merge(&mut p, c1, b2);
    assert_valid(&p);
    (p, a, b, c)
}

/// A closure from B:2 (via B) to `to` (via `to_line`), as a file carries it.
fn plant(p: &mut Project, b: LineId, to: StationId, to_line: LineId) -> ClosureId {
    let b2 = st(p, b, 2);
    p.push_closure(b2, Some(b), to, Some(to_line), ClosureStrategy::Batch)
}

fn last_shot(p: &Project, line: LineId) -> ShotId {
    let count = p.line(line).unwrap().shots.len();
    shot_id(p, line, count - 1)
}

/// The delete's preview must describe exactly the delete (it runs it on a
/// copy); returns the deleted project's promotions and removed closures.
fn delete_as_previewed(p: &mut Project, line: LineId) -> (Vec<ClosureId>, Vec<ClosureId>) {
    let shot = last_shot(p, line);
    let preview = p.preview_delete_shot(line, shot).unwrap();
    let edited = p.delete_shot(line, shot).unwrap();
    let promoted: Vec<ClosureId> = edited
        .report
        .sweep
        .promoted
        .iter()
        .map(|promotion| promotion.closure.id)
        .collect();
    let swept: Vec<ClosureId> = edited.report.sweep.removed.iter().map(|c| c.id).collect();
    assert_eq!(
        preview
            .promoted_closures
            .iter()
            .map(|promotion| promotion.closure.id)
            .collect::<Vec<_>>(),
        promoted,
        "the dialog lists the promotions the delete makes"
    );
    assert_eq!(
        preview
            .swept_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        swept,
        "and the closures it removes"
    );
    assert_valid(p);
    (promoted, swept)
}

#[test]
fn a_closure_the_first_promotion_connects_stays_a_loop() {
    // The issue's case: B:2 closes onto A:1, then onto A:3. The first
    // promotion makes C:1 = A:1; A:3 is then reachable along A, so the second
    // closure is a loop and stays one (0.6.0 promoted it too: one station on
    // two points 40 m apart).
    let (mut p, a, b, c) = survey();
    let (a1, a3) = (st(&p, a, 1), st(&p, a, 3));
    let first = plant(&mut p, b, a1, a);
    let second = plant(&mut p, b, a3, a);
    let (promoted, swept) = delete_as_previewed(&mut p, b);
    assert_eq!(promoted, [first]);
    assert!(swept.is_empty());
    assert_eq!(st(&p, c, 1), a1, "C:1 is the junction A:1");
    assert_eq!(p.closures.len(), 1);
    assert_eq!(p.closures[0].id, second);
    assert_eq!(
        (
            p.closures[0].a.min(p.closures[0].b),
            p.closures[0].a.max(p.closures[0].b)
        ),
        (a1.min(a3), a1.max(a3))
    );
    let raw = p.raw_positions();
    near(
        xy(&raw, st(&p, c, 0)),
        [30.0, 0.0],
        TOL,
        "C moved 10 m north",
    );
    near(xy(&raw, a3), [60.0, 0.0], TOL, "A stays");
}

#[test]
fn a_closure_the_first_promotion_makes_one_station_is_removed() {
    // D starts at A:1 (a junction). B:2 closes onto A:1 via A, then onto the
    // same station via D (0.6.0: onto D:0, merged with A:1). The first
    // promotion makes C:1 = A:1 = D:0, so the second closure closes nothing.
    let (mut p, a, b, c) = survey();
    let a1 = st(&p, a, 1);
    let d = line_from(&mut p, "D", a1, run("d", 180.0, 10.0, 1));
    let first = plant(&mut p, b, a1, a);
    let second = plant(&mut p, b, a1, d);
    let (promoted, swept) = delete_as_previewed(&mut p, b);
    assert_eq!(promoted, [first]);
    assert_eq!(swept, [second]);
    assert!(p.closures.is_empty(), "{:?}", p.closures);
    assert_eq!(st(&p, c, 1), a1);
    assert_eq!(st(&p, d, 0), a1);
}

#[test]
fn a_promotion_joins_nothing_it_does_not_touch() {
    // B:2 closes onto A:1, then onto D:1, D being its own placed piece. The
    // first promotion (C:1 = A:1) does not connect C to D, so the second
    // closure still joins two pieces; both hold a datum, so it stays a
    // closure (promoting it would double-anchor), and nothing moves for it.
    let (mut p, a, b, c) = survey();
    let d = line_at(&mut p, "D", [100.0, 10.0], run("d", 180.0, 10.0, 1));
    let d0 = st(&p, d, 0);
    add_datum(&mut p, d0, gps_at(100.0, 10.0));
    let (a1, d1) = (st(&p, a, 1), st(&p, d, 1));
    let first = plant(&mut p, b, a1, a);
    let second = plant(&mut p, b, d1, d);
    let shot = last_shot(&p, b);
    let edited = p.clone().delete_shot(b, shot).unwrap();
    assert_eq!(edited.report.sweep.kept_between_placed, [second]);
    let (promoted, swept) = delete_as_previewed(&mut p, b);
    assert_eq!(promoted, [first]);
    assert!(swept.is_empty());
    assert_eq!(st(&p, c, 1), a1);
    assert_eq!(p.closures.len(), 1);
    let kept = p.closures[0];
    assert_eq!(kept.id, second);
    assert_eq!(
        (kept.a.min(kept.b), kept.a.max(kept.b)),
        (a1.min(d1), a1.max(d1))
    );
    let raw = p.raw_positions();
    near(
        xy(&raw, d1),
        [100.0, 0.0],
        TOL,
        "D stays where its datum holds it",
    );
}

#[test]
fn a_closure_doubled_from_the_replacement_member_leaves_with_the_line() {
    // B:2 closes onto A:1 via B, and the junction closes onto A:1 again via
    // C (C:1 is 0.6.0's replacement member there: the remaining slot with the
    // lowest line id and index). 0.6.0 did not re-anchor B's closure onto
    // C:1, which a closure already tied to A:1 (`closure_rewire_skipped`):
    // the delete dropped it and the sweep promoted C's. The station graph
    // used to promote B's first and sweep C's, which with a third bridge
    // earlier in the list moved C by another closure's misclosure (farm seed
    // 34014759, 18.8 m).
    let (mut p, a, b, c) = survey();
    let (a1, c1) = (st(&p, a, 1), st(&p, c, 1));
    let doubled = plant(&mut p, b, a1, a);
    let kept = p.push_closure(c1, Some(c), a1, Some(a), ClosureStrategy::Batch);
    let (promoted, swept) = delete_as_previewed(&mut p, b);
    assert_eq!(
        promoted,
        [kept],
        "the replacement member's closure promotes"
    );
    assert!(swept.is_empty(), "{swept:?}");
    assert!(
        p.closures.iter().all(|closure| closure.id != doubled),
        "the doubled closure left with the line"
    );
    assert_eq!(st(&p, c, 1), a1, "C:1 is the junction A:1");
    let raw = p.raw_positions();
    near(
        xy(&raw, st(&p, c, 0)),
        [30.0, 0.0],
        TOL,
        "C moved 10 m north",
    );
}
