//! Combine Lines on the wire: M1's [`CombineCandidate`] and
//! [`CombineReport`] as today's `CombineCandidate` and `CombinePlan`.
//!
//! Everything is named as it is BEFORE the combine (today's rule), so each
//! mapping takes the project state the preview ran on. A closure the report
//! names is looked up there by id (its ends as the user created them); a
//! closure the combine itself made from a torn junction has no such entry
//! and is named by that torn tie's two slots.
//!
//! Kinds and reasons (plan "Deviations", S2):
//! - a torn junction is today's torn `Merge`; a torn snap is a
//!   `SnappedClosure`, or a `FormerSnappedClosure` when it was not aligned
//!   before the combine;
//! - `PassesTwice` is reported as `Loop` (today a tie reaching a non-join
//!   station was a loop), `Moved` as `JoinedStationMoves`.

use std::collections::BTreeMap;

use crate::{
    CombineJoinedMove, CombinePlan, CombineRemovedClosure, CombineRemovedKind,
    CombineReplacedClosure, CombineReplacedKind, StationRef, TornTieKind as WireTornTieKind,
    TornTieReason as WireTornTieReason,
};

use super::super::{
    ClosureId, ClosureStrategy, CombineCandidate, CombineReport, JoinedMove, LoopClosure, Project,
    RemovedClosure, ReplacedClosure, TornTie, TornTieKind, TornTieReason,
};

/// The pre-combine facts a mapping reads: every closure's alignment.
struct Before<'p> {
    project: &'p Project,
    aligned: BTreeMap<ClosureId, bool>,
}

impl<'p> Before<'p> {
    fn of(project: &'p Project) -> Self {
        let raw = project.raw_positions();
        let aligned = project
            .closures
            .iter()
            .zip(project.closure_alignment(&raw))
            .map(|(closure, aligned)| (closure.id, aligned))
            .collect();
        Self { project, aligned }
    }

    fn closure(&self, id: ClosureId) -> Option<&'p LoopClosure> {
        self.project.closures.iter().find(|c| c.id == id)
    }

    fn was_aligned(&self, id: ClosureId) -> bool {
        self.aligned.get(&id).copied().unwrap_or(false)
    }

    fn tie(&self, tie: &TornTie) -> crate::TornTie {
        crate::TornTie {
            station_a: tie.slot,
            station_b: tie.other_slot,
            kind: match tie.kind {
                TornTieKind::Junction => WireTornTieKind::Merge,
                TornTieKind::SnappedClosure
                    if tie.closure.is_some_and(|id| !self.was_aligned(id)) =>
                {
                    WireTornTieKind::FormerSnappedClosure
                }
                TornTieKind::SnappedClosure => WireTornTieKind::SnappedClosure,
            },
            reason: match tie.reason {
                TornTieReason::Loop | TornTieReason::PassesTwice => WireTornTieReason::Loop,
                TornTieReason::ZeroDatum => WireTornTieReason::ZeroDatum,
                TornTieReason::Moved => WireTornTieReason::JoinedStationMoves,
            },
        }
    }

    /// A closure the report names, by its ends before the combine and what
    /// it was then.
    fn named(
        &self,
        closure: &LoopClosure,
        report: &CombineReport,
    ) -> Option<(StationRef, StationRef, CombineRemovedKind)> {
        let torn = report
            .torn_ties
            .iter()
            .find(|tie| tie.closure == Some(closure.id));
        if let Some(before) = self.closure(closure.id) {
            let pair = self.project.closure_pair(before)?;
            let aligned = self.was_aligned(before.id);
            let snap = matches!(
                before.strategy,
                ClosureStrategy::Move | ClosureStrategy::Tie
            );
            let kind = match torn {
                Some(_) if aligned => CombineRemovedKind::SnappedClosure,
                Some(_) => CombineRemovedKind::FormerSnappedClosure,
                None if aligned && snap => CombineRemovedKind::SnappedClosure,
                None => CombineRemovedKind::LoopClosure,
            };
            return Some((pair.station_a, pair.station_b, kind));
        }
        let tie = torn?;
        let kind = match tie.kind {
            TornTieKind::Junction => CombineRemovedKind::Merge,
            TornTieKind::SnappedClosure => CombineRemovedKind::SnappedClosure,
        };
        Some((tie.slot, tie.other_slot, kind))
    }

    fn removed(
        &self,
        removed: &RemovedClosure,
        report: &CombineReport,
    ) -> Option<CombineRemovedClosure> {
        let named = match removed.torn_tie.and_then(|i| report.torn_ties.get(i)) {
            Some(tie) if self.closure(removed.closure.id).is_none() => {
                let kind = match tie.kind {
                    TornTieKind::Junction => CombineRemovedKind::Merge,
                    TornTieKind::SnappedClosure => CombineRemovedKind::SnappedClosure,
                };
                (tie.slot, tie.other_slot, kind)
            }
            _ => self.named(&removed.closure, report)?,
        };
        let (station_a, station_b, kind) = named;
        Some(CombineRemovedClosure {
            station_a,
            station_b,
            kind,
        })
    }

    fn replaced(
        &self,
        replaced: &ReplacedClosure,
        report: &CombineReport,
    ) -> Option<CombineReplacedClosure> {
        let gone = self
            .closure(replaced.dropped.id)
            .unwrap_or(&replaced.dropped);
        let pair = self.project.closure_pair(gone)?;
        let aligned = self.was_aligned(gone.id);
        let kind = if aligned {
            CombineReplacedKind::SnappedClosure
        } else if gone.strategy == ClosureStrategy::Move {
            CombineReplacedKind::FormerSnap
        } else {
            CombineReplacedKind::LegacyClosure
        };
        let (by_station_a, by_station_b, by_kind) = self.named(&replaced.by, report)?;
        Some(CombineReplacedClosure {
            station_a: pair.station_a,
            station_b: pair.station_b,
            kind,
            by_station_a,
            by_station_b,
            by_kind,
        })
    }

    fn joined_move(
        &self,
        joined: &JoinedMove,
        report: &CombineReport,
    ) -> Option<CombineJoinedMove> {
        let topology = self.project.topology();
        let station = report
            .moving_line
            .and_then(|line| topology.slot_on(joined.station, line))
            .or_else(|| topology.first_slot(joined.station))?;
        Some(CombineJoinedMove {
            station,
            lines: joined.lines.clone(),
            shift_m: joined.shift_m,
        })
    }

    fn plan(&self, report: &CombineReport) -> CombinePlan {
        let removed: Vec<CombineRemovedClosure> = report
            .closures_removed_across_one_shot
            .iter()
            .filter_map(|r| self.removed(r, report))
            .collect();
        let replaced: Vec<CombineReplacedClosure> = report
            .closures_replaced
            .iter()
            .filter_map(|r| self.replaced(r, report))
            .collect();
        CombinePlan {
            kept: report.kept,
            moving_line: report.moving_line,
            kept_moves_because: report.kept_moves_because,
            moved_lines: report.moved_lines.clone(),
            shift_m: report.shift_m,
            torn_ties: report.torn_ties.iter().map(|tie| self.tie(tie)).collect(),
            azimuth_rotation_deg: report.azimuth_rotation_deg,
            join_offset_change_m: report.join_offset_change_m,
            closures_removed_across_one_shot: (!removed.is_empty()).then_some(removed),
            closures_replaced: (!replaced.is_empty()).then_some(replaced),
            moved_with_joined_station: report
                .moved_with_joined_station
                .as_ref()
                .and_then(|joined| self.joined_move(joined, report)),
            station_renames: report.station_renames.clone(),
        }
    }
}

