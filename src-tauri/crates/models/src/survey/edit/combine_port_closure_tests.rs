//! Port of the legacy Combine Lines closure tests
//! (`project/combine_closure_tests.rs`, the oracle) to the station-graph
//! model: combining across loop closures, side-by-side ends, which side moves
//! (datum, then the adjusted-view gauge), torn ties, both sides held, and the
//! closures a combine leaves across one shot (#746).
//!
//! Declared changes the port adapts to (plan 5.4, 12.2): a merge is one
//! station, so a torn junction is a detach (the moving line gets a new
//! station, a `Batch` closure ties old and new) and closure ends and ids
//! differ from legacy while the geometry matches; a tie reaching a station of
//! both lines besides the join tears as `PassesTwice` (legacy `Loop`);
//! closure metrics are derived; the gauge is the stored `Placement` station;
//! there is no closure-adjusted view yet (stream M3), so "the view does not
//! move" is checked on the gauge station and "the view solves" by the
//! solve's rule ([`solved_at`]). A closure between two free pieces is invalid
//! (plan 2 invariant 7).
//!
//! Fate of each legacy test:
//! - `two_lines_in_a_loop_combine_at_the_closure`: ported.
//! - `a_closure_join_moves_the_side_without_the_adjustment_anchor`: keeping K
//!   ported (in the test above); keeping O adapted: a known difference (the
//!   gauge on a station both lines pass stays on the line that does not move,
//!   so the plain rule applies).
//! - `the_anchor_counts_only_where_the_closure_adjusted_view_solves`: adapted:
//!   the loop variants as the previous line (F1 for keeping O); the unmerged
//!   variants not applicable (a closure-only tie between free pieces).
//! - `a_pair_meeting_twice_at_a_station_is_offered_once_by_its_best_join`,
//!   `a_zero_datum_on_the_other_line_moves_the_kept_side`,
//!   `a_snapped_closure_at_the_join_joins_and_goes`,
//!   `a_closure_already_across_one_shot_is_left_alone`,
//!   `a_closure_the_join_puts_across_one_shot_is_removed`: ported.
//! - `a_three_line_loop_tears_the_merge_nearest_the_moving_line`,
//!   `both_sides_held_tear_the_merge_to_the_moving_sides_datum`: ported as
//!   `..._junction_...`; the sweep found the deeper both-held case (fixed,
//!   `both_sides_held_tear_the_junction_to_a_datum_beyond_it`).
//! - `the_anchor_rule_follows_the_solve_of_the_closure_the_combine_makes`,
//!   `a_combine_leaves_no_closure_across_one_shot_it_made`: ported for the
//!   merged passage; the snapped passage not applicable (a snap as a free
//!   piece's only tie is invalid; the sweep makes it the junction). The
//!   removed tie is still in `torn_ties`, named by `RemovedClosure::torn_tie`.
//! - `a_component_whose_only_closure_is_skipped_keeps_its_anchor_but_is_not_adjusted`:
//!   adapted (O reached by a one-shot passage, not a closure-only tie).
//! - `the_backstop_names_a_missed_closure_by_the_tie_it_came_from`: not
//!   applicable (no backstop: the facts are read off the result); the naming
//!   is `RemovedClosure::torn_tie`, asserted in the passage test.
//! - `the_sweep_seeds_that_replace_a_closure_keep_the_invariants`: adapted as
//!   `of_two_closures_the_combine_puts_on_one_pair_the_solved_one_stays`.
//! - The eight random sweeps and chains (`every_offered_combine_*`,
//!   `chains_*`): adapted into one sweep and one chain test over four network
//!   shapes; the chains found a lettering clash on a third line (fixed,
//!   `lettered_names_stay_unique_on_the_other_lines_of_a_junction`).
//! - `merge_stations_never_leaves_a_closure_between_merged_stations`,
//!   `the_sweep_removes_exactly_the_closures_between_merged_stations`: not
//!   applicable (Merge and the sweep are stream M2's; a closure on one station
//!   is removed by the fold, #738, and refused by `validate`).
//! - `replay_a_combine_chain`, `chains_the_campaigns_failed_on_keep_the_invariants`:
//!   not applicable (replay tooling and trails of the legacy generators' random
//!   streams; their shapes are hand-built here or reached by the sweeps).

use std::collections::BTreeSet;

use crate::{
    CombineJoinEnds, CombineJoinKind, CombineLinesOptions, CombinePick, GpsCoordinates,
    KeptMovesReason, LineEnd, LineId, StationId, StationRef,
};

use super::super::test_helpers::gps;
use super::super::{ClosureStrategy, EditContext, Project, ZeroDatum};
use super::edit_test_helpers::{
    add_datum, assert_valid, empty, flat_u, line, line_at, line_from, merge, near, ok, run,
    skipped_ids, st, xy,
};
use super::{CombineCandidate, CombineReport, TornTieKind, TornTieReason};

const TOL: f64 = 1e-9;

// ---- Shared builders ----------------------------------------------------------

/// Today's dialog options (the legacy oracle's), joining at `join`.
fn opts(join: CombineJoinEnds) -> CombineLinesOptions {
    CombineLinesOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: Some(join),
    }
}

fn ends(kept: LineEnd, other: LineEnd) -> CombineJoinEnds {
    CombineJoinEnds { kept, other }
}

