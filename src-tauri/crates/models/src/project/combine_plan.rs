//! Combine Lines, what has to give so two lines that meet can become
//! one: which side moves to close the join, by how much, and which rigid ties
//! become ordinary loop closures. Read-only; the candidate query reports it
//! (one plan per choice of kept line) and `combine_lines` applies it.
//!
//! The rules, from the merge forest the app keeps (`merge_stations` refuses a
//! merge between lines that are already rigidly connected, so merges never
//! close a loop; a loop is closed by a loop closure):
//!
//! - **Join ties** make the two join stations one point: the merges and
//!   closures between the two join stations' merge groups (for a merged join,
//!   every tie inside its one group). The combine folds them into the joined
//!   station, so they are never torn. A tie that reaches a station of either
//!   line OTHER than its join station is not one: that line passes
//!   through the join point again, and the tie would tie the combined line
//!   to itself.
//! - **Loop ties.** A merge from the moving line M to a line that still
//!   reaches the fixed one F without M is the other end of a loop the combine
//!   closes through the combined line: it becomes a loop closure (the one
//!   nearest M, so the least moves). Merges form a forest, so there is at most
//!   one per path. A snapped (aligned, rigid) closure between M's side and
//!   F's side becomes an ordinary one for the same reason.
//! - **The shift.** Two ends tied by a loop closure are apart by its
//!   misclosure; M's side (M and everything still merged to it) moves by it.
//!   Two side-by-side ends (within the tolerance) or merged ends whose chain
//!   positions differ are apart by a residual, which moves M's side the same
//!   way (#740): the combined line hangs the other line's stations off
//!   the kept join station, so one side moves by the residual whatever the
//!   plan says, and what is merged to it must follow. Only float dust is
//!   left in place.
//!   M is the other line, unless its side is held by a zero datum and the
//!   kept line's is not, or (no datum deciding) M's side holds the station
//!   the closure-adjusted view holds the survey at
//!   (`Project::adjustment_anchor_of`) and the kept side does not: moving it
//!   would shift the whole adjusted survey by the gap. That applies only where
//!   the view solves the joined survey - a closure it solves there now, or a
//!   tie the combine turns into one it will solve - and a closure that joins
//!   the two ends of one shot is one the solve skips (#737), so it counts for
//!   neither. When both sides are held, the merges from M to the
//!   datum sides also become closures (a closure between two anchored groups
//!   is legitimate; a merge is not). Only when BOTH lines carry a zero datum
//!   on one of their own stations is the combine refused: the combined line
//!   would hold two.
//!   Moving M's side tears every snapped closure from it to anything that
//!   stays, so those become ordinary closures too. One into a part a datum
//!   merge is torn from tears for that datum, as the merge does (#756): the
//!   datum holds the part where it is.
//! - **Hand moves at the join (#755).** A manual station offset moves one
//!   station as shown and nothing beyond it, and the joined station keeps
//!   only one: the fixed side's. When the two join stations' offsets differ,
//!   the side that moves goes by the chain gap (the combined line hangs
//!   the other line's stations off the kept join station's chain), but
//!   the join station whose offset is dropped lands on the joined station as
//!   shown, which is the gap between the two as shown away. The lines
//!   merged at that station (on a join that is not merged) stay merged to
//!   the joined station, so they move with it, by that gap, and not with the
//!   side: otherwise the offset solve closed the merge later, the in-session
//!   repair past 1 cm and a reopen below it, moving them unannounced. They
//!   are still on the moving side for the anchor rule (a datum there holds
//!   that side); a snap between them and the rest of the side is torn, as
//!   the two now move apart. Nothing is folded into the shots: the
//!   Closures Computed View solves shots, merges and closures, never manual
//!   offsets, so it does not move for any of this.

use std::collections::{HashMap, HashSet};

use crate::{
    ClosureStrategy, CombineJoinedMove, CombinePlan, CombineRemovedClosure, CombineRemovedKind,
    CombineReplacedClosure, KeptMovesReason, LoopClosure, LineId, StationRef, TornTie,
    TornTieKind, TornTieReason,
};

use super::closure_shots::Knots;
use super::Project;

/// A rigid tie: a merge (by its index in `station_merges`) or a snapped,
/// aligned loop closure (by id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TieId {
    Merge(usize),
    Closure(u64),
}

/// One rigid tie with its two resolved stations.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tie {
    pub id: TieId,
    pub a: StationRef,
    pub b: StationRef,
}

/// A loop closure with its two resolved stations and whether the solve takes
/// its strategy, for the anchor rule's solve check, and its strategy and
/// alignment, for naming one the combine replaces (#749).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedClosure {
    pub id: u64,
    pub a: StationRef,
    pub b: StationRef,
    pub eligible: bool,
    pub strategy: ClosureStrategy,
    pub aligned: bool,
}

/// What the anchor rule reads about loop closures, worked out once per
/// candidate query (the station menu and Combine Lines plan two choices of
/// kept line for every partner) and only when a shifting plan needs it:
/// the closures the solve skips because they join the two ends of one shot
/// (#737), and every closure resolved.
#[derive(Debug, Default)]
pub(crate) struct ClosureFacts {
    skipped: std::cell::OnceCell<HashSet<u64>>,
    closures: std::cell::OnceCell<Vec<ResolvedClosure>>,
}

