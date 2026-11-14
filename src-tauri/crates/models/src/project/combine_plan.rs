//! Combine Sections, what has to give so two sections that meet can become
//! one: which side moves to close the join, by how much, and which rigid ties
//! become ordinary loop closures. Read-only; the candidate query reports it
//! (one plan per choice of kept section) and `combine_sections` applies it.
//!
//! The rules, from the merge forest the app keeps (`merge_stations` refuses a
//! merge between sections that are already rigidly connected, so merges never
//! close a loop; a loop is closed by a loop closure):
//!
//! - **Join ties** make the two join stations one point: the merges and
//!   closures between the two join stations' merge groups (for a merged join,
//!   every tie inside its one group). The combine folds them into the joined
//!   station, so they are never torn. A tie that reaches a station of either
//!   section OTHER than its join station is not one: that section passes
//!   through the join point again, and the tie would tie the combined section
//!   to itself.
//! - **Loop ties.** A merge from the moving section M to a section that still
//!   reaches the fixed one F without M is the other end of a loop the combine
//!   closes through the combined section: it becomes a loop closure (the one
//!   nearest M, so the least moves). Merges form a forest, so there is at most
//!   one per path. A snapped (aligned, rigid) closure between M's side and
//!   F's side becomes an ordinary one for the same reason.
//! - **The shift.** Two ends tied by a loop closure are apart by its
//!   misclosure; M's side (M and everything still merged to it) moves by it.
//!   M is the other section, unless its side is held by a zero datum and the
//!   kept section's is not, or (no datum deciding) M's side holds the station
//!   the closure-adjusted view holds the survey at
//!   (`Project::adjustment_anchor_of`) and the kept side does not: moving it
//!   would shift the whole adjusted survey by the gap. When both sides are held, the merges from M to the
//!   datum sides also become closures (a closure between two anchored groups
//!   is legitimate; a merge is not). Only when BOTH sections carry a zero datum
//!   on one of their own stations is the combine refused: the combined section
//!   would hold two.
//!   Moving M's side tears every snapped closure from it to anything that
//!   stays, so those become ordinary closures too.

use std::collections::{HashMap, HashSet};

use crate::{
    ClosureStrategy, CombinePlan, KeptMovesReason, LoopClosure, SectionId, StationRef, TornTie,
    TornTieKind, TornTieReason,
};

use super::closure_shots::Knots;
use super::combine_candidate::COMBINE_TOLERANCE_M;
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
/// its strategy, for the anchor rule's solve check.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedClosure {
    pub id: u64,
    pub a: StationRef,
    pub b: StationRef,
    pub eligible: bool,
}

/// What the anchor rule reads about loop closures, worked out once per
/// candidate query (the station menu and Combine Sections plan two choices of
/// kept section for every partner) and only when a shifting plan needs it:
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

/// The point two sections are joined at, from the kept section's side.
#[derive(Debug, Clone)]
pub(crate) struct JoinPoint {
    pub kept: SectionId,
    pub other: SectionId,
    pub join_k: StationRef,
    pub join_o: StationRef,
    /// The merge group of each join station (one group when merged).
    pub group_k: HashSet<StationRef>,
    pub group_o: HashSet<StationRef>,
    pub merged: bool,
}