/// The legacy oracle's GPS position.
fn gps0() -> GpsCoordinates {
    gps(20.0, -87.0)
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn chain(p: &Project, line: LineId) -> Vec<StationId> {
    p.line_stations(line).unwrap().to_vec()
}

/// The candidate's plan for keeping `kept`.
fn plan_keeping(c: &CombineCandidate, kept: LineId) -> &CombineReport {
    if c.line_a == kept {
        &c.plan_keeping_a
    } else {
        &c.plan_keeping_b
    }
}

/// How many stations are junctions (what legacy counted as merges, one per
/// extra line through a station).
fn junction_slots(p: &Project) -> usize {
    p.stations
        .keys()
        .map(|s| p.topology().lines_of(*s).len().saturating_sub(1))
        .sum()
}

/// Does the Closures Computed View solve something (today's
/// `plan_adjustment_anchors` non-empty), in the piece of `at` when given: a
/// GPS fix, or a closure the solve takes (`Batch`, `LeastSquaresWeighted`)
/// that it does not skip as across one shot (#737)?
fn solved_at(p: &Project, at: Option<StationId>) -> bool {
    let topology = p.topology();
    let inside =
        |s: StationId| at.is_none_or(|a| topology.component_of(a) == topology.component_of(s));
    let placement: BTreeSet<usize> = p.placement_datums().into_iter().flatten().collect();
    let skipped = skipped_ids(&p);
    let fix = p
        .datums
        .iter()
        .enumerate()
        .any(|(i, d)| !placement.contains(&i) && inside(d.station));
    let eligible = [
        ClosureStrategy::Batch,
        ClosureStrategy::LeastSquaresWeighted,
    ];
    fix || p
        .closures
        .iter()
        .any(|c| eligible.contains(&c.strategy) && !skipped.contains(&c.id) && inside(c.a))
}

fn solved(p: &Project) -> bool {
    solved_at(p, None)
}

/// A datum on `station` that holds its piece where it is (a correction, as an
/// older file or Add .stalx carries it), instead of moving it onto the GPS
/// as Set Datum does. The legacy oracle pushed such datums raw.
fn plant_datum_where_it_is(p: &mut Project, station: StationId, at: GpsCoordinates) {
    let before = p.anchor_snapshot();
    let here = xy(&p.raw_positions(), station);
    let index = p.push_datum(ZeroDatum::new(station, at));
    p.datums[index].correction = p.correction_to_hold(index, here);
    p.settle_anchors(&before, &EditContext::none().with_corrected(index));
    near(
        xy(&p.raw_positions(), station),
        here,
        TOL,
        "setup: held where it is",
    );
    assert_valid(p);
}

/// K runs 10 m east from (100, -40); O runs 9.6 m west and its END is the
/// same station as K's start (legacy: merged there), so O's start sits 0.4 m
/// short of K's end, where a loop closure of `strategy` ties them. K was made
/// first: its start holds the piece's placement (the adjusted-view gauge;
/// legacy: the lowest line id's gauge station).
fn two_line_loop(strategy: ClosureStrategy) -> (Project, LineId, LineId) {
    let mut p = empty();
    let k = line_at(&mut p, "K", [100.0, -40.0], run("k", 90.0, 10.0, 1));
    let o = line_at(&mut p, "O", [109.6, -40.0], run("o", 270.0, 9.6, 1));
    let (k0, o1) = (st(&p, k, 0), st(&p, o, 1));
    merge(&mut p, o1, k0);
    let (k1, o0) = (st(&p, k, 1), st(&p, o, 0));
    p.push_closure(k1, Some(k), o0, Some(o), strategy);
    assert_valid(&p);
    assert_eq!(p.placements.len(), 1, "setup: one piece");
    assert_eq!(
        p.placements[0].station, k0,
        "setup: K's start holds the gauge"
    );
    (p, k, o)
}

// ---- Closure joins: which side moves, what tears ------------------------------

/// The station menu on K's end offers O across the closure. Combining there
/// moves O 0.4 m east onto K's end, the junction at K's start becomes the
/// loop closure, and K (whose start is the gauge) does not move; O moves for
/// no reason whether the loop's closure is one the solve takes or a legacy
/// one (the halves of the legacy
/// `a_closure_join_moves_the_side_without_the_adjustment_anchor` and
/// `the_anchor_counts_only_where_the_closure_adjusted_view_solves` that keep
/// K; the halves that keep O are adapted below). Declared (plan 5.4 step 6, 12.2): the
/// tie reaches K's start, a station of both lines besides the join, so it
/// tears with reason `PassesTwice` (today: `Loop`), and the detached O gets a
/// new station there (today: a merge turned into a closure).
#[test]
fn two_lines_in_a_loop_combine_at_the_closure() {
    let (mut p, k, o) = two_line_loop(ClosureStrategy::Batch);
    let (k0, k1, o0) = (st(&p, k, 0), st(&p, k, 1), st(&p, o, 0));
    let at_end = p.combine_candidates(StationRef::new(k, 1));
    assert_eq!(at_end.len(), 1);
    let candidate = &at_end[0];
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    assert_eq!((candidate.station_a, candidate.station_b), (k1, o0));
    let plan = &candidate.plan_keeping_a;
    assert_eq!((plan.moving_line, plan.kept_moves_because), (Some(o), None));
    assert_eq!(plan.moved_lines, vec![o]);
    assert!((plan.shift_m - 0.4).abs() < 1e-9, "{}", plan.shift_m);
    let (legacy, ..) = two_line_loop(ClosureStrategy::ProportionalByLength);
    let legacy_plan = &legacy.combine_candidates(StationRef::new(k, 1))[0].plan_keeping_a;
    assert_eq!(legacy_plan.moving_line, Some(o));
    assert_eq!(legacy_plan.kept_moves_because, None);
    assert_eq!(plan.torn_ties.len(), 1);
    assert_eq!(plan.torn_ties[0].kind, TornTieKind::Junction);
    assert_eq!(plan.torn_ties[0].reason, TornTieReason::PassesTwice);
    assert_eq!(plan.torn_ties[0].station, k0);

    // The two-line query prefers the junction at K's start instead.
    let pair = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(pair.join_kind, CombineJoinKind::Merged);
    assert!(pair.plan_keeping_a.torn_ties.is_empty());

    let before = p.raw_positions();
    let report = ok(p.combine_lines(k, o, &opts(ends(LineEnd::End, LineEnd::Start))));
    assert_eq!(report, *plan, "the command does what the menu said");
    let after = p.raw_positions();
    let combined = chain(&p, k);
    assert_eq!(combined.len(), 3);
    assert_eq!(combined[..2], [k0, k1]);
    // K stays bit for bit (its start is the placement); O's far end moved
    // 0.4 m east of where it was (on K's start).
    let (was, now) = (xy(&before, k0), xy(&after, k0));
    assert!(was[0] == now[0] && was[1] == now[1]);
    near(xy(&after, k1), xy(&before, k1), TOL, "K's end");
    near(
        xy(&after, combined[2]),
        [was[0] + 0.4, was[1]],
        TOL,
        "O's old end",
    );
    assert_eq!(p.lines().len(), 1);
    assert_eq!(junction_slots(&p), 0, "no junction is left");
    assert_eq!(p.closures.len(), 1);
    let closure = p.closures[0];
    assert_eq!(
        BTreeSet::from([closure.a, closure.b]),
        BTreeSet::from([k0, combined[2]])
    );
    assert_eq!(closure.strategy, ClosureStrategy::Batch);
    // Misclosure 0.4 m over a path of 19.6 m (metrics are derived: plan 3.6).
    let misclosure = dist(xy(&after, closure.a), xy(&after, closure.b));
    assert!((misclosure - 0.4).abs() < 1e-9, "{misclosure}");
    let path: f64 = p
        .line(k)
        .unwrap()
        .shots
        .iter()
        .map(super::super::Shot::horizontal_length)
        .sum();
    assert!((path - 19.6).abs() < 1e-9, "{path}");
    assert_valid(&p);
}

/// Keeping O across the closure join (legacy
/// `a_closure_join_moves_the_side_without_the_adjustment_anchor`, and
/// `the_anchor_counts_only_where_the_closure_adjusted_view_solves` for a legacy
/// closure in the loop). ADAPTED, a known difference: today the gauge sat on
/// K's member of the merge at K's start, so K's side held it and the kept O
/// moved (AdjustmentAnchor). With shared stations K's start is ONE station on
/// both lines, and the combine detaches the moving line from it (plan 5.4
/// step 6): the station, and the Placement (the gauge) on it, stays on the
/// line that does not move whichever line that is. So no side holds the gauge
/// and the plain rule applies: K (the other line) moves 0.4 m west, and the
/// gauge does not move, which is what today's rule protects.
#[test]
fn a_closure_join_moves_the_side_without_the_adjustment_anchor() {
    for strategy in [
        ClosureStrategy::Batch,
        ClosureStrategy::ProportionalByLength,
    ] {
        let (p, k, o) = two_line_loop(strategy);
        let candidate = p.combine_candidates(StationRef::new(k, 1)).remove(0);
        assert_eq!(
            candidate.plan_keeping_b.moving_line,
            Some(k),
            "{strategy:?}"
        );
        assert_eq!(
            candidate.plan_keeping_b.kept_moves_because, None,
            "{strategy:?}"
        );
        let _ = o;
    }
    let (mut p, k, o) = two_line_loop(ClosureStrategy::Batch);
    let (k0, k1, o0) = (st(&p, k, 0), st(&p, k, 1), st(&p, o, 0));
    let before = p.raw_positions();
    ok(p.combine_lines(o, k, &opts(ends(LineEnd::Start, LineEnd::End))));
    let after = p.raw_positions();
    near(
        xy(&after, k0),
        xy(&before, k0),
        TOL,
        "the gauge station (K's start) stays",
    );
    assert_eq!(p.placements.len(), 1);
    assert_eq!(p.placements[0].station, k0, "the gauge keeps its station");
    // K was prepended: its new start (detached from k0) moved west with it,
    // and the joined station is O's start.
    near(
        xy(&after, o0),
        xy(&before, o0),
        TOL,
        "O's start (the joined station) stays",
    );
    let new_start = st(&p, o, 0);
    assert_ne!(new_start, k0);
    near(
        xy(&after, new_start),
        [xy(&before, k0)[0] - 0.4, xy(&before, k0)[1]],
        TOL,
        "K moved 0.4 m west",
    );
    assert!(p.station(k1).is_none(), "K's end folded into O's start");
    assert_valid(&p);
}

/// K east, X north from K's end, O from X's end back toward K's start,
/// falling 0.5 m short, with a closure there. Combining K and O at that
/// closure tears the junction between O and X (the one nearest O), moves
/// only O, and leaves K and X where they were.
#[test]
fn a_three_line_loop_tears_the_junction_nearest_the_moving_line() {
    let mut p = empty();
    let k = line(&mut p, "K", run("k", 90.0, 10.0, 1));
    let k1 = st(&p, k, 1);
    let x = line_from(&mut p, "X", k1, run("x", 0.0, 10.0, 1));
    let x1 = st(&p, x, 1);
    let o = line_from(&mut p, "O", x1, run("o", 225.0, 200.0_f64.sqrt() - 0.5, 1));
    let (k0, o1) = (st(&p, k, 0), st(&p, o, 1));
    p.push_closure(o1, Some(o), k0, Some(k), ClosureStrategy::Batch);
    assert_valid(&p);

    let candidate = p.combine_candidate(StationRef::new(k, 0)).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moved_lines, vec![o]);
    assert!((plan.shift_m - 0.5).abs() < 1e-9);
    let torn: Vec<_> = plan
        .torn_ties
        .iter()
        .map(|t| (t.kind, t.reason, t.station, t.slot, t.other_slot))
        .collect();
    assert_eq!(
        torn,
        vec![(
            TornTieKind::Junction,
            TornTieReason::Loop,
            x1,
            StationRef::new(o, 0),
            StationRef::new(x, 1)
        )]
    );

    let before = p.raw_positions();
    ok(p.combine_lines(k, o, &opts(ends(LineEnd::Start, LineEnd::End))));
    let after = p.raw_positions();
    // O was prepended: K's stations are now 1 and 2.
    let combined = chain(&p, k);
    assert_eq!(combined.len(), 3);
    assert_eq!(combined[1..], [k0, k1]);
    for s in [k0, k1, x1] {
        near(xy(&after, s), xy(&before, s), TOL, &format!("station {s}"));
    }
    assert_eq!(chain(&p, x), [k1, x1]);
    assert_eq!(junction_slots(&p), 1, "K-X stays");
    assert_eq!(p.closures.len(), 1);
    let closure = p.closures[0];
    assert_eq!(
        BTreeSet::from([closure.a, closure.b]),
        BTreeSet::from([x1, combined[0]])
    );
    let misclosure = dist(xy(&after, closure.a), xy(&after, closure.b));
    assert!((misclosure - 0.5).abs() < 1e-9, "{misclosure}");
    assert_valid(&p);
}