impl ClosureFacts {
    fn skipped(&self, project: &Project) -> &HashSet<u64> {
        self.skipped.get_or_init(|| {
            project
                .closures_across_one_shot()
                .into_iter()
                .map(|c| c.closure_id)
                .collect()
        })
    }

    fn closures(&self, project: &Project) -> &[ResolvedClosure] {
        self.closures.get_or_init(|| project.resolved_closures())
    }
}

/// The point two lines are joined at, from the kept line's side.
#[derive(Debug, Clone)]
pub(crate) struct JoinPoint {
    pub kept: LineId,
    pub other: LineId,
    pub join_k: StationRef,
    pub join_o: StationRef,
    /// The merge group of each join station (one group when merged).
    pub group_k: HashSet<StationRef>,
    pub group_o: HashSet<StationRef>,
    pub merged: bool,
}

impl JoinPoint {
    /// A station of one of the two lines other than its join station.
    fn is_loose(&self, at: StationRef) -> bool {
        (at.line_id == self.kept || at.line_id == self.other)
            && at != self.join_k
            && at != self.join_o
    }

    /// Does the tie between `a` and `b` make the two join stations one point
    /// (see the module docs)? The combine folds such a tie into the joined
    /// station; a merge becomes a merge of the joined station, a closure goes
    /// (it would tie the joined station to itself).
    pub(crate) fn is_join_tie(&self, a: StationRef, b: StationRef) -> bool {
        if self.is_loose(a) || self.is_loose(b) {
            return false;
        }
        if self.merged {
            self.group_k.contains(&a) && self.group_k.contains(&b)
        } else {
            (self.group_k.contains(&a) && self.group_o.contains(&b))
                || (self.group_o.contains(&a) && self.group_k.contains(&b))
        }
    }
}

/// What `combine_lines` changes before it joins the shots.
#[derive(Debug, Clone, Default)]
pub(crate) struct TiePlan {
    /// The line whose side moves, when the two join stations are apart at
    /// all: past the tolerance, or side by side within it (#740). `None` only
    /// for a gap of float dust (`Project::OFFSET_DUST_M` in each axis).
    pub moving: Option<LineId>,
    /// The lines that move, the moving line first.
    pub moved: Vec<LineId>,
    /// The translation applied to every moved line.
    pub shift: (f64, f64),
    /// The ties that become ordinary loop closures.
    pub torn: Vec<Tie>,
    /// Which of `torn` tear because a zero datum holds the line on their
    /// far side (the rest close a loop through the combined line): the
    /// moving line's merges to its datum sides, and the snaps the move
    /// tears into those parts (#756). Only a shift tears them, so the far
    /// side is the one that does not move.
    pub held_by_datum: Vec<TieId>,
    /// Why the kept line's side moves, when it does.
    pub kept_moves_because: Option<KeptMovesReason>,
    /// Whether the joined station keeps the kept line's manual offset
    /// (the fixed side's: false when the kept line is the one that moves).
    pub kept_offset_wins: bool,
    /// The ties the combine removes instead of keeping (#746): once combined,
    /// the loop closure each is or becomes would join the two ends of one
    /// shot, which the solve only skips. A torn merge or snapped closure among
    /// them is left out of `torn` (it is dropped, not torn); an existing
    /// closure is one the project does not skip now. Named by its stations
    /// before the combine. The dialog lists exactly these and the combine
    /// removes exactly these: both read this plan.
    pub dropped: Vec<(Tie, CombineRemovedKind)>,
    /// The closures the combine drops for one the solve takes on the same
    /// two stations while it is not one itself (#749), each with the one
    /// that replaces it (`closure_dedupe.rs`). The dialog lists exactly
    /// these, and the combine checks it replaced exactly these.
    pub replaced: Vec<CombineReplacedClosure>,
    /// The join station whose manual offset the joined station does not keep
    /// (the moving side's; the other line's when nothing moves), and how
    /// far it moves to land on the joined station as shown, when the two
    /// join stations' offsets differ on a join that is not merged (#755).
    pub dropped_hand_move: Option<(StationRef, (f64, f64))>,
    /// The lines merged at that station, and what hangs off them: they
    /// move with the joined station, by its move above, not by `shift`.
    /// Not in `moved`.
    pub joined_moved: Vec<LineId>,
}

type Adjacency = HashMap<LineId, Vec<(LineId, TieId)>>;

/// The moving line, the fixed one, the ties to tear, which of those tear
/// for a zero datum, and why the kept side moves (when it does).
type MovingSide = (
    LineId,
    LineId,
    HashSet<TieId>,
    Vec<TieId>,
    Option<KeptMovesReason>,
);

/// Every line reachable from `start` over `adjacency` without entering
/// `blocked` (`start` included).
fn reach(adjacency: &Adjacency, start: LineId, blocked: LineId) -> HashSet<LineId> {
    let mut seen = HashSet::new();
    let mut queue = vec![start];
    while let Some(current) = queue.pop() {
        if current == blocked && current != start {
            continue;
        }
        if seen.insert(current) {
            for (next, _) in adjacency.get(&current).into_iter().flatten() {
                if !seen.contains(next) && *next != blocked {
                    queue.push(*next);
                }
            }
        }
    }
    seen
}

