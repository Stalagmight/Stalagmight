//! Legacy Combine Lines oracles from `project/combine_*_tests.rs` (#756
//! snaps, #755 hand moves at the join, #740 residuals, #736 realignment,
//! #749 closure dedupe) on the station graph. Fate of each legacy test:
//!
//! `combine_snap_tests`
//! - `the_issue_survey_is_one_a_reopen_already_settles`: adapted as
//!   `the_issue_survey_is_refused_and_the_sweep_settles_it` (a load never
//!   heals, plan 6.6: `validate` refuses it, the sweep, 5.5, settles it).
//! - `a_snap_into_a_part_a_zero_datum_holds_tears_for_the_datum`: ported.
//!
//! `combine_hand_move_tests`
//! - `a_30_cm_hand_move_carries_its_branch_as_planned_and_nothing_after`,
//!   `hand_moves_on_the_join_for_every_pairing` (640 combines): ported.
//! - `nothing_merged_at_the_dropped_join_station_moves_nothing`: ported;
//!   ignored (finding).
//! - `a_snap_between_what_moves_with_the_joined_station_and_its_side_tears`:
//!   adapted (reason `Moved`, legacy `Loop`).
//! - `hand_moves_at_the_float_dust_edge_leave_nothing_to_heal`: ported as
//!   two tests, `..._within_...` and `..._past_...` (ignored, finding).
//! - `a_merge_loop_at_a_hand_moved_join_tears_there_instead_of_pulling_apart`:
//!   adapted as `a_tie_loop_...` (the cycle-closing merge migrates to a
//!   `Tie`, plan 7.2.1); ignored (finding).
//! - `closures_whose_ends_the_hand_move_moves_apart_are_measured_again`: not
//!   applicable (closure metrics are derived, plan 1.3, 3.6).
//! - `a_combine_at_two_hand_moved_join_stations_moves_the_view_as_worked_out`:
//!   adapted (the Raw Data View half).
//!
//! `combine_residual_tests`
//! - `a_residual_moves_the_other_side_with_what_is_merged_to_it`,
//!   `a_zero_datum_decides_which_side_a_residual_moves`,
//!   `both_sides_held_tear_the_moving_sides_merge_for_a_residual`,
//!   `merged_ends_moved_by_hand_carry_their_residual_too`,
//!   `a_branch_at_merged_ends_moved_by_hand_stays_on_the_joined_station`:
//!   ported.
//! - `the_adjustment_anchor_decides_which_side_a_residual_moves`,
//!   `a_residual_of_float_dust_moves_nothing_as_before`,
//!   `a_residual_tears_a_snap_from_the_side_that_moves`: adapted (see each).
//!
//! `combine_realign_tests`
//! - `a_torn_snap_the_shift_recloses_across_the_two_sides_...` (adapted) and
//!   `..._inside_the_combined_line_...` (ported): ignored (one finding).
//! - `a_torn_snap_a_merged_join_recloses_beside_it_snaps_again`: ported.
//! - `a_snap_a_dropped_hand_move_pulls_apart_becomes_an_ordinary_closure`:
//!   adapted (reason `Moved`, legacy `JoinedStationMoves`).
//!
//! `combine_dedupe_tests`
//! - `the_closure_dedupe_keeps_the_first_closure_the_solve_takes`: adapted
//!   (through a combine: the rule is private to `combine/apply.rs`).
//! - `the_closure_dedupe_drops_by_position_so_a_repeated_id_keeps_its_twin`:
//!   adapted as `a_repeated_closure_id_is_a_violation` (invariant 1);
//!   `the_load_gives_a_repeated_closure_id_a_fresh_one`: not applicable (a
//!   load never re-mints, plan 6.6).
//! - `a_merge_torn_onto_a_legacy_closure_leaves_the_closure_the_solve_takes`:
//!   adapted (reason `PassesTwice`, plan 5.4 step 6); its gauge half,
//!   `..._keeps_the_gauge_where_b_started`: ignored (finding).
//! - `a_snapped_and_an_ordinary_closure_made_identical_keep_the_ordinary_one`:
//!   adapted (gauge on K:1).
//! - `a_snap_the_combine_tears_replaces_a_legacy_closure_on_its_stations`:
//!   ported.
//!
//! Declared changes throughout (plan 1.3, 3.7, 5.1, 5.2, 5.4, 5.6, 12.2): a
//! merge is one station (`Junction` for `Merge`); a torn junction detaches
//! the moving line onto a new station; hand moves are per line; alignment is
//! derived (`SnappedClosure` for `FormerSnappedClosure`); the gauge is the
//! `Placement` station; nothing derived is stored (no `realign_merges`,
//! re-drive, line offsets or held gauges; a reopen is a JSON round trip).
//! The Closures Computed View checks wait for stream M3.

use crate::{
    CombineJoinEnds, CombineJoinKind, CombineLinesOptions, CombinePick, KeptMovesReason, LineEnd,
    LineId, ShotData, StationId, StationRef,
};

use super::super::test_helpers::{gps, shot};
use super::super::{ClosureId, ClosureStrategy, EditContext, LoopClosure, Project, Violation};
use super::edit_test_helpers::{assert_valid, empty, line, line_at, line_from, merge, near, ok};
use super::edit_test_helpers::{st, xy};
use super::{CombineCandidate, CombineReport, TornTieKind as Kind, TornTieReason as Reason};

/// Below this in each axis a gap is float dust (legacy `OFFSET_DUST_M`).
const DUST: f64 = 1e-6;
const NONE: [f64; 3] = [0.0, 0.0, 0.0];
const ZERO: Xy = [0.0, 0.0];
const LEGACY: ClosureStrategy = ClosureStrategy::ProportionalByLength;
const EQUAL: ClosureStrategy = ClosureStrategy::EqualDistribution;
const BATCH: ClosureStrategy = ClosureStrategy::Batch;
const LSQ: ClosureStrategy = ClosureStrategy::LeastSquaresWeighted;
const MOVE: ClosureStrategy = ClosureStrategy::Move;
const START: LineEnd = LineEnd::Start;
const END: LineEnd = LineEnd::End;

type Xy = [f64; 2];
type Slot = (LineId, usize);
type Tear = (Kind, Reason, StationId, StationRef, StationRef);
type Swap = (ClosureId, ClosureStrategy, ClosureId, ClosureStrategy);
type Run = (CombineCandidate, CombineReport, Project);

/// `a` when `first`, else `b`.
fn either<T>(first: bool, a: T, b: T) -> T {
    [b, a].into_iter().nth(usize::from(first)).unwrap()
}

/// Flat shots `(azimuth, length)`, uuids `"{tag}-{i}"`.
fn flat(tag: &str, legs: &[(f64, f64)]) -> Vec<ShotData> {
    let legs: Vec<_> = legs.iter().map(|&(a, l)| (a, l, 0.0, 0.0)).collect();
    deep(tag, &legs)
}

/// Shots `(azimuth, length, depth_in, depth_out)`, uuids `"{tag}-{i}"`.
fn deep(tag: &str, legs: &[(f64, f64, f64, f64)]) -> Vec<ShotData> {
    let shot = |(i, &(a, l, d_in, d_out)): (usize, _)| ShotData {
        uuid: Some(format!("{tag}-{i}")),
        ..shot(a, l, d_in, d_out)
    };
    legs.iter().enumerate().map(shot).collect()
}

fn toward(tag: &str, from: Xy, to: Xy) -> ShotData {
    let azimuth = (to[0] - from[0]).atan2(to[1] - from[1]).to_degrees();
    flat(tag, &[(azimuth.rem_euclid(360.0), dist(from, to))]).remove(0)
}

fn dist(a: Xy, b: Xy) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn plus(a: Xy, b: Xy) -> Xy {
    [a[0] + b[0], a[1] + b[1]]
}

fn minus(a: Xy, b: Xy) -> Xy {
    [a[0] - b[0], a[1] - b[1]]
}

fn pos(p: &Project, s: StationId) -> Xy {
    xy(&p.raw_positions(), s)
}

fn at(p: &Project, line: LineId, i: usize) -> Xy {
    pos(p, st(p, line, i))
}