/// A zero datum on the other line holds it: the kept line's side moves
/// instead, and the datum station stays exactly where it was.
#[test]
fn a_zero_datum_on_the_other_line_moves_the_kept_side() {
    let (mut p, k, o) = two_line_loop(ClosureStrategy::Batch);
    let o0 = st(&p, o, 0);
    add_datum(&mut p, o0, gps0());
    let datum_at = xy(&p.raw_positions(), o0);
    let candidate = p.combine_candidates(StationRef::new(k, 1)).remove(0);
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.plan_keeping_a.moving_line, Some(k));
    assert_eq!(
        candidate.plan_keeping_a.kept_moves_because,
        Some(KeptMovesReason::ZeroDatum)
    );
    assert_eq!(candidate.plan_keeping_b.moving_line, Some(k));
    assert_eq!(candidate.plan_keeping_b.kept_moves_because, None);

    ok(p.combine_lines(k, o, &opts(ends(LineEnd::End, LineEnd::Start))));
    let joined = st(&p, k, 1);
    assert_eq!(
        p.datums[0].station, joined,
        "the datum is on the joined station"
    );
    near(
        xy(&p.raw_positions(), joined),
        datum_at,
        TOL,
        "the datum stays",
    );
    assert_valid(&p);
}

/// Two datum-held pieces, each with a junction to one of the two lines, tied
/// only by a closure between the lines' ends: the other line tears off its
/// datum side and moves alone; the torn junction stays a closure (it ties two
/// placed pieces, so the sweep does not promote it).
#[test]
fn both_sides_held_tear_the_junction_to_the_moving_sides_datum() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 0.0, 5.0, 1));
    let (a0, a1) = (st(&p, a, 0), st(&p, a, 1));
    let k = line_from(&mut p, "K", a1, run("k", 90.0, 10.0, 1));
    add_datum(&mut p, a0, gps0());
    let k_end = xy(&p.raw_positions(), st(&p, k, 1));
    let o = line_at(
        &mut p,
        "O",
        [k_end[0] + 0.3, k_end[1] + 0.2],
        run("o", 90.0, 8.0, 1),
    );
    let o1 = st(&p, o, 1);
    let b = line_from(&mut p, "B", o1, run("b", 180.0, 5.0, 1));
    let b1 = st(&p, b, 1);
    plant_datum_where_it_is(&mut p, b1, gps0());
    let (k1, o0) = (st(&p, k, 1), st(&p, o, 0));
    p.push_closure(k1, Some(k), o0, Some(o), ClosureStrategy::Batch);
    assert_valid(&p);

    let candidate = p.combine_candidates(StationRef::new(k, 1)).remove(0);
    assert_eq!(candidate.blocked_reason, None);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moved_lines, vec![o]);
    assert_eq!(plan.torn_ties.len(), 1);
    assert_eq!(plan.torn_ties[0].kind, TornTieKind::Junction);
    assert_eq!(plan.torn_ties[0].station, o1);
    assert_eq!(
        plan.torn_ties[0].reason,
        TornTieReason::ZeroDatum,
        "it tears because B's datum holds B, not to open a loop"
    );
    let before = p.raw_positions();
    let (a_was, b_was) = (chain(&p, a), chain(&p, b));
    ok(p.combine_lines(k, o, &opts(ends(LineEnd::End, LineEnd::Start))));
    let after = p.raw_positions();
    assert_eq!((chain(&p, a), chain(&p, b)), (a_was.clone(), b_was.clone()));
    for s in a_was.into_iter().chain(b_was) {
        near(xy(&after, s), xy(&before, s), TOL, &format!("station {s}"));
    }
    assert_eq!(p.closures.len(), 1);
    let last = *chain(&p, k).last().unwrap();
    assert_eq!(
        BTreeSet::from([p.closures[0].a, p.closures[0].b]),
        BTreeSet::from([o1, last])
    );
    // Nothing a later topology edit's sweep would promote.
    let mut swept = p.clone();
    assert!(swept.sweep_orphan_closures().is_empty());
    assert_eq!(swept, p);
    assert_valid(&p);
}

/// Both sides held, the moving side's datum one junction further on: K
/// hangs south from the start of C (two shots east); D hangs north from C's
/// end and carries the frame datum; T, held by its own datum, ends 0.3 m
/// beside K's end, tied by a closure. Keeping K, T carries its datum, so K
/// moves: it tears off C (its datum side, through C and D) and moves alone
/// onto T's end; C, D and T stay, and neither datum moves (the legacy
/// `both_sides_held_tear_the_merge_to_the_moving_sides_datum` with the
/// datum past the junction, as the random sweep met it).
#[test]
fn both_sides_held_tear_the_junction_to_a_datum_beyond_it() {
    let mut p = empty();
    let c = line(&mut p, "C", run("c", 90.0, 10.0, 2));
    let (c0, c2) = (st(&p, c, 0), st(&p, c, 2));
    let d = line_from(&mut p, "D", c2, run("d", 0.0, 5.0, 1));
    let d1 = st(&p, d, 1);
    add_datum(&mut p, d1, gps0());
    let k = line_from(&mut p, "K", c0, run("k", 180.0, 10.0, 1));
    let k1 = st(&p, k, 1);
    let k_end = xy(&p.raw_positions(), k1);
    let t = line_at(
        &mut p,
        "T",
        [k_end[0] - 8.0, k_end[1] - 0.3],
        run("t", 90.0, 8.0, 1),
    );
    let (t0, t1) = (st(&p, t, 0), st(&p, t, 1));
    plant_datum_where_it_is(&mut p, t0, gps(20.0001, -87.0002));
    p.push_closure(k1, Some(k), t1, Some(t), ClosureStrategy::Batch);
    assert_valid(&p);

    let candidate = p.combine_candidates(StationRef::new(k, 1)).remove(0);
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_line, Some(k));
    assert_eq!(plan.kept_moves_because, Some(KeptMovesReason::ZeroDatum));
    let torn: Vec<_> = plan
        .torn_ties
        .iter()
        .map(|t| (t.kind, t.reason, t.station))
        .collect();
    assert_eq!(
        torn,
        vec![(TornTieKind::Junction, TornTieReason::ZeroDatum, c0)]
    );
    let before = p.raw_positions();
    ok(p.combine_lines(k, t, &opts(ends(LineEnd::End, LineEnd::End))));
    let after = p.raw_positions();
    for s in [c0, c2, d1, t0] {
        near(
            xy(&after, s),
            xy(&before, s),
            TOL,
            &format!("station {s} stays"),
        );
    }
    // T's end is folded into K's end, the joined station, where T's end was.
    assert!(p.station(t1).is_none());
    near(xy(&after, k1), xy(&before, t1), TOL, "the joined station");
    near(
        xy(&after, st(&p, k, 0)),
        [xy(&before, c0)[0], xy(&before, c0)[1] - 0.3],
        TOL,
        "K moved alone, 0.3 m south",
    );
    assert_valid(&p);
}

/// A snapped closure between the two ends (aligned, a few mm apart) is a
/// closure join: it goes, and the other line moves the few mm. The junction
/// that closed the loop becomes the loop's closure.
#[test]
fn a_snapped_closure_at_the_join_joins_and_goes() {
    let mut p = empty();
    let k = line(&mut p, "K", run("k", 90.0, 10.0, 1));
    // O loops back to K's start, 4 mm longer than K: its start lands 4 mm
    // from K's end, and a Move closure snaps the two.
    let o = line_at(
        &mut p,
        "O",
        [10.004, 0.0],
        vec![
            flat_u(0.0, 6.0, "o-0"),
            flat_u(270.0, 10.004, "o-1"),
            flat_u(180.0, 6.0, "o-2"),
        ],
    );
    let (o3, k0) = (st(&p, o, 3), st(&p, k, 0));
    merge(&mut p, o3, k0);
    let (k1, o0) = (st(&p, k, 1), st(&p, o, 0));
    let positions = p.raw_positions();
    let gap = dist(xy(&positions, k1), xy(&positions, o0));
    assert!((gap - 0.004).abs() < 1e-9, "{gap}");
    let id = p.push_closure(k1, Some(k), o0, Some(o), ClosureStrategy::Move);
    let snap = *p.closures.iter().find(|c| c.id == id).unwrap();
    assert!(p.closure_is_aligned(&positions, &snap), "within 1 cm");

    let candidate = p.combine_candidates(StationRef::new(k, 1)).remove(0);
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    ok(p.combine_lines(k, o, &opts(ends(LineEnd::End, LineEnd::Start))));
    assert_eq!(junction_slots(&p), 0);
    assert_eq!(
        p.closures.len(),
        1,
        "the junction became the loop's closure"
    );
    let closure = p.closures[0];
    assert_ne!(closure.id, id, "the join's snap is gone");
    assert!(!p.closure_is_aligned(&p.raw_positions(), &closure));
    assert_valid(&p);
}