impl Project {
    /// Every resolvable merge and snapped (aligned) closure, as ties.
    pub(crate) fn rigid_ties(&self) -> Vec<Tie> {
        let merges = self.station_merges.iter().enumerate().filter_map(|(i, m)| {
            Some(Tie {
                id: TieId::Merge(i),
                a: self.resolve_key(&m.station_a)?,
                b: self.resolve_key(&m.station_b)?,
            })
        });
        let closures = self
            .loop_closures
            .iter()
            .filter(|c| c.is_aligned)
            .filter_map(|c| {
                Some(Tie {
                    id: TieId::Closure(c.id),
                    a: self.resolve_key(&c.station_a)?,
                    b: self.resolve_key(&c.station_b)?,
                })
            });
        merges.chain(closures).collect()
    }

    /// Does any zero datum, placement or GPS fix, resolve to `at`?
    pub(crate) fn station_holds_datum(&self, at: StationRef) -> bool {
        self.zero_datums
            .iter()
            .any(|datum| self.resolve_key(&datum.station_ref) == Some(at))
    }

    /// Does a PLACEMENT zero datum sit on one of `line`'s own stations? A
    /// GPS fix (#741) holds nothing in raw geometry, so a line carrying only
    /// fixes may move like any other.
    pub(crate) fn carries_zero_datum(&self, line: LineId) -> bool {
        self.placement_datum_refs()
            .iter()
            .any(|at| at.line_id == line)
    }