fn slot(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

/// K: east 10 m, then 80 degrees 5 m, from (100, -40).
fn k_line(p: &mut Project) -> LineId {
    let legs = flat("k", &[(90.0, 10.0), (80.0, 5.0)]);
    line_at(p, "K", [100.0, -40.0], legs)
}

/// O: north 4 m, then 300 degrees 6 m, from `start`.
fn o_line(p: &mut Project, start: Xy) -> LineId {
    line_at(p, "O", start, flat("o", &[(0.0, 4.0), (300.0, 6.0)]))
}

/// A one-shot line `name` from station `from` to where `to` is.
fn bridge(p: &mut Project, name: &str, from: StationId, to: StationId) -> LineId {
    let shot = toward(name, pos(p, from), pos(p, to));
    line_from(p, name, from, vec![shot])
}

/// A hand move on `line` at `station`, as a migrated file carries it (plan
/// 5.6). The piece's anchor stays.
fn hand(p: &mut Project, line: LineId, station: StationId, h: [f64; 3]) {
    if h != NONE {
        p.line_mut(line).unwrap().hand_moves.insert(station, h);
    }
}

/// Move the free piece of `s` so `s` is shown at `target`.
fn place(p: &mut Project, s: StationId, target: Xy) {
    let now = pos(p, s);
    let index = {
        let t = p.topology();
        let piece = t.component_of(s);
        let mut all = p.placements.iter();
        all.position(|x| t.component_of(x.station) == piece)
            .unwrap()
    };
    let placement = &mut p.placements[index];
    if placement.station == s {
        (placement.x, placement.y) = (target[0], target[1]);
    } else {
        placement.x += target[0] - now[0];
        placement.y += target[1] - now[1];
    }
}

/// Hold `s`'s piece with a zero datum where it is (the legacy tests pushed
/// one). The first datum is the frame and translates the survey rigidly
/// (plan 4.3); a later one holds its piece by a declared correction (D4).
fn hold(p: &mut Project, s: StationId) {
    let before = p.anchor_snapshot();
    let datum = super::super::ZeroDatum::new(s, gps(20.0, -87.0));
    let i = p.push_datum(datum);
    let mut ctx = EditContext::none();
    if before.frame().is_some() {
        let at = before.position(s).unwrap();
        p.datums[i].correction = p.correction_to_hold(i, at);
        ctx = ctx.with_corrected(i);
    }
    p.settle_anchors(&before, &ctx);
    assert_valid(p);
}

fn tie(p: &mut Project, a: Slot, b: Slot, strategy: ClosureStrategy) -> ClosureId {
    let (sa, sb) = (st(p, a.0, a.1), st(p, b.0, b.1));
    p.push_closure(sa, Some(a.0), sb, Some(b.0), strategy)
}

/// A loop closure with a given id, as a file carries it.
fn plant(p: &mut Project, a: Slot, b: Slot, strategy: ClosureStrategy, id: ClosureId) {
    tie(p, a, b, strategy);
    p.closures.last_mut().unwrap().id = id;
    p.counters.next_closure_id = p.counters.next_closure_id.max(id + 1);
}

fn closure(p: &Project, id: ClosureId) -> LoopClosure {
    *p.closures.iter().find(|c| c.id == id).unwrap()
}

/// Is closure `id` a snap (an aligned `Move` closure, plan 3.6) now?
fn snapped(p: &Project, id: ClosureId) -> bool {
    let c = closure(p, id);
    c.strategy == MOVE && p.closure_is_aligned(&p.raw_positions(), &c)
}

fn between(p: &Project, a: StationId, b: StationId) -> Vec<(ClosureId, ClosureStrategy)> {
    let on = |c: &&LoopClosure| (c.a, c.b) == (a, b) || (c.a, c.b) == (b, a);
    p.closures
        .iter()
        .filter(on)
        .map(|c| (c.id, c.strategy))
        .collect()
}

fn gap(p: &Project, id: ClosureId) -> f64 {
    let c = closure(p, id);
    dist(pos(p, c.a), pos(p, c.b))
}

fn ends(kept: LineEnd, other: LineEnd) -> CombineJoinEnds {
    CombineJoinEnds { kept, other }
}

fn options(join: Option<CombineJoinEnds>) -> CombineLinesOptions {
    let (name, color) = ("Joined".to_string(), "#abcdef".to_string());
    let (comment_pick, name_pick) = (CombinePick::Kept, CombinePick::Kept);
    let swap_lr = true;
    CombineLinesOptions {
        name,
        color,
        swap_lr,
        comment_pick,
        name_pick,
        join,
    }
}

fn plan_for(c: &CombineCandidate, kept: LineId) -> &CombineReport {
    either(c.line_a == kept, &c.plan_keeping_a, &c.plan_keeping_b)
}

fn torn(plan: &CombineReport) -> Vec<Tear> {
    let tie = |t: &super::TornTie| (t.kind, t.reason, t.station, t.slot, t.other_slot);
    plan.torn_ties.iter().map(tie).collect()
}

fn tear_of(plan: &CombineReport, id: ClosureId) -> Option<(Kind, Reason)> {
    let mut all = plan.torn_ties.iter();
    all.find(|t| t.closure == Some(id))
        .map(|t| (t.kind, t.reason))
}

fn replaced(plan: &CombineReport) -> Vec<Swap> {
    let r = |r: &super::ReplacedClosure| (r.dropped.id, r.dropped.strategy, r.by.id, r.by.strategy);
    plan.closures_replaced.iter().map(r).collect()
}

/// Does `line` carry a datum on one of its own stations?
fn carries_datum(p: &Project, line: LineId) -> bool {
    let t = p.topology();
    let own = |d: &&super::super::ZeroDatum| d.via.is_none_or(|v| v == line);
    p.datums
        .iter()
        .filter(own)
        .any(|d| t.slot_on(d.station, line).is_some())
}

fn junctions(p: &Project) -> usize {
    let t = p.topology();
    t.stations().iter().filter(|s| t.is_junction(**s)).count()
}

fn lengths(p: &Project, lines: &[LineId]) -> Vec<f64> {
    let of = |id: &LineId| p.line(*id).unwrap().shots.iter().map(|s| s.length);
    let mut all: Vec<f64> = lines.iter().flat_map(of).collect();
    all.sort_by(f64::total_cmp);
    all
}

/// The file holds no derived state (plan 1.3, 6.6): a JSON round trip lays
/// every station out on the same bits.
fn reopen_moves_nothing(p: &Project, what: &str) {
    let json = serde_json::to_string(p).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_valid(&back);
    let now = back.raw_positions();
    for (s, [x, y, _]) in p.raw_positions().iter() {
        let [bx, by] = now.xy(s).unwrap();
        assert!(bx == x && by == y, "{what}: station {s} moved");
    }
}

/// Combine `other` into `kept` at `join` as the dialog would: the candidate,
/// the command, its report equal to the plan offered, [`check_combined`].
fn combine_checked(
    p: &Project,
    kept: LineId,
    other: LineId,
    join: Option<CombineJoinEnds>,
    what: &str,
) -> Run {
    let c = p.combine_candidate_joining(kept, other, join).unwrap();
    assert_eq!(c.blocked_reason, None, "{what}");
    let mut after = p.clone();
    let report = ok(after.combine_lines(kept, other, &options(join)));
    assert_eq!(&report, plan_for(&c, kept), "{what}: as planned");
    check_combined(p, &after, (&c, &report), kept, what);
    (c, report, after)
}

/// The legacy `check_combined` minus the Closures Computed View: every line
/// moves as the plan says (by the shift, with the joined station by the
/// dropped hand move's gap, #755, or not at all); nothing moved holds a datum.
fn check_combined(
    before: &Project,
    after: &Project,
    (c, plan): (&CombineCandidate, &CombineReport),
    kept: LineId,
    what: &str,
) {
    let a = c.line_a == kept;
    let other = either(a, c.line_b, c.line_a);
    let (k_slot, o_slot) = either(a, (c.slot_a, c.slot_b), (c.slot_b, c.slot_a));
    let (k_end, o_end) = either(a, (c.end_a, c.end_b), (c.end_b, c.end_a));
    let kept_moves = plan.moving_line == Some(kept);
    let because = plan.kept_moves_because.is_some();
    assert_eq!(because, kept_moves, "{what}: {plan:?}");
    let (old, new) = (before.raw_positions(), after.raw_positions());
    let was = |s: StationRef| xy(&old, before.station_at(s).unwrap());
    let hand_at = |s: StationRef| {
        let h = before.line(s.line_id).unwrap().hand(was_station(before, s));
        [h[0], h[1]]
    };
    let chain = |s: StationRef| minus(was(s), hand_at(s));
    let gap = minus(chain(k_slot), chain(o_slot));
    let shift = match plan.moving_line {
        None => ZERO,
        Some(_) => either(kept_moves, minus(ZERO, gap), gap),
    };
    let off = (dist(shift, ZERO) - plan.shift_m).abs();
    assert!(off < 1e-6, "{what}: shift {shift:?}");
    let shots = |l: LineId| before.line(l).unwrap().shots.len();
    let (nk, no) = (shots(kept), shots(other));
    let offset = either(k_end == END, 0, no);
    let dropped = either(kept_moves, k_slot, o_slot);
    let carried = hand_at(k_slot) != hand_at(o_slot);
    let line = after.line_stations(kept).unwrap();
    assert_eq!(line.len(), nk + no + 1, "{what}");
    let k_index = k_slot.station_index as usize + offset;
    let toward = minus(xy(&new, line[k_index]), was(dropped));
    let joined = plan.moved_with_joined_station.as_ref();
    let carries: Vec<LineId> = joined.map(|j| j.lines.clone()).unwrap_or_default();
    if let Some(j) = joined {
        let merged = c.join_kind == CombineJoinKind::Merged;
        assert!(carried && !merged, "{what}: {j:?}");
        assert_eq!(j.station, was_station(before, dropped), "{what}");
        let off = (dist(toward, ZERO) - j.shift_m).abs();
        assert!(off < 1e-6, "{what}: the joined station moved {toward:?}");
    }
    let moved = |l: LineId| either(plan.moved_lines.contains(&l), shift, ZERO);
    for l in after.lines().iter().filter(|l| l.id != kept) {
        let d = either(carries.contains(&l.id), toward, moved(l.id));
        let w = before.line_stations(l.id).unwrap();
        let n = after.line_stations(l.id).unwrap();
        assert_eq!(w.len(), n.len(), "{what}");
        for (i, (&w, &n)) in w.iter().zip(n).enumerate() {
            let label = format!("{what}: line {} station {i}", l.id);
            near(xy(&new, n), plus(xy(&old, w), d), 1e-6, &label);
        }
    }
    // A plan that moves nothing leaves at most float dust at the join.
    let tol = either(plan.moving_line.is_none(), 2.0 * DUST, 1e-6);
    let mut want = Vec::new();
    for i in 0..=nk {
        let s = slot(kept, i as u32);
        want.push((s, i + offset, moved(kept), 1e-6));
    }
    let base = either(k_end == END, nk, 0);
    for i in 0..=no {
        let along = either(k_end == o_end, no - i, i);
        want.push((slot(other, i as u32), along + base, moved(other), tol));
    }
    for (s, index, d, tol) in want {
        if !(carried && dropped == s) {
            let label = format!("{what}: {s:?} at combined {index}");
            near(xy(&new, line[index]), plus(was(s), d), tol, &label);
        }
    }
    for id in plan.moved_lines.iter().chain(&carries) {
        assert!(!carries_datum(before, *id), "{what}: moved a datum");
    }
    assert_valid(after);
}

fn was_station(p: &Project, s: StationRef) -> StationId {
    p.station_at(s).unwrap()
}

// ----- combine_snap_tests -----------------------------------------------

fn unplaced(v: Violation) -> Option<ClosureId> {
    match v {
        Violation::ClosureAcrossUnplacedPieces { closure, .. } => Some(closure),
        _ => None,
    }
}

/// The #756 survey: K's end and O's start 5 mm apart, tied by a closure; N
/// at K's start, M at O's end; Z snapped to O:1, W at Z's end tied to M.
#[test]
fn the_issue_survey_is_refused_and_the_sweep_settles_it() {
    let mut p = empty();
    let k = k_line(&mut p);
    let k2 = at(&p, k, 2);
    let o = o_line(&mut p, [k2[0] + 0.004, k2[1] - 0.003]);
    let (k0, o2, o1_at) = (st(&p, k, 0), st(&p, o, 2), at(&p, o, 1));
    line_from(&mut p, "N", k0, flat("n", &[(0.0, 3.0)]));
    let m = line_from(&mut p, "M", o2, flat("m", &[(90.0, 3.0)]));
    let z = line_at(&mut p, "Z", o1_at, flat("z", &[(180.0, 5.0)]));
    let z1 = st(&p, z, 1);
    let w = line_from(&mut p, "W", z1, flat("w", &[(90.0, 4.0)]));
    let snap = tie(&mut p, (o, 1), (z, 0), MOVE);
    let w_to_m = tie(&mut p, (w, 1), (m, 1), BATCH);
    let join = tie(&mut p, (k, 2), (o, 0), BATCH);
    let refused: Vec<ClosureId> = p.validate().into_iter().filter_map(unplaced).collect();
    assert_eq!(refused, [snap, w_to_m, join], "two free pieces each");
    let report = p.sweep_orphan_closures();
    let promoted: Vec<ClosureId> = report.promoted.iter().map(|x| x.closure.id).collect();
    assert_eq!(promoted, [snap, join], "these become merges");
    assert!(report.removed.is_empty());
    let left: Vec<_> = p.closures.iter().map(|c| (c.id, c.strategy)).collect();
    assert_eq!(left, [(w_to_m, BATCH)], "W's closure to M stays");
    assert_valid(&p);
    assert!(p.sweep_orphan_closures().is_empty(), "a second sweep");
}

/// Datums hold both sides (N's end on K's, D's end on O's); K's end and O's
/// start 5 mm apart, tied; D from O's end back to O:1, snapped there.
#[test]
fn a_snap_into_a_part_a_zero_datum_holds_tears_for_the_datum() {
    let mut p = empty();
    let k = k_line(&mut p);
    let k2 = at(&p, k, 2);
    let o = o_line(&mut p, [k2[0] + 0.004, k2[1] - 0.003]);
    let (k0, o1, o2) = (st(&p, k, 0), st(&p, o, 1), st(&p, o, 2));
    let n = line_from(&mut p, "N", k0, flat("n", &[(0.0, 3.0)]));
    let d = bridge(&mut p, "D", o2, o1);
    let (n1, d1) = (st(&p, n, 1), st(&p, d, 1));
    hold(&mut p, n1);
    hold(&mut p, d1);
    tie(&mut p, (k, 2), (o, 0), BATCH);
    let snap = tie(&mut p, (o, 1), (d, 1), MOVE);
    assert!(snapped(&p, snap), "setup: a snap");
    let settled = p.clone().sweep_orphan_closures().is_empty();
    assert!(settled, "a survey the app leaves");
    // Keeping K, O leaves D's junction and the snap into D's part tears, both
    // for D's datum (the dialog names D as the line that stays).
    let (c, plan, mut after) = combine_checked(&p, k, o, Some(ends(END, START)), "keep K");
    assert_eq!(c.join_kind, CombineJoinKind::Closure);
    assert_eq!((plan.moving_line, plan.moved_lines), (Some(o), vec![o]));
    let zero = Reason::ZeroDatum;
    let junction = (Kind::Junction, zero, o2, slot(o, 2), slot(d, 0));
    let snap_tear = (Kind::SnappedClosure, zero, o1, slot(o, 1), slot(d, 1));
    assert_eq!(torn(plan_for(&c, k)), [junction, snap_tear]);
    assert_eq!(closure(&after, snap).strategy, BATCH);
    assert!(after.sweep_orphan_closures().is_empty(), "settled");
    // Keeping O, K leaves N's junction; the snap stays inside O's side.
    let (_, plan, after) = combine_checked(&p, o, k, Some(ends(START, END)), "keep O");
    assert_eq!(plan.moving_line, Some(k));
    let junction = (Kind::Junction, zero, k0, slot(k, 0), slot(n, 0));
    assert_eq!(torn(&plan), [junction]);
    assert!(snapped(&after, snap), "keep O: the snap stays");
}

// ----- combine_hand_move_tests ------------------------------------------

/// Two lines meeting at a join, T at O's join station and U at K's (legacy
/// `Fixture`); slots are pre-combine. Line ids: K 0, O 1, T 2, U 3.
struct Join {
    p: Project,
    k: LineId,
    o: LineId,
    t: LineId,
    u: LineId,
    k_end: LineEnd,
    o_end: LineEnd,
}

fn end_index(p: &Project, line: LineId, end: LineEnd) -> usize {
    either(end == START, 0, p.line(line).unwrap().shots.len())
}

/// K and O meet at `ends`: one station (each line keeps its hand move) or
/// side by side, chain positions on one point. T and U start on the join
/// stations (T free when `t_on` is false). Every end reads its own depth.
fn join_fixture(ends: (LineEnd, LineEnd), merged: bool, hands: [[f64; 3]; 2], t_on: bool) -> Join {
    let ([hand_k, hand_o], (k_end, o_end)) = (hands, ends);
    let mut p = empty();
    let k_legs = deep("k", &[(90.0, 10.0, 1.0, 2.0), (70.0, 5.0, 2.0, 2.5)]);
    let k = line_at(&mut p, "K", [100.0, -40.0], k_legs);
    let o_legs = deep("o", &[(0.0, 4.0, 3.0, 3.5), (300.0, 6.0, 3.5, 4.5)]);
    let o = line_at(&mut p, "O", ZERO, o_legs);
    let (ik, io) = (end_index(&p, k, k_end), end_index(&p, o, o_end));
    let (jk, jo) = (st(&p, k, ik), st(&p, o, io));
    hand(&mut p, k, jk, hand_k);
    hand(&mut p, o, jo, hand_o);
    let shown_k = pos(&p, jk);
    let jo = if merged {
        place(&mut p, jo, shown_k);
        merge(&mut p, jo, jk);
        jk
    } else {
        let chain_k = minus(shown_k, [hand_k[0], hand_k[1]]);
        place(&mut p, jo, plus(chain_k, [hand_o[0], hand_o[1]]));
        jo
    };
    let t_legs = deep("t", &[(180.0, 3.0, 6.0, 6.5)]);
    let t = if t_on {
        line_from(&mut p, "T", jo, t_legs)
    } else {
        let shown = pos(&p, jo);
        line_at(&mut p, "T", shown, t_legs)
    };
    let u = line_from(&mut p, "U", jk, deep("u", &[(225.0, 2.0, 8.0, 8.4)]));
    assert_valid(&p);
    Join {
        p,
        k,
        o,
        t,
        u,
        k_end,
        o_end,
    }
}

fn end_reading(p: &Project, s: StationRef) -> f64 {
    let line = p.line(s.line_id).unwrap();
    match s.station_index as usize {
        0 => line.shots[0].depth_in,
        i => line.shots[i - 1].depth_out,
    }
}

/// K runs 10 m east from the origin; O (`legs`) is shown starting at (`x`,
/// 0), its start hand-moved `h` east; T (`t_legs`) starts on O's start.
fn hand_moved_start(
    x: f64,
    h: f64,
    legs: &[(f64, f64)],
    t: &[(f64, f64)],
) -> (Project, [LineId; 3]) {
    let mut p = empty();
    let k = line(&mut p, "K", flat("k", &[(90.0, 10.0)]));
    let o = line_at(&mut p, "O", [x, 0.0], flat("o", legs));
    let o0 = st(&p, o, 0);
    hand(&mut p, o, o0, [h, 0.0, 0.0]);
    let t = line_from(&mut p, "T", o0, flat("t", t));
    (p, [k, o, t])
}

/// K's end and O's start side by side, O's start shown 0.3 m east by its
/// hand move, T on it: T moves with the joined station by 0.3 m and the plan
/// says so; O's other stations stay (chain gap 0); no shot or hand move left.
#[test]
fn a_30_cm_hand_move_carries_its_branch_as_planned_and_nothing_after() {
    let (p, [k, o, t]) = hand_moved_start(10.3, 0.3, &[(90.0, 10.0)], &[(0.0, 5.0)]);
    let o0 = st(&p, o, 0);
    let c = p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(c.join_kind, CombineJoinKind::CoLocated);
    assert_eq!(c.branches, [t]);
    let plan = &c.plan_keeping_a;
    assert_eq!(plan.moving_line, Some(o));
    assert_eq!(plan.moved_lines, [o], "T is not moved by the shift");
    assert!(plan.shift_m < 1e-12, "the chain gap is 0: {}", plan.shift_m);
    let joined = plan.moved_with_joined_station.clone().unwrap();
    assert_eq!((joined.station, joined.lines), (o0, vec![t]));
    assert!((joined.shift_m - 0.3).abs() < 1e-12, "{}", joined.shift_m);
    assert!((plan.join_offset_change_m.unwrap() - 0.3).abs() < 1e-12);
    let (_, _, after) = combine_checked(&p, k, o, None, "0.3 m");
    let wants = [(k, 0, ZERO), (k, 1, [10.0, 0.0]), (k, 2, [20.0, 0.0])];
    let t_wants = [(t, 0, [10.0, 0.0]), (t, 1, [10.0, 5.0])];
    for (line, i, want) in wants.into_iter().chain(t_wants) {
        near(at(&after, line, i), want, 1e-9, &format!("{line}:{i}"));
    }
    assert_eq!(st(&after, t, 0), st(&after, k, 1), "T on the join");
    assert_eq!(lengths(&after, &[k]), [10.0, 10.0], "no shot changes");
    let no_hand = after.lines().iter().all(|l| l.hand_moves.is_empty());
    assert!(no_hand, "the joined station keeps K's hand move: none");
    reopen_moves_nothing(&after, "0.3 m");
}

/// Hand moves: under 1 mm, float dust, 0.3 m, 0.2 m in depth, both ways.
const HANDS: [(&str, [f64; 3]); 5] = [
    ("0.6 mm", [0.0006, 0.0, 0.0]),
    ("dust", [4e-7, -3e-7, 0.0]),
    ("0.3 m", [0.21, -0.2142, 0.0]),
    ("depth", [0.0, 0.0, 0.2]),
    ("both ways", [-0.12, 0.07, -0.15]),
];

fn second(h: [f64; 3]) -> [f64; 3] {
    [0.5 * h[1] - 0.0003, -0.5 * h[0], -0.5 * h[2]]
}

/// Every pairing of the ends, merged or side by side, either line kept, T's
/// side free or held, a hand move on the kept join slot, the other, both
/// (different) or both the same, of each size in [`HANDS`]: [`check_one`].
#[test]
fn hand_moves_on_the_join_for_every_pairing() {
    let mut combined = 0;
    let pairs = [(START, START), (START, END), (END, START), (END, END)];
    let shapes = [(false, false), (false, true), (true, false), (true, true)];
    for (k_end, o_end) in pairs {
        for (merged, held) in shapes {
            for (size, h) in HANDS {
                let on = [("K", h, NONE), ("O", NONE, h), ("both", h, second(h))];
                for (on, hand_k, hand_o) in on.into_iter().chain([("same", h, h)]) {
                    for keep_k in [true, false] {
                        let what = format!(
                            "K {k_end:?}, O {o_end:?}, merged {merged}, held {held}, \
                             {size} on {on}, keep K {keep_k}"
                        );
                        let hands = [hand_k, hand_o];
                        let mut j = join_fixture((k_end, o_end), merged, hands, true);
                        if held {
                            let t1 = st(&j.p, j.t, 1);
                            hold(&mut j.p, t1);
                        }
                        check_one(&j, keep_k, hands, &what);
                        combined += 1;
                    }
                }
            }
        }
    }
    assert_eq!(combined, 640);
}

/// One combine of the sweep: the plan holds; T and U stay on the joined
/// station, which keeps the fixed side's hand move (nothing else has one);
/// depths are the four readings' mean; no shot changes; a reopen is still.
fn check_one(j: &Join, keep_k: bool, [hand_k, hand_o]: [[f64; 3]; 2], what: &str) {
    let p = &j.p;
    let join_k = slot(j.k, end_index(p, j.k, j.k_end) as u32);
    let join_o = slot(j.o, end_index(p, j.o, j.o_end) as u32);
    let (kept, other) = either(keep_k, (j.k, j.o), (j.o, j.k));
    let join = either(keep_k, ends(j.k_end, j.o_end), ends(j.o_end, j.k_end));
    let (c, plan, after) = combine_checked(p, kept, other, Some(join), what);
    let merged = c.join_kind == CombineJoinKind::Merged;
    let same = p.station_at(join_k) == p.station_at(join_o);
    assert_eq!(merged, same, "{what}");
    // The dropped join slot: the moving side's, the other's when none moves.
    let dropped_is_k = (plan.moving_line == Some(kept)) == keep_k;
    let k_side = (join_k, hand_o, hand_k);
    let (dropped, fixed, lost) = either(dropped_is_k, k_side, (join_o, hand_k, hand_o));
    let apart = |i: usize| (hand_k[i] - hand_o[i]).abs() > DUST;
    let carried = !merged && (apart(0) || apart(1));
    let branch = either(dropped_is_k, j.u, j.t);
    let station = was_station(p, dropped);
    let want = carried.then(|| (station, vec![branch]));
    let joined_move = plan.moved_with_joined_station.as_ref();
    let got = joined_move.map(|m| (m.station, m.lines.clone()));
    assert_eq!(got, want, "{what}");
    let sq = |i: usize| (fixed[i] - lost[i]).powi(2);
    let noted = !merged && (sq(0) + sq(1) + sq(2)).sqrt() > 0.001;
    assert_eq!(plan.join_offset_change_m.is_some(), noted, "{what}");
    let kept_slot = either(keep_k, join_k, join_o);
    let other_shots = p.line(other).unwrap().shots.len();
    let offset = either(join.kept == END, 0, other_shots);
    let index = kept_slot.station_index as usize + offset;
    let joined = after.line_stations(kept).unwrap()[index];
    assert_eq!(after.line(j.t).unwrap().start, joined, "{what}: T");
    assert_eq!(after.line(j.u).unwrap().start, joined, "{what}: U");
    let all = after.lines().iter().flat_map(|l| l.hand_moves.values());
    let hands: Vec<[f64; 3]> = all.copied().collect();
    assert_eq!(hands, either(fixed == NONE, vec![], vec![fixed]), "{what}");
    assert_eq!(after.line(kept).unwrap().hand(joined), fixed, "{what}");
    let (t0, u0) = (slot(j.t, 0), slot(j.u, 0));
    let readings = [join_k, join_o, t0, u0].map(|s| end_reading(p, s));
    let mean = readings.iter().sum::<f64>() / 4.0;
    let positions = after.raw_positions();
    let joined_slot = slot(kept, index as u32);
    for (s, want) in [(joined_slot, mean + fixed[2]), (t0, mean), (u0, mean)] {
        let z = positions.slot(&after, s).unwrap()[2];
        assert!((z - want).abs() < 1e-9, "{what}: {s:?} at depth {z}");
    }
    let (now, was) = (lengths(&after, &[kept]), lengths(p, &[j.k, j.o]));
    assert_eq!(now, was, "{what}: no shot changes");
    reopen_moves_nothing(&after, what);
}

/// The hand moves differ (K's end 0.3 m) but nothing is on the dropped join
/// station (O's start; T is free); U is on K's end, which keeps its 0.3 m.
/// Nothing moves, and a snap from O:1 to Z (along O, ending on O's end) stays.
#[test]
fn nothing_merged_at_the_dropped_join_station_moves_nothing() {
    let mut j = join_fixture((END, START), false, [[0.3, 0.0, 0.0], NONE], false);
    let (k, o, p) = (j.k, j.o, &mut j.p);
    let (o1_at, o2) = (at(p, o, 1), st(p, o, 2));
    let z = line_at(p, "Z", o1_at, deep("z", &[(300.0, 6.0, 3.5, 4.5)]));
    let z1 = st(p, z, 1);
    merge(p, z1, o2);
    let snap = tie(p, (o, 1), (z, 0), MOVE);
    assert!(snapped(p, snap), "setup: a snap");
    let before = p.raw_positions();
    let what = "dropped, unmerged";
    let (_, plan, after) = combine_checked(p, k, o, Some(ends(END, START)), what);
    assert!(snapped(&after, snap), "the snap stays");
    for (s, [x, y, _]) in after.raw_positions().iter() {
        if let Some(was) = before.xy(s) {
            near([x, y], was, 1e-12, &format!("station {s}"));
        }
    }
    assert!((plan.join_offset_change_m.unwrap() - 0.3).abs() < 1e-12);
    assert!(plan.torn_ties.is_empty(), "{:?}", plan.torn_ties);
    assert_eq!(plan.moved_with_joined_station, None);
    assert_eq!(plan.moving_line, None, "{plan:?}");
    assert!(plan.moved_lines.is_empty(), "{plan:?}");
}

/// The 0.3 m shape with C off T's end and Z off C's, Z's end snapped to O:1:
/// T, C and Z move with the joined station while O stays, so the snap
/// becomes an ordinary closure 0.3 m long (reason `Moved`, legacy `Loop`).
#[test]
fn a_snap_between_what_moves_with_the_joined_station_and_its_side_tears() {
    let (mut p, [k, o, t]) = hand_moved_start(10.3, 0.3, &[(90.0, 10.0)], &[(0.0, 5.0)]);
    let t1 = st(&p, t, 1);
    let c_line = line_from(&mut p, "C", t1, flat("c", &[(90.0, 9.7)]));
    let c1 = st(&p, c_line, 1);
    let z = line_from(&mut p, "Z", c1, flat("z", &[(180.0, 5.0)]));
    let z1 = st(&p, z, 1);
    near(pos(&p, z1), at(&p, o, 1), 1e-9, "setup");
    let snap = tie(&mut p, (z, 1), (o, 1), MOVE);
    assert!(snapped(&p, snap), "setup: a snap");
    let c = p.combine_candidate_for_lines(k, o).unwrap();
    let plan = &c.plan_keeping_a;
    assert_eq!(plan.moved_lines, [o]);
    let joined = plan.moved_with_joined_station.clone().unwrap();
    assert_eq!(joined.lines, [t, c_line, z], "T and what hangs off it");
    assert!((joined.shift_m - 0.3).abs() < 1e-12);
    let moved = Reason::Moved;
    let tear = (Kind::SnappedClosure, moved, z1, slot(z, 1), slot(o, 1));
    assert_eq!(torn(plan), [tear]);
    let (_, _, after) = combine_checked(&p, k, o, None, "apart");
    for (id, i) in [(t, 0), (t, 1), (c_line, 0), (c_line, 1), (z, 0), (z, 1)] {
        let want = minus(at(&p, id, i), [0.3, 0.0]);
        near(at(&after, id, i), want, 1e-9, &format!("{id}:{i}"));
    }
    assert_eq!(closure(&after, snap).strategy, BATCH, "an ordinary closure");
    assert!((gap(&after, snap) - 0.3).abs() < 1e-9);
    reopen_moves_nothing(&after, "apart");
}

/// The dust edge (#755): the chain gap 0.9 um east, O's start moved by hand
/// so the join stations as shown are 1.8 um apart (past dust) or 0.9 um the
/// other way (within): past dust T moves with the joined station.
fn dust_edge(what: &str, h: f64, carried: bool) {
    let x = 10.0 - 0.9 * DUST + h;
    let (p, [k, o, t]) = hand_moved_start(x, h, &[(90.0, 10.0)], &[(0.0, 5.0)]);
    let (_, plan, after) = combine_checked(&p, k, o, None, what);
    let joined = plan.moved_with_joined_station.as_ref();
    let lines = joined.map(|j| j.lines.clone());
    assert_eq!(lines, carried.then(|| vec![t]), "{what}: {plan:?}");
    assert_eq!(st(&after, t, 0), st(&after, k, 1), "{what}");
    reopen_moves_nothing(&after, what);
}

#[test]
fn hand_moves_past_the_float_dust_edge_carry_the_branch() {
    dust_edge("past dust", -0.9 * DUST, true);
}

#[test]
fn hand_moves_within_the_float_dust_edge_move_nothing() {
    dust_edge("within", 1.8 * DUST, false);
}

/// The legacy merge loop (T from O's hand-moved start to O's end) migrated:
/// a junction at O's start and a rigid `Tie` at O's end (plan 7.2.1, 3.6). T
/// stays with O, its tie at the join becoming a closure carrying the 0.3 m.
#[test]
#[ignore = "M1 known difference: a legacy merge loop (now a junction and a Tie) at a hand-moved join; T moves with the joined station and the Tie is made Batch, where today T stayed and its merge at the join tore (not representable: T would hang only by closures)"]
fn a_tie_loop_at_a_hand_moved_join_tears_there_instead_of_pulling_apart() {
    let legs = [(90.0, 10.0), (90.0, 10.0)];
    let (mut p, [k, o, t]) = hand_moved_start(10.3, 0.3, &legs, &[(90.0, 19.7)]);
    let o0 = st(&p, o, 0);
    near(at(&p, t, 1), at(&p, o, 2), 1e-9, "setup");
    tie(&mut p, (t, 1), (o, 2), ClosureStrategy::Tie);
    assert_valid(&p);
    let plan = p.combine_candidate_for_lines(k, o).unwrap().plan_keeping_a;
    let carried = &plan.moved_with_joined_station;
    assert_eq!(*carried, None, "T stays with O: {plan:?}");
    let tears = plan.torn_ties.iter().map(|t| (t.kind, t.reason, t.station));
    let want = (Kind::Junction, Reason::Loop, o0);
    assert_eq!(tears.collect::<Vec<_>>(), [want]);
    let (_, _, after) = combine_checked(&p, k, o, None, "tie loop");
    let (t_end, o_end) = (at(&after, t, 1), at(&after, k, 3));
    near(t_end, o_end, 1e-9, "T stays on O's end");
    let (t0, joined) = (st(&after, t, 0), st(&after, k, 1));
    assert_ne!(t0, joined, "T left the joined station");
    let torn_closure = after.closures.last().unwrap().id;
    assert_eq!(closure(&after, torn_closure).strategy, BATCH);
    assert!((gap(&after, torn_closure) - 0.3).abs() < 1e-9);
    reopen_moves_nothing(&after, "tie loop");
}

/// The Raw Data View half: K's end and O's start hand-moved by different
/// amounts, side by side (T moves with the joined station) or one station
/// (O moves by the chain gap, T stays); L closes a loop on K.
#[test]
fn a_combine_at_two_hand_moved_join_stations_moves_the_raw_view_as_worked_out() {
    let (hand_k, hand_o) = ([0.2, -0.1, 0.0], [-0.15, 0.25, 0.0]);
    for merged in [false, true] {
        let what = either(merged, "merged join", "side by side");
        let mut p = empty();
        let k = line(&mut p, "K", flat("k", &[(90.0, 10.0), (90.0, 10.0)]));
        let k0 = st(&p, k, 0);
        let legs = flat("l", &[(0.0, 1.0), (90.0, 10.2), (180.0, 1.0)]);
        let l = line_from(&mut p, "L", k0, legs);
        tie(&mut p, (l, 3), (k, 1), BATCH);
        let k_end = st(&p, k, 2);
        hand(&mut p, k, k_end, hand_k);
        let shown_k = [20.0 + hand_k[0], hand_k[1]];
        near(pos(&p, k_end), shown_k, 1e-9, what);
        let o_at = either(merged, shown_k, [20.0 + hand_o[0], hand_o[1]]);
        let o = line_at(&mut p, "O", o_at, flat("o", &[(0.0, 8.0)]));
        let o_start = st(&p, o, 0);
        hand(&mut p, o, o_start, hand_o);
        if merged {
            merge(&mut p, o_start, k_end);
        }
        let o_start = either(merged, k_end, o_start);
        let shown_o = pos(&p, o_start);
        let t = line_from(&mut p, "T", o_start, flat("t", &[(90.0, 6.0)]));
        let c = p.combine_candidate_for_lines(k, o).unwrap();
        assert_eq!(c.line_a, k, "{what}");
        assert_eq!(c.plan_keeping_a.moving_line, Some(o), "{what}");
        let (_, _, after) = combine_checked(&p, k, o, None, what);
        let t_at = either(merged, shown_o, shown_k);
        let k_wants = [(k, 0, ZERO), (k, 1, [10.0, 0.0]), (k, 2, shown_k)];
        let wants = [
            (k, 3, [20.0, 8.0]),
            (t, 0, t_at),
            (t, 1, plus(t_at, [6.0, 0.0])),
        ];
        for (line, i, want) in k_wants.into_iter().chain(wants) {
            let label = format!("{what}: {line}:{i}");
            near(at(&after, line, i), want, 1e-9, &label);
        }
    }
}

// ----- combine_residual_tests -------------------------------------------

/// A residual under the 1 mm tolerance, in both axes: 0.5 mm.
const RESIDUAL: Xy = [0.0004, -0.0003];

/// K and O meet at `k_end` and `o_end`; N starts at K's other end and M at
/// O's other end (legacy `Pair`). Line ids: K 0, O 1, N 2, M 3.
struct Pair {
    p: Project,
    k: LineId,
    o: LineId,
    n: LineId,
    m: LineId,
    k_end: LineEnd,
    o_end: LineEnd,
}

fn far(end: LineEnd) -> LineEnd {
    either(end == START, END, START)
}

/// K and O as separate pieces, O's `o_end` placed `residual` from K's
/// `k_end`; N (north 3 m) at K's other end, M (east 3 m) at O's.
fn side_by_side(k_end: LineEnd, o_end: LineEnd, residual: Xy) -> Pair {
    let mut p = empty();
    let k = k_line(&mut p);
    let o = o_line(&mut p, ZERO);
    let k_join = at(&p, k, end_index(&p, k, k_end));
    let jo = st(&p, o, end_index(&p, o, o_end));
    place(&mut p, jo, plus(k_join, residual));
    let k_tip = st(&p, k, end_index(&p, k, far(k_end)));
    let n = line_from(&mut p, "N", k_tip, flat("n", &[(0.0, 3.0)]));
    let o_tip = st(&p, o, end_index(&p, o, far(o_end)));
    let m = line_from(&mut p, "M", o_tip, flat("m", &[(90.0, 3.0)]));
    Pair {
        p,
        k,
        o,
        n,
        m,
        k_end,
        o_end,
    }
}

fn combine_pair(pair: &Pair, kept: LineId, what: &str) -> (CombineReport, Project) {
    let keep_k = kept == pair.k;
    let other = either(keep_k, pair.o, pair.k);
    let (k_end, o_end) = (pair.k_end, pair.o_end);
    let join = either(keep_k, ends(k_end, o_end), ends(o_end, k_end));
    let (_, plan, after) = combine_checked(&pair.p, kept, other, Some(join), what);
    (plan, after)
}

fn moved_by(before: &Project, after: &Project, line: LineId) -> f64 {
    dist(at(before, line, 0), at(after, line, 0))
}

/// K's end and O's start made one station, K's slot hand-moved by
/// [`RESIDUAL`]: their chain positions differ by it.
fn merged_by_hand() -> Pair {
    let mut pair = side_by_side(END, START, ZERO);
    let (k2, o0) = (st(&pair.p, pair.k, 2), st(&pair.p, pair.o, 0));
    hand(&mut pair.p, pair.k, k2, [RESIDUAL[0], RESIDUAL[1], 0.0]);
    merge(&mut pair.p, o0, k2);
    pair
}

/// Every pairing of the ends, either line kept: the other line moves by the
/// residual with the line on it; the kept side stays exactly.
#[test]
fn a_residual_moves_the_other_side_with_what_is_merged_to_it() {
    let residual = dist(RESIDUAL, ZERO);
    let pairs = [(START, START), (START, END), (END, START), (END, END)];
    for ((k_end, o_end), keep_k) in pairs.into_iter().flat_map(|e| [(e, true), (e, false)]) {
        let what = format!("K {k_end:?}, O {o_end:?}, keep K {keep_k}");
        let pair = side_by_side(k_end, o_end, RESIDUAL);
        let (kept, other) = either(keep_k, (pair.k, pair.o), (pair.o, pair.k));
        let (stays, follows) = either(keep_k, (pair.n, pair.m), (pair.m, pair.n));
        let (plan, after) = combine_pair(&pair, kept, &what);
        assert_eq!(plan.moving_line, Some(other), "{what}");
        assert_eq!(plan.moved_lines, [other, follows], "{what}");
        assert_eq!(plan.kept_moves_because, None, "{what}");
        assert!(plan.torn_ties.is_empty(), "{what}");
        assert!((plan.shift_m - residual).abs() < 1e-9, "{what}");
        let follow = moved_by(&pair.p, &after, follows);
        assert!((follow - residual).abs() < 1e-9, "{what}: {follow}");
        assert!(moved_by(&pair.p, &after, stays) < 1e-12, "{what}");
    }
}

/// A datum holds its side: on O's side (M's end, or O's end, which M
/// shares) the kept K moves with N; on K's side (N's end) O moves with M.
#[test]
fn a_zero_datum_decides_which_side_a_residual_moves() {
    let (k, o, n, m) = (0, 1, 2, 3);
    for (name, (line, index), moving) in [("M", (m, 1), k), ("O", (o, 2), k), ("N", (n, 1), o)] {
        let what = format!("datum on {name}");
        let mut pair = side_by_side(END, START, RESIDUAL);
        assert_eq!([pair.k, pair.o, pair.n, pair.m], [k, o, n, m]);
        let held = st(&pair.p, line, index);
        hold(&mut pair.p, held);
        let (plan, after) = combine_pair(&pair, k, &what);
        let (follows, stays) = either(moving == k, (n, m), (m, n));
        assert_eq!(plan.moving_line, Some(moving), "{what}");
        assert_eq!(plan.moved_lines, [moving, follows], "{what}");
        let because = (moving == k).then_some(KeptMovesReason::ZeroDatum);
        assert_eq!(plan.kept_moves_because, because, "{what}");
        assert!(moved_by(&pair.p, &after, stays) < 1e-12, "{what}");
    }
}

/// One piece through C (N's end to M's end): O leaves M's junction for the
/// loop. Adapted: the legacy anchor (K's start slot) is a junction that stays
/// with N when K is detached (5.4 step 2), so the gauge goes on K:1.
#[test]
fn the_adjustment_anchor_decides_which_side_a_residual_moves() {
    for keep_k in [true, false] {
        let what = format!("keep K {keep_k}");
        let mut pair = side_by_side(END, START, RESIDUAL);
        let (n1, m1) = (st(&pair.p, pair.n, 1), st(&pair.p, pair.m, 1));
        let c = bridge(&mut pair.p, "C", n1, m1);
        let c1 = st(&pair.p, c, 1);
        merge(&mut pair.p, m1, c1);
        let (k1, k1_at) = (st(&pair.p, pair.k, 1), at(&pair.p, pair.k, 1));
        pair.p.set_placement(k1, k1_at[0], k1_at[1]).unwrap();
        assert_eq!(pair.p.placements.len(), 1, "{what}: one piece");
        let (o, m) = (pair.o, pair.m);
        let (plan, _) = combine_pair(&pair, either(keep_k, pair.k, o), &what);
        assert_eq!((plan.moving_line, plan.moved_lines), (Some(o), vec![o]));
        let because = (!keep_k).then_some(KeptMovesReason::AdjustmentAnchor);
        assert_eq!(plan.kept_moves_because, because, "{what}");
        let lines = |t: &super::TornTie| (t.kind, t.reason, t.slot.line_id, t.other_slot.line_id);
        let tears: Vec<_> = plan.torn_ties.iter().map(lines).collect();
        assert_eq!(tears, [(Kind::Junction, Reason::Loop, o, m)], "{what}");
    }
}

/// Datums hold both sides (N's end and M's end; neither join line carries
/// one): O moves by the residual alone, detached from the held M's junction.
#[test]
fn both_sides_held_tear_the_moving_sides_junction_for_a_residual() {
    let mut pair = side_by_side(END, START, RESIDUAL);
    let (o, m, n) = (pair.o, pair.m, pair.n);
    let (n1, m1) = (st(&pair.p, n, 1), st(&pair.p, m, 1));
    hold(&mut pair.p, n1);
    hold(&mut pair.p, m1);
    let (plan, after) = combine_pair(&pair, pair.k, "both held");
    assert_eq!((plan.moving_line, plan.moved_lines), (Some(o), vec![o]));
    assert_eq!(plan.kept_moves_because, None);
    let lines = |t: &super::TornTie| (t.reason, t.slot.line_id, t.other_slot.line_id);
    let tears: Vec<_> = plan.torn_ties.iter().map(lines).collect();
    assert_eq!(tears, [(Reason::ZeroDatum, o, m)]);
    assert!(moved_by(&pair.p, &after, m) < 1e-12);
    assert!(moved_by(&pair.p, &after, n) < 1e-12);
    assert_eq!(junctions(&after), 1, "only N's junction is left");
}

/// One join station whose K slot is moved by hand: O moves by the hand move
/// (the chain gap) with M; the joined station keeps K's and stays.
#[test]
fn merged_ends_moved_by_hand_carry_their_residual_too() {
    let pair = merged_by_hand();
    let (k, o, m, n) = (pair.k, pair.o, pair.m, pair.n);
    let j = st(&pair.p, k, 2);
    let c = pair.p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(c.join_kind, CombineJoinKind::Merged);
    let plan = &c.plan_keeping_a;
    assert_eq!(plan.moving_line, Some(o));
    assert_eq!(plan.moved_lines, [o, m]);
    assert!((plan.shift_m - dist(RESIDUAL, ZERO)).abs() < 1e-9);
    let (plan, after) = combine_pair(&pair, k, "merged by hand");
    near(pos(&after, j), pos(&pair.p, j), 1e-12, "the join stays");
    assert!(moved_by(&pair.p, &after, n) < 1e-12, "K's side stays");
    let follow = moved_by(&pair.p, &after, m);
    assert!((follow - plan.shift_m).abs() < 1e-9, "M follows O");
    assert_eq!(st(&after, k, 4), st(&after, m, 0), "O's end is M's start");
    reopen_moves_nothing(&after, "merged by hand");
}

/// The shape above with T starting at the join station too: a branch of the
/// joined station, it stays exactly while O and M move by the residual.
#[test]
fn a_branch_at_merged_ends_moved_by_hand_stays_on_the_joined_station() {
    let mut pair = merged_by_hand();
    let (k, o, m) = (pair.k, pair.o, pair.m);
    let j = st(&pair.p, k, 2);
    let t = line_from(&mut pair.p, "T", j, flat("t", &[(180.0, 2.0)]));
    let c = pair.p.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(c.join_kind, CombineJoinKind::Merged);
    assert_eq!(c.branches, [t]);
    assert_eq!(c.plan_keeping_a.moved_lines, [o, m]);
    assert_eq!(c.plan_keeping_a.moved_with_joined_station, None);
    let (_, after) = combine_pair(&pair, k, "a branch at merged ends");
    assert!(moved_by(&pair.p, &after, t) < 1e-12, "T stays");
    assert_eq!(st(&after, t, 0), j, "T on the joined station");
    reopen_moves_nothing(&after, "branch");
}

/// Float dust moves nothing but O's stations; K and N keep their bits.
/// Adapted: M shares O's end (one station) and moves with it by the dust.
#[test]
fn a_residual_of_float_dust_moves_nothing_as_before() {
    for (what, residual) in [("coincident", ZERO), ("dust", [0.6 * DUST, -0.6 * DUST])] {
        let pair = side_by_side(END, START, residual);
        let (plan, after) = combine_pair(&pair, pair.k, what);
        let because = plan.kept_moves_because;
        assert_eq!((plan.moving_line, because), (None, None), "{what}");
        assert!(plan.moved_lines.is_empty(), "{what}");
        assert!(plan.torn_ties.is_empty(), "{what}");
        assert_eq!(plan.shift_m.to_bits(), 0, "{what}: no shift");
        let (was, now) = (pair.p.raw_positions(), after.raw_positions());
        let exact = either(residual == ZERO, vec![0, 2, 3], vec![0, 2]);
        for (s, [x, y, _]) in was.iter() {
            let on = |line: &LineId| pair.p.topology().slot_on(s, *line).is_some();
            let Some([nx, ny]) = now.xy(s) else {
                continue; // O's start, folded into K's end
            };
            if exact.iter().any(on) {
                assert!(nx == x && ny == y, "{what}: station {s} moved");
            } else if on(&pair.m) {
                let d = minus([nx, ny], [x, y]);
                assert!(d[0].abs() <= DUST && d[1].abs() <= DUST, "{what}: {d:?}");
            }
        }
        reopen_moves_nothing(&after, what);
    }
    let mut pair = side_by_side(END, START, ZERO);
    let (k2, o0) = (st(&pair.p, pair.k, 2), st(&pair.p, pair.o, 0));
    merge(&mut pair.p, o0, k2);
    let (plan, _) = combine_pair(&pair, pair.k, "merged");
    assert_eq!(plan.moving_line, None, "merged");
    let pair = side_by_side(END, START, [2.0 * DUST, 0.0]);
    let (plan, _) = combine_pair(&pair, pair.k, "past dust");
    assert_eq!(plan.moving_line, Some(pair.o));
    assert_eq!(plan.moved_lines, [pair.o, pair.m]);
}

/// The residual comes from the hand-moved merged join; Z is a branch of the
/// join station (on neither side) snapped to O:1: O and M move by the
/// residual and Z does not, so the plan lists the snap among the tears.
#[test]
fn a_residual_tears_a_snap_from_the_side_that_moves() {
    let mut pair = merged_by_hand();
    let (k, o, m) = (pair.k, pair.o, pair.m);
    let (j, o1) = (st(&pair.p, k, 2), st(&pair.p, o, 1));
    let z = bridge(&mut pair.p, "Z", j, o1);
    let snap = tie(&mut pair.p, (o, 1), (z, 1), MOVE);
    assert!(snapped(&pair.p, snap), "setup: a snap");
    let (c, plan, after) = combine_checked(&pair.p, k, o, Some(ends(END, START)), "snap");
    assert_eq!(c.join_kind, CombineJoinKind::Merged);
    assert_eq!(plan.moving_line, Some(o));
    assert_eq!(plan.moved_lines, [o, m]);
    assert!(plan.shift_m < 1e-3, "a residual, not a shift");
    let t = plan
        .torn_ties
        .iter()
        .find(|t| t.closure == Some(snap))
        .unwrap();
    let want = (Kind::SnappedClosure, slot(o, 1), slot(z, 1));
    assert_eq!((t.kind, t.slot, t.other_slot), want);
    assert_eq!(closure(&after, snap).strategy, BATCH);
}

// ----- combine_realign_tests --------------------------------------------

/// A passage P from station `from` by way of `via` to station `to` on
/// another free piece, merged onto P's end: the loop the combine closes.
fn passage(p: &mut Project, from: StationId, via: Xy, to: StationId) {
    let (a, b) = (pos(p, from), pos(p, to));
    let legs = vec![toward("p0", a, via), toward("p1", via, b)];
    let line = line_from(p, "P", from, legs);
    let end = st(p, line, 2);
    merge(p, to, end);
}

/// The plan lists the tear of closure `id` as a `SnappedClosure` for the
/// loop and the combine made it an ordinary closure between stations `a`
/// and `b`, which coincide.
fn assert_reclosed_ordinary(run: &Run, id: ClosureId, [a, b]: [StationId; 2]) {
    let (_, plan, after) = run;
    near(pos(after, a), pos(after, b), 1e-9, "re-closed");
    assert_eq!(between(after, a, b), [(id, BATCH)]);
    let want = (Kind::SnappedClosure, Reason::Loop);
    assert_eq!(tear_of(plan, id), Some(want), "{plan:?}");
}

/// O moves back 0.3 m onto K's end; P closes the loop; the torn snap from B
/// (K's side) to C (O's end) re-closes across the sides. Adapted: P meets
/// O's middle (a tear detaches O alone, 5.4 step 2, so C must ride on O).
#[test]
fn a_torn_snap_the_shift_recloses_across_the_two_sides_becomes_an_ordinary_closure() {
    let mut p = empty();
    let k = line(&mut p, "K", flat("k", &[(90.0, 10.0)]));
    let legs = flat("o", &[(90.0, 2.5), (90.0, 2.5)]);
    let o = line_at(&mut p, "O", [10.3, 0.0], legs);
    let (k0, o1, o2) = (st(&p, k, 0), st(&p, o, 1), st(&p, o, 2));
    let b = line_from(&mut p, "B", k0, vec![toward("b", ZERO, [15.0, 2.0])]);
    let c = line_from(&mut p, "C", o2, flat("c", &[(0.0, 2.0)]));
    passage(&mut p, k0, [7.0, -3.0], o1);
    let torn_snap = tie(&mut p, (b, 1), (c, 1), MOVE);
    tie(&mut p, (k, 1), (o, 0), BATCH);
    assert!(!snapped(&p, torn_snap), "setup: a torn snap");
    let run = combine_checked(&p, k, o, Some(ends(END, START)), "across the sides");
    assert_eq!(run.1.moving_line, Some(o));
    assert!((run.1.shift_m - 0.3).abs() < 1e-9, "{}", run.1.shift_m);
    let ab = [st(&run.2, b, 1), st(&run.2, c, 1)];
    assert_reclosed_ordinary(&run, torn_snap, ab);
}

/// O moves back 0.3 m onto K's end; P from K's start ends on O's end; the
/// torn snap from O's end to K:1 re-closes inside the combined line.
#[test]
fn a_torn_snap_the_shift_recloses_inside_the_combined_line_becomes_an_ordinary_closure() {
    let mut p = empty();
    let k = line(&mut p, "K", flat("k", &[(90.0, 10.0), (0.0, 2.0)]));
    let o = line_at(&mut p, "O", [10.3, 2.0], flat("o", &[(180.0, 2.0)]));
    let (k0, o1) = (st(&p, k, 0), st(&p, o, 1));
    passage(&mut p, k0, [5.0, -2.0], o1);
    let torn_snap = tie(&mut p, (o, 1), (k, 1), MOVE);
    tie(&mut p, (k, 2), (o, 0), BATCH);
    assert!(!snapped(&p, torn_snap), "setup: a torn snap");
    let run = combine_checked(&p, k, o, Some(ends(END, START)), "inside");
    assert_eq!(run.1.moving_line, Some(o));
    // O's end (detached from P's) is the combined line's station 3 now.
    let ab = [st(&run.2, k, 3), st(&run.2, k, 1)];
    assert_reclosed_ordinary(&run, torn_snap, ab);
}

/// One join station, O's hand move 0.4 m: O moves back onto B (a branch at
/// the join), its torn snap to B's end snaps again (derived, plan 3.6), and
/// the closure from B:1 to O's end, now across one shot, goes (#746).
#[test]
fn a_torn_snap_a_merged_join_recloses_beside_it_snaps_again() {
    let mut p = empty();
    let k = line(&mut p, "K", flat("k", &[(90.0, 10.0)]));
    let o = line_at(&mut p, "O", [10.0, 0.0], flat("o", &[(0.0, 3.0)]));
    let (k1, o0) = (st(&p, k, 1), st(&p, o, 0));
    hand(&mut p, o, o0, [-0.4, 0.0, 0.0]);
    merge(&mut p, o0, k1);
    let b = line_from(&mut p, "B", k1, flat("b", &[(0.0, 1.5), (0.0, 1.5)]));
    let snap = tie(&mut p, (o, 1), (b, 2), MOVE);
    let loop_closure = tie(&mut p, (b, 1), (o, 1), BATCH);
    assert!(!snapped(&p, snap), "setup: a torn snap");
    assert!(p.closures_across_one_shot().is_empty());
    let (b1, o1) = (st(&p, b, 1), st(&p, o, 1));
    let what = "beside a merged join";
    let (_, plan, after) = combine_checked(&p, k, o, Some(ends(END, START)), what);
    assert_eq!(plan.moving_line, Some(o), "{plan:?}");
    assert!(plan.torn_ties.is_empty(), "{plan:?}");
    let removed = &plan.closures_removed_across_one_shot;
    let removed: Vec<_> = removed
        .iter()
        .map(|r| (r.closure.id, r.closure.a, r.closure.b))
        .collect();
    assert_eq!(removed, [(loop_closure, b1, o1)]);
    // O's end is the combined line's station 2; the snap holds there.
    let (k2, b2) = (st(&after, k, 2), st(&after, b, 2));
    assert_eq!(between(&after, k2, b2), [(snap, MOVE)]);
    assert!(snapped(&after, snap));
    assert!(between(&after, b1, k2).is_empty());
}

/// O's start, side by side with K's end but shown 0.5 m north by hand, is
/// snapped to S's end; the joined station keeps K's hand move (none), so the
/// snap is pulled 0.5 m apart into an ordinary closure (reason `Moved`).
#[test]
fn a_snap_a_dropped_hand_move_pulls_apart_becomes_an_ordinary_closure() {
    let mut p = empty();
    let k = line(&mut p, "K", flat("k", &[(90.0, 10.0)]));
    let o = line_at(&mut p, "O", [10.0, 0.5], flat("o", &[(90.0, 5.0)]));
    let (o0, o1) = (st(&p, o, 0), st(&p, o, 1));
    hand(&mut p, o, o0, [0.0, 0.5, 0.0]);
    let back = toward("s", [15.0, 0.0], [10.0, 0.5]);
    let s = line_from(&mut p, "S", o1, vec![back]);
    let snap = tie(&mut p, (o, 0), (s, 1), MOVE);
    assert!(snapped(&p, snap), "setup: a snap");
    let what = "pulled apart";
    let (_, plan, after) = combine_checked(&p, k, o, Some(ends(END, START)), what);
    assert_eq!(plan.moving_line, None, "{plan:?}");
    let want = (Kind::SnappedClosure, Reason::Moved);
    assert_eq!(tear_of(&plan, snap), Some(want));
    let (k1, s1) = (st(&after, k, 1), st(&after, s, 1));
    assert_eq!(between(&after, k1, s1), [(snap, BATCH)]);
    assert!((gap(&after, snap) - 0.5).abs() < 1e-9);
}

// ----- combine_dedupe_tests ---------------------------------------------

/// A tail combined into S's end (nothing moves) runs the dedupe on S; a snap
/// across the two lines would turn `Batch` first (5.4 step 5). On S:4 and
/// S:0 the closures `pair` ("L" legacy, "E" equal, "B", "Q" least squares,
/// "S" snap, "T" torn snap), ids 1, 2, ...; a `Batch` S:1 to S:3 (id 100)
/// after the first; legacy and `Batch` on T (200, 201). The ids left.
fn dedupe(pair: &str) -> Vec<ClosureId> {
    let code = |c: char| match c {
        'L' => (LEGACY, false),
        'E' => (EQUAL, false),
        'B' => (BATCH, false),
        'Q' => (LSQ, false),
        'S' => (MOVE, true),
        _ => (MOVE, false),
    };
    let last = either(pair.contains('S'), 10.0, 9.0);
    let mut p = empty();
    let legs = [(90.0, 10.0), (0.0, 10.0), (270.0, 10.0), (180.0, last)];
    let s = line(&mut p, "S", flat("s", &legs));
    let s4 = st(&p, s, 4);
    let tail = line_from(&mut p, "E", s4, flat("e", &[(90.0, 1.0)]));
    let t = line_at(&mut p, "T", [50.0, 0.0], flat("t", &[(0.0, 3.0)]));
    for (n, c) in pair.chars().filter(|c| *c != ' ').enumerate() {
        let ((strategy, aligned), id) = (code(c), n as ClosureId + 1);
        let (a, b) = either(n == 0, ((s, 4), (s, 0)), ((s, 0), (s, 4)));
        plant(&mut p, a, b, strategy, id);
        if n == 0 {
            plant(&mut p, (s, 1), (s, 3), BATCH, 100);
        }
        assert!(strategy != MOVE || snapped(&p, id) == aligned, "setup");
    }
    plant(&mut p, (t, 0), (t, 1), LEGACY, 200);
    plant(&mut p, (t, 0), (t, 1), BATCH, 201);
    assert_valid(&p);
    let report = ok(p.combine_lines(s, tail, &options(Some(ends(END, START)))));
    assert_eq!(report.moving_line, None);
    assert!(report.torn_ties.is_empty(), "{report:?}");
    p.closures.iter().map(|c| c.id).collect()
}

/// Of closures the combine leaves on the same two stations, the dedupe keeps
/// the first one the solve takes (`Batch`, `LeastSquaresWeighted`), else the
/// first; kept closures keep their place; closures off the line stay.
#[test]
fn the_closure_dedupe_keeps_the_first_closure_the_solve_takes() {
    let cases: [(&str, &str, &[ClosureId]); 12] = [
        ("#749: legacy, then Batch", "L B", &[100, 2]),
        ("Batch, then legacy", "B L", &[1, 100]),
        ("#749: equal, then least squares", "E Q", &[100, 2]),
        ("#749: torn snap, then Batch", "T B", &[100, 2]),
        ("#749: snap, then Batch", "S B", &[100, 2]),
        ("Batch, then snap", "B S", &[1, 100]),
        ("both eligible", "B Q", &[1, 100]),
        ("both eligible, swapped", "Q B", &[1, 100]),
        ("neither eligible", "L S", &[1, 100]),
        ("neither eligible, swapped", "S E", &[1, 100]),
        ("#749: the first eligible of many", "L S B E Q", &[100, 3]),
        ("one closure alone", "L", &[1, 100]),
    ];
    for (what, pair, kept) in cases {
        let want: Vec<ClosureId> = kept.iter().copied().chain([200, 201]).collect();
        assert_eq!(dedupe(pair), want, "{what}");
    }
}

fn repeated(v: Violation) -> Option<ClosureId> {
    match v {
        Violation::DuplicateClosureId { closure } => Some(closure),
        _ => None,
    }
}

/// Closure ids are unique (invariant 1): `validate` refuses a repeated one,
/// so the dedupe never meets it.
#[test]
fn a_repeated_closure_id_is_a_violation() {
    let mut p = empty();
    let legs = flat("s", &[(90.0, 10.0), (0.0, 10.0), (270.0, 9.0)]);
    let s = line(&mut p, "S", legs);
    plant(&mut p, (s, 0), (s, 2), LEGACY, 5);
    plant(&mut p, (s, 0), (s, 2), BATCH, 5);
    plant(&mut p, (s, 0), (s, 3), BATCH, 5);
    let ids: Vec<ClosureId> = p.validate().into_iter().filter_map(repeated).collect();
    assert!(!ids.is_empty() && ids.iter().all(|id| *id == 5), "{ids:?}");
}

/// The smallest survey of #749. B (its start holds the gauge) runs 10 m
/// east, 9.6 m back west and 5 m north; a closure with `strategy` ties B:2,
/// 0.4 m east of B's start, back to it; A runs 4 m south and ends on B:2.
fn legacy_loop(strategy: ClosureStrategy) -> (Project, LineId, LineId, ClosureId) {
    let mut p = empty();
    let legs = flat("b", &[(90.0, 10.0), (270.0, 9.6), (0.0, 5.0)]);
    let b = line(&mut p, "B", legs);
    let (b2, a_at) = (st(&p, b, 2), plus(at(&p, b, 2), [0.0, 4.0]));
    let a = line_at(&mut p, "A", a_at, flat("a", &[(180.0, 4.0)]));
    let a1 = st(&p, a, 1);
    merge(&mut p, a1, b2);
    let planted = tie(&mut p, (b, 2), (b, 0), strategy);
    assert_valid(&p);
    (p, b, a, planted)
}

/// Keeping A, A leaves B:2 (a `Batch` closure closing B's loop) and moves
/// 0.4 m onto B's start (B holds the gauge); of the two closures then on one
/// pair, one the solve takes stays and the plan lists a replaced one.
#[test]
fn a_merge_torn_onto_a_legacy_closure_leaves_the_closure_the_solve_takes() {
    for strategy in [LEGACY, EQUAL, MOVE, BATCH, LSQ] {
        let what = format!("B:2 to B:0 {strategy:?}");
        let (p, b, a, planted) = legacy_loop(strategy);
        let (b0, b2, b0_at) = (st(&p, b, 0), st(&p, b, 2), at(&p, b, 0));
        let mut offered = p.combine_candidates_at_point(slot(a, 1)).into_iter();
        let c = offered.find(|c| [c.line_a, c.line_b] == [b, a]).unwrap();
        assert_eq!(c.join_kind, CombineJoinKind::Closure, "{what}");
        let plan = plan_for(&c, a);
        assert_eq!(plan.moving_line, Some(a), "{what}");
        let because = Some(KeptMovesReason::AdjustmentAnchor);
        assert_eq!(plan.kept_moves_because, because, "{what}");
        assert!((plan.shift_m - 0.4).abs() < 1e-9, "{what}");
        let twice = Reason::PassesTwice;
        let passes = (Kind::Junction, twice, b2, slot(a, 1), slot(b, 2));
        assert_eq!(torn(plan), [passes], "{what}");
        let eligible = matches!(strategy, BATCH | LSQ);
        let tear = plan.torn_ties[0].closure.unwrap();
        let listed = either(eligible, vec![], vec![(planted, strategy, tear, BATCH)]);
        assert_eq!(replaced(plan), listed, "{what}");
        let (_, _, after) = combine_checked(&p, a, b, Some(ends(END, START)), &what);
        // B's stations follow A's two: B:2 is A:3, B:0 the joined A:1.
        let joined = st(&after, a, 1);
        assert_eq!(st(&after, a, 3), b2, "{what}");
        let aliases = &after.station(joined).unwrap().aliases;
        assert!(aliases.iter().any(|x| x.id == b0), "{what}: B:0 folded");
        assert_eq!(junctions(&after), 0, "{what}");
        assert_eq!(after.closures.len(), 1, "{what}");
        let left = after.closures[0];
        assert_eq!(between(&after, joined, b2), [(left.id, left.strategy)]);
        assert!(matches!(left.strategy, BATCH | LSQ), "{what}: {left:?}");
        let aligned = after.closure_is_aligned(&after.raw_positions(), &left);
        assert!(!aligned, "{what}");
        assert_eq!(left.id, either(eligible, planted, tear), "{what}");
        assert!((gap(&after, left.id) - 0.4).abs() < 1e-9, "{what}");
        near(pos(&after, joined), b0_at, 1e-9, &what);
    }
}

/// The gauge half of the test above: the joined survey's `Placement` (plan
/// 3.7) is on the joined station, where B's start was: B's start held it
/// and is folded into the joined station, which replaces it (plan 5.1).
#[test]
fn a_merge_torn_onto_a_legacy_closure_keeps_the_gauge_where_b_started() {
    let (p, b, a, _) = legacy_loop(BATCH);
    let b0_at = at(&p, b, 0);
    let (_, _, after) = combine_checked(&p, a, b, Some(ends(END, START)), "gauge");
    let placed: Vec<StationId> = after.placements.iter().map(|x| x.station).collect();
    assert_eq!(placed, [st(&after, a, 1)], "{:?}", after.placements);
    let g = after.placements[0];
    near([g.x, g.y], b0_at, 1e-9, "the gauge");
}

/// K runs 10 m east; O runs 9.6 m west from 0.4 m short of K's end onto K's
/// start; X from (`x`, 0), `x_len` west, onto K's start too; a `Batch`
/// closure ties K's end to O's start (the join).
fn kox(x: f64, x_len: f64) -> (Project, [LineId; 3]) {
    let mut p = empty();
    let k = line(&mut p, "K", flat("k", &[(90.0, 10.0)]));
    let o = line_at(&mut p, "O", [9.6, 0.0], flat("o", &[(270.0, 9.6)]));
    let x = line_at(&mut p, "X", [x, 0.0], flat("x", &[(270.0, x_len)]));
    let (k0, o1, x1) = (st(&p, k, 0), st(&p, o, 1), st(&p, x, 1));
    merge(&mut p, o1, k0);
    merge(&mut p, x1, k0);
    tie(&mut p, (k, 1), (o, 0), BATCH);
    (p, [k, o, x])
}

/// X's snap to K's end and closure to O's start meet on the joined station:
/// the ordinary one stays, either order, either line kept. Adapted: the
/// gauge is on K's end (see the residual anchor test).
#[test]
fn a_snapped_and_an_ordinary_closure_made_identical_keep_the_ordinary_one() {
    for (snap_first, keep_o) in [(true, false), (true, true), (false, false), (false, true)] {
        let what = format!("snap first {snap_first}, keeping O {keep_o}");
        let (mut p, [k, o, x]) = kox(10.0, 10.0);
        let (k1, k1_at) = (st(&p, k, 1), at(&p, k, 1));
        p.set_placement(k1, k1_at[0], k1_at[1]).unwrap();
        let (to_k, to_o) = ((k, 1), (o, 0));
        let (snap_to, batch_to) = ((to_k, MOVE), (to_o, BATCH));
        let (first, then) = either(snap_first, (snap_to, batch_to), (batch_to, snap_to));
        let first = tie(&mut p, (x, 0), first.0, first.1);
        let then = tie(&mut p, (x, 0), then.0, then.1);
        let (snap, ordinary) = either(snap_first, (first, then), (then, first));
        assert!(snapped(&p, snap), "setup: X's start sits on K's end");
        let join = Some(either(keep_o, ends(START, END), ends(END, START)));
        let (kept, other) = either(keep_o, (o, k), (k, o));
        let c = p.combine_candidate_joining(kept, other, join).unwrap();
        assert_eq!(c.join_kind, CombineJoinKind::Closure, "{what}");
        let plan = plan_for(&c, kept);
        let snap_tears = torn(plan)
            .into_iter()
            .filter(|t| t.0 == Kind::SnappedClosure);
        assert_eq!(snap_tears.count(), 0, "{what}: setup: the snap is not torn");
        assert_eq!(replaced(plan), [(snap, MOVE, ordinary, BATCH)], "{what}");
        let (_, _, after) = combine_checked(&p, kept, other, join, &what);
        // K's end is the joined station: station 1 of the combined line.
        let (x0, joined) = (st(&after, x, 0), st(&after, kept, 1));
        assert_eq!(between(&after, x0, joined), [(ordinary, BATCH)], "{what}");
    }
}

/// X's start snapped to O's start, a legacy closure to K's end: O moves, the
/// torn snap lands on the legacy closure's stations and replaces it.
#[test]
fn a_snap_the_combine_tears_replaces_a_legacy_closure_on_its_stations() {
    let (mut p, [k, o, x]) = kox(9.6, 9.6);
    let snap = tie(&mut p, (x, 0), (o, 0), MOVE);
    assert!(snapped(&p, snap), "setup: X's start sits on O's start");
    let legacy = tie(&mut p, (x, 0), (k, 1), LEGACY);
    let join = Some(ends(END, START));
    let c = p.combine_candidate_joining(k, o, join).unwrap();
    assert_eq!(c.join_kind, CombineJoinKind::Closure);
    let plan = plan_for(&c, k);
    assert_eq!(plan.moving_line, Some(o));
    let kind = tear_of(plan, snap).map(|t| t.0);
    assert_eq!(kind, Some(Kind::SnappedClosure), "setup: the snap tears");
    assert_eq!(replaced(plan), [(legacy, LEGACY, snap, BATCH)]);
    let (_, _, after) = combine_checked(&p, k, o, join, "torn snap");
    let (x0, k1) = (st(&after, x, 0), st(&after, k, 1));
    assert_eq!(between(&after, x0, k1), [(snap, BATCH)], "the torn snap");
}