// ---- The anchor rule and closures across one shot (#737, #746) ----------------

/// K runs 10 m east. A passage X starting on K's end (one station, legacy:
/// merged) reaches 0.4 m further east (one shot, or two that go north and
/// come back); O starts on X's end and runs 5 m east. K's end and O's start
/// are tied by a legacy proportional closure (one the solve does not take),
/// so the station menu offers them as a closure join that closes by a 0.4 m
/// shift, and nothing else is solved: whether the closure-adjusted view
/// solves the joined survey depends only on the tie the combine tears.
///
/// The legacy `XToO::Snapped` shapes (O tied to X only by a snap) are not
/// applicable: a snap that is a free piece's only tie is invalid (plan 2
/// invariant 7) and the sweep makes it the junction this shape already has.
fn passage_join(x_shots: usize) -> (Project, LineId, LineId, LineId) {
    let mut p = empty();
    let k = line(&mut p, "K", run("k", 90.0, 10.0, 1));
    let k1 = st(&p, k, 1);
    let path = if x_shots == 1 {
        vec![flat_u(90.0, 0.4, "x-0")]
    } else {
        // North 1 m, then back down to 0.4 m east of the start.
        vec![
            flat_u(0.0, 1.0, "x-0"),
            flat_u(158.198_590_513_648_2, 1.077_032_961_426_9, "x-1"),
        ]
    };
    let x = line_from(&mut p, "X", k1, path);
    let x_end = st(&p, x, x_shots);
    let o = line_from(&mut p, "O", x_end, run("o", 90.0, 5.0, 1));
    p.push_closure(
        k1,
        Some(k),
        x_end,
        Some(o),
        ClosureStrategy::ProportionalByLength,
    );
    assert_valid(&p);
    assert!(
        !solved(&p),
        "setup: nothing in the survey is solved before the combine"
    );
    (p, k, x, o)
}

/// Combine K and O at the closure join, keeping O when `keep_o`; the plan the
/// menu offered, whether the view solves the result, and the result.
fn combine_at_the_join(
    p: &Project,
    k: LineId,
    o: LineId,
    keep_o: bool,
) -> (CombineReport, bool, Project) {
    let candidate = p
        .combine_candidates(StationRef::new(k, 1))
        .into_iter()
        .find(|c| BTreeSet::from([c.line_a, c.line_b]) == BTreeSet::from([k, o]))
        .unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    let (kept, other, join) = if keep_o {
        (o, k, ends(LineEnd::Start, LineEnd::End))
    } else {
        (k, o, ends(LineEnd::End, LineEnd::Start))
    };
    let plan = plan_keeping(&candidate, kept).clone();
    assert!(plan.shift_m > 0.39, "setup: the join closes by a shift");
    let mut combined = p.clone();
    let report = ok(combined.combine_lines(kept, other, &opts(join)));
    assert_eq!(
        report, plan,
        "the plan the menu offered is what the combine did, removals included (#746)"
    );
    let solves = solved(&combined);
    (plan, solves, combined)
}

/// The #743 review, point 1: the kept side moves for the adjusted-view gauge
/// only where that view solves the joined survey. Nothing is solved now, and
/// the only closure the combine makes is the junction it tears:
/// - X one shot: the torn junction's closure spans X's one shot; the combine
///   removes it (#746), the view shows the joined survey measured, and the
///   plain rule holds (keeping O, K moves);
/// - X two shots (control): the closure closes a real loop, the view solves
///   it, and keeping O moves O because K holds the gauge.
#[test]
fn the_anchor_rule_follows_the_solve_of_the_closure_the_combine_makes() {
    for x_shots in [1, 2] {
        let what = format!("X {x_shots} shot(s)");
        let (p, k, _, o) = passage_join(x_shots);
        assert_eq!(p.placements[0].station, st(&p, k, 0), "{what}: setup");
        // Keeping K: O moves whatever the view does (K holds the gauge).
        let (plan, _, _) = combine_at_the_join(&p, k, o, false);
        assert_eq!(plan.moving_line, Some(o), "{what}");
        assert_eq!(plan.kept_moves_because, None, "{what}");
        // Keeping O: the gauge rule moves O only where the view solves.
        let (plan, solves, combined) = combine_at_the_join(&p, k, o, true);
        assert_eq!(solves, x_shots == 2, "{what}: setup of the shape");
        assert!(skipped_ids(&combined).is_empty(), "{what}");
        assert_eq!(
            plan.closures_removed_across_one_shot.len(),
            usize::from(x_shots == 1),
            "{what}: the plan names the closure the combine removes (#746)"
        );
        if solves {
            assert_eq!(plan.moving_line, Some(o), "{what}");
            assert_eq!(
                plan.kept_moves_because,
                Some(KeptMovesReason::AdjustmentAnchor),
                "{what}"
            );
        } else {
            assert_eq!(plan.moving_line, Some(k), "{what}: shown measured, K moves");
            assert_eq!(plan.kept_moves_because, None, "{what}");
        }
    }
}

/// The #743 review, point 4. K runs three shots east; a one-shot passage X
/// from K's end leads to O's start 0.4 m further, tied to K's end by a legacy
/// closure join (legacy: O tied by that closure only, which is invalid
/// between two free pieces here, plan 2 invariant 7; the passage keeps O in
/// the piece and its torn junction is removed as across one shot, #746, so
/// the combine itself makes nothing solvable). K also carries a Batch closure
/// across its first shot only (K0-K1, which the solve skips) or across two
/// (K0-K2, a real loop, the control). Where the piece's only eligible closure
/// is skipped:
/// - the gauge is the same station as for the control and without that
///   closure (a stored placement: closures never move it);
/// - the view solves nothing;
/// - Combine treats it as not adjusted: keeping O, the plain rule moves K.
///
/// In the control the view solves, so keeping O moves O (K holds the gauge).
#[test]
fn a_piece_whose_only_closure_is_skipped_keeps_its_gauge_but_is_not_adjusted() {
    let build = |closure_end: Option<usize>| {
        let mut p = empty();
        let k = line(&mut p, "K", run("k", 90.0, 10.0, 3));
        let k3 = st(&p, k, 3);
        let x = line_from(&mut p, "X", k3, vec![flat_u(90.0, 0.4, "x-0")]);
        let x1 = st(&p, x, 1);
        let o = line_from(&mut p, "O", x1, run("o", 90.0, 5.0, 1));
        p.push_closure(
            k3,
            Some(k),
            x1,
            Some(o),
            ClosureStrategy::ProportionalByLength,
        );
        let id = closure_end.map(|end| {
            let (k0, kn) = (st(&p, k, 0), st(&p, k, end));
            p.push_closure(k0, Some(k), kn, Some(k), ClosureStrategy::Batch)
        });
        assert_valid(&p);
        (p, k, o, id)
    };
    let (skipped, k, o, id) = build(Some(1));
    let (control, ..) = build(Some(2));
    let (without, ..) = build(None);
    assert_eq!(skipped_ids(&skipped), vec![id.unwrap()], "setup");
    assert!(skipped_ids(&control).is_empty(), "setup");

    // The gauge ignores the closures.
    let gauge = skipped.placements[0].station;
    assert_eq!(gauge, st(&skipped, k, 0));
    assert_eq!(control.placements, skipped.placements);
    assert_eq!(without.placements, skipped.placements);

    // The view solves nothing where its only closure is skipped.
    assert!(!solved(&skipped));
    assert!(solved(&control));

    // Combine at the closure join, keeping O.
    let keeping_o = |p: &Project| {
        let candidate = p
            .combine_candidates(StationRef::new(k, 3))
            .into_iter()
            .find(|c| c.line_a == o || c.line_b == o)
            .unwrap();
        assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
        plan_keeping(&candidate, o).clone()
    };
    let plain = keeping_o(&skipped);
    assert!(plain.shift_m > 0.39, "setup: the join closes by a shift");
    assert_eq!(
        plain.moving_line,
        Some(k),
        "not adjusted: the other line moves"
    );
    assert_eq!(plain.kept_moves_because, None);
    let held = keeping_o(&control);
    assert_eq!(
        held.moving_line,
        Some(o),
        "adjusted: K holds the gauge, O moves"
    );
    assert_eq!(
        held.kept_moves_because,
        Some(KeptMovesReason::AdjustmentAnchor)
    );
}

/// Where the tie a combine tears was: X's junction with K's end when K
/// moves, else with O's start; the torn station, its slot on the line that
/// leaves it, and its slot on X.
fn torn_in_passage(
    p: &Project,
    (k, x, o): (LineId, LineId, LineId),
    k_moves: bool,
) -> (StationId, StationRef, StationRef) {
    if k_moves {
        (st(p, k, 1), StationRef::new(k, 1), StationRef::new(x, 0))
    } else {
        let x_end = p.line(x).unwrap().shots.len() as u32;
        (
            st(p, o, 0),
            StationRef::new(o, 0),
            StationRef::new(x, x_end),
        )
    }
}