    /// Plan the shift and the torn ties for joining `join`, whose join stations'
    /// chain positions are `gap` apart (the kept one minus the other one: the
    /// translation that brings the other line's join station onto the kept
    /// one's) and, as shown (with their manual offsets), `shown` apart, against
    /// `ties` ([`Self::rigid_ties`]). `Err` is the refusal: both lines carry
    /// a zero datum.
    pub(crate) fn plan_ties(
        &self,
        join: &JoinPoint,
        (gap, shown): ((f64, f64), (f64, f64)),
        ties: &[Tie],
        facts: &ClosureFacts,
    ) -> Result<TiePlan, String> {
        let merges: Vec<&Tie> = ties
            .iter()
            .filter(|t| matches!(t.id, TieId::Merge(_)))
            .filter(|t| t.a.line_id != t.b.line_id && !join.is_join_tie(t.a, t.b))
            .collect();
        let adjacency_without = |torn: &HashSet<TieId>| -> Adjacency {
            let mut adjacency: Adjacency = HashMap::new();
            for tie in merges.iter().filter(|t| !torn.contains(&t.id)) {
                let (x, y) = (tie.a.line_id, tie.b.line_id);
                adjacency.entry(x).or_default().push((y, tie.id));
                adjacency.entry(y).or_default().push((x, tie.id));
            }
            adjacency
        };
        // The merges from M into a line that still reaches F without M.
        let loop_ties = |m: LineId, f: LineId| -> HashSet<TieId> {
            let adjacency = adjacency_without(&HashSet::new());
            let fixed = reach(&adjacency, f, m);
            adjacency
                .get(&m)
                .into_iter()
                .flatten()
                .filter(|(n, _)| fixed.contains(n))
                .map(|(_, id)| *id)
                .collect()
        };
        // The lines that carry a placement zero datum on a station of their
        // own, read once: every anchored-side question below is a lookup in it.
        // GPS fixes do not hold a side (#741).
        let datum_lines: HashSet<LineId> = self.placement_datum_lines();
        let anchored =
            |lines: &HashSet<LineId>| lines.iter().any(|s| datum_lines.contains(s));
        // The snapped closures a plan tears when `moving` moves off `fixed`
        // with `torn` merges torn: one across the two sides closes a loop
        // through the combined line, and any from a side that moves stops
        // being snapped.
        let snaps_torn = |moving: LineId,
                          fixed: LineId,
                          torn: &HashSet<TieId>,
                          shifting: bool|
         -> HashSet<TieId> {
            let adjacency = adjacency_without(torn);
            let side = reach(&adjacency, moving, fixed);
            let fixed_side = reach(&adjacency, fixed, moving);
            ties.iter()
                .filter(|t| matches!(t.id, TieId::Closure(_)))
                .filter(|t| !join.is_join_tie(t.a, t.b))
                .filter(|t| {
                    let (in_a, in_b) = (
                        side.contains(&t.a.line_id),
                        side.contains(&t.b.line_id),
                    );
                    let crosses = (in_a && fixed_side.contains(&t.b.line_id))
                        || (in_b && fixed_side.contains(&t.a.line_id));
                    crosses || (shifting && in_a != in_b)
                })
                .map(|t| t.id)
                .collect()
        };

        // A non-finite gap never reaches here (`pairing_touch` refuses
        // non-finite ends); checked again so a shift can never write one.
        //
        // Any gap moves a side, not only one past the tolerance (#740). The
        // combined line hangs the other line's stations off the kept
        // join station, so a side-by-side residual left out of the plan moved
        // the other line alone: a residual away from what is merged to it,
        // and a reopen (whose offset solve closes merges) then moved the
        // combined line. The residual is carried exactly like a shift, so
        // the anchor rule picks the side and what is merged to it moves along.
        // Float dust stays where it is: a gap within `OFFSET_DUST_M` in each
        // axis, the bound under which the offset solve corrects nothing, so
        // merged or truly coincident ends join as they always did.
        let beyond_dust =
            |v: (f64, f64)| v.0.abs() > Self::OFFSET_DUST_M || v.1.abs() > Self::OFFSET_DUST_M;
        let finite = [gap.0, gap.1, shown.0, shown.1]
            .iter()
            .all(|v| v.is_finite());
        // The two join stations' hand moves (manual offsets) differ (#755): the
        // joined station keeps one, so the other join station lands on it by
        // the gap as shown. Exactly: `shown` is `gap` itself when the two
        // agree, and whether anything moves is decided on the gap it moves by
        // below, so what is merged there lands on the joined station whenever
        // anything moves and stays within float dust of it otherwise. A merged
        // join's stations are one point already, and all that is merged there
        // are join ties, which never move.
        let carried = finite && !join.merged && shown != gap;
        // What is merged at a join station (a third line) moves with the
        // joined station when that station's hand move is dropped, so a side
        // moves when the gap as shown is past dust too, even with none in the
        // chain: the anchor rule picks which, as for a shift.
        let third = |group: &HashSet<StationRef>| {
            group
                .iter()
                .any(|at| at.line_id != join.kept && at.line_id != join.other)
        };
        let carries_branches =
            carried && beyond_dust(shown) && (third(&join.group_k) || third(&join.group_o));
        let gap_moves = finite && beyond_dust(gap);
        let shifting = gap_moves || (finite && carries_branches);
        let (moving, fixed, torn, held_by_datum, kept_moves_because) = if shifting {
            // What the closure-adjusted view holds the survey at: the side that
            // holds it must stay, or the whole cave shifts there by the gap.
            // Only when that view adjusts the joined survey: it solves a
            // closure there now (the join's own included), or the combine
            // turns a tie into one it will solve (a merge torn to close a
            // loop, a snapped closure made ordinary). A survey the view shows
            // as measured keeps the plain rule, and the other line moves.
            //
            // Both halves apply the across-one-shot skip (#737): a closure
            // that joins the two ends of one shot is skipped by the solve, so
            // it adjusts nothing. The first reads the skip set of the project
            // as it is (once per query); the second runs the joined survey's
            // own sweep on the plain plan's ties (`joined_survey_is_solved`),
            // because a torn tie can join the two ends of one shot only once
            // the two join stations are one.
            let component = self.get_connected_line_ids(join.kept);
            // A GPS fix is an observation the solve fits on its own (#741), so a
            // component holding one is adjusted with or without a closure.
            let solved_now = self.component_has_gps_fix(&component)
                || (self.component_has_eligible_closure(&component, &HashSet::new())
                    && self.component_has_eligible_closure(&component, facts.skipped(self)));
            let adjusted = solved_now || {
                let plain_torn = loop_ties(join.other, join.kept);
                let plain_snaps = snaps_torn(join.other, join.kept, &plain_torn, true);
                (!plain_torn.is_empty() || !plain_snaps.is_empty())
                    && self.joined_survey_is_solved(
                        join,
                        &component,
                        ties,
                        facts.closures(self),
                        &plain_torn,
                        &plain_snaps,
                    )
            };
            let anchor = if adjusted {
                self.adjustment_anchor_of(&component)
                    .map(|at| at.line_id)
            } else {
                None
            };
            Self::pick_moving_side(
                join,
                &loop_ties,
                &adjacency_without,
                &anchored,
                &datum_lines,
                anchor,
            )?
        } else {
            // Nothing moves (the two join stations are one point already, to
            // float dust): only the loops the combine would close inside one
            // line tear.
            (
                join.other,
                join.kept,
                loop_ties(join.other, join.kept),
                Vec::new(),
                None,
            )
        };
        // The side whose join station's hand move the joined station drops,
        // and that station.
        let picked = moving;
        let dropped_at = if picked == join.kept {
            join.join_k
        } else {
            join.join_o
        };
        // What is merged there moves with the joined station: each third
        // line still merged at it once the plan's ties tear, and what hangs
        // off it, never through the moving line. Merges form a forest
        // (`merge_stations` refuses a loop), so that is all of it; but nothing
        // breaks a merge loop an older or hand-edited file holds, and a part
        // merged at the dropped station that is also merged to the moving
        // line at another station cannot follow both. It stays with its
        // side, and its merge at the join becomes a loop closure carrying the
        // two hand moves' difference as its misclosure, as a loop the combine
        // closes through the combined line does.
        let (joined, looped): (HashSet<LineId>, HashSet<TieId>) = if carried && shifting {
            let adjacency = adjacency_without(&torn);
            let side = reach(&adjacency, moving, fixed);
            let kept_merges = || merges.iter().filter(|t| !torn.contains(&t.id));
            let mut joined = HashSet::new();
            let mut looped = HashSet::new();
            for tie in kept_merges() {
                let partner = if tie.a == dropped_at {
                    tie.b
                } else if tie.b == dropped_at {
                    tie.a
                } else {
                    continue;
                };
                if partner.line_id == moving || partner.line_id == fixed {
                    continue;
                }
                let part = reach(&adjacency, partner.line_id, moving);
                let back = kept_merges().any(|t| {
                    (part.contains(&t.a.line_id)
                        && t.b.line_id == moving
                        && t.b != dropped_at)
                        || (part.contains(&t.b.line_id)
                            && t.a.line_id == moving
                            && t.a != dropped_at)
                });
                if back {
                    looped.insert(tie.id);
                } else {
                    joined.extend(part);
                }
            }
            joined.retain(|s| side.contains(s) && *s != moving && *s != fixed);
            (joined, looped)
        } else {
            (HashSet::new(), HashSet::new())
        };
        // The other line's side, picked only to carry what is merged at its
        // join station, with nothing left there to carry (none there, its
        // merge torn to close a loop, or one of a merge loop): nothing moves
        // after all. (The kept side picked goes on moving, by the float dust
        // of the chain gap: the plan then says the joined station keeps the
        // other's hand move, which it does.)
        let idle = shifting && !gap_moves && picked == join.other && joined.is_empty();
        let (shifting, moving, fixed, mut torn, mut held_by_datum, kept_moves_because) = if idle {
            let torn = loop_ties(join.other, join.kept);
            (false, join.other, join.kept, torn, Vec::new(), None)
        } else {
            (
                shifting,
                moving,
                fixed,
                torn,
                held_by_datum,
                kept_moves_because,
            )
        };
        torn.extend(looped);
        let adjacency = adjacency_without(&torn);
        let side = reach(&adjacency, moving, fixed);
        // Snapped closures: one across the two sides closes a loop through the
        // combined line, and any from a side that moves stops being snapped.
        let snaps = snaps_torn(moving, fixed, &torn, shifting);
        // One the move tears into a part a datum merge is torn from tears
        // for that datum, like the merge (#756): the datum holds the part
        // where it is, and no loop through the combined line closes there.
        if !held_by_datum.is_empty() {
            let mut held_parts: HashSet<LineId> = HashSet::new();
            for tie in ties.iter().filter(|t| held_by_datum.contains(&t.id)) {
                let far = if tie.a.line_id == moving {
                    tie.b
                } else {
                    tie.a
                };
                if !held_parts.contains(&far.line_id) {
                    held_parts.extend(reach(&adjacency, far.line_id, moving));
                }
            }
            held_by_datum.extend(
                ties.iter()
                    .filter(|t| snaps.contains(&t.id))
                    .filter(|t| {
                        held_parts.contains(&t.a.line_id) || held_parts.contains(&t.b.line_id)
                    })
                    .map(|t| t.id),
            );
        }
        torn.extend(snaps);
        // A snap between what moves with the joined station and the rest of
        // the side: the two move apart now.
        if !joined.is_empty() {
            let apart: HashSet<TieId> = ties
                .iter()
                .filter(|t| matches!(t.id, TieId::Closure(_)) && !join.is_join_tie(t.a, t.b))
                .filter(|t| {
                    let (a, b) = (t.a.line_id, t.b.line_id);
                    (joined.contains(&a) && side.contains(&b) && !joined.contains(&b))
                        || (joined.contains(&b) && side.contains(&a) && !joined.contains(&a))
                })
                .map(|t| t.id)
                .collect();
            torn.extend(apart);
        }
        // The dropped station's move onto the joined station as shown.
        let dropped_hand_move = carried.then(|| {
            let toward = if dropped_at == join.join_o {
                shown
            } else {
                (-shown.0, -shown.1)
            };
            (dropped_at, toward)
        });

        let torn_ties: Vec<Tie> = ties
            .iter()
            .filter(|t| torn.contains(&t.id))
            .copied()
            .collect();
        // What the combine would leave joining the two ends of one shot is
        // removed instead (#746). The anchor verdict above does not change for
        // it: a closure that collapses a shot is one the joined survey's solve
        // skips, so it never made that survey "solved".
        let dropped = self.joined_across_one_shot(join, ties, &torn_ties, facts);
        let torn_ties: Vec<Tie> = torn_ties
            .into_iter()
            .filter(|t| !dropped.iter().any(|(d, _)| d.id == t.id))
            .collect();
        // What the combine's closure dedupe will drop for a closure the solve
        // takes (#749), from the same rule, on the closures this plan leaves.
        let replaced = if self.combine_may_replace(join.kept, join.other) {
            Self::closures_replaced(join, facts.closures(self), &torn_ties, &dropped)
        } else {
            Vec::new()
        };
        if !shifting {
            // Nothing moves only with the other line picked (the plain
            // rule, or idle above), so the joined station keeps the kept one's
            // hand move, as it always did.
            debug_assert_eq!(picked, join.other, "a still plan picked the kept side");
            return Ok(TiePlan {
                torn: torn_ties,
                kept_offset_wins: true,
                dropped,
                replaced,
                dropped_hand_move,
                ..TiePlan::default()
            });
        }
        let mut moved: Vec<LineId> = side
            .into_iter()
            .filter(|s| *s != moving && !joined.contains(s))
            .collect();
        let mut joined_moved: Vec<LineId> = joined.into_iter().collect();
        joined_moved.sort_unstable();
        moved.sort_unstable();
        moved.insert(0, moving);
        let sign = if moving == join.other { 1.0 } else { -1.0 };
        Ok(TiePlan {
            moving: Some(moving),
            moved,
            shift: (gap.0 * sign, gap.1 * sign),
            torn: torn_ties,
            held_by_datum,
            kept_moves_because,
            kept_offset_wins: moving == join.other,
            dropped,
            replaced,
            dropped_hand_move,
            joined_moved,
        })
    }

