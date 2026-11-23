//! Combine Lines, the join (plan 5.4 steps 2-6 applied) and the facts the
//! dialog shows, read off the result.

use std::collections::{BTreeMap, BTreeSet};

use crate::station_naming::plan_combine_prefixes;
use crate::{
    normalize_azimuth, CombineLinesOptions, CombinePick, LineEnd, LineId, StationId, StationRef,
};

use super::super::super::archive::has_text;
use super::super::super::ops_detach::DetachMode;
use super::super::super::{
    ClosureEnd, ClosureId, ClosureStrategy, EditContext, IdentityReport, LoopClosure, Placement,
    Project, RawPositions,
};
use super::super::lines::absorb_legacy_credit;
use super::super::EditError;
use super::plan::{moving_side, Decision, DUST_M};
use super::{
    CombineReport, JoinedMove, RemovedClosure, ReplacedClosure, TornTie, TornTieKind, TornTieReason,
};

/// Positions agree within this when the facts compare moves (metres): well
/// under the float dust a shift may be (`DUST_M`), well over the rounding of
/// a walk.
const SAME_MOVE_M: f64 = 1e-9;

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

/// Which side of the combine a closure end is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Moving,
    Fixed,
    Neither,
}

/// The stations of `line`'s side at a merged join station `j`: the line
/// without `j` and what hangs off it.
fn moving_side_of(p: &Project, line: LineId, j: StationId) -> BTreeSet<StationId> {
    moving_side(p, line, j, true).0
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
        let skipped_before = self.skipped_closure_ids(&p0);
        let pre = self.clone();
        // The rigid ties before the combine: snaps (aligned `Move` closures)
        // and legacy `Tie`s; and the torn snaps (`Move`, not aligned), which
        // the move may bring back together.
        let snaps_before: BTreeSet<ClosureId> = self
            .closures
            .iter()
            .filter(|c| match c.strategy {
                ClosureStrategy::Move => self.closure_is_aligned(&p0, c),
                ClosureStrategy::Tie => true,
                _ => false,
            })
            .map(|c| c.id)
            .collect();
        let torn_snaps_before: BTreeSet<ClosureId> = self
            .closures
            .iter()
            .filter(|c| c.strategy == ClosureStrategy::Move && !snaps_before.contains(&c.id))
            .map(|c| c.id)
            .collect();
        let gap_of = |p: &RawPositions, c: &LoopClosure| -> Option<f64> {
            let (a, b) = (p.xy(c.a)?, p.xy(c.b)?);
            Some((a[0] - b[0]).hypot(a[1] - b[1]))
        };
        let tie_close_before: BTreeSet<ClosureId> = self
            .closures
            .iter()
            .filter(|c| {
                c.strategy == ClosureStrategy::Tie
                    && gap_of(&p0, c)
                        .is_some_and(|g| g < super::super::super::ALIGNMENT_TOLERANCE_M)
            })
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

        // Steps 2, 3, 6: the junctions torn into Batch closures. A tear is a
        // restoring detach of one line (RS 4.4, `DetachMode::Tear`); its
        // closure archives, once the joins are known, the end that does not
        // hold the identity the torn slot showed, so nothing visible changes
        // and Remove Closure gives the line its own station back.
        let mut renamed: Vec<(StationId, StationId)> = Vec::new();
        let mut slot_map: BTreeMap<(LineId, StationId), StationId> = BTreeMap::new();
        let mut torn: Vec<TornTie> = Vec::new();
        let mut tears: Vec<TearEnds> = Vec::with_capacity(decision.detaches.len());
        // Torn junctions whose id went with the torn line (it was their only
        // native line, RS 4.4), to the node their other lines are on: where
        // the junction's position (and a gauge on it) stays.
        let mut rest_of: BTreeMap<StationId, StationId> = BTreeMap::new();
        let (mut jk, mut jo) = (join.kept_station, join.other_station);
        let slot = |p: &Project, s: StationId, line: Option<LineId>| -> StationRef {
            let topology = p.topology();
            line.and_then(|l| topology.slot_on(s, l))
                .or_else(|| topology.first_slot(s))
                .unwrap_or(StationRef::new(0, 0))
        };
        for d in &decision.detaches {
            let shown_before = self.shown(d.station);
            let through = self.topology().lines_of(d.station);
            let report = self.detach_ids(d.station, d.line, DetachMode::Tear)?;
            let (line_node, rest_node) = (report.line_node(), report.rest_node());
            renamed.push((d.station, report.new_station));
            if report.kept {
                rest_of.insert(d.station, rest_node);
            }
            for line in through {
                let now = report.node_of(line);
                if now != d.station {
                    slot_map.insert((line, d.station), now);
                }
            }
            if jk == d.station {
                jk = report.node_of(k);
            }
            if jo == d.station {
                jo = report.node_of(o);
            }
            let id = self.push_closure(
                rest_node,
                d.toward,
                line_node,
                Some(d.line),
                ClosureStrategy::Batch,
            );
            tears.push(TearEnds {
                closure: id,
                shown_before,
                line_node,
                rest_node,
            });
            torn.push(TornTie {
                kind: TornTieKind::Junction,
                reason: d.reason,
                station: d.station,
                slot: slot(&pre, d.station, Some(d.line)),
                other_slot: slot(&pre, d.station, d.toward),
                closure: Some(id),
            });
        }

        // The two sides, after the tears (step 5's snaps read them). At a
        // merged join each side is its line without the join station and
        // what hangs off it; the lines at the join station are on neither
        // (they stay with the joined station), and a closure end AT the join
        // station belongs to the side its `via` names.
        let (jm, jf) = if decision.moving == o {
            (jo, jk)
        } else {
            (jk, jo)
        };
        let merged_now = jm == jf;
        let fixed_line = if decision.moving == o { k } else { o };
        let (moving_side, _) = moving_side(self, decision.moving, jm, merged_now);
        let fixed_side: BTreeSet<StationId> = if merged_now {
            moving_side_of(self, fixed_line, jf)
        } else {
            self.component_stations(jf)
        };
        let side_of = |s: StationId, via: Option<LineId>| -> Side {
            if merged_now && s == jm {
                if via == Some(decision.moving) {
                    Side::Moving
                } else if via.is_none() || via == Some(fixed_line) {
                    Side::Fixed
                } else {
                    Side::Neither
                }
            } else if moving_side.contains(&s) {
                Side::Moving
            } else if fixed_side.contains(&s) {
                Side::Fixed
            } else {
                Side::Neither
            }
        };
        let crossing = |c: &LoopClosure| {
            let (a, b) = (side_of(c.a, c.a_via), side_of(c.b, c.b_via));
            (a == Side::Moving && b == Side::Fixed) || (a == Side::Fixed && b == Side::Moving)
        };
        let held_parts: BTreeSet<StationId> = decision
            .detaches
            .iter()
            .zip(&tears)
            .filter(|(d, _)| d.reason == TornTieReason::ZeroDatum)
            .flat_map(|(_, t)| self.component_stations(t.rest_node))
            .collect();
        let mut reclose_check: BTreeSet<ClosureId> = BTreeSet::new();
        for closure in &mut self.closures {
            if torn_snaps_before.contains(&closure.id) && crossing(closure) {
                // A torn snap across the two sides: if the move brings it back
                // together it would tie the combined line to itself (checked
                // once the geometry is final).
                reclose_check.insert(closure.id);
                continue;
            }
            if !snaps_before.contains(&closure.id) {
                continue;
            }
            let (a, b) = (
                side_of(closure.a, closure.a_via),
                side_of(closure.b, closure.b_via),
            );
            let crosses = crossing(closure);
            let pulled = decision.shifting && (a == Side::Moving) != (b == Side::Moving);
            if crosses || pulled {
                closure.strategy = ClosureStrategy::Batch;
                let reason = if held_parts.contains(&closure.a) || held_parts.contains(&closure.b) {
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

        // The tear closures' archives (RS 4.4), now that the joins are known.
        for tear in &tears {
            self.archive_tear(tear, (jo != jk).then_some(jo));
        }

        // Step 4: the two join stations become one node, `jk`'s (RS 4.4
        // join): `jo` is archived in it with its native lines but the
        // absorbed one, and the comment pick applies when it becomes history.
        // A closure between the two that archives `jk` forces `jo`'s
        // identity instead (RS 4.1 step 4), given to the node once the
        // geometry is settled ([`Self::keep_other_join`]).
        let mut folded: Option<(StationId, StationId)> = None;
        let mut join_forced: Option<(ClosureId, Option<String>, Option<String>)> = None;
        if jo != jk {
            let forced = self
                .forced_merge_keep(jk, jo)
                .filter(|&(_, kept)| kept == jo)
                .map(|(closure, _)| closure);
            let comment = |s: StationId| self.stations.get(&s).and_then(|s| s.comment.clone());
            let comments = (comment(jk), comment(jo));
            self.merge_ids(jk, jo)?;
            match forced {
                None => self.pick_join_comment(jk, jo, o, options.comment_pick),
                Some(closure) => join_forced = Some((closure, comments.0, comments.1)),
            }
            // Closures between the two join stations close nothing now (#738).
            self.closures.retain(|c| c.a != c.b);
            renamed.push((jo, jk));
            folded = Some((jo, jk));
        }

        // The shots: the other line's join the kept line's, with new ids.
        let Some(position) = self.line_position(o) else {
            return Err(EditError::line_gone());
        };
        let other = self.lines_mut().remove(position);
        // The other line's stations come onto the kept line: the records
        // naming the other line there name the kept one (RS 4.4). A join
        // merged here is the kept line's own station: its new record (the
        // retired join) loses the absorbed line. A join both lines already
        // shared is native on the combined line
        // ([`Self::settle_shared_join_records`]).
        let arrived: BTreeSet<StationId> =
            other.stations().filter(|&s| s != jk && s != jo).collect();
        if jo == jk {
            self.settle_shared_join_records(jk, k, o);
        }
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
        self.repoint_record_lines(o, k, &arrived);

        // Step 1's outcome: the fixed side stays, the moving side moves. `s`
        // is a pre-edit station, `at` where it is after the tears.
        let intended = |s: StationId, at: StationId, p: &RawPositions| -> Option<[f64; 2]> {
            let [x, y] = p.xy(s)?;
            Some(if moving_side.contains(&at) {
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
            // The old placement stations that survive in the joined piece (the
            // other join station's is the joined station now, plan 5.1), the
            // fixed side's first; laid out where the combine puts them. A
            // placement on a junction a tear took its id from stays with the
            // junction's other lines (`rest_of`): a placement is a position.
            let stays_at = |s: StationId| {
                let mut at = s;
                for _ in 0..=rest_of.len() {
                    match rest_of.get(&at) {
                        Some(&next) => at = next,
                        None => break,
                    }
                }
                at
            };
            let follow = |s: StationId| match folded {
                Some((from, to)) if s == from => to,
                _ => s,
            };
            let candidates: Vec<(StationId, StationId, StationId)> = pre
                .placements
                .iter()
                .map(|p| (p.station, stays_at(p.station)))
                .map(|(was, at)| (was, at, follow(at)))
                .filter(|(_, _, now)| joined.contains(now))
                .collect();
            let g = candidates
                .iter()
                .copied()
                .find(|(_, at, _)| !moving_side.contains(at))
                .or_else(|| candidates.first().copied())
                .or_else(|| {
                    joined
                        .iter()
                        .copied()
                        .find(|s| p0.xy(*s).is_some())
                        .map(|s| (s, s, s))
                });
            if let Some((was, at, g)) = g {
                if let Some([x, y]) = intended(was, at, &p0) {
                    self.placements.retain(|p| !joined.contains(&p.station));
                    self.placements.push(Placement { station: g, x, y });
                    ctx = ctx.with_placed(g);
                }
            }
        }
        // The epilogue (RS 3): stale attachments, the forced join identity,
        // the A4 restore, anchors; then (below) the closure rules, the
        // normalisation and rule L.
        self.drop_stale_attachments();
        let mut reidentified: Vec<(StationId, StationId)> = Vec::new();
        if let Some((_, kept_comment, other_comment)) = &join_forced {
            self.keep_other_join(
                (jk, jo),
                k,
                options.comment_pick,
                (kept_comment.clone(), other_comment.clone()),
            );
            reidentified.push((jk, jo));
        }
        let restored = self.restore_archived_identities();
        reidentified.extend(restored.iter().map(|r| (r.old, r.new)));
        for &(old, new) in &reidentified {
            if ctx.placed.contains(&old) {
                ctx.placed.push(new);
            }
        }
        ctx = ctx.with_reidentified(&reidentified);
        self.settle_anchors(&before, &ctx);

        // Snaps (and legacy ties) the move pulled apart become ordinary, as
        // today; so does a torn snap across the two sides that the move
        // brought back together (it would tie the combined line to itself).
        let positions = self.raw_positions();
        let mut ordinary: Vec<(ClosureId, TornTieReason)> = Vec::new();
        for c in &self.closures {
            let pulled = match c.strategy {
                ClosureStrategy::Move => {
                    snaps_before.contains(&c.id) && !self.closure_is_aligned(&positions, c)
                }
                ClosureStrategy::Tie => {
                    tie_close_before.contains(&c.id)
                        && gap_of(&positions, c)
                            .is_none_or(|g| g >= super::super::super::ALIGNMENT_TOLERANCE_M)
                }
                _ => false,
            };
            if pulled {
                ordinary.push((c.id, TornTieReason::Moved));
            } else if reclose_check.contains(&c.id)
                && c.strategy == ClosureStrategy::Move
                && self.closure_is_aligned(&positions, c)
            {
                ordinary.push((c.id, TornTieReason::Loop));
            }
        }
        for closure in &mut self.closures {
            if let Some(&(_, reason)) = ordinary.iter().find(|(id, _)| *id == closure.id) {
                closure.strategy = ClosureStrategy::Batch;
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

        let replaced = self.dedupe_closures_on(k);
        let removed = self.remove_closures_newly_across_one_shot(&skipped_before);
        let unarchived = self.normalise_closure_archives();
        // Rule L (RS 4.4: collisions on the combined and third lines).
        let lettered = self.letter_collisions(&pre);
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
                for &(old, new) in &reidentified {
                    if now == old {
                        now = new;
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
        let moved_with_joined_station = (!joined_lines.is_empty()).then_some(JoinedMove {
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
            join_keep_forced_by: join_forced.map(|(closure, _, _)| closure),
            identities: IdentityReport {
                restored,
                unarchived,
                lettered,
            },
            reidentified,
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
        let mut archives: Vec<(usize, StationId)> = Vec::new();
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
                // RS 4.4 dedupe: an archive moves to the closure that stays.
                if let Some((archived, _)) = c.archive() {
                    archives.push((stays, archived));
                }
            }
        }
        for (stays, archived) in archives {
            if let Some(closure) = self.closures.get_mut(stays) {
                if closure.archived.is_none() {
                    closure.archived = Some(if closure.a == archived {
                        ClosureEnd::A
                    } else {
                        ClosureEnd::B
                    });
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

    /// RS 4.4 shared join (`j` on both lines): the combined line is native at
    /// `j` when either line was; a record naming either line drops it (and is
    /// history when it names no other). When neither was native, the record
    /// naming the kept line keeps it (the combined line still shows `j`'s
    /// identity through it, and Detach still gives that station back), and
    /// only the absorbed line is dropped. Nothing visible changes: both lines
    /// showed `j`'s identity.
    fn settle_shared_join_records(&mut self, j: StationId, kept: LineId, other: LineId) {
        if let Some(station) = self.stations.get_mut(&j) {
            let native = station.record_for(kept).is_none() || station.record_for(other).is_none();
            for record in &mut station.archived {
                record
                    .lines
                    .retain(|&l| l != other && (l != kept || !native));
            }
        }
    }

    /// A tear closure's archive (RS 4.4): the end that does not hold the
    /// identity the torn slot showed before, so nothing visible changes;
    /// none when neither end holds it, when the keep is not allowed (C1,
    /// C2), or when the end it would archive is `folded` (the other line's
    /// join station, which the join folds into the kept one: the combined
    /// line shows the kept join station there).
    fn archive_tear(&mut self, tear: &TearEnds, folded: Option<StationId>) {
        let (archived, kept, end) = if self.shown(tear.rest_node) == tear.shown_before {
            (tear.line_node, tear.rest_node, ClosureEnd::B)
        } else if self.shown(tear.line_node) == tear.shown_before {
            (tear.rest_node, tear.line_node, ClosureEnd::A)
        } else {
            return;
        };
        if folded == Some(archived)
            || !self.closure_keep_allowed(tear.rest_node, tear.line_node, kept)
        {
            return;
        }
        if let Some(closure) = self.closures.iter_mut().find(|c| c.id == tear.closure) {
            closure.archived = Some(end);
        }
    }

    /// RS 4.1 step 4 at the join: a closure between the two join stations
    /// archived `jk`, so the joined node takes `jo`'s identity (after the
    /// fold, which kept `jk`'s id for the geometry): `jk`'s identity becomes
    /// a record naming its native lines but the combined line `kept` (which
    /// is `jo`'s now: its own line was absorbed into it), and the node's id
    /// becomes `jo` everywhere. The comment pick applies when `jk` becomes
    /// history, between `comments` (`jk`'s, then `jo`'s, before the join).
    fn keep_other_join(
        &mut self,
        (jk, jo): (StationId, StationId),
        kept: LineId,
        pick: CombinePick,
        comments: (Option<String>, Option<String>),
    ) {
        let native: Vec<LineId> = self
            .native_lines(jk)
            .into_iter()
            .filter(|&l| l != kept)
            .collect();
        let Some(node) = self.stations.get_mut(&jk) else {
            return;
        };
        let Some(index) = node.archived.iter().position(|r| r.id == jo) else {
            return;
        };
        let record = node.archived.remove(index);
        let old = node.as_record(native);
        node.name = record.name;
        node.comment = if old.lines.is_empty() {
            joined_comment(comments.0, comments.1, pick)
        } else {
            record.comment
        };
        node.external = record.external;
        node.archived.push(old);
        self.rekey_node(jk, jo);
    }

    /// RS 4.4 join: Combine's comment pick between the kept join's comment
    /// and the retired join's applies only when the retired join `jo`
    /// becomes history (the absorbed line `other` was its only native line);
    /// otherwise its comment stays in its record, shown again by a Detach.
    /// Identical texts are kept once; with one side empty, whichever exists
    /// (today's `pick_text`).
    fn pick_join_comment(
        &mut self,
        jk: StationId,
        jo: StationId,
        other: LineId,
        pick: CombinePick,
    ) {
        let Some(station) = self.stations.get_mut(&jk) else {
            return;
        };
        let Some(record) = station.archived.iter().find(|r| r.id == jo) else {
            return;
        };
        if record.lines.iter().any(|&l| l != other) {
            return;
        }
        let gone = record.comment.clone();
        station.comment = joined_comment(station.comment.clone(), gone, pick);
    }

    /// The prefix plan (`station_naming::plan_combine_prefixes`) on the names
    /// the two lines show, `jo` being the other line's join station (its
    /// name is retired). At a merged join `jo` is a station of the kept line
    /// too, and the combined line shows it by the kept line's name: it is
    /// left out of the other line's names, as 0.6.0's retired member name
    /// was the other line's own, so a lettered kept name there does not count
    /// as a letter family of the other line (which would stop the kept line
    /// continuing its scheme).
    fn combine_prefix_plan(
        &self,
        kept: LineId,
        other: LineId,
        jo: StationId,
    ) -> crate::station_naming::CombinePrefixes {
        let kept_chain = self.line_stations(kept).unwrap_or(&[]);
        let shared_join = kept_chain.contains(&jo);
        // Each line's names as it shows them (`Project::shown_name`): the
        // combined line shows every station as its line did.
        let names = |_line: LineId, chain: &[StationId], skip: Option<StationId>| -> Vec<String> {
            chain
                .iter()
                .filter(|s| Some(**s) != skip)
                .filter_map(|s| self.shown_name(*s).map(str::to_string))
                .collect()
        };
        let kept_names = names(kept, kept_chain, None);
        let other_names = names(
            other,
            self.line_stations(other).unwrap_or(&[]),
            shared_join.then_some(jo),
        );
        let join_name = self.shown_name(jo).map(str::to_string);
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
                let Some(name) = self.shown_name(s).map(str::to_string) else {
                    continue;
                };
                if let Some(to) = renames.get(&name) {
                    // The shown identity's name, which its other lines show
                    // too; a repeat there is rule L's (RS 4.4), settled with
                    // the combine's other collisions.
                    self.set_shown_name(s, to.clone());
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
        use crate::station_naming::CombinePrefixReason;
        let plan = self.combine_prefix_plan(kept, other, jo);
        let reason = match plan.reason? {
            CombinePrefixReason::Collision => crate::CombineRenameReason::Collision,
            CombinePrefixReason::Lettered => crate::CombineRenameReason::Lettered,
        };
        let names = |line: LineId| -> Vec<String> {
            self.line_stations(line)
                .unwrap_or(&[])
                .iter()
                .filter_map(|s| self.shown_name(*s).map(str::to_string))
                .collect()
        };
        let retired = self.shown_name(jo).map(str::to_string);
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
        let join = self.shown_name(jk)?.to_string();
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

/// One tear (RS 4.4): its closure, the identity the torn slot showed, the
/// node the torn line is on and the node the other lines are on.
#[derive(Debug, Clone, Copy)]
struct TearEnds {
    closure: ClosureId,
    shown_before: StationId,
    line_node: StationId,
    rest_node: StationId,
}

/// Combine's comment pick between the kept join's comment and the other
/// join's (today's `pick_text`): identical texts are kept once; with one
/// side empty, whichever exists.
fn joined_comment(
    kept: Option<String>,
    other: Option<String>,
    pick: CombinePick,
) -> Option<String> {
    let kept = kept.filter(|c| has_text(Some(c)));
    let other = other.filter(|c| has_text(Some(c)));
    match (kept, other) {
        (Some(k), Some(g)) if k == g => Some(k),
        (Some(k), Some(g)) => Some(match pick {
            CombinePick::Kept => k,
            CombinePick::Other => g,
            CombinePick::Both => format!("{k}\n{g}"),
        }),
        (k, g) => k.or(g),
    }
}
