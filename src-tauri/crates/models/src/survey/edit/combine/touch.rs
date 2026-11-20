//! Combine Lines, the read-only half: where two lines meet, the candidates
//! the context menu, the Sections panel and the dialog ask for, and the
//! entry points (preview and command).

use std::collections::BTreeSet;

use crate::{
    CombineJoinEnds, CombineJoinKind, CombineLinesOptions, LineEnd, LineId, StationId, StationRef,
    COMBINE_DEPTH_NOTICE_M, COMBINE_TOLERANCE_M, MAX_COMBINE_PARTNERS,
};

use super::super::super::{Project, RawPositions};
use super::super::{EditError, EditReport, Edited};
use super::plan::end_station;
use super::{CombineCandidate, CombineJoin, CombineReport};

/// What a touching pairing looks like.
#[derive(Debug, Clone)]
struct Touch {
    blocked: Option<String>,
    branches: Vec<LineId>,
    passing_through: Vec<LineId>,
    loose_ends: Vec<LineId>,
}

impl Touch {
    fn refused(reason: String) -> Self {
        Self {
            blocked: Some(reason),
            branches: Vec::new(),
            passing_through: Vec::new(),
            loose_ends: Vec::new(),
        }
    }
}

/// Precedence when two lines meet at more than one place: merged, then
/// co-located, then across a closure.
fn rank(kind: CombineJoinKind) -> u8 {
    match kind {
        CombineJoinKind::Merged => 0,
        CombineJoinKind::CoLocated => 1,
        CombineJoinKind::Closure => 2,
    }
}