    /// Every loop closure, resolved, for [`Self::joined_survey_is_solved`].
    pub(crate) fn resolved_closures(&self) -> Vec<ResolvedClosure> {
        self.loop_closures
            .iter()
            .filter_map(|c| {
                Some(ResolvedClosure {
                    id: c.id,
                    a: self.resolve_key(&c.station_a)?,
                    b: self.resolve_key(&c.station_b)?,
                    eligible: Self::closure_is_eligible(c.strategy),
                    strategy: c.strategy,
                    aligned: c.is_aligned,
                })
            })
            .collect()
    }

    /// The across-one-shot sweep of the survey this combine joins - the
    /// combined project's `closure_knots`, run on this one: every merge but
    /// the `torn` ones, the two join stations as the one station they become,
    /// then the closures in `loop_closures` order (the join's own gone, the
    /// `snaps` made ordinary taking part), then the torn merges as the `Batch`
    /// closures `tear_ties` appends. Calls `visit` with each closure the solve
    /// would see, in that order, and whether it would skip it (it collapses a
    /// shot); a skipped one unites nothing, as in the real sweep. Stops when
    /// `visit` returns false. `O(n)`.
    ///
    /// The one model of the joined survey the anchor rule
    /// ([`Self::joined_survey_is_solved`]) and the closures the combine
    /// removes ([`Self::joined_across_one_shot`]) both read.
    fn joined_sweep(
        &self,
        join: &JoinPoint,
        ties: &[Tie],
        closures: &[ResolvedClosure],
        torn: &HashSet<TieId>,
        snaps: &HashSet<TieId>,
        mut visit: impl FnMut(Tie, bool) -> bool,
    ) {
        let mut knots = Knots::new(self);
        let merges = || ties.iter().filter(|t| matches!(t.id, TieId::Merge(_)));
        for tie in merges().filter(|t| !torn.contains(&t.id)) {
            knots.join(tie.a, tie.b);
        }
        knots.join(join.join_k, join.join_o);
        let mut sweep = |knots: &mut Knots, tie: Tie| -> bool {
            let skipped = knots.collapsed_shot(tie.a, tie.b).is_some();
            if !skipped {
                knots.join(tie.a, tie.b);
            }
            visit(tie, skipped)
        };
        for closure in closures {
            if join.is_join_tie(closure.a, closure.b) {
                continue; // the combine removes it
            }
            let id = TieId::Closure(closure.id);
            if !(closure.eligible || snaps.contains(&id)) {
                continue;
            }
            let tie = Tie {
                id,
                a: closure.a,
                b: closure.b,
            };
            if !sweep(&mut knots, tie) {
                return;
            }
        }
        for tie in merges().filter(|t| torn.contains(&t.id)) {
            if !sweep(&mut knots, *tie) {
                return;
            }
        }
    }

