//! Combine Sections making two loop closures identical (#749): which one
//! stays, applied to the combined project (`Project::dedupe_closures_in`) and
//! predicted by the plan the dialog shows (`Project::closures_replaced`),
//! both through the one rule [`closure_dedupe`].
//!
//! Once the two join stations are one, two closures can tie the same two
//! stations: a tie the combine tears (a merge becomes a `Batch` closure,
//! appended last) can land on a legacy closure's stations, or on a snapped
//! one's. Of each such set the first closure the solve takes
//! ([`Project::closure_is_eligible`]) stays, or the first one when the solve
//! takes none of them; the others go, by position, the way the plan counts
//! them.
//!
//! Why an eligible one wins: the plan's model of the joined survey
//! (`joined_sweep`, which the anchor rule and the across-one-shot removal
//! read) sweeps every closure before this dedupe, the ones the solve ties
//! only, in vector order, and only an eligible one gives the survey something
//! to solve. Keeping the first one instead dropped, whenever an ineligible
//! closure came first, the only closure the solve would have joined there:
//! the plan moved a side for a solved survey that the Closures Computed View
//! then showed unadjusted.
//!
//! With this rule the stations the solve joins are the same with or without
//! the dropped closures, so the plan's model holds after the dedupe too: a
//! dropped ineligible closure that is not a snap never takes part in the
//! solve, and a dropped eligible one comes after an eligible closure on the
//! same two stations, which either joined them already (the later one changes
//! nothing) or collapsed a shot between their two groups, in which case the
//! later one collapses one too or finds them joined by something in between -
//! it never joins anything itself. A snap (an aligned closure the solve ties
//! as one point, #736) dropped for an EARLIER eligible one is the same later
//! duplicate; one dropped for a LATER eligible one would have joined the pair
//! where it stands, so the plan's model leaves it out where it stands and
//! lets that later closure join the pair (`Project::joined_sweep`).
//! Connectivity and the held gauges see the same pair either way. What goes
//! with a dropped snapped (aligned) closure is its rigidity and its depth
//! co-location, which the plan never models after the join: its two stations
//! stay on one point horizontally, but each reads its own depth from then on.
//!
//! A closure dropped for one the solve takes while it is not one itself
//! changes something the user can see (a legacy closure the view ignored, a
//! snap and its shared depth), so the plan lists those (`closures_replaced`)
//! and the dialog shows them. Two closures the view treats alike keep the
//! first silently: nothing changes for the view.

use std::collections::{HashMap, HashSet};

use crate::{
    ClosureStrategy, CombineRemovedKind, CombineReplacedClosure, CombineReplacedKind, LoopClosure,
    SectionId, StationRef,
};

use super::combine_plan::{JoinPoint, ResolvedClosure, Tie, TieId};
use super::Project;

/// One loop closure as [`closure_dedupe`] sees it: its two stations once the
/// combine is done (in either order) and whether the solve takes it. `None`
/// for a closure the dedupe leaves alone (not on the combined section, or one
/// whose stations do not resolve).
pub(crate) type DedupeEntry = Option<((StationRef, StationRef), bool)>;

/// The two stations of a pair in a fixed order.
fn ordered((a, b): (StationRef, StationRef)) -> (StationRef, StationRef) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The dedupe rule (see the module docs), on `entries` in `loop_closures`
/// order: of the entries on the same two stations, the first the solve takes
/// stays, or the first one when it takes none. Returns, per entry, the index
/// of the entry that stays on its two stations when this one goes, and
/// `None` for an entry that stays or is left alone. `O(n)`.
pub(crate) fn closure_dedupe(entries: &[DedupeEntry]) -> Vec<Option<usize>> {
    let eligible = |at: usize| matches!(entries.get(at), Some(Some((_, true))));
    let mut kept: HashMap<(StationRef, StationRef), usize> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some((pair, is_eligible)) = entry else {
            continue;
        };
        let pair = ordered(*pair);
        match kept.get(&pair) {
            None => {
                kept.insert(pair, index);
            }
            Some(&first) if *is_eligible && !eligible(first) => {
                kept.insert(pair, index);
            }
            Some(_) => {}
        }
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let (pair, _) = entry.as_ref()?;
            let stays = *kept.get(&ordered(*pair))?;
            (stays != index).then_some(stays)
        })
        .collect()
}

/// What a closure the solve does not take was, for the dialog (#749): a
/// snapped (aligned) closure of any strategy, a `Move` closure no longer
/// aligned, or an older kind (proportional or equal distribution).
pub(crate) fn replaced_kind(strategy: ClosureStrategy, is_aligned: bool) -> CombineReplacedKind {
    if is_aligned {
        CombineReplacedKind::SnappedClosure
    } else if strategy == ClosureStrategy::Move {
        CombineReplacedKind::FormerSnap
    } else {
        CombineReplacedKind::LegacyClosure
    }
}