/// #746: the junction the combine tears between X and the joined stations
/// becomes a loop closure between the two ends of X's one shot. The combine
/// removes it in the same edit, whichever line is kept, and the plan the
/// menu offered names it, by the tie it came from (legacy: by its stations
/// before the combine, as a merge; the backstop of the legacy
/// `the_backstop_names_a_missed_closure_by_the_tie_it_came_from` is this
/// naming here). In the controls (X two shots) the closure closes a real loop
/// and stays.
///
/// Adapted: the removed tie is still listed in `torn_ties`, and its
/// `RemovedClosure::torn_tie` points at it (legacy listed it as removed
/// only).
#[test]
fn a_combine_leaves_no_closure_across_one_shot_it_made() {
    for x_shots in [1, 2] {
        for keep_o in [false, true] {
            let what = format!("X {x_shots} shot(s), keeping O {keep_o}");
            let (p, k, x, o) = passage_join(x_shots);
            let (plan, _, combined) = combine_at_the_join(&p, k, o, keep_o);
            let across = skipped_ids(&combined);
            assert!(across.is_empty(), "{what}: {across:?}");
            // Every tie torn and not removed is an ordinary closure now; the
            // join's legacy closure is gone; a removed junction is gone too.
            let removed_ties = plan
                .closures_removed_across_one_shot
                .iter()
                .filter(|r| r.torn_tie.is_some())
                .count();
            let eligible = combined
                .closures
                .iter()
                .filter(|c| c.strategy == ClosureStrategy::Batch)
                .count();
            assert_eq!(eligible, plan.torn_ties.len() - removed_ties, "{what}");
            assert_eq!(
                combined.closures.len(),
                eligible,
                "{what}: the join's closure is gone"
            );
            let torn_junctions = plan
                .torn_ties
                .iter()
                .filter(|t| t.kind == TornTieKind::Junction)
                .count();
            assert_eq!(
                junction_slots(&combined),
                junction_slots(&p) - torn_junctions,
                "{what}"
            );
            assert_eq!(plan.torn_ties.len(), 1, "{what}");
            let tie = plan.torn_ties[0];
            assert_eq!(
                (tie.station, tie.slot, tie.other_slot),
                torn_in_passage(&p, (k, x, o), plan.moving_line == Some(k)),
                "{what}"
            );
            if x_shots == 2 {
                assert!(plan.closures_removed_across_one_shot.is_empty(), "{what}");
                assert_eq!(eligible, 1, "{what}: the real loop's closure stays");
                continue;
            }
            assert_eq!(plan.closures_removed_across_one_shot.len(), 1, "{what}");
            let removed = plan.closures_removed_across_one_shot[0];
            assert_eq!(removed.torn_tie, Some(0), "{what}: named by its tie");
            assert_eq!(Some(removed.closure.id), tie.closure, "{what}");
        }
    }
}

/// A closure that already joins the two ends of one shot before the combine
/// is the user's, with its own warning: the combine neither lists nor
/// removes it, as Delete with shift leaves one (#737). Here a `Batch` closure
/// across K's one shot sits next to the one-shot passage, whose torn junction
/// is still removed.
#[test]
fn a_closure_already_across_one_shot_is_left_alone() {
    for keep_o in [false, true] {
        let what = format!("keeping O {keep_o}");
        let (mut p, k, x, o) = passage_join(1);
        let (k0, k1) = (st(&p, k, 0), st(&p, k, 1));
        let id = p.push_closure(k0, Some(k), k1, Some(k), ClosureStrategy::Batch);
        assert_eq!(skipped_ids(&p), vec![id], "setup");
        let (plan, _, combined) = combine_at_the_join(&p, k, o, keep_o);
        let removed: Vec<_> = plan
            .closures_removed_across_one_shot
            .iter()
            .map(|r| r.torn_tie.map(|i| plan.torn_ties[i].station))
            .collect();
        assert_eq!(
            removed,
            vec![Some(torn_in_passage(&p, (k, x, o), keep_o).0)],
            "{what}: K moves exactly when O is kept"
        );
        assert_eq!(skipped_ids(&combined), vec![id], "{what}: the user's stays");
    }
}

/// An ordinary closure can come to join the two ends of one shot through the
/// join alone. K runs two shots east; O starts on K's end (side by side, not
/// one station) and loops back north, west and south onto K's start (one
/// station). A `Batch` closure ties K's middle station to O's start.
/// Combined, O's start is K's end, so the closure spans K's last shot: the
/// combine removes it and the plan names it (no tie of the combine: its own
/// stations), while the junction that would make the combined line pass K's
/// start twice tears into a closure around a real loop, which stays. Control:
/// the closure to O's second station spans two shots once joined, and stays.
#[test]
fn a_closure_the_join_puts_across_one_shot_is_removed() {
    for to_o in [0usize, 1] {
        for keep_o in [false, true] {
            let what = format!("closure K:1 to O:{to_o}, keeping O {keep_o}");
            let mut p = empty();
            let k = line(&mut p, "K", run("k", 90.0, 5.0, 2));
            let at = xy(&p.raw_positions(), st(&p, k, 2));
            let o = line_at(
                &mut p,
                "O",
                at,
                vec![
                    flat_u(0.0, 4.0, "o-0"),
                    flat_u(270.0, 10.0, "o-1"),
                    flat_u(180.0, 4.0, "o-2"),
                ],
            );
            let (o3, k0) = (st(&p, o, 3), st(&p, k, 0));
            merge(&mut p, o3, k0);
            assert!(
                dist(xy(&p.raw_positions(), st(&p, o, 0)), at) < 1e-9,
                "setup"
            );
            let (k1, ot) = (st(&p, k, 1), st(&p, o, to_o));
            let closure = p.push_closure(k1, Some(k), ot, Some(o), ClosureStrategy::Batch);
            assert!(skipped_ids(&p).is_empty(), "{what}: setup");
            // K and O meet twice; the station menu at K's end offers the
            // side-by-side join there.
            let candidate = p
                .combine_candidates_at_point(StationRef::new(k, 2))
                .into_iter()
                .find(|c| BTreeSet::from([c.line_a, c.line_b]) == BTreeSet::from([k, o]))
                .unwrap();
            assert_eq!(candidate.blocked_reason, None, "{what}");
            assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated, "{what}");
            let (kept, other, join) = if keep_o {
                (o, k, ends(LineEnd::Start, LineEnd::End))
            } else {
                (k, o, ends(LineEnd::End, LineEnd::Start))
            };
            let plan = plan_keeping(&candidate, kept).clone();
            let mut combined = p.clone();
            let report = ok(combined.combine_lines(kept, other, &opts(join)));
            assert_eq!(
                report, plan,
                "{what}: the plan names exactly what the combine does"
            );
            assert!(skipped_ids(&combined).is_empty(), "{what}");
            assert_eq!(plan.torn_ties.len(), 1, "{what}: the loop's junction tears");
            assert_eq!(plan.torn_ties[0].station, k0, "{what}");
            assert_eq!(
                plan.torn_ties[0].reason,
                TornTieReason::PassesTwice,
                "{what}"
            );
            let kept_closure = combined.closures.iter().any(|c| c.id == closure);
            let removed: Vec<_> = plan
                .closures_removed_across_one_shot
                .iter()
                .map(|r| (r.closure.id, r.torn_tie))
                .collect();
            if to_o == 1 {
                assert!(removed.is_empty(), "{what}: {removed:?}");
                assert!(kept_closure, "{what}: the control closure stays");
            } else {
                assert_eq!(removed, vec![(closure, None)], "{what}");
                assert!(!kept_closure, "{what}");
            }
        }
    }
}

/// Combine letters the two lines' station names when they collide
/// (`plan_combine_prefixes`); a junction is one station with one name, so a
/// lettered name must stay unique on its other lines too (plan 5.3). A and B
/// start on one station; C passes A's end and already has a station named
/// "A1", the name A's end gets. The menu offers the combine; it must
/// succeed, with every line's names unique (legacy: names were per slot, so
/// this could not collide).
#[test]
fn lettered_names_stay_unique_on_the_other_lines_of_a_junction() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 1));
    let (a0, a1) = (st(&p, a, 0), st(&p, a, 1));
    let b = line_from(&mut p, "B", a0, run("b", 0.0, 10.0, 2));
    let c = line_from(&mut p, "C", a1, run("c", 180.0, 5.0, 1));
    let c1 = st(&p, c, 1);
    p.station_mut(c1).unwrap().name = "A1".to_string();
    assert_valid(&p);
    let candidate = p.combine_candidate_for_lines(a, b).unwrap();
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(candidate.join_kind, CombineJoinKind::Merged);
    let plan = &candidate.plan_keeping_a;
    assert!(plan.station_renames.is_some(), "the names collide");
    let report = ok(p.combine_lines(a, b, &opts(ends(LineEnd::Start, LineEnd::Start))));
    assert_eq!(report, *plan, "the dialog showed what the combine does");
    let on_c: BTreeSet<String> = chain(&p, c)
        .iter()
        .map(|s| p.station(*s).unwrap().name.clone())
        .collect();
    assert_eq!(on_c.len(), 2, "{on_c:?}");
    assert_valid(&p);
}