    /// Would the closure-adjusted view solve the survey this combine joins,
    /// were `torn` merges torn and `snaps` snapped closures made ordinary
    /// (the plain plan's, where the other line moves)? Solved when one
    /// closure in `component` takes part in the joined survey's sweep
    /// ([`Self::joined_sweep`]) without collapsing a shot - what
    /// `plan_adjustment_anchors` asks of the combined project. Reached only
    /// for a shifting plan with something to solve.
    fn joined_survey_is_solved(
        &self,
        join: &JoinPoint,
        component: &HashSet<LineId>,
        ties: &[Tie],
        closures: &[ResolvedClosure],
        torn: &HashSet<TieId>,
        snaps: &HashSet<TieId>,
    ) -> bool {
        let mut solved = false;
        self.joined_sweep(join, ties, closures, torn, snaps, |tie, skipped| {
            solved = !skipped
                && component.contains(&tie.a.line_id)
                && component.contains(&tie.b.line_id);
            !solved
        });
        solved
    }

    /// The ties a combine tearing `torn_ties` would leave as loop closures
    /// that join the two ends of one shot, and that the combine therefore
    /// removes (#746), each with what it is now: a merge or snapped closure
    /// of `torn_ties`, or a closure already there. A closure the project
    /// skips now is left alone (it keeps its own warning), as Delete with
    /// shift leaves one: the same set `closures_newly_across_one_shot` names
    /// after the combine. From the joined
    /// survey's sweep ([`Self::joined_sweep`]), so the dialog and the combine
    /// read one answer.
    ///
    /// Nothing changes for the sweep when no tie tears and the join stations
    /// are already one point (merged), or when no closure is solved at all:
    /// then this is empty without the `O(n)` sweep.
    fn joined_across_one_shot(
        &self,
        join: &JoinPoint,
        ties: &[Tie],
        torn_ties: &[Tie],
        facts: &ClosureFacts,
    ) -> Vec<(Tie, CombineRemovedKind)> {
        let closures = facts.closures(self);
        if torn_ties.is_empty() && (join.merged || !closures.iter().any(|c| c.eligible)) {
            return Vec::new();
        }
        let torn: HashSet<TieId> = torn_ties
            .iter()
            .filter(|t| matches!(t.id, TieId::Merge(_)))
            .map(|t| t.id)
            .collect();
        let snaps: HashSet<TieId> = torn_ties
            .iter()
            .filter(|t| matches!(t.id, TieId::Closure(_)))
            .map(|t| t.id)
            .collect();
        let skipped_now = facts.skipped(self);
        let mut dropped = Vec::new();
        self.joined_sweep(join, ties, closures, &torn, &snaps, |tie, skipped| {
            if skipped {
                let kind = match tie.id {
                    TieId::Merge(_) => Some(CombineRemovedKind::Merge),
                    // Skipped already (an eligible snap counts): it carries
                    // its own warning, and the combine answers only for what
                    // it causes.
                    TieId::Closure(id) if skipped_now.contains(&id) => None,
                    TieId::Closure(_) if snaps.contains(&tie.id) => {
                        Some(CombineRemovedKind::SnappedClosure)
                    }
                    TieId::Closure(_) => Some(CombineRemovedKind::LoopClosure),
                };
                dropped.extend(kind.map(|kind| (tie, kind)));
            }
            true
        });
        dropped
    }