fn opposite(end: LineEnd) -> LineEnd {
    match end {
        LineEnd::Start => LineEnd::End,
        LineEnd::End => LineEnd::Start,
    }
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

impl Project {
    /// The ends of `line` a join can sit on: one for a shot-less line.
    fn joinable_ends(&self, line: LineId) -> &'static [LineEnd] {
        if self.line(line).is_some_and(|l| l.shots.is_empty()) {
            &[LineEnd::Start]
        } else {
            &[LineEnd::Start, LineEnd::End]
        }
    }

    /// The chain position of `line`'s `end` station.
    fn end_chain(&self, positions: &RawPositions, line: LineId, end: LineEnd) -> Option<[f64; 2]> {
        let station = end_station(self, line, end)?;
        self.chain_position(positions, line, station)
    }

    /// Whether `a`'s `end_a` and `b`'s `end_b` are the same point, as a join
    /// with `a` kept, and what else is there. `None` when they do not meet.
    fn pairing_touch(
        &self,
        positions: &RawPositions,
        (a, end_a): (LineId, LineEnd),
        (b, end_b): (LineId, LineEnd),
    ) -> Option<(CombineJoin, Touch)> {
        if a == b {
            return None;
        }
        let (sa, sb) = (end_station(self, a, end_a)?, end_station(self, b, end_b)?);
        let (pa, pb) = (
            self.chain_position(positions, a, sa)?,
            self.chain_position(positions, b, sb)?,
        );
        if !pa.iter().chain(&pb).all(|v| v.is_finite()) {
            return None;
        }
        let kind = if sa == sb {
            CombineJoinKind::Merged
        } else if distance(pa, pb) <= COMBINE_TOLERANCE_M {
            CombineJoinKind::CoLocated
        } else if self
            .closures
            .iter()
            .any(|c| (c.a == sa && c.b == sb) || (c.a == sb && c.b == sa))
        {
            CombineJoinKind::Closure
        } else {
            return None;
        };
        let join = CombineJoin {
            kept: a,
            other: b,
            kept_end: end_a,
            other_end: end_b,
            kept_station: sa,
            other_station: sb,
            kind,
        };
        let topology = self.topology();
        let mut branches = Vec::new();
        let mut passing_through = Vec::new();
        let stations: BTreeSet<StationId> = [sa, sb].into_iter().collect();
        for &s in &stations {
            for slot in topology.slots_of(s) {
                if slot.line_id == a || slot.line_id == b {
                    continue;
                }
                let n = self.line(slot.line_id).map_or(0, |l| l.shots.len());
                if slot.station_index == 0 || slot.station_index as usize == n {
                    branches.push(slot.line_id);
                } else {
                    passing_through.push(slot.line_id);
                }
            }
        }
        branches.sort_unstable();
        branches.dedup();
        passing_through.sort_unstable();
        passing_through.dedup();
        passing_through.retain(|id| !branches.contains(id));
        let mut loose_ends: Vec<LineId> = Vec::new();
        for line in self.lines() {
            if line.id == a
                || line.id == b
                || branches.contains(&line.id)
                || passing_through.contains(&line.id)
            {
                continue;
            }
            let near = self.joinable_ends(line.id).iter().any(|&end| {
                self.end_chain(positions, line.id, end).is_some_and(|p| {
                    distance(p, pa) <= COMBINE_TOLERANCE_M
                        || (kind == CombineJoinKind::Closure
                            && distance(p, pb) <= COMBINE_TOLERANCE_M)
                })
            });
            if near {
                loose_ends.push(line.id);
            }
        }
        let touch = Touch {
            blocked: self.combine_blocked_reason(&join),
            branches,
            passing_through,
            loose_ends,
        };
        Some((join, touch))
    }

    /// The report for one choice of the kept line: the combine run on a
    /// copy. An empty report (with the renames) for a blocked choice.
    fn report_keeping(&self, join: &CombineJoin, blocked: bool) -> CombineReport {
        let empty = || {
            let mut report = CombineReport::empty(join.kept);
            report.station_renames = self.combine_rename_report(
                join.kept,
                join.other,
                join.kept_station,
                join.other_station,
            );
            report
        };
        if blocked {
            return empty();
        }
        let positions = self.raw_positions();
        let Ok(decision) = self.decide_combine(join, &positions) else {
            return empty();
        };
        let mut copy = self.clone();
        copy.run_combine(&decision, &super::apply::plain_options())
            .unwrap_or_else(|_| empty())
    }

    /// Why `kept` cannot take `other`'s shots: they get new session ids from
    /// `kept`, which has only so many left (today's words).
    fn shot_id_room_refusal(&self, kept: LineId, other: LineId) -> Option<String> {
        let (k, o) = (self.line(kept)?, self.line(other)?);
        (o.shots.len() as u64 > super::super::shots::shot_ids_left(k)).then(|| {
            format!(
                "{} has used up its shot numbers for this session: keep {}, or save and reopen",
                self.line_label_of(kept),
                self.line_label_of(other)
            )
        })
    }

    /// The candidate for one touching pairing (`join` names `a` as kept).
    fn candidate_for(
        &self,
        positions: &RawPositions,
        join: CombineJoin,
        touch: Touch,
    ) -> Option<CombineCandidate> {
        let mut join = if join.kept <= join.other {
            join
        } else {
            CombineJoin {
                kept: join.other,
                other: join.kept,
                kept_end: join.other_end,
                other_end: join.kept_end,
                kept_station: join.other_station,
                other_station: join.kept_station,
                kind: join.kind,
            }
        };
        let (a, b) = (self.line(join.kept)?, self.line(join.other)?);
        if a.shots.is_empty() {
            join.kept_end = opposite(join.other_end);
        } else if b.shots.is_empty() {
            join.other_end = opposite(join.kept_end);
        }
        let slot_a = self.topology().slot_on(join.kept_station, join.kept)?;
        let slot_b = self.topology().slot_on(join.other_station, join.other)?;
        let merged = join.kind == CombineJoinKind::Merged;
        let comment = |s: StationId| {
            self.stations
                .get(&s)
                .and_then(|st| st.comment.clone())
                .filter(|c| !c.is_empty())
        };
        let (comment_a, comment_b) = match (comment(join.kept_station), comment(join.other_station))
        {
            (Some(x), Some(y)) if !merged && x != y => (Some(x), Some(y)),
            _ => (None, None),
        };
        let depth_difference_m = if merged {
            None
        } else {
            match (
                positions.z(join.kept_station),
                positions.z(join.other_station),
            ) {
                (Some(x), Some(y)) => Some((x - y).abs()).filter(|gap| *gap > COMBINE_DEPTH_NOTICE_M),
                _ => None,
            }
        };
        let blocked = touch.blocked.is_some();
        let keeping_b = CombineJoin {
            kept: join.other,
            other: join.kept,
            kept_end: join.other_end,
            other_end: join.kept_end,
            kept_station: join.other_station,
            other_station: join.kept_station,
            kind: join.kind,
        };
        Some(CombineCandidate {
            line_a: join.kept,
            line_b: join.other,
            end_a: join.kept_end,
            end_b: join.other_end,
            station_a: join.kept_station,
            station_b: join.other_station,
            slot_a,
            slot_b,
            join_kind: join.kind,
            reverses: join.kept_end == join.other_end,
            blocked_reason: touch.blocked,
            blocked_keeping_a: self.shot_id_room_refusal(join.kept, join.other),
            blocked_keeping_b: self.shot_id_room_refusal(join.other, join.kept),
            shot_count_a: a.shots.len() as u32,
            shot_count_b: b.shots.len() as u32,
            comment_a,
            comment_b,
            depth_difference_m,
            branches: touch.branches,
            loose_ends: touch.loose_ends,
            passing_through: touch.passing_through,
            plan_keeping_a: self.report_keeping(&join, blocked),
            plan_keeping_b: self.report_keeping(&keeping_b, blocked),
        })
    }

    /// Can lines `s1` and `s2` be combined, and where? Tries all four end
    /// pairings; when they meet at more than one, the join is a merged one
    /// first, then co-located ends, then a closure, then the smaller gap, then
    /// one that needs no reverse. `None` when either is missing or they are
    /// the same line.
    pub fn combine_candidate_for_lines(&self, s1: LineId, s2: LineId) -> Option<CombineCandidate> {
        if s1 == s2 {
            return None;
        }
        let (s1, s2) = (s1.min(s2), s1.max(s2));
        self.line(s1)?;
        self.line(s2)?;
        let positions = self.raw_positions();
        let gap = |join: &CombineJoin| {
            match (
                self.chain_position(&positions, join.kept, join.kept_station),
                self.chain_position(&positions, join.other, join.other_station),
            ) {
                (Some(x), Some(y)) => distance(x, y),
                _ => f64::INFINITY,
            }
        };
        let mut best: Option<(CombineJoin, Touch)> = None;
        for &end_a in self.joinable_ends(s1) {
            for &end_b in self.joinable_ends(s2) {
                let Some((join, touch)) =
                    self.pairing_touch(&positions, (s1, end_a), (s2, end_b))
                else {
                    continue;
                };
                let better = best.as_ref().is_none_or(|(b, _)| {
                    rank(join.kind)
                        .cmp(&rank(b.kind))
                        .then_with(|| gap(&join).total_cmp(&gap(b)))
                        .then_with(|| {
                            (join.kept_end == join.other_end).cmp(&(b.kept_end == b.other_end))
                        })
                        .is_lt()
                });
                if better {
                    best = Some((join, touch));
                }
            }
        }
        let (join, touch) = match best {
            Some(found) => found,
            None => {
                let join = CombineJoin {
                    kept: s1,
                    other: s2,
                    kept_end: LineEnd::End,
                    other_end: LineEnd::Start,
                    kept_station: end_station(self, s1, LineEnd::End)?,
                    other_station: end_station(self, s2, LineEnd::Start)?,
                    kind: CombineJoinKind::CoLocated,
                };
                let reason = format!(
                    "{} and {} do not meet at an end station: merge or close a loop between two \
                     of their ends first",
                    self.line_label_of(s1),
                    self.line_label_of(s2)
                );
                (join, Touch::refused(reason))
            }
        };
        self.candidate_for(&positions, join, touch)
    }

    /// The candidate for combining `other` into `kept` at `join` (the kept and
    /// the other line's ends), or at the join the two-line query prefers when
    /// `join` is `None`.
    pub fn combine_candidate_joining(
        &self,
        kept: LineId,
        other: LineId,
        join: Option<CombineJoinEnds>,
    ) -> Option<CombineCandidate> {
        let Some(ends) = join else {
            return self.combine_candidate_for_lines(kept, other);
        };
        if kept == other {
            return None;
        }
        self.line(kept)?;
        self.line(other)?;
        let positions = self.raw_positions();
        let (join, touch) = match self.pairing_touch(&positions, (kept, ends.kept), (other, ends.other))
        {
            Some(found) => found,
            None => {
                let join = CombineJoin {
                    kept,
                    other,
                    kept_end: ends.kept,
                    other_end: ends.other,
                    kept_station: end_station(self, kept, ends.kept)?,
                    other_station: end_station(self, other, ends.other)?,
                    kind: CombineJoinKind::CoLocated,
                };
                let reason = format!(
                    "{} and {} do not meet at those ends",
                    self.line_label_of(kept),
                    self.line_label_of(other)
                );
                (join, Touch::refused(reason))
            }
        };
        self.candidate_for(&positions, join, touch)
    }

    /// Why combining `other` into `kept` at `join` is refused, if it is: the
    /// pair's reason, else the one that depends on which line is kept.
    pub fn combine_refusal(
        &self,
        kept: LineId,
        other: LineId,
        join: Option<CombineJoinEnds>,
    ) -> Option<String> {
        let Some(candidate) = self.combine_candidate_joining(kept, other, join) else {
            return Some(self.cannot_combine_message(kept, other));
        };
        candidate.blocked_reason.or(if candidate.line_a == kept {
            candidate.blocked_keeping_a
        } else {
            candidate.blocked_keeping_b
        })
    }

    /// The join `combine_lines` would make, from the kept line's side.
    fn combine_join(
        &self,
        kept: LineId,
        other: LineId,
        join: Option<CombineJoinEnds>,
    ) -> Result<CombineJoin, EditError> {
        if let Some(refusal) = self.combine_refusal(kept, other, join) {
            return Err(EditError::Refused(refusal));
        }
        let candidate = self
            .combine_candidate_joining(kept, other, join)
            .ok_or_else(|| EditError::Refused(self.cannot_combine_message(kept, other)))?;
        Ok(if candidate.line_a == kept {
            CombineJoin {
                kept,
                other,
                kept_end: candidate.end_a,
                other_end: candidate.end_b,
                kept_station: candidate.station_a,
                other_station: candidate.station_b,
                kind: candidate.join_kind,
            }
        } else {
            CombineJoin {
                kept,
                other,
                kept_end: candidate.end_b,
                other_end: candidate.end_a,
                kept_station: candidate.station_b,
                other_station: candidate.station_a,
                kind: candidate.join_kind,
            }
        })
    }

    /// What combining `other` into `kept` would do (the combine run on a
    /// copy), refused exactly as the combine is.
    pub fn preview_combine(
        &self,
        kept: LineId,
        other: LineId,
        join: Option<CombineJoinEnds>,
    ) -> Result<CombineReport, EditError> {
        let join = self.combine_join(kept, other, join)?;
        let decision = self
            .decide_combine(&join, &self.raw_positions())
            .map_err(EditError::Refused)?;
        let mut copy = self.clone();
        copy.run_combine(&decision, &super::apply::plain_options())
    }

    /// Combine `other` into `kept` (module docs of `combine.rs`). Refused, with
    /// nothing changed, as the candidate says; the combined line's name is
    /// validated as a rename when it is not empty.
    pub fn combine_lines(
        &mut self,
        kept: LineId,
        other: LineId,
        options: &CombineLinesOptions,
    ) -> Result<Edited<CombineReport>, EditError> {
        let join = self.combine_join(kept, other, options.join)?;
        if !options.name.trim().is_empty() {
            self.validate_line_name(&options.name, &[kept, other])?;
        }
        let decision = self
            .decide_combine(&join, &self.raw_positions())
            .map_err(EditError::Refused)?;
        let mut work = self.clone();
        let report = work.run_combine(&decision, options)?;
        *self = work;
        let removed = report
            .closures_removed_across_one_shot
            .iter()
            .map(|r| r.closure)
            .collect();
        Ok(Edited {
            value: report,
            report: EditReport {
                removed_across_one_shot: removed,
                ..EditReport::default()
            },
        })
    }

    /// What the line that `slot` ends can be combined with: one candidate per
    /// other line with an end at the same point (on the same station,
    /// side by side, or across a loop closure), each joined at `slot`.
    pub fn combine_candidates(&self, slot: StationRef) -> Vec<CombineCandidate> {
        self.station_candidates(slot, false)
    }

    /// [`Self::combine_candidates`], then every pair of the other lines
    /// ending at this point (the station menu).
    pub fn combine_candidates_at_point(&self, slot: StationRef) -> Vec<CombineCandidate> {
        self.station_candidates(slot, true)
    }

    fn station_candidates(&self, slot: StationRef, all_pairs: bool) -> Vec<CombineCandidate> {
        let Some(line) = self.line(slot.line_id) else {
            return Vec::new();
        };
        let n = line.shots.len();
        let index = slot.station_index as usize;
        let end = if index == 0 {
            LineEnd::Start
        } else if index == n {
            LineEnd::End
        } else {
            return Vec::new();
        };
        let Some(station) = line.station(index) else {
            return Vec::new();
        };
        let positions = self.raw_positions();
        let Some(at) = self.chain_position(&positions, line.id, station) else {
            return Vec::new();
        };
        let end_of = |l: LineId, s: StationId| -> Option<(LineId, LineEnd)> {
            let other = self.line(l)?;
            if other.start == s {
                Some((l, LineEnd::Start))
            } else if other.shots.last().is_some_and(|shot| shot.to == s) {
                Some((l, LineEnd::End))
            } else {
                None
            }
        };
        let mut other_ends: Vec<(LineId, LineEnd)> = Vec::new();
        for other in self.lines() {
            for &e in self.joinable_ends(other.id) {
                if self
                    .end_chain(&positions, other.id, e)
                    .is_some_and(|p| distance(p, at) <= COMBINE_TOLERANCE_M)
                {
                    other_ends.push((other.id, e));
                }
            }
        }
        let topology = self.topology();
        let through: Vec<LineId> = topology
            .lines_of(station)
            .into_iter()
            .filter(|l| *l != line.id)
            .collect();
        other_ends.extend(through.iter().filter_map(|&l| end_of(l, station)));
        let here: Vec<(LineId, LineEnd)> = other_ends.clone();
        for c in &self.closures {
            let far = if c.a == station {
                c.b
            } else if c.b == station {
                c.a
            } else {
                continue;
            };
            for l in topology.lines_of(far) {
                other_ends.extend(end_of(l, far));
            }
        }
        other_ends.retain(|(id, _)| *id != line.id);
        other_ends.sort_by_key(|(id, e)| (*id, *e == LineEnd::End));
        other_ends.dedup();
        let shotless_start = |(id, e): (LineId, LineEnd)| {
            if self.line(id).is_some_and(|l| l.shots.is_empty()) {
                (id, LineEnd::Start)
            } else {
                (id, e)
            }
        };
        if other_ends.len() > MAX_COMBINE_PARTNERS {
            let count = other_ends.len() + 1;
            let Some((join, mut touch)) = other_ends.iter().find_map(|&o| {
                self.pairing_touch(&positions, (line.id, end), shotless_start(o))
            }) else {
                return Vec::new();
            };
            touch.blocked = Some(format!(
                "{count} line ends meet here: select the two to combine in the Sections panel"
            ));
            return self.candidate_for(&positions, join, touch).into_iter().collect();
        }
        if other_ends.is_empty() {
            let Some(&other) = through.first() else {
                return Vec::new();
            };
            let join = CombineJoin {
                kept: line.id,
                other,
                kept_end: end,
                other_end: LineEnd::Start,
                kept_station: station,
                other_station: station,
                kind: CombineJoinKind::Merged,
            };
            let touch = Touch::refused("Merged to the middle of another line".to_string());
            return self.candidate_for(&positions, join, touch).into_iter().collect();
        }
        let mut touching: Vec<(CombineJoin, Touch)> = other_ends
            .iter()
            .filter_map(|&o| self.pairing_touch(&positions, (line.id, end), shotless_start(o)))
            .collect();
        if all_pairs {
            for (i, x) in other_ends.iter().enumerate() {
                for y in &other_ends[i + 1..] {
                    if x.0 == y.0 || !(here.contains(x) || here.contains(y)) {
                        continue;
                    }
                    if let Some(found) =
                        self.pairing_touch(&positions, shotless_start(*x), shotless_start(*y))
                    {
                        touching.push(found);
                    }
                }
            }
        }
        let gap = |join: &CombineJoin| {
            match (
                self.chain_position(&positions, join.kept, join.kept_station),
                self.chain_position(&positions, join.other, join.other_station),
            ) {
                (Some(x), Some(y)) => distance(x, y),
                _ => f64::INFINITY,
            }
        };
        let mut best: Vec<(CombineJoin, Touch)> = Vec::new();
        for (join, touch) in touching {
            let key = (join.kept.min(join.other), join.kept.max(join.other));
            let better = |(b, _): &(CombineJoin, Touch)| {
                rank(join.kind)
                    .cmp(&rank(b.kind))
                    .then_with(|| gap(&join).total_cmp(&gap(b)))
                    .then_with(|| (join.kept_end == join.other_end).cmp(&(b.kept_end == b.other_end)))
                    .is_lt()
            };
            match best
                .iter_mut()
                .find(|(b, _)| (b.kept.min(b.other), b.kept.max(b.other)) == key)
            {
                Some(slot) => {
                    if better(slot) {
                        *slot = (join, touch);
                    }
                }
                None => best.push((join, touch)),
            }
        }
        best.into_iter()
            .filter_map(|(join, touch)| self.candidate_for(&positions, join, touch))
            .collect()
    }
}