// ---- Two closures on the same two stations (#749) -----------------------------

/// The legacy sweep seeds that replace a closure (#749) pin, by hand: K runs
/// 10 m east; Y leaves K's start north, east and south, ending on K's end's
/// spot (side by side, not one station); O starts there too and runs north
/// onto Y's second corner (one station). Closure `first` ties Y's end to K's
/// end and a `Batch` closure ties Y's end to O's start (zero misclosure).
/// Combining K and O folds O's start into K's end, so both closures land on
/// the same two stations: the one the solve takes stays, whichever comes
/// first, and the dialog lists the other as replaced by it.
#[test]
fn of_two_closures_the_combine_puts_on_one_pair_the_solved_one_stays() {
    for (first, second) in [
        (
            ClosureStrategy::ProportionalByLength,
            ClosureStrategy::Batch,
        ),
        (ClosureStrategy::Move, ClosureStrategy::Batch),
        (
            ClosureStrategy::Batch,
            ClosureStrategy::ProportionalByLength,
        ),
    ] {
        let what = format!("{first:?} then {second:?}");
        let mut p = empty();
        let k = line(&mut p, "K", run("k", 90.0, 10.0, 1));
        let (k0, k1) = (st(&p, k, 0), st(&p, k, 1));
        let y = line_from(
            &mut p,
            "Y",
            k0,
            vec![
                flat_u(0.0, 5.0, "y-0"),
                flat_u(90.0, 10.0, "y-1"),
                flat_u(180.0, 5.0, "y-2"),
            ],
        );
        let (y2, y3) = (st(&p, y, 2), st(&p, y, 3));
        let at = xy(&p.raw_positions(), k1);
        let o = line_at(&mut p, "O", at, run("o", 0.0, 5.0, 1));
        let o1 = st(&p, o, 1);
        merge(&mut p, o1, y2);
        let o0 = st(&p, o, 0);
        let a = p.push_closure(y3, Some(y), k1, Some(k), first);
        let b = p.push_closure(y3, Some(y), o0, Some(o), second);
        assert_valid(&p);
        let solved_one = if first == ClosureStrategy::Batch {
            a
        } else {
            b
        };
        let candidate = p
            .combine_candidate_joining(k, o, Some(ends(LineEnd::End, LineEnd::Start)))
            .unwrap();
        assert_eq!(candidate.blocked_reason, None, "{what}");
        assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated, "{what}");
        let report = ok(p.combine_lines(k, o, &opts(ends(LineEnd::End, LineEnd::Start))));
        assert_eq!(
            report, candidate.plan_keeping_a,
            "{what}: the dialog said so"
        );
        let replaced: Vec<_> = report
            .closures_replaced
            .iter()
            .map(|r| (r.dropped.id, r.dropped.strategy, r.by.id, r.by.strategy))
            .collect();
        let dropped = if solved_one == a {
            (b, second)
        } else {
            (a, first)
        };
        assert_eq!(
            replaced,
            vec![(dropped.0, dropped.1, solved_one, ClosureStrategy::Batch)],
            "{what}"
        );
        let on_pair: Vec<_> = p
            .closures
            .iter()
            .filter(|c| BTreeSet::from([c.a, c.b]) == BTreeSet::from([y3, k1]))
            .map(|c| c.id)
            .collect();
        assert_eq!(on_pair, vec![solved_one], "{what}");
        assert_valid(&p);
    }
}

// ---- Randomised sweeps (adapted) ----------------------------------------------

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

    fn bearing(&mut self) -> f64 {
        self.range(0.0, std::f64::consts::TAU)
    }
}

/// The extra shape a sweep network carries (the legacy generators): `Plain`
/// (`network`: a tree of lines, snapped and recorded loops, datums, a second
/// datum-held piece tied on by a closure), `Passage` (#746: a one- or
/// two-shot passage X from a line end P, a line O from X's end, a legacy or
/// Batch closure P-O), `SideBySide` (#740: a line O whose near end sits a
/// residual within the tolerance from P, with a branch, a connector back into
/// P's piece, or nothing, sometimes held by a datum), `HandMoved` (#755: as
/// `SideBySide` or O starting on P, with legacy hand moves on P's line at P
/// and on O at its near end, and a branch at O's near end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Plain,
    Passage,
    SideBySide,
    HandMoved,
}

const SHAPES: [Shape; 4] = [
    Shape::Plain,
    Shape::Passage,
    Shape::SideBySide,
    Shape::HandMoved,
];

/// Seeds per shape (legacy: 120, 60, 40 and 40; chains 40 each).
const SEEDS: u64 = 40;

fn random_shots(rng: &mut Rng, tag: &str) -> Vec<crate::ShotData> {
    (0..=rng.below(3))
        .map(|i| {
            flat_u(
                rng.range(0.0, 360.0),
                rng.range(2.0, 15.0),
                &format!("{tag}-{i}"),
            )
        })
        .collect()
}

fn any_station(p: &Project, rng: &mut Rng) -> StationId {
    let all = p.topology().stations();
    all[rng.below(all.len())]
}

fn last(p: &Project, line: LineId) -> StationId {
    *chain(p, line).last().unwrap()
}

fn line_ends(p: &Project) -> Vec<StationRef> {
    let ends = |l: &super::super::Line| [0, l.shots.len() as u32].map(|i| StationRef::new(l.id, i));
    p.lines().iter().flat_map(ends).collect()
}

fn bearing(from: [f64; 2], to: [f64; 2]) -> f64 {
    (to[0] - from[0])
        .atan2(to[1] - from[1])
        .to_degrees()
        .rem_euclid(360.0)
}

fn norm(v: [f64; 2]) -> f64 {
    v[0].hypot(v[1])
}

/// Move `station`'s free piece so `station` sits on `target` (legacy
/// `place`: the piece's placement translated).
fn place(p: &mut Project, station: StationId, target: [f64; 2]) {
    let now = xy(&p.raw_positions(), station);
    let topology = p.topology_arc();
    let piece = topology.component_of(station);
    let placement = p
        .placements
        .iter_mut()
        .find(|pl| topology.component_of(pl.station) == piece);
    let placement = placement.unwrap();
    placement.x += target[0] - now[0];
    placement.y += target[1] - now[1];
}

/// Hold `station`'s piece with a datum where it is (the frame when there is
/// none yet, which moves nothing relative to anything else).
fn hold_with_datum(p: &mut Project, station: StationId) {
    let topology = p.topology_arc();
    let piece = topology.component_of(station);
    if p.datums
        .iter()
        .any(|d| topology.component_of(d.station) == piece)
    {
        return;
    }
    if p.frame_datum().is_some() {
        plant_datum_where_it_is(p, station, gps(20.0001, -87.0002));
    } else {
        add_datum(p, station, gps0());
    }
}