    /// Which side moves when the join has to close by a shift, and the merges
    /// that tear for it:
    /// - a side a zero datum holds stays;
    /// - between two free sides, the one holding `anchor` (the line the
    ///   closure-adjusted view is held at, when no datum is in the survey)
    ///   stays, or Plan and Export would shift the whole cave by the gap;
    /// - otherwise the other line's side moves.
    ///
    /// With both sides held by datums, the moving line (one that carries
    /// no datum itself) also tears its merges towards its datum sides.
    fn pick_moving_side(
        join: &JoinPoint,
        loop_ties: &dyn Fn(LineId, LineId) -> HashSet<TieId>,
        adjacency_without: &dyn Fn(&HashSet<TieId>) -> Adjacency,
        anchored: &dyn Fn(&HashSet<LineId>) -> bool,
        datum_lines: &HashSet<LineId>,
        anchor: Option<LineId>,
    ) -> Result<MovingSide, String> {
        let sides = [(join.other, join.kept), (join.kept, join.other)];
        // The sides no zero datum holds, in preference order.
        let mut free: Vec<(LineId, LineId, HashSet<TieId>, bool)> = Vec::new();
        for (m, f) in sides {
            let torn = loop_ties(m, f);
            let side = reach(&adjacency_without(&torn), m, f);
            if !anchored(&side) {
                let holds_anchor = anchor.is_some_and(|a| side.contains(&a));
                free.push((m, f, torn, holds_anchor));
            }
        }
        let pick = free
            .iter()
            .position(|(_, _, _, holds)| !holds)
            .or(if free.is_empty() { None } else { Some(0) });
        if let Some(pick) = pick {
            let other_free = free.iter().any(|(m, ..)| *m == join.other);
            let (m, f, torn, _) = free.swap_remove(pick);
            let because = (m == join.kept).then_some(if other_free {
                KeptMovesReason::AdjustmentAnchor
            } else {
                KeptMovesReason::ZeroDatum
            });
            return Ok((m, f, torn, Vec::new(), because));
        }
        for (m, f) in sides {
            if datum_lines.contains(&m) {
                continue;
            }
            let mut torn = loop_ties(m, f);
            let adjacency = adjacency_without(&torn);
            // Each part of the graph that hangs off M is walked once, however
            // many of M's merges lead into it.
            let mut anchored_part: HashMap<LineId, bool> = HashMap::new();
            let mut datum_ties: Vec<TieId> = Vec::new();
            for (n, id) in adjacency.get(&m).into_iter().flatten() {
                let held = if let Some(held) = anchored_part.get(n) {
                    *held
                } else {
                    let part = reach(&adjacency, *n, m);
                    let held = anchored(&part);
                    anchored_part.extend(part.into_iter().map(|s| (s, held)));
                    held
                };
                if held {
                    datum_ties.push(*id);
                }
            }
            torn.extend(datum_ties.iter().copied());
            let because = (m == join.kept).then_some(KeptMovesReason::ZeroDatum);
            return Ok((m, f, torn, datum_ties, because));
        }
        Err(
            "Both lines are placed by a zero datum on one of their stations: merge them \
             first, so the later datum becomes a GPS fix"
                .to_string(),
        )
    }