impl JoinPoint {
    /// A station of one of the two sections other than its join station.
    fn is_loose(&self, at: StationRef) -> bool {
        (at.section_id == self.kept || at.section_id == self.other)
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

/// What `combine_sections` changes before it joins the shots.
#[derive(Debug, Clone, Default)]
pub(crate) struct TiePlan {
    /// The section whose side moves, when the ends are more than the
    /// tolerance apart.
    pub moving: Option<SectionId>,
    /// The sections that move, the moving section first.
    pub moved: Vec<SectionId>,
    /// The translation applied to every moved section.
    pub shift: (f64, f64),
    /// The ties that become ordinary loop closures.
    pub torn: Vec<Tie>,
    /// Which of `torn` tear because a zero datum holds the section on their
    /// far side (the rest close a loop through the combined section).
    pub held_by_datum: Vec<TieId>,
    /// Why the kept section's side moves, when it does.
    pub kept_moves_because: Option<KeptMovesReason>,
    /// Whether the joined station keeps the kept section's manual offset
    /// (the fixed side's: false when the kept section is the one that moves).
    pub kept_offset_wins: bool,
}

type Adjacency = HashMap<SectionId, Vec<(SectionId, TieId)>>;

/// The moving section, the fixed one, the ties to tear, which of those tear
/// for a zero datum, and why the kept side moves (when it does).
type MovingSide = (
    SectionId,
    SectionId,
    HashSet<TieId>,
    Vec<TieId>,
    Option<KeptMovesReason>,
);

/// Every section reachable from `start` over `adjacency` without entering
/// `blocked` (`start` included).
fn reach(adjacency: &Adjacency, start: SectionId, blocked: SectionId) -> HashSet<SectionId> {
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

    /// Does a zero datum sit on one of `section`'s own stations?
    pub(crate) fn carries_zero_datum(&self, section: SectionId) -> bool {
        self.zero_datums.iter().any(|datum| {
            self.resolve_key(&datum.station_ref)
                .is_some_and(|at| at.section_id == section)
        })
    }

    /// Plan the shift and the torn ties for joining `join`, whose join stations'
    /// chain positions are `gap` apart (the kept one minus the other one: the
    /// translation that brings the other section's join station onto the kept
    /// one's), against `ties` ([`Self::rigid_ties`]). `Err` is the refusal:
    /// both sections carry a zero datum.
    pub(crate) fn plan_ties(
        &self,
        join: &JoinPoint,
        gap: (f64, f64),
        ties: &[Tie],
        facts: &ClosureFacts,
    ) -> Result<TiePlan, String> {
        let merges: Vec<&Tie> = ties
            .iter()
            .filter(|t| matches!(t.id, TieId::Merge(_)))
            .filter(|t| t.a.section_id != t.b.section_id && !join.is_join_tie(t.a, t.b))
            .collect();
        let adjacency_without = |torn: &HashSet<TieId>| -> Adjacency {
            let mut adjacency: Adjacency = HashMap::new();
            for tie in merges.iter().filter(|t| !torn.contains(&t.id)) {
                let (x, y) = (tie.a.section_id, tie.b.section_id);
                adjacency.entry(x).or_default().push((y, tie.id));
                adjacency.entry(y).or_default().push((x, tie.id));
            }
            adjacency
        };
        // The merges from M into a section that still reaches F without M.
        let loop_ties = |m: SectionId, f: SectionId| -> HashSet<TieId> {
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
        // The sections that carry a zero datum on a station of their own, read
        // once: every anchored-side question below is a lookup in it.
        let datum_sections: HashSet<SectionId> = self
            .zero_datums
            .iter()
            .filter_map(|datum| self.resolve_key(&datum.station_ref))
            .map(|at| at.section_id)
            .collect();
        let anchored =
            |sections: &HashSet<SectionId>| sections.iter().any(|s| datum_sections.contains(s));
        // The snapped closures a plan tears when `moving` moves off `fixed`
        // with `torn` merges torn: one across the two sides closes a loop
        // through the combined section, and any from a side that moves stops
        // being snapped.
        let snaps_torn = |moving: SectionId,
                          fixed: SectionId,
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
                        side.contains(&t.a.section_id),
                        side.contains(&t.b.section_id),
                    );
                    let crosses = (in_a && fixed_side.contains(&t.b.section_id))
                        || (in_b && fixed_side.contains(&t.a.section_id));
                    crosses || (shifting && in_a != in_b)
                })
                .map(|t| t.id)
                .collect()
        };