/// A random network of `shape` (see [`Shape`]); valid, every piece anchored.
fn network(seed: u64, shape: Shape) -> Project {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut p = empty();
    let at = [rng.range(-50.0, 50.0), rng.range(-50.0, 50.0)];
    let shots = random_shots(&mut rng, "S0");
    line_at(&mut p, "S0", at, shots);
    for i in 1..3 + rng.below(4) {
        let (host, name) = (any_station(&p, &mut rng), format!("S{i}"));
        let shots = random_shots(&mut rng, &name);
        if rng.below(2) == 0 {
            line_from(&mut p, &name, host, shots);
        } else {
            let s = line_at(&mut p, &name, [0.0, 0.0], shots);
            let end = last(&p, s);
            merge(&mut p, end, host);
        }
    }
    for j in 0..=rng.below(2) {
        // A loop back onto the network: snapped exactly, or recorded long.
        let (a, b) = (any_station(&p, &mut rng), any_station(&p, &mut rng));
        let positions = p.raw_positions();
        let (pa, pb) = (xy(&positions, a), xy(&positions, b));
        let snapped = rng.below(2) == 0;
        let extra = if snapped { 0.0 } else { rng.range(0.2, 1.5) };
        if dist(pa, pb) < 1.0 {
            continue;
        }
        let name = format!("L{j}");
        let shot = flat_u(bearing(pa, pb), dist(pa, pb) + extra, &format!("{name}-0"));
        let l = line_from(&mut p, &name, a, vec![shot]);
        let b_line = p.topology().lines_of(b)[0];
        let strategy = [ClosureStrategy::Batch, ClosureStrategy::Move][usize::from(snapped)];
        p.push_closure(st(&p, l, 1), Some(l), b, Some(b_line), strategy);
    }
    if rng.below(3) == 0 {
        let s = any_station(&p, &mut rng);
        add_datum(&mut p, s, gps0());
    }
    if rng.below(3) == 0 {
        // A second piece held on its own, tied on by a closure between a line
        // end of each (legal: both pieces are placed).
        let ends = line_ends(&p);
        let main = ends[rng.below(ends.len())];
        let main_s = p.station_at(main).unwrap();
        hold_with_datum(&mut p, main_s);
        let at = [rng.range(-50.0, 50.0), rng.range(-50.0, 50.0)];
        let shots = random_shots(&mut rng, "T");
        let t = line_at(&mut p, "T", at, shots);
        let t0 = st(&p, t, 0);
        hold_with_datum(&mut p, t0);
        let batch = ClosureStrategy::Batch;
        p.push_closure(main_s, Some(main.line_id), last(&p, t), Some(t), batch);
    }
    let ends = line_ends(&p);
    let end = ends[rng.below(ends.len())];
    let p_s = p.station_at(end).unwrap();
    if shape == Shape::Passage {
        let path = (0..=rng.below(2))
            .map(|i| {
                flat_u(
                    rng.range(0.0, 360.0),
                    rng.range(0.3, 3.0),
                    &format!("X-{i}"),
                )
            })
            .collect();
        let x = line_from(&mut p, "X", p_s, path);
        let x_end = last(&p, x);
        let shots = random_shots(&mut rng, "O");
        let o = line_from(&mut p, "O", x_end, shots);
        let strategy = [
            ClosureStrategy::ProportionalByLength,
            ClosureStrategy::Batch,
        ];
        p.push_closure(
            p_s,
            Some(end.line_id),
            x_end,
            Some(o),
            strategy[rng.below(2)],
        );
    }
    if matches!(shape, Shape::SideBySide | Shape::HandMoved) {
        let hands = shape == Shape::HandMoved;
        let hand = |rng: &mut Rng, on: bool| {
            let sizes = [
                rng.range(0.02e-3, 0.95e-3),
                rng.range(0.002, 0.05),
                rng.range(0.1, 0.5),
            ];
            let (size, b) = (sizes[rng.below(3)], rng.bearing());
            [size * b.sin(), size * b.cos(), 0.0].map(|v| if on { v } else { 0.0 })
        };
        let on = hands && rng.below(4) != 1;
        let hand_p = hand(&mut rng, on);
        if hands {
            let line = p.line_mut(end.line_id).unwrap();
            line.hand_moves.insert(p_s, hand_p);
        }
        let on = hands && rng.below(4) != 0;
        let hand_o = hand(&mut rng, on);
        let shots = random_shots(&mut rng, "O");
        let mut near_start = rng.below(2) == 0;
        let o = if hands && rng.below(3) == 0 {
            near_start = true;
            line_from(&mut p, "O", p_s, shots)
        } else {
            // O's near end's chain position on P's, or a residual from it.
            let r = if !hands || rng.below(2) == 0 {
                rng.range(0.02e-3, 0.95e-3)
            } else {
                0.0
            };
            let b = rng.bearing();
            let shown = xy(&p.raw_positions(), p_s);
            let target = [
                shown[0] - hand_p[0] + r * b.sin() + hand_o[0],
                shown[1] - hand_p[1] + r * b.cos() + hand_o[1],
            ];
            let o = line_at(&mut p, "O", [0.0, 0.0], shots);
            let near_s = if near_start {
                st(&p, o, 0)
            } else {
                last(&p, o)
            };
            place(&mut p, near_s, target);
            o
        };
        let (first, end_o) = (st(&p, o, 0), last(&p, o));
        let (near_s, far_s) = if near_start {
            (first, end_o)
        } else {
            (end_o, first)
        };
        if hands && near_s != p_s {
            p.line_mut(o).unwrap().hand_moves.insert(near_s, hand_o);
        }
        if hands {
            let shots = random_shots(&mut rng, "TB");
            line_from(&mut p, "TB", near_s, shots);
        }
        let mut held = far_s;
        match rng.below(3) {
            0 => {
                let shots = random_shots(&mut rng, "B");
                let b = line_from(&mut p, "B", far_s, shots);
                held = st(&p, b, 1);
            }
            1 => {
                // A connector from O's far end back into P's piece.
                let topology = p.topology_arc();
                let q = any_station(&p, &mut rng);
                let positions = p.raw_positions();
                let (from, to) = (xy(&positions, far_s), xy(&positions, q));
                let piece = topology.component_of(p_s);
                let apart = piece != topology.component_of(far_s);
                if apart && topology.component_of(q) == piece && dist(from, to) >= 0.5 {
                    let shot = flat_u(bearing(from, to), dist(from, to), "C-0");
                    let c = line_from(&mut p, "C", far_s, vec![shot]);
                    let c1 = last(&p, c);
                    merge(&mut p, c1, q);
                }
            }
            _ => {}
        }
        if rng.below(4) == 0 {
            hold_with_datum(&mut p, held);
        }
    }
    assert_valid(&p);
    p
}

/// The invariants after one combine of `candidate` keeping `kept` (`report`
/// is what it did): the legacy `check_combined`, `check_removed_across_one_shot`
/// and `check_replaced`, on what the station graph keeps (no closure-adjusted
/// view yet, stream M3: the gauge rule is checked on the placement station).
/// Returns `true`, checked only up to the closures, when two datum-held
/// pieces were joined with no tear for a datum (the sweep asserts it never
/// happens; see [`both_sides_held_tear_the_junction_to_a_datum_beyond_it`]).
fn check_combine(
    before: &Project,
    after: &Project,
    candidate: &CombineCandidate,
    kept: LineId,
    report: &CombineReport,
    what: &str,
) -> bool {
    assert_valid(after);
    let other = [candidate.line_a, candidate.line_b]
        .into_iter()
        .find(|l| *l != kept)
        .unwrap();
    let because = report.kept_moves_because.is_some();
    assert_eq!(
        because,
        report.moving_line == Some(kept),
        "{what}: {report:?}"
    );
    let (pb, pa) = (before.raw_positions(), after.raw_positions());
    let moved = |was: StationId, now: StationId| {
        let (a, b) = (xy(&pa, now), xy(&pb, was));
        [a[0] - b[0], a[1] - b[1]]
    };
    // Closures: none newly across one shot (#746); the removed and replaced
    // ones are gone; a replacement is one the solve takes (#749), which can
    // then go as across one shot (legacy order too); no snap inside the
    // combined line.
    let skipped = skipped_ids(&before);
    for id in skipped_ids(&after) {
        assert!(
            skipped.contains(&id),
            "{what}: closure {id} left across one shot"
        );
    }
    let present = |id| after.closures.iter().any(|c| c.id == id);
    let removed: Vec<_> = report
        .closures_removed_across_one_shot
        .iter()
        .map(|r| r.closure.id)
        .collect();
    assert!(
        removed.iter().all(|id| !present(*id)),
        "{what}: {removed:?}"
    );
    for r in &report.closures_replaced {
        let by = present(r.by.id) || removed.contains(&r.by.id);
        assert!(!present(r.dropped.id) && by, "{what}: {r:?}");
        assert_eq!(r.by.strategy, ClosureStrategy::Batch, "{what}: {r:?}");
    }
    let on_line: BTreeSet<StationId> = chain(after, kept).into_iter().collect();
    for c in &after.closures {
        let inside = on_line.contains(&c.a) && on_line.contains(&c.b);
        let snap = c.strategy == ClosureStrategy::Move && after.closure_is_aligned(&pa, c);
        assert!(
            !(inside && snap),
            "{what}: snapped closure {} inside the line",
            c.id
        );
    }
    let topology = before.topology();
    let piece = |s: StationId| topology.component_of(s);
    let held = |s: StationId| before.datums.iter().any(|d| piece(d.station) == piece(s));
    let (a, b) = (candidate.station_a, candidate.station_b);
    let for_datum = report
        .torn_ties
        .iter()
        .any(|t| t.reason == TornTieReason::ZeroDatum);
    if piece(a) != piece(b) && held(a) && held(b) && !for_datum {
        return true;
    }
    // Placement datums do not move (the side a datum holds stays).
    for d in before.placement_datums().into_iter().flatten() {
        let m = moved(before.datums[d].station, after.datums[d].station);
        assert!(norm(m) < 1e-6, "{what}: datum {d} moved {m:?}");
    }
    // Third lines: rigidly by the shift, with the joined station, or not.
    let joined = report.moved_with_joined_station.as_ref();
    for line in before
        .lines()
        .iter()
        .filter(|l| l.id != kept && l.id != other)
    {
        let (was, now) = (chain(before, line.id), chain(after, line.id));
        assert_eq!(was.len(), now.len(), "{what}: line {}", line.id);
        let moves: Vec<[f64; 2]> = was.iter().zip(&now).map(|(w, n)| moved(*w, *n)).collect();
        assert!(
            moves.iter().all(|m| dist(*m, moves[0]) < 1e-6),
            "{what}: line {} bent",
            line.id
        );
        let expected = match joined.filter(|j| j.lines.contains(&line.id)) {
            _ if report.moved_lines.contains(&line.id) => report.shift_m,
            Some(j) => j.shift_m,
            None => 0.0,
        };
        let by = norm(moves[0]);
        assert!(
            (by - expected).abs() < 1e-6,
            "{what}: line {} moved {by}",
            line.id
        );
    }
    // The two lines: the moving one's stations by the shift (one vector), the
    // fixed one's not at all; the join stations aside (hand moves, #755). A
    // station on both lines before is a junction the moving line left.
    let combined = chain(after, kept);
    let shots = |l: LineId| before.line(l).unwrap().shots.len();
    assert_eq!(combined.len(), shots(kept) + shots(other) + 1, "{what}");
    let (on_k, on_o) = (chain(before, kept), chain(before, other));
    let mut shift: Option<[f64; 2]> = None;
    for s in combined
        .iter()
        .filter(|s| pb.xy(**s).is_some() && ![a, b].contains(s))
    {
        let from = match (on_k.contains(s), on_o.contains(s)) {
            (true, false) => kept,
            (false, true) => other,
            _ => continue,
        };
        let m = moved(*s, *s);
        if report.moving_line == Some(from) {
            assert!(
                (norm(m) - report.shift_m).abs() < 1e-6,
                "{what}: {s} moved {m:?}"
            );
            assert!(
                dist(*shift.get_or_insert(m), m) < 1e-6,
                "{what}: the moving line bent"
            );
        } else {
            assert!(norm(m) < 2e-6, "{what}: {s} of the fixed side moved {m:?}");
        }
    }
    // A torn junction's station stays (unless the fold retired it: the other
    // line's join station, torn off the kept line).
    for t in &report.torn_ties {
        if t.kind == TornTieKind::Junction && pa.xy(t.station).is_some() {
            let m = moved(t.station, t.station);
            assert!(norm(m) < 1e-6, "{what}: torn {t:?} moved {m:?}");
        }
    }
    // The gauge rule: where the view solves the joined survey (no datum
    // deciding), the piece's placement station stays (legacy: the
    // closure-adjusted view does not move).
    let gauge = before
        .placements
        .iter()
        .find(|g| piece(g.station) == piece(a));
    if let Some(g) = gauge.filter(|g| piece(a) == piece(b) && pa.xy(g.station).is_some()) {
        if solved_at(after, Some(g.station)) && report.kept_moves_because.is_none() {
            let m = moved(g.station, g.station);
            assert!(
                norm(m) < 1e-6,
                "{what}: the gauge {} moved {m:?}",
                g.station
            );
        }
    }
    false
}