    /// The plan as the dialog shows it.
    pub(crate) fn plan_for_display(
        plan: &TiePlan,
        kept: LineId,
        azimuth_rotation_deg: Option<f64>,
        join_offset_change_m: Option<f64>,
    ) -> CombinePlan {
        CombinePlan {
            kept,
            moving_line: plan.moving,
            kept_moves_because: plan.kept_moves_because,
            moved_lines: plan.moved.clone(),
            shift_m: plan.shift.0.hypot(plan.shift.1),
            torn_ties: plan
                .torn
                .iter()
                .map(|tie| TornTie {
                    station_a: tie.a,
                    station_b: tie.b,
                    kind: match tie.id {
                        TieId::Merge(_) => TornTieKind::Merge,
                        TieId::Closure(_) => TornTieKind::SnappedClosure,
                    },
                    reason: if plan.held_by_datum.contains(&tie.id) {
                        TornTieReason::ZeroDatum
                    } else {
                        TornTieReason::Loop
                    },
                })
                .collect(),
            azimuth_rotation_deg,
            join_offset_change_m,
            closures_removed_across_one_shot: (!plan.dropped.is_empty()).then(|| {
                plan.dropped
                    .iter()
                    .map(|(tie, kind)| CombineRemovedClosure {
                        station_a: tie.a,
                        station_b: tie.b,
                        kind: *kind,
                    })
                    .collect()
            }),
            closures_replaced: (!plan.replaced.is_empty()).then(|| plan.replaced.clone()),
            moved_with_joined_station: match plan.dropped_hand_move {
                Some((station, (dx, dy))) if !plan.joined_moved.is_empty() => {
                    Some(CombineJoinedMove {
                        station,
                        lines: plan.joined_moved.clone(),
                        shift_m: dx.hypot(dy),
                    })
                }
                _ => None,
            },
        }
    }

    /// Turn `torn` into ordinary loop closures: a merge is replaced by a
    /// `Batch` closure between the same two stored keys, a snapped closure
    /// becomes a `Batch` one. Remove the `dropped` ties outright (#746): the
    /// closure each is or would become joins the two ends of one shot. One
    /// pass over the merges for both, since a merge is named by its index.
    /// Returns every closure touched, by id, with the tie of `torn` it came
    /// from: [`Self::remeasure_closures`] fills in their figures once the
    /// geometry is final, and the combine's backstop names one by its tie.
    pub(crate) fn tear_ties(&mut self, torn: &[Tie], dropped: &[Tie]) -> Vec<(u64, Tie)> {
        let dropped_merges: HashSet<usize> = dropped
            .iter()
            .filter_map(|t| match t.id {
                TieId::Merge(i) => Some(i),
                TieId::Closure(_) => None,
            })
            .collect();
        let dropped_closures: HashSet<u64> = dropped
            .iter()
            .filter_map(|t| match t.id {
                TieId::Closure(id) => Some(id),
                TieId::Merge(_) => None,
            })
            .collect();
        if !dropped_closures.is_empty() {
            self.loop_closures
                .retain(|c| !dropped_closures.contains(&c.id));
        }
        let torn_merges: HashMap<usize, Tie> = torn
            .iter()
            .filter_map(|t| match t.id {
                TieId::Merge(i) => Some((i, *t)),
                TieId::Closure(_) => None,
            })
            .collect();
        let torn_closures: HashMap<u64, Tie> = torn
            .iter()
            .filter_map(|t| match t.id {
                TieId::Closure(id) => Some((id, *t)),
                TieId::Merge(_) => None,
            })
            .collect();
        let mut touched: Vec<(u64, Tie)> = Vec::new();
        for closure in &mut self.loop_closures {
            if let Some(tie) = torn_closures.get(&closure.id) {
                closure.strategy = ClosureStrategy::Batch;
                closure.is_aligned = false;
                touched.push((closure.id, *tie));
            }
        }
        let mut index = 0;
        let mut removed = Vec::new();
        self.station_merges.retain(|merge| {
            index += 1;
            if let Some(tie) = torn_merges.get(&(index - 1)) {
                removed.push((merge.clone(), *tie));
                false
            } else {
                !dropped_merges.contains(&(index - 1))
            }
        });
        self.normalize_next_closure_id();
        for (merge, tie) in removed {
            let id = self.next_closure_id;
            self.next_closure_id += 1;
            self.loop_closures.push(LoopClosure {
                id,
                station_a: merge.station_a,
                station_b: merge.station_b,
                path_length: 0.0,
                misclosure_distance: 0.0,
                error_percentage: 0.0,
                horizontal_misclosure: 0.0,
                vertical_misclosure: 0.0,
                strategy: ClosureStrategy::Batch,
                is_aligned: false,
            });
            touched.push((id, tie));
        }
        touched
    }

    /// Measure the closures `ids` again off the current geometry, each with
    /// none of them present: a closure's path length must not run through
    /// itself (`create_loop_closure` measures before it records, too).
    pub(crate) fn remeasure_closures(&mut self, ids: &HashSet<u64>) {
        if ids.is_empty() {
            return;
        }
        let mut taken: Vec<(usize, LoopClosure)> = Vec::new();
        let mut kept: Vec<LoopClosure> = Vec::with_capacity(self.loop_closures.len());
        for (index, closure) in std::mem::take(&mut self.loop_closures)
            .into_iter()
            .enumerate()
        {
            if ids.contains(&closure.id) {
                taken.push((index, closure));
            } else {
                kept.push(closure);
            }
        }
        self.loop_closures = kept;
        for (_, closure) in &mut taken {
            let (Some(a), Some(b)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) else {
                continue;
            };
            if let Ok(metrics) = self.compute_closure_metrics(a, b) {
                closure.path_length = metrics.path_length;
                closure.misclosure_distance = metrics.misclosure_distance;
                closure.error_percentage = metrics.error_percentage;
                closure.horizontal_misclosure = metrics.horizontal_misclosure;
                closure.vertical_misclosure = metrics.vertical_misclosure;
            }
        }
        for (at, closure) in taken {
            let at = at.min(self.loop_closures.len());
            self.loop_closures.insert(at, closure);
        }
    }
}
