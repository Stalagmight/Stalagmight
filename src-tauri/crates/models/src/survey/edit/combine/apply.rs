//! Combine Lines, the join (plan 5.4 steps 2-6 applied) and the facts the
//! dialog shows, read off the result.

use std::collections::{BTreeMap, BTreeSet};

use crate::project::station_naming::plan_combine_prefixes;
use crate::{
    normalize_azimuth, CombineLinesOptions, CombinePick, LineEnd, LineId, StationId, StationRef,
};

use super::super::super::{
    ClosureId, ClosureStrategy, EditContext, LoopClosure, Placement, Project, RawPositions,
};
use super::super::fold::CommentRule;
use super::super::lines::absorb_legacy_credit;
use super::super::EditError;
use super::plan::{moving_side, Decision, DUST_M};
use super::{
    CombineReport, JoinedMove, RemovedClosure, ReplacedClosure, TornTie, TornTieKind,
    TornTieReason,
};

/// Positions agree within this when the facts compare moves (metres).
const SAME_MOVE_M: f64 = 1e-6;

/// The options of a dry run that only asks what the combine does to the
/// closures (no name, no colour, nothing reversed for walls).
pub(super) fn plain_options() -> CombineLinesOptions {
    CombineLinesOptions {
        name: String::new(),
        color: String::new(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

/// Is the closure a solve-taken (eligible) one?
fn eligible(strategy: ClosureStrategy) -> bool {
    matches!(
        strategy,
        ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted
    )
}

impl Project {
    /// Degrees to add to `other`'s stored bearings so they read against
    /// `kept`'s north reference, when the two STORED references differ
    /// (today's `azimuth_rotation`).
    pub(super) fn azimuth_rotation(&self, kept: LineId, other: LineId) -> Option<f64> {
        let (k, o) = (self.line(kept)?, self.line(other)?);
        let same = match (k.azimuth_reference, o.azimuth_reference) {
            (None, None) => true,
            (Some(x), Some(y)) => x.north == y.north && (x.correction - y.correction).abs() <= 1e-9,
            _ => false,
        };
        (!same).then(|| {
            self.azimuth_reference_for(o).correction - self.azimuth_reference_for(k).correction
        })
    }

    /// Run the combine `decision` on `self` (a working copy) with the whole
    /// protocol: anchors, the snap and closure rules, validation. Returns
    /// what it did; `Err` leaves `self` part-way (drop it).
    pub(super) fn run_combine(
        &mut self,
        decision: &Decision,
        options: &CombineLinesOptions,
    ) -> Result<CombineReport, EditError> {
        let join = decision.join;
        let (k, o) = (join.kept, join.other);
        let before = self.anchor_snapshot();
        let p0 = before.positions().clone();
        let skipped_before = self.closures_across_one_shot_in(&p0);
        let pre = self.clone();
        let snaps_before: BTreeSet<ClosureId> = self
            .closures
            .iter()
            .filter(|c| c.strategy == ClosureStrategy::Move && self.closure_is_aligned(&p0, c))
            .map(|c| c.id)
            .collect();
        let rotation = self.azimuth_rotation(k, o);
        let renames = self.combine_rename_report(k, o, join.kept_station, join.other_station);
        let hand_k = self.line(k).map(|l| l.hand(join.kept_station));
        let hand_o = self.line(o).map(|l| l.hand(join.other_station));
        let merged = join.kept_station == join.other_station;
        let join_offset_change_m = match (merged, hand_k, hand_o) {
            (false, Some(a), Some(b)) => {
                let apart =
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
                (apart > crate::COMBINE_TOLERANCE_M).then_some(apart)
            }
            _ => None,
        };

        // The other line's bearings read against the kept line's reference.
        if let Some(rotation) = rotation {
            let reference = self.line(k).and_then(|l| l.azimuth_reference);
            if let Some(line) = self.line_mut(o) {
                if rotation.abs() > 0.0 {
                    for shot in &mut line.shots {
                        shot.azimuth_in = normalize_azimuth(shot.azimuth_in + rotation);
                        shot.azimuth_out = normalize_azimuth(shot.azimuth_out + rotation);
                    }
                }
                line.azimuth_reference = reference;
            }
        }

        // Steps 2, 3, 6: the junctions torn into Batch closures.
        let mut renamed: Vec<(StationId, StationId)> = Vec::new();
        let mut slot_map: BTreeMap<(LineId, StationId), StationId> = BTreeMap::new();
        let mut torn: Vec<TornTie> = Vec::new();
        let (mut jk, mut jo) = (join.kept_station, join.other_station);
        let slot = |p: &Project, s: StationId, line: Option<LineId>| -> StationRef {
            let topology = p.topology();
            line.and_then(|l| topology.slot_on(s, l))
                .or_else(|| topology.first_slot(s))
                .unwrap_or(StationRef::new(0, 0))
        };
        for d in &decision.detaches {
            let fresh = self.detach_line_from(d.line, d.station)?;
            renamed.push((d.station, fresh));
            slot_map.insert((d.line, d.station), fresh);
            if d.line == k && d.station == jk {
                jk = fresh;
            }
            if d.line == o && d.station == jo {
                jo = fresh;
            }
            let id = self.push_closure(
                d.station,
                d.toward,
                fresh,
                Some(d.line),
                ClosureStrategy::Batch,
            );
            torn.push(TornTie {
                kind: TornTieKind::Junction,
                reason: d.reason,
                station: d.station,
                slot: slot(&pre, d.station, Some(d.line)),
                other_slot: slot(&pre, d.station, d.toward),
                closure: Some(id),
            });
        }

        // The two sides, after the tears (step 5's snaps read them).
        let (jm, jf) = if decision.moving == o { (jo, jk) } else { (jk, jo) };
        let (moving_side, _) = moving_side(self, decision.moving, jm, jm == jf);
        let fixed_side: BTreeSet<StationId> = if jm == jf {
            let all = self.component_stations(jf);
            all.difference(&moving_side).copied().collect()
        } else {
            self.component_stations(jf)
        };
        let held_parts: BTreeSet<StationId> = decision
            .detaches
            .iter()
            .filter(|d| d.reason == TornTieReason::ZeroDatum)
            .flat_map(|d| self.component_stations(d.station))
            .collect();
        for closure in &mut self.closures {
            if !snaps_before.contains(&closure.id) {
                continue;
            }
            let (in_a, in_b) = (
                moving_side.contains(&closure.a),
                moving_side.contains(&closure.b),
            );
            let crosses = (in_a && fixed_side.contains(&closure.b))
                || (in_b && fixed_side.contains(&closure.a));
            let pulled = decision.shifting && in_a != in_b;
            if crosses || pulled {
                closure.strategy = ClosureStrategy::Batch;
                let reason = if held_parts.contains(&closure.a) || held_parts.contains(&closure.b)
                {
                    TornTieReason::ZeroDatum
                } else if crosses {
                    TornTieReason::Loop
                } else {
                    TornTieReason::Moved
                };
                torn.push(TornTie {
                    kind: TornTieKind::SnappedClosure,
                    reason,
                    station: closure.a,
                    slot: slot(&pre, closure.a, closure.a_via),
                    other_slot: slot(&pre, closure.b, closure.b_via),
                    closure: Some(closure.id),
                });
            }
        }

        // The other line lines up start to end with the kept one.
        if join.kept_end == join.other_end {
            self.reverse_line_in_place(o, options.swap_lr)?;
        }
        let append = join.kept_end == LineEnd::End;

        // Letter the two lines' station names when they collide.
        self.prefix_combined_names(k, o, jk, jo);

        // The fixed side's hand move wins at the join.
        let hand_jo = self.line(o).and_then(|l| l.hand_moves.get(&jo).copied());
        if let Some(line) = self.line_mut(o) {
            line.hand_moves.remove(&jo);
        }
        if !decision.kept_hand_wins {
            if let Some(line) = self.line_mut(k) {
                match hand_jo {
                    Some(hand) => line.hand_moves.insert(jk, hand),
                    None => line.hand_moves.remove(&jk),
                };
            }
        }

        // Step 4: the two join stations become one, the kept one.
        let mut folded: Option<(StationId, StationId)> = None;
        if jo != jk {
            self.fold_station(jo, jk, CommentRule::Pick(options.comment_pick))?;
            renamed.push((jo, jk));
            folded = Some((jo, jk));
        }

        // The shots: the other line's join the kept line's, with new ids.
        let Some(position) = self.line_position(o) else {
            return Err(EditError::line_gone());
        };
        let other = self.lines_mut().remove(position);
        let Some(line) = self.line_mut(k) else {
            return Err(EditError::line_gone());
        };
        let mut arriving = Vec::with_capacity(other.shots.len());
        for mut shot in other.shots {
            shot.id = line.mint_shot_id();
            arriving.push(shot);
        }
        if append {
            line.shots.extend(arriving);
        } else {
            line.start = other.start;
            arriving.extend(std::mem::take(&mut line.shots));
            line.shots = arriving;
        }
        for (station, hand) in other.hand_moves {
            line.hand_moves.entry(station).or_insert(hand);
        }
        line.name = Some(options.name.trim().to_string()).filter(|n| !n.is_empty());
        if !options.color.is_empty() {
            line.color.clone_from(&options.color);
        }
        absorb_legacy_credit(line);
        let kept_stations: BTreeSet<StationId> = line.stations().collect();
        self.repoint_vias(o, k, &kept_stations);
        self.uniquify_line_names(k);

        // Step 1's outcome: the fixed side stays, the moving side moves.
        let intended = |s: StationId, p: &RawPositions| -> Option<[f64; 2]> {
            let [x, y] = p.xy(s)?;
            Some(if moving_side.contains(&s) {
                [x + decision.shift[0], y + decision.shift[1]]
            } else {
                [x, y]
            })
        };
        let mut ctx = EditContext::line(k);
        for &(old, new) in &renamed {
            ctx = ctx.with_renamed(old, new);
        }
        let joined = self.component_stations(jk);
        let held = self.datums.iter().any(|d| joined.contains(&d.station));
        if !held {
            let candidates: Vec<StationId> = pre
                .placements
                .iter()
                .map(|p| p.station)
                .filter(|s| joined.contains(s))
                .collect();
            let g = candidates
                .iter()
                .copied()
                .find(|s| !moving_side.contains(s))
                .or_else(|| candidates.first().copied())
                .or_else(|| joined.iter().copied().find(|s| p0.xy(*s).is_some()));
            if let Some(g) = g {
                if let Some([x, y]) = intended(g, &p0) {
                    self.placements.retain(|p| !joined.contains(&p.station));
                    self.placements.push(Placement { station: g, x, y });
                    ctx = ctx.with_placed(g);
                }
            }
        }
        self.drop_stale_attachments();
        self.settle_anchors(&before, &ctx);

        // Snaps the move pulled apart become ordinary (as today).
        let positions = self.raw_positions();
        let pulled: Vec<ClosureId> = self
            .closures
            .iter()
            .filter(|c| {
                c.strategy == ClosureStrategy::Move
                    && snaps_before.contains(&c.id)
                    && !self.closure_is_aligned(&positions, c)
            })
            .map(|c| c.id)
            .collect();
        for closure in &mut self.closures {
            if pulled.contains(&closure.id) {
                closure.strategy = ClosureStrategy::Batch;
                torn.push(TornTie {
                    kind: TornTieKind::SnappedClosure,
                    reason: TornTieReason::Moved,
                    station: closure.a,
                    slot: slot(&pre, closure.a, closure.a_via),
                    other_slot: slot(&pre, closure.b, closure.b_via),
                    closure: Some(closure.id),
                });
            }
        }

        let replaced = self.dedupe_closures_on(k);
        let removed = self.remove_closures_newly_across_one_shot(&skipped_before);
        let violations = self.validate();
        if !violations.is_empty() {
            return Err(EditError::Invalid(violations));
        }

        // The facts, read off the result.
        let after = self.raw_positions();
        let displacement = |line: LineId| -> Option<[f64; 2]> {
            let chain = pre.line_stations(line)?;
            let mut moved: Option<[f64; 2]> = None;
            for &s in chain {
                let mut now = slot_map.get(&(line, s)).copied().unwrap_or(s);
                if let Some((from, to)) = folded {
                    if now == from {
                        now = to;
                    }
                }
                let (Some(a), Some(b)) = (p0.xy(s), after.xy(now)) else {
                    return None;
                };
                let d = [b[0] - a[0], b[1] - a[1]];
                match moved {
                    None => moved = Some(d),
                    Some(m) if (m[0] - d[0]).hypot(m[1] - d[1]) <= SAME_MOVE_M => {}
                    Some(_) => return None,
                }
            }
            moved
        };
        let shift = decision.shift;
        let shift_m = if decision.shifting {
            shift[0].hypot(shift[1])
        } else {
            0.0
        };
        let mut moved_lines: Vec<LineId> = Vec::new();
        let mut joined_lines: Vec<LineId> = Vec::new();
        let mut joined_shift = 0.0;
        for line in pre.lines() {
            let Some(d) = displacement(line.id) else {
                continue;
            };
            if d[0].abs() <= DUST_M && d[1].abs() <= DUST_M {
                continue;
            }
            if decision.shifting && (d[0] - shift[0]).hypot(d[1] - shift[1]) <= SAME_MOVE_M {
                if line.id != decision.moving {
                    moved_lines.push(line.id);
                }
            } else if line.id != k && line.id != o {
                joined_lines.push(line.id);
                joined_shift = d[0].hypot(d[1]);
            }
        }
        moved_lines.sort_unstable();
        if decision.shifting {
            moved_lines.insert(0, decision.moving);
        }
        joined_lines.sort_unstable();
        let dropped_station = if decision.kept_hand_wins { jo } else { jk };
        let moved_with_joined_station = (!joined_lines.is_empty()).then(|| JoinedMove {
            station: if dropped_station == jo {
                join.other_station
            } else {
                join.kept_station
            },
            lines: joined_lines,
            shift_m: joined_shift,
        });
        let torn_closures: BTreeMap<ClosureId, usize> = torn
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.closure.map(|c| (c, i)))
            .collect();
        Ok(CombineReport {
            kept: k,
            moving_line: decision.shifting.then_some(decision.moving),
            kept_moves_because: decision.kept_moves_because,
            moved_lines,
            shift_m,
            torn_ties: torn,
            azimuth_rotation_deg: rotation,
            join_offset_change_m,
            closures_removed_across_one_shot: removed
                .into_iter()
                .map(|closure| RemovedClosure {
                    closure,
                    torn_tie: torn_closures.get(&closure.id).copied(),
                })
                .collect(),
            closures_replaced: replaced,
            moved_with_joined_station,
            station_renames: renames,
        })
    }

    /// The stations of `station`'s component (empty when it is on no line).
    fn component_stations(&self, station: StationId) -> BTreeSet<StationId> {
        let topology = self.topology();
        topology
            .component_of(station)
            .and_then(|c| topology.component(c))
            .map(|c| c.stations.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Today's dedupe (#749) on the closures with a station on `line`: of the
    /// closures on the same two stations, the first the solve takes stays (or
    /// the first one when it takes none); the others go. Returns the ones
    /// dropped for one the solve takes while not one themselves.
    fn dedupe_closures_on(&mut self, line: LineId) -> Vec<ReplacedClosure> {
        let on_line: BTreeSet<StationId> = self
            .line_stations(line)
            .map(|c| c.iter().copied().collect())
            .unwrap_or_default();
        let pair = |c: &LoopClosure| (c.a.min(c.b), c.a.max(c.b));
        let mut kept: BTreeMap<(StationId, StationId), usize> = BTreeMap::new();
        for (i, c) in self.closures.iter().enumerate() {
            if !on_line.contains(&c.a) && !on_line.contains(&c.b) {
                continue;
            }
            match kept.get(&pair(c)) {
                None => {
                    kept.insert(pair(c), i);
                }
                Some(&first)
                    if eligible(c.strategy) && !eligible(self.closures[first].strategy) =>
                {
                    kept.insert(pair(c), i);
                }
                Some(_) => {}
            }
        }
        let mut replaced = Vec::new();
        let mut drop: BTreeSet<usize> = BTreeSet::new();
        for (i, c) in self.closures.iter().enumerate() {
            if !on_line.contains(&c.a) && !on_line.contains(&c.b) {
                continue;
            }
            let Some(&stays) = kept.get(&pair(c)) else {
                continue;
            };
            if stays != i {
                drop.insert(i);
                let by = self.closures[stays];
                if eligible(by.strategy) && !eligible(c.strategy) {
                    replaced.push(ReplacedClosure { dropped: *c, by });
                }
            }
        }
        if !drop.is_empty() {
            let mut index = 0;
            self.closures.retain(|_| {
                index += 1;
                !drop.contains(&(index - 1))
            });
        }
        replaced
    }

    /// The prefix plan (`station_naming::plan_combine_prefixes`) on the two
    /// lines' station names, `jo` being the other line's join station (its
    /// name is retired).
    fn combine_prefix_plan(
        &self,
        kept: LineId,
        other: LineId,
        jo: StationId,
    ) -> crate::project::station_naming::CombinePrefixes {
        let names = |line: LineId| -> Vec<String> {
            self.line_stations(line)
                .unwrap_or(&[])
                .iter()
                .filter_map(|s| self.stations.get(s).map(|st| st.name.clone()))
                .collect()
        };
        let (kept_names, other_names) = (names(kept), names(other));
        let join_name = self.stations.get(&jo).map(|s| s.name.clone());
        plan_combine_prefixes(
            &kept_names.iter().map(String::as_str).collect::<Vec<_>>(),
            &other_names.iter().map(String::as_str).collect::<Vec<_>>(),
            join_name.as_deref(),
        )
    }

    /// Rename the two lines' stations by the prefix plan (before the join).
    fn prefix_combined_names(&mut self, kept: LineId, other: LineId, jk: StationId, jo: StationId) {
        let plan = self.combine_prefix_plan(kept, other, jo);
        if plan.is_empty() {
            return;
        }
        let kept_chain: Vec<StationId> = self.line_stations(kept).unwrap_or(&[]).to_vec();
        let other_chain: Vec<StationId> = self.line_stations(other).unwrap_or(&[]).to_vec();
        let mut done: BTreeSet<StationId> = BTreeSet::new();
        for (chain, renames) in [(kept_chain, &plan.kept), (other_chain, &plan.other)] {
            for s in chain {
                if s == jo && s != jk || !done.insert(s) {
                    continue;
                }
                if let Some(station) = self.stations.get_mut(&s) {
                    if let Some(to) = renames.get(&station.name) {
                        station.name.clone_from(to);
                    }
                }
            }
        }
    }

    /// What combining `other` into `kept` (joined at `jk` and `jo`) renames,
    /// for the dialog (today's `combine_station_renames`).
    pub(super) fn combine_rename_report(
        &self,
        kept: LineId,
        other: LineId,
        jk: StationId,
        jo: StationId,
    ) -> Option<crate::CombineStationRenames> {
        use crate::project::station_naming::CombinePrefixReason;
        let plan = self.combine_prefix_plan(kept, other, jo);
        let reason = match plan.reason? {
            CombinePrefixReason::Collision => crate::CombineRenameReason::Collision,
            CombinePrefixReason::Lettered => crate::CombineRenameReason::Lettered,
        };
        let names = |line: LineId| -> Vec<String> {
            self.line_stations(line)
                .unwrap_or(&[])
                .iter()
                .filter_map(|s| self.stations.get(s).map(|st| st.name.clone()))
                .collect()
        };
        let retired = self.stations.get(&jo).map(|s| s.name.clone());
        let no_suffixes = BTreeSet::new();
        let suffixed_set: BTreeSet<String> = plan.other_suffixed.iter().cloned().collect();
        let summarize = |line: LineId,
                         renames: &std::collections::HashMap<String, String>,
                         suffixed: &BTreeSet<String>,
                         skip: Option<&str>|
         -> Option<crate::CombineLineRenames> {
            let names: Vec<String> = names(line)
                .into_iter()
                .filter(|name| Some(name.as_str()) != skip)
                .collect();
            let lettered: Vec<(&String, &String)> = names
                .iter()
                .filter(|old| !suffixed.contains(*old))
                .filter_map(|old| renames.get(old).map(|new| (old, new)))
                .collect();
            let (first, last) = (lettered.first()?, lettered.last()?);
            let renamed = lettered.len().saturating_add(suffixed.len());
            Some(crate::CombineLineRenames {
                count: u32::try_from(lettered.len()).unwrap_or(u32::MAX),
                unchanged: u32::try_from(names.len().saturating_sub(renamed)).unwrap_or(u32::MAX),
                first_from: first.0.clone(),
                first_to: first.1.clone(),
                last_from: last.0.clone(),
                last_to: last.1.clone(),
            })
        };
        let kept_renames = summarize(kept, &plan.kept, &no_suffixes, None);
        let other_renames = summarize(other, &plan.other, &suffixed_set, retired.as_deref());
        let suffixed: Vec<crate::CombineStationRename> = names(other)
            .into_iter()
            .filter(|old| plan.other_suffixed.contains(old))
            .filter_map(|old| {
                let to = plan.other.get(&old)?.clone();
                Some(crate::CombineStationRename { from: old, to })
            })
            .collect();
        if kept_renames.is_none() && other_renames.is_none() && suffixed.is_empty() {
            return None;
        }
        let join = self.stations.get(&jk)?.name.clone();
        let join_name = plan.kept.get(&join).cloned().unwrap_or(join);
        Some(crate::CombineStationRenames {
            reason,
            kept: kept_renames,
            other: other_renames,
            suffixed: (!suffixed.is_empty()).then_some(suffixed),
            join_name,
        })
    }
}