        // A non-finite gap never reaches here (`pairing_touch` refuses
        // non-finite ends); checked again so a shift can never write one.
        let shifting =
            gap.0.is_finite() && gap.1.is_finite() && gap.0.hypot(gap.1) > COMBINE_TOLERANCE_M;
        let (moving, fixed, mut torn, held_by_datum, kept_moves_because) = if shifting {
            // What the closure-adjusted view holds the survey at: the side that
            // holds it must stay, or the whole cave shifts there by the gap.
            // Only when that view adjusts the joined survey: it solves a
            // closure there now (the join's own included), or the combine
            // turns a tie into one it will solve (a merge torn to close a
            // loop, a snapped closure made ordinary). A survey the view shows
            // as measured keeps the plain rule, and the other section moves.
            //
            // Both halves apply the across-one-shot skip (#737): a closure
            // that joins the two ends of one shot is skipped by the solve, so
            // it adjusts nothing. The first reads the skip set of the project
            // as it is (once per query); the second runs the joined survey's
            // own sweep on the plain plan's ties (`joined_survey_is_solved`),
            // because a torn tie can join the two ends of one shot only once
            // the two join stations are one.
            let component = self.get_connected_section_ids(join.kept);
            let solved_now = self.component_has_eligible_closure(&component, &HashSet::new())
                && self.component_has_eligible_closure(&component, facts.skipped(self));
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
                    .map(|at| at.section_id)
            } else {
                None
            };
            Self::pick_moving_side(
                join,
                &loop_ties,
                &adjacency_without,
                &anchored,
                &datum_sections,
                anchor,
            )?
        } else {
            // Nothing moves (beyond the tolerance the join already allows):
            // only the loops the combine would close inside one section tear.
            (
                join.other,
                join.kept,
                loop_ties(join.other, join.kept),
                Vec::new(),
                None,
            )
        };
        let side = reach(&adjacency_without(&torn), moving, fixed);
        // Snapped closures: one across the two sides closes a loop through the
        // combined section, and any from a side that moves stops being snapped.
        let snaps = snaps_torn(moving, fixed, &torn, shifting);
        torn.extend(snaps);

        let torn_ties: Vec<Tie> = ties
            .iter()
            .filter(|t| torn.contains(&t.id))
            .copied()
            .collect();
        if !shifting {
            return Ok(TiePlan {
                torn: torn_ties,
                kept_offset_wins: true,
                ..TiePlan::default()
            });
        }
        let mut moved: Vec<SectionId> = side.into_iter().filter(|s| *s != moving).collect();
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
                })
            })
            .collect()
    }

    /// Would the closure-adjusted view solve the survey this combine joins,
    /// were `torn` merges torn and `snaps` snapped closures made ordinary
    /// (the plain plan's, where the other section moves)? The
    /// combined project's across-one-shot sweep (`closure_knots`), run on this
    /// one: every merge but the torn ones, the two join stations as the one
    /// station they become, then the closures in `loop_closures` order (the
    /// join's own gone, the made-ordinary ones taking part), then the torn
    /// merges as the `Batch` closures `tear_ties` appends. The survey is solved
    /// when one closure in `component` takes part without collapsing a shot -
    /// what `plan_adjustment_anchors` asks of the combined project. `O(n)`;
    /// reached only for a shifting plan with something to solve.
    fn joined_survey_is_solved(
        &self,
        join: &JoinPoint,
        component: &HashSet<SectionId>,
        ties: &[Tie],
        closures: &[ResolvedClosure],
        torn: &HashSet<TieId>,
        snaps: &HashSet<TieId>,
    ) -> bool {
        let mut knots = Knots::new(self);
        let merges = || ties.iter().filter(|t| matches!(t.id, TieId::Merge(_)));
        for tie in merges().filter(|t| !torn.contains(&t.id)) {
            knots.join(tie.a, tie.b);
        }
        knots.join(join.join_k, join.join_o);
        let in_component = |a: StationRef, b: StationRef| {
            component.contains(&a.section_id) && component.contains(&b.section_id)
        };
        let takes = |knots: &mut Knots, a: StationRef, b: StationRef| -> bool {
            if knots.collapsed_shot(a, b).is_some() {
                return false; // skipped: it would join the two ends of one shot
            }
            knots.join(a, b);
            in_component(a, b)
        };
        for closure in closures {
            if join.is_join_tie(closure.a, closure.b) {
                continue; // the combine removes it
            }
            if !(closure.eligible || snaps.contains(&TieId::Closure(closure.id))) {
                continue;
            }
            if takes(&mut knots, closure.a, closure.b) {
                return true;
            }
        }
        merges()
            .filter(|t| torn.contains(&t.id))
            .any(|t| takes(&mut knots, t.a, t.b))
    }

    /// Which side moves when the join has to close by a shift, and the merges
    /// that tear for it:
    /// - a side a zero datum holds stays;
    /// - between two free sides, the one holding `anchor` (the section the
    ///   closure-adjusted view is held at, when no datum is in the survey)
    ///   stays, or Plan and Export would shift the whole cave by the gap;
    /// - otherwise the other section's side moves.
    ///
    /// With both sides held by datums, the moving section (one that carries
    /// no datum itself) also tears its merges towards its datum sides.
    fn pick_moving_side(
        join: &JoinPoint,
        loop_ties: &dyn Fn(SectionId, SectionId) -> HashSet<TieId>,
        adjacency_without: &dyn Fn(&HashSet<TieId>) -> Adjacency,
        anchored: &dyn Fn(&HashSet<SectionId>) -> bool,
        datum_sections: &HashSet<SectionId>,
        anchor: Option<SectionId>,
    ) -> Result<MovingSide, String> {
        let sides = [(join.other, join.kept), (join.kept, join.other)];
        // The sides no zero datum holds, in preference order.
        let mut free: Vec<(SectionId, SectionId, HashSet<TieId>, bool)> = Vec::new();
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
            if datum_sections.contains(&m) {
                continue;
            }
            let mut torn = loop_ties(m, f);
            let adjacency = adjacency_without(&torn);
            // Each part of the graph that hangs off M is walked once, however
            // many of M's merges lead into it.
            let mut anchored_part: HashMap<SectionId, bool> = HashMap::new();
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
            "Both sections carry a zero datum on one of their stations: a section can hold \
             only one"
                .to_string(),
        )
    }

    /// The plan as the dialog shows it.
    pub(crate) fn plan_for_display(
        plan: &TiePlan,
        kept: SectionId,
        azimuth_rotation_deg: Option<f64>,
        join_offset_change_m: Option<f64>,
    ) -> CombinePlan {
        CombinePlan {
            kept,
            moving_section: plan.moving,
            kept_moves_because: plan.kept_moves_because,
            moved_sections: plan.moved.clone(),
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
        }
    }

    /// Turn `torn` into ordinary loop closures: a merge is replaced by a
    /// `Batch` closure between the same two stored keys, a snapped closure
    /// becomes a `Batch` one. Returns the ids of every closure touched, whose
    /// figures [`Self::remeasure_closures`] fills in once the geometry is final.
    pub(crate) fn tear_ties(&mut self, torn: &[Tie]) -> Vec<u64> {
        let merge_indices: HashSet<usize> = torn
            .iter()
            .filter_map(|t| match t.id {
                TieId::Merge(i) => Some(i),
                TieId::Closure(_) => None,
            })
            .collect();
        let closure_ids: HashSet<u64> = torn
            .iter()
            .filter_map(|t| match t.id {
                TieId::Closure(id) => Some(id),
                TieId::Merge(_) => None,
            })
            .collect();
        let mut touched: Vec<u64> = Vec::new();
        for closure in self
            .loop_closures
            .iter_mut()
            .filter(|c| closure_ids.contains(&c.id))
        {
            closure.strategy = ClosureStrategy::Batch;
            closure.is_aligned = false;
            touched.push(closure.id);
        }
        let mut index = 0;
        let mut removed = Vec::new();
        self.station_merges.retain(|merge| {
            index += 1;
            if merge_indices.contains(&(index - 1)) {
                removed.push(merge.clone());
                false
            } else {
                true
            }
        });
        self.normalize_next_closure_id();
        for merge in removed {
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
            touched.push(id);
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