/// Where a closure of the plan's model of the combined closure list came
/// from: a closure already there (`snap_torn` when the combine tears it into
/// an ordinary one), or a merge the combine tears into one.
enum Origin<'a> {
    Closure(&'a ResolvedClosure, bool),
    Merge(&'a Tie),
}

impl Project {
    /// Drop loop closures onto `section_id` that the combine made identical,
    /// by [`closure_dedupe`] (see the module docs). Each kept closure stays
    /// where it was in `loop_closures`.
    ///
    /// Returns each closure it dropped for one the solve takes while it is
    /// not one itself, with the closure that stays on its two stations: what
    /// the plan lists as replaced (`closures_replaced`).
    pub(super) fn dedupe_closures_in(
        &mut self,
        section_id: SectionId,
    ) -> Vec<(LoopClosure, LoopClosure)> {
        let entries: Vec<DedupeEntry> = self
            .loop_closures
            .iter()
            .map(|closure| {
                if closure.station_a.section_id != section_id
                    && closure.station_b.section_id != section_id
                {
                    return None;
                }
                Some((
                    (
                        self.resolve_key(&closure.station_a)?,
                        self.resolve_key(&closure.station_b)?,
                    ),
                    Self::closure_is_eligible(closure.strategy),
                ))
            })
            .collect();
        let stays = closure_dedupe(&entries);
        #[cfg(test)]
        count_closure_dedupes(&self.loop_closures, &stays);
        let replaced: Vec<(LoopClosure, LoopClosure)> = stays
            .iter()
            .enumerate()
            .filter_map(|(index, stays)| {
                let dropped = self.loop_closures.get(index)?;
                let kept = self.loop_closures.get((*stays)?)?;
                (!Self::closure_is_eligible(dropped.strategy)
                    && Self::closure_is_eligible(kept.strategy))
                .then(|| (dropped.clone(), kept.clone()))
            })
            .collect();
        if stays.iter().any(Option::is_some) {
            let mut stays = stays.into_iter();
            self.loop_closures
                .retain(|_| stays.next().flatten().is_none());
        }
        replaced
    }

    /// The closures a combine at `join` drops for one the solve takes on the
    /// same two stations while it is not one itself (#749), each with the
    /// closure that replaces it, by their stations before the combine. The
    /// dedupe's own rule ([`closure_dedupe`]) run on the plan's model of the
    /// combined closure list, which is the list the combine builds: every
    /// closure in `closures` (the project's, in order) except the join's own
    /// (`JoinPoint::is_join_tie`) and the ones `dropped` removes across one
    /// shot (#746), a snapped one of `torn_ties` made ordinary, then each
    /// merge of `torn_ties` as the `Batch` closure `tear_ties` appends. A
    /// station keeps its identity through the combine except the other
    /// section's join station, which becomes the kept one's.
    ///
    /// `O(closures)`, and nothing at all for a combine that touches no
    /// closure the solve does not take ([`Self::combine_may_replace`]).
    pub(crate) fn closures_replaced(
        join: &JoinPoint,
        closures: &[ResolvedClosure],
        torn_ties: &[Tie],
        dropped: &[(Tie, CombineRemovedKind)],
    ) -> Vec<CombineReplacedClosure> {
        let removed: HashSet<TieId> = dropped.iter().map(|(tie, _)| tie.id).collect();
        let torn_snaps: HashSet<u64> = torn_ties
            .iter()
            .filter_map(|tie| match tie.id {
                TieId::Closure(id) => Some(id),
                TieId::Merge(_) => None,
            })
            .collect();
        let joined = |at: StationRef| if at == join.join_o { join.join_k } else { at };
        let on_combined = |a: StationRef, b: StationRef| {
            [a, b]
                .iter()
                .any(|at| at.section_id == join.kept || at.section_id == join.other)
        };
        let entry = |a: StationRef, b: StationRef, eligible: bool| -> DedupeEntry {
            on_combined(a, b).then_some(((joined(a), joined(b)), eligible))
        };
        let mut model: Vec<(DedupeEntry, Origin)> = Vec::new();
        for closure in closures {
            if join.is_join_tie(closure.a, closure.b)
                || removed.contains(&TieId::Closure(closure.id))
            {
                continue;
            }
            let torn = torn_snaps.contains(&closure.id);
            model.push((
                entry(closure.a, closure.b, closure.eligible || torn),
                Origin::Closure(closure, torn),
            ));
        }
        for tie in torn_ties.iter().filter(|t| matches!(t.id, TieId::Merge(_))) {
            model.push((entry(tie.a, tie.b, true), Origin::Merge(tie)));
        }
        let entries: Vec<DedupeEntry> = model.iter().map(|(entry, _)| *entry).collect();
        let eligible = |at: usize| matches!(entries.get(at), Some(Some((_, true))));
        closure_dedupe(&entries)
            .into_iter()
            .enumerate()
            .filter_map(|(index, stays)| {
                let stays = stays?;
                if eligible(index) || !eligible(stays) {
                    return None;
                }
                // Not eligible, so neither torn nor a torn merge.
                let Some((_, Origin::Closure(gone, _))) = model.get(index) else {
                    return None;
                };
                let (by_station_a, by_station_b, by_kind) = match model.get(stays)? {
                    (_, Origin::Merge(tie)) => (tie.a, tie.b, CombineRemovedKind::Merge),
                    // A tie the combine tears: a snap, or a torn snap it
                    // re-closes (#736).
                    (_, Origin::Closure(by, true)) if by.aligned => {
                        (by.a, by.b, CombineRemovedKind::SnappedClosure)
                    }
                    (_, Origin::Closure(by, true)) => {
                        (by.a, by.b, CombineRemovedKind::FormerSnappedClosure)
                    }
                    (_, Origin::Closure(by, false)) => {
                        (by.a, by.b, CombineRemovedKind::LoopClosure)
                    }
                };
                Some(CombineReplacedClosure {
                    station_a: gone.a,
                    station_b: gone.b,
                    kind: replaced_kind(gone.strategy, gone.aligned),
                    by_station_a,
                    by_station_b,
                    by_kind,
                })
            })
            .collect()
    }

    /// Can a combine of `kept` and `other` replace a closure at all (#749)?
    /// Only when a closure the solve does not take has a station on one of
    /// the two: checked off the stored keys, without resolving anything, so
    /// the station menu pays for [`Self::closures_replaced`] only where it can
    /// say something.
    pub(crate) fn combine_may_replace(&self, kept: SectionId, other: SectionId) -> bool {
        self.loop_closures.iter().any(|closure| {
            !Self::closure_is_eligible(closure.strategy)
                && [&closure.station_a, &closure.station_b]
                    .iter()
                    .any(|key| key.section_id == kept || key.section_id == other)
        })
    }

    /// Name what [`Self::dedupe_closures_in`] replaced as the plan does
    /// ([`Self::closures_replaced`]): each closure by its stations before the
    /// combine (`before`, by id), the replacing one by the tie it came from
    /// when the combine tore one into it (`torn_into`, a torn snap of
    /// `former` named as such, #736), else by its own stations before, as a
    /// loop closure already there.
    pub(super) fn name_replaced(
        replaced: &[(LoopClosure, LoopClosure)],
        before: &HashMap<u64, (StationRef, StationRef)>,
        torn_into: &HashMap<u64, Tie>,
        former: &HashSet<u64>,
    ) -> Vec<CombineReplacedClosure> {
        replaced
            .iter()
            .filter_map(|(gone, by)| {
                let &(station_a, station_b) = before.get(&gone.id)?;
                let (by_station_a, by_station_b, by_kind) = if let Some(tie) = torn_into.get(&by.id)
                {
                    (tie.a, tie.b, tie.removed_kind(former))
                } else {
                    let &(a, b) = before.get(&by.id)?;
                    (a, b, CombineRemovedKind::LoopClosure)
                };
                Some(CombineReplacedClosure {
                    station_a,
                    station_b,
                    kind: replaced_kind(gone.strategy, gone.is_aligned),
                    by_station_a,
                    by_station_b,
                    by_kind,
                })
            })
            .collect()
    }
}

/// What [`Project::dedupe_closures_in`] decided on this thread, so the
/// combine sweeps can report how often they reach each case (#749).
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ClosureDedupes {
    /// The dropped closure and the one that stays are both taken by the
    /// solve, or neither: the first stays.
    pub same_eligibility: usize,
    /// The solve takes only the one that stays, which comes later (#749): the
    /// rule before #749 kept the first.
    pub eligible_after_ineligible: usize,
    /// The solve takes only the one that stays, which comes first, as before.
    pub eligible_before_ineligible: usize,
    /// Of the two above, the dropped closure was snapped (aligned).
    pub snapped_dropped_for_eligible: usize,
}

#[cfg(test)]
thread_local! {
    pub(crate) static CLOSURE_DEDUPES: std::cell::Cell<ClosureDedupes> =
        const { std::cell::Cell::new(ClosureDedupes {
            same_eligibility: 0,
            eligible_after_ineligible: 0,
            eligible_before_ineligible: 0,
            snapped_dropped_for_eligible: 0,
        }) };
}

/// Count what [`Project::dedupe_closures_in`] decided: each dropped closure
/// against the one that stays on its two stations.
#[cfg(test)]
fn count_closure_dedupes(closures: &[LoopClosure], stays: &[Option<usize>]) {
    CLOSURE_DEDUPES.with(|cell| {
        let mut counts = cell.get();
        for (index, stays) in stays.iter().enumerate() {
            let (Some(gone), Some(kept)) =
                (closures.get(index), stays.and_then(|s| closures.get(s)))
            else {
                continue;
            };
            let (gone_eligible, kept_eligible) = (
                Project::closure_is_eligible(gone.strategy),
                Project::closure_is_eligible(kept.strategy),
            );
            if gone_eligible == kept_eligible {
                counts.same_eligibility += 1;
                continue;
            }
            if stays.is_some_and(|s| s > index) {
                counts.eligible_after_ineligible += 1;
            } else {
                counts.eligible_before_ineligible += 1;
            }
            if gone.is_aligned {
                counts.snapped_dropped_for_eligible += 1;
            }
        }
        cell.set(counts);
    });
}