impl Project {
    /// Today's `CombineCandidate` of M1's (both plans included). `merged` is
    /// the join kind; the always-`None` name fields stay `None`, and
    /// `repeated_shot_ids` is 0 (shot uuids are project-unique).
    pub fn combine_candidate_wire(&self, candidate: &CombineCandidate) -> crate::CombineCandidate {
        let before = Before::of(self);
        crate::CombineCandidate {
            line_a: candidate.line_a,
            line_b: candidate.line_b,
            end_a: candidate.end_a,
            end_b: candidate.end_b,
            station_a: candidate.slot_a,
            station_b: candidate.slot_b,
            merged: candidate.join_kind == crate::CombineJoinKind::Merged,
            join_kind: candidate.join_kind,
            reverses: candidate.reverses,
            blocked_reason: candidate.blocked_reason.clone(),
            blocked_keeping_a: candidate.blocked_keeping_a.clone(),
            blocked_keeping_b: candidate.blocked_keeping_b.clone(),
            shot_count_a: candidate.shot_count_a,
            shot_count_b: candidate.shot_count_b,
            comment_a: candidate.comment_a.clone(),
            comment_b: candidate.comment_b.clone(),
            station_name_a: None,
            station_name_b: None,
            depth_difference_m: candidate.depth_difference_m,
            branches: candidate.branches.clone(),
            loose_ends: candidate.loose_ends.clone(),
            passing_through: candidate.passing_through.clone(),
            repeated_shot_ids: 0,
            plan_keeping_a: before.plan(&candidate.plan_keeping_a),
            plan_keeping_b: before.plan(&candidate.plan_keeping_b),
        }
    }

    /// Today's `CombinePlan` of an M1 [`CombineReport`] (a preview or the
    /// command's own), named in this pre-combine state.
    pub fn combine_plan_wire(&self, report: &CombineReport) -> CombinePlan {
        Before::of(self).plan(report)
    }
}