/// What a sweep reached.
#[derive(Debug, Default)]
struct Reached {
    combined: usize,
    kinds: BTreeSet<String>,
    torn: usize,
    shifted: usize,
    kept_moved: usize,
    with_branches: usize,
    residual: usize,
    removed: usize,
    joined_moved: usize,
}

/// Combine `candidate` keeping `kept` on a copy of `p`, check it, count it.
fn combine_and_check(
    p: &Project,
    candidate: &CombineCandidate,
    kept: LineId,
    what: &str,
    reached: &mut Reached,
) -> Project {
    let (other, join) = if kept == candidate.line_a {
        (candidate.line_b, ends(candidate.end_a, candidate.end_b))
    } else {
        (candidate.line_a, ends(candidate.end_b, candidate.end_a))
    };
    let mut after = p.clone();
    let mut options = opts(join);
    options.name = format!("Joined {}", reached.combined);
    let result = after.combine_lines(kept, other, &options);
    assert!(result.is_ok(), "{what}: {result:?}");
    let report = result.unwrap().value;
    let plan = plan_keeping(candidate, kept);
    assert_eq!(&report, plan, "{what}: the command does what the menu said");
    let datum_pieces_joined = check_combine(p, &after, candidate, kept, &report, what);
    assert!(
        !datum_pieces_joined,
        "{what}: two datum-held pieces joined with no tear for a datum"
    );
    reached.combined += 1;
    reached.kinds.insert(format!("{:?}", candidate.join_kind));
    reached.torn += report.torn_ties.len();
    reached.shifted += usize::from(report.shift_m > 0.1);
    reached.kept_moved += usize::from(report.moving_line == Some(kept));
    reached.with_branches += usize::from(report.moved_lines.len() > 1);
    let residual = report.moving_line.is_some() && report.shift_m <= crate::COMBINE_TOLERANCE_M;
    reached.residual += usize::from(residual);
    reached.removed += report.closures_removed_across_one_shot.len();
    reached.joined_moved += usize::from(report.moved_with_joined_station.is_some());
    after
}

/// The candidates the station menu offers at every line end, unblocked;
/// refused only for two datums or a join in the middle of a line.
fn offered(p: &Project, what: &str) -> Vec<CombineCandidate> {
    let mut out = Vec::new();
    for end in line_ends(p) {
        for c in p.combine_candidates_at_point(end) {
            let Some(reason) = &c.blocked_reason else {
                out.push(c);
                continue;
            };
            let known = ["zero datum", "middle", "meet here"];
            assert!(
                known.iter().any(|k| reason.contains(k)),
                "{what} {end:?}: {reason}"
            );
        }
    }
    out
}

/// Every combine the station menu offers in random networks of each shape,
/// keeping either line, checked against the invariants (legacy
/// `every_offered_combine_in_random_networks_keeps_the_invariants` and its
/// `around_a_passage`, `beside_a_side_by_side_pair` and `at_a_hand_moved_join`
/// sweeps).
#[test]
fn every_offered_combine_in_random_networks_keeps_the_invariants() {
    for shape in SHAPES {
        let mut reached = Reached::default();
        for seed in 0..SEEDS {
            let p = network(seed, shape);
            for c in offered(&p, &format!("{shape:?} seed {seed}")) {
                for kept in [c.line_a, c.line_b] {
                    let what = format!("{shape:?} seed {seed}: keep {kept} ({c:?})");
                    combine_and_check(&p, &c, kept, &what, &mut reached);
                }
            }
        }
        let what = format!("{shape:?}: {reached:?}");
        assert!(reached.combined > SEEDS as usize, "{what}");
        assert!(
            reached.torn > 0 && reached.shifted > 0 && reached.kept_moved > 0,
            "{what}"
        );
        let reach = match shape {
            Shape::Plain => {
                let kinds = ["Merged", "CoLocated", "Closure"];
                reached.with_branches > 0 && kinds.iter().all(|k| reached.kinds.contains(*k))
            }
            Shape::Passage => reached.removed > 0,
            Shape::SideBySide => reached.residual > 0,
            Shape::HandMoved => reached.joined_moved > 0,
        };
        assert!(reach, "{what}");
    }
}

/// Random chains of combines (any pair the menu offers, either line kept),
/// Split Line Here at a random station and reverses, from each shape's
/// networks, each step checked (a combine as in the sweep; a split or a
/// reverse moves no station), so an edit also meets what earlier edits leave
/// (legacy `chains_of_combines_splits_and_reverses_keep_the_invariants` and
/// its passage, side-by-side and hand-moved chains).
#[test]
fn chains_of_combines_splits_and_reverses_keep_the_invariants() {
    let mut counts = [0usize; 3];
    let mut reached = Reached::default();
    for shape in SHAPES {
        for seed in 0..SEEDS / 2 {
            let mut rng = Rng(seed.wrapping_mul(0xA24B_AED4_963E_E407) | 1);
            let mut p = network(seed, shape);
            let mut trail: Vec<String> = Vec::new();
            for step in 0..8 {
                let what = format!("{shape:?} seed {seed} step {step} after {trail:?}");
                let before = p.raw_positions();
                let op = rng.below(5);
                if op <= 2 {
                    let offers = offered(&p, &what);
                    if offers.is_empty() {
                        continue;
                    }
                    let c = offers[rng.below(offers.len())].clone();
                    let kept = [c.line_a, c.line_b][rng.below(2)];
                    trail.push(format!("combine keep {kept} of {}-{}", c.line_a, c.line_b));
                    p = combine_and_check(&p, &c, kept, &what, &mut reached);
                    counts[0] += 1;
                    continue;
                }
                if op == 3 {
                    let long: Vec<&super::super::Line> =
                        p.lines().iter().filter(|l| l.shots.len() >= 2).collect();
                    if long.is_empty() {
                        continue;
                    }
                    let line = long[rng.below(long.len())];
                    let (id, at) = (line.id, 1 + rng.below(line.shots.len() - 1) as u32);
                    let options = super::SplitOptions {
                        name_a: format!("S{id}a{step}"),
                        name_b: format!("S{id}b{step}"),
                        color_a: "#123456".to_string(),
                        color_b: "#654321".to_string(),
                    };
                    let at_station = crate::SplitAt::Station { station_index: at };
                    ok(p.split_line(id, at_station, &options));
                    trail.push(format!("split({id} at {at})"));
                    counts[1] += 1;
                } else {
                    let id = p.lines()[rng.below(p.lines().len())].id;
                    ok(p.reverse_line(id, rng.below(2) == 0));
                    trail.push(format!("reverse({id})"));
                    counts[2] += 1;
                }
                assert_valid(&p);
                for (s, [x, y, _]) in p.raw_positions().iter() {
                    near(
                        [x, y],
                        xy(&before, s),
                        1e-6,
                        &format!("{what}: station {s}"),
                    );
                }
            }
        }
    }
    let [combines, splits, reverses] = counts;
    assert!(
        combines > SEEDS as usize && splits > 0 && reverses > 0,
        "{counts:?}"
    );
}
