//! Combine Lines, the read-only half: can two lines be joined into one,
//! where, and what has to change for it. The context menu, the Sections panel
//! and the dialog ask this, and `combine_lines` asks it again before it
//! changes anything.
//!
//! The rule: the two chosen lines meet end to end; other lines there
//! stay attached as branches. An end station of each is the same point when:
//! - **merged**: the two end stations are in one merge group;
//! - **co-located**: unmerged, their chain positions (line offset plus the
//!   shot walk, without the stations' manual offsets) agree within
//!   [`COMBINE_TOLERANCE_M`] horizontally;
//! - **closure**: a loop closure ties the two (or their merge groups). The
//!   survey says they are one point; one side moves by the misclosure.
//!
//! Two lines that meet are always combinable, whatever else ties them
//! (`combine_plan.rs` turns the ties in the way into loop closures and moves a
//! side), except when both carry a zero datum on one of their own stations,
//! or the combined line would run out of shot numbers. Two lines that
//! meet nowhere are refused: merge two of their ends, or close a loop between
//! them, first.
//!
//! Every other line with an END merged at the point stays merged to the
//! joined station, which becomes a mid-line station of the combined
//! line (a branch); one merged there at a station in its MIDDLE passes
//! through, and stays merged too (a crossing); one with an end there that is
//! not merged is left as it is (`CombineCandidate::loose_ends`). At a junction
//! the station query lists every partner, and the user picks one. Depth is not
//! part of the match: the two stations read the depth of different shots, and
//! the joined station takes their average. A gap above
//! [`COMBINE_DEPTH_NOTICE_M`] is reported for the dialog.

use std::collections::{HashMap, HashSet};

use crate::{
    CombineCandidate, CombineJoinEnds, CombineJoinKind, CombinePlan, Line, LineEnd, LineId,
    StationKey, StationRef, FEATURE_ID_STRIDE,
};

use super::combine_plan::{ClosureFacts, JoinPoint, Tie, TiePlan};
use super::station_rekey::value_naming;
use super::Project;

/// How far apart (metres) the two join stations may be and still count as the
/// same point (side by side); also the bound on how far a co-located join
/// moves a side, which it does by the residual (#740, `combine_plan.rs`).
pub const COMBINE_TOLERANCE_M: f64 = 0.001;

/// Above this depth gap (metres) between two co-located, unmerged join
/// stations, the candidate reports it and the dialog shows it: the joined
/// station takes their average, so each end moves by half the gap. Half the
/// 0.1 m resolution depth gauges and dive computers record; a smaller gap is
/// below what the survey can tell apart.
pub const COMBINE_DEPTH_NOTICE_M: f64 = 0.05;

/// One way two lines can meet: an end of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pairing {
    pub a: LineId,
    pub end_a: LineEnd,
    pub b: LineId,
    pub end_b: LineEnd,
}

/// What a touching pairing looks like.
#[derive(Debug, Clone)]
pub(crate) struct Touch {
    pub kind: CombineJoinKind,
    pub blocked: Option<String>,
    /// The merge group of each join station (one group when merged).
    pub group_a: HashSet<StationRef>,
    pub group_b: HashSet<StationRef>,
    /// Other lines merged at the join point with an end: they stay
    /// attached to the joined station, a branch of the combined line.
    pub branches: Vec<LineId>,
    /// Other lines merged at the join point at a station in their middle:
    /// they pass through, and stay merged to the joined station.
    pub passing_through: Vec<LineId>,
    /// Other lines with an end at the join point that is NOT merged to
    /// it: left untouched.
    pub loose_ends: Vec<LineId>,
}

impl Touch {
    /// A pairing that does not touch, reported with `reason`.
    fn refused(reason: String) -> Self {
        Self {
            kind: CombineJoinKind::CoLocated,
            blocked: Some(reason),
            group_a: HashSet::new(),
            group_b: HashSet::new(),
            branches: Vec::new(),
            passing_through: Vec::new(),
            loose_ends: Vec::new(),
        }
    }

    /// Precedence when two lines meet at more than one place: a merge
    /// first, then co-located ends, then a closure.
    fn rank(&self) -> u8 {
        match self.kind {
            CombineJoinKind::Merged => 0,
            CombineJoinKind::CoLocated => 1,
            CombineJoinKind::Closure => 2,
        }
    }
}

/// The most other line ends the station query lists at one point. Past it
/// the point is a pile (unplaced lines all start at the origin), not a
/// junction to choose from in a menu, and every partner costs a pair query;
/// the Sections panel still combines any two of them.
pub const MAX_COMBINE_PARTNERS: usize = 16;

fn end_index(line: &Line, end: LineEnd) -> u32 {
    match end {
        LineEnd::Start => 0,
        LineEnd::End => line.shots.len() as u32,
    }
}

/// A line as the user knows it: its name, or "Line N" when it has none
/// (the label the Combine dialog and its refusals use).
pub fn line_label(line: &Line) -> String {
    line.name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Line {}", line.id))
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Every line end's chain position (line offset plus the shot walk,
/// without manual station offsets), computed ONCE per query: a candidate
/// query looks for co-located ends across the whole project, and walking every
/// line per pairing made a right-click O(lines x shots).
///
/// The ends are also bucketed on a grid one tolerance wide, so the ends near a
/// point are in its cell or the eight around it and a lookup reads only those,
/// not every end in the project: a pile of lines at one point (unplaced
/// lines all start at the origin) made each partner's third-end check a
/// full scan, O(lines^2) per right-click.
pub(crate) struct EndTable {
    ends: Vec<(LineId, LineEnd, (f64, f64))>,
    by_end: HashMap<(LineId, LineEnd), (f64, f64)>,
    cells: HashMap<(i64, i64), Vec<usize>>,
    /// The project's rigid ties, resolved once per query on first use: a
    /// junction's station query plans two choices of kept line for every
    /// partner, and each plan reads all of them.
    ties: std::cell::OnceCell<Vec<Tie>>,
    /// The loop-closure facts the anchor rule reads, once per query and only
    /// when a shifting plan needs them (#737; [`ClosureFacts`]).
    closure_facts: ClosureFacts,
}

#[cfg(test)]
thread_local! {
    /// How many [`EndTable`]s this thread built: a query must build one.
    pub(crate) static END_TABLE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many ends [`EndTable::near`] has compared against a point.
    pub(crate) static END_TABLE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The grid cell `point` falls in. `as` saturates (and reads NaN as 0); a
/// non-finite position then fails the exact distance test in `near`, so the
/// cell it lands in does not matter.
fn cell_of(point: (f64, f64)) -> (i64, i64) {
    (
        (point.0 / COMBINE_TOLERANCE_M).floor() as i64,
        (point.1 / COMBINE_TOLERANCE_M).floor() as i64,
    )
}

impl EndTable {
    fn new(ends: Vec<(LineId, LineEnd, (f64, f64))>) -> Self {
        let by_end = ends.iter().map(|(id, end, p)| ((*id, *end), *p)).collect();
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, (_, _, p)) in ends.iter().enumerate() {
            cells.entry(cell_of(*p)).or_default().push(i);
        }
        Self {
            ends,
            by_end,
            cells,
            ties: std::cell::OnceCell::new(),
            closure_facts: ClosureFacts::default(),
        }
    }

    fn at(&self, id: LineId, end: LineEnd) -> Option<(f64, f64)> {
        self.by_end.get(&(id, end)).copied()
    }

    /// Every line end within the tolerance of `point`, lazily: a caller
    /// that stops at the first match reads only the ends before it.
    fn near(&self, point: (f64, f64)) -> impl Iterator<Item = (LineId, LineEnd)> + '_ {
        let (cx, cy) = cell_of(point);
        (-1..=1)
            .flat_map(move |dx| {
                (-1..=1).map(move |dy| (cx.saturating_add(dx), cy.saturating_add(dy)))
            })
            .filter_map(|cell| self.cells.get(&cell))
            .flatten()
            .map(|&i| &self.ends[i])
            .filter(move |(_, _, p)| {
                #[cfg(test)]
                END_TABLE_PROBES.with(|probes| probes.set(probes.get() + 1));
                distance(*p, point) <= COMBINE_TOLERANCE_M
            })
            .map(|(id, end, _)| (*id, *end))
    }
}

impl Project {
    /// See [`EndTable`]. The walk is the coordinate solver's
    /// (`compute_station_raw_xy`) without the station offsets.
    pub(crate) fn end_table(&self) -> EndTable {
        #[cfg(test)]
        END_TABLE_BUILDS.with(|builds| builds.set(builds.get() + 1));
        let mut ends = Vec::with_capacity(self.lines.len() * 2);
        for line in &self.lines {
            let start = self.line_offset(line.id);
            let reference = self.azimuth_reference_for(line);
            let (mut x, mut y) = start;
            for shot in &line.shots {
                let azimuth = shot.average_azimuth_true_rad(reference);
                let horizontal = shot.horizontal_length();
                x += horizontal * azimuth.sin();
                y += horizontal * azimuth.cos();
            }
            ends.push((line.id, LineEnd::Start, start));
            ends.push((line.id, LineEnd::End, (x, y)));
        }
        EndTable::new(ends)
    }

    /// Does a loop closure tie a station of `group_a` to one of `group_b`?
    fn closure_between(
        &self,
        group_a: &HashSet<StationRef>,
        group_b: &HashSet<StationRef>,
    ) -> bool {
        self.loop_closures.iter().any(|closure| {
            match (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) {
                (Some(x), Some(y)) => {
                    (group_a.contains(&x) && group_b.contains(&y))
                        || (group_b.contains(&x) && group_a.contains(&y))
                }
                _ => false,
            }
        })
    }

    /// Whether `pairing`'s two end stations are the same point (`None` when
    /// they are not), how, what else is there, and why combining the two
    /// lines is refused, if it is.
    pub(crate) fn pairing_touch(&self, pairing: Pairing, ends: &EndTable) -> Option<Touch> {
        let line_a = self.get_line(pairing.a)?;
        let line_b = self.get_line(pairing.b)?;
        let at_a = StationRef::new(pairing.a, end_index(line_a, pairing.end_a));
        let at_b = StationRef::new(pairing.b, end_index(line_b, pairing.end_b));
        let (pa, pb) = (
            ends.at(pairing.a, pairing.end_a)?,
            ends.at(pairing.b, pairing.end_b)?,
        );
        // An end the walk cannot place (an overflowing or non-finite shot)
        // meets nothing: joining there would shift a side by a non-finite gap.
        if ![pa.0, pa.1, pb.0, pb.1].iter().all(|v| v.is_finite()) {
            return None;
        }
        let group_a = self.get_merge_group(at_a);
        let merged = group_a.contains(&at_b);
        let group_b = if merged {
            group_a.clone()
        } else {
            self.get_merge_group(at_b)
        };
        let kind = if merged {
            CombineJoinKind::Merged
        } else if distance(pa, pb) <= COMBINE_TOLERANCE_M {
            CombineJoinKind::CoLocated
        } else if self.closure_between(&group_a, &group_b) {
            CombineJoinKind::Closure
        } else {
            return None;
        };
        // Every other line at the join point: merged there with an end (a
        // branch) or with a station in its middle (passing through).
        let mut branches: Vec<LineId> = Vec::new();
        let mut passing_through: Vec<LineId> = Vec::new();
        for at in group_a.iter().chain(group_b.iter()) {
            if at.line_id == pairing.a || at.line_id == pairing.b {
                continue;
            }
            let Some(line) = self.get_line(at.line_id) else {
                continue;
            };
            if at.station_index == 0 || at.station_index as usize == line.shots.len() {
                branches.push(at.line_id);
            } else {
                passing_through.push(at.line_id);
            }
        }
        branches.sort_unstable();
        branches.dedup();
        passing_through.sort_unstable();
        passing_through.dedup();
        passing_through.retain(|id| !branches.contains(id));
        let mut loose_ends: Vec<LineId> = ends.near(pa).map(|(id, _)| id).collect();
        if kind == CombineJoinKind::Closure {
            loose_ends.extend(ends.near(pb).map(|(id, _)| id));
        }
        loose_ends.retain(|id| {
            *id != pairing.a
                && *id != pairing.b
                && !branches.contains(id)
                && !passing_through.contains(id)
        });
        loose_ends.sort_unstable();
        loose_ends.dedup();
        Some(Touch {
            kind,
            blocked: self.combine_blocked_reason(line_a, line_b, [at_a, at_b]),
            group_a,
            group_b,
            branches,
            passing_through,
            loose_ends,
        })
    }

    /// Why combining `a` and `b` at their join stations `join` (one of each)
    /// is refused, or `None`. Everything else that stands in the way is
    /// changed, and the plan says what (`combine_plan.rs`).
    fn combine_blocked_reason(&self, a: &Line, b: &Line, join: [StationRef; 2]) -> Option<String> {
        // The two join stations become ONE station of the combined line, and
        // a station holds at most one datum (#741 D2): a placement datum and a
        // GPS fix folded onto one station key would lose the fix on the next
        // save, and Remove Zero Datum would clear both at once.
        if join.iter().all(|at| self.station_holds_datum(*at)) {
            return Some(format!(
                "{} #{} and {} #{} each hold a zero datum, and combining makes them one \
                 station: remove one of the two datums first",
                line_label(a),
                join[0].station_index,
                line_label(b),
                join[1].station_index
            ));
        }
        if self.carries_zero_datum(a.id) && self.carries_zero_datum(b.id) {
            return Some(format!(
                "{} and {} are each placed by their own zero datum: merge them first, so the \
                 later datum becomes a GPS fix",
                line_label(a),
                line_label(b)
            ));
        }
        let budget = FEATURE_ID_STRIDE.saturating_sub(1) as usize;
        if a.shots.len() + b.shots.len() > budget {
            return Some(format!(
                "The combined line would have {} shots (at most {budget})",
                a.shots.len() + b.shots.len()
            ));
        }
        None
    }

    /// Why `kept` cannot take `other`'s shots: they get new ids from `kept`,
    /// which has only [`Line::shot_ids_left`] of them. Not renumbered in
    /// place: position-
    /// keyed ignored warnings and the frontend's selection, hover and table
    /// state address shots by id. A reload renumbers every line compactly.
    pub(crate) fn shot_id_room_refusal(kept: &Line, other: &Line) -> Option<String> {
        (other.shots.len() as u64 > kept.shot_ids_left()).then(|| {
            format!(
                "{} has used up its shot numbers for this session: keep {}, or save and \
                 reopen",
                line_label(kept),
                line_label(other)
            )
        })
    }

    /// Why combining `other` into `kept` at `join` is refused, if it is: the
    /// pair's reason, else the one that depends on which line is kept. The
    /// one check `combine_lines` and the command both run before changing
    /// anything.
    pub fn combine_refusal(
        &self,
        kept: LineId,
        other: LineId,
        join: Option<CombineJoinEnds>,
    ) -> Option<String> {
        let Some(candidate) = self.combine_candidate_joining(kept, other, join) else {
            return Some(format!("Lines {kept} and {other} cannot be combined"));
        };
        candidate.blocked_reason.or(if candidate.line_a == kept {
            candidate.blocked_keeping_a
        } else {
            candidate.blocked_keeping_b
        })
    }

    /// The candidate for combining `other` into `kept` at `join` (the kept and
    /// the other line's ends), or at the join the two-line query
    /// prefers when `join` is `None`. `None` when either line is missing or
    /// they are the same line.
    pub fn combine_candidate_joining(
        &self,
        kept: LineId,
        other: LineId,
        join: Option<CombineJoinEnds>,
    ) -> Option<CombineCandidate> {
        let Some(join) = join else {
            return self.combine_candidate_for_lines(kept, other);
        };
        if kept == other {
            return None;
        }
        let (k, o) = (self.get_line(kept)?, self.get_line(other)?);
        let pairing = Pairing {
            a: kept,
            end_a: join.kept,
            b: other,
            end_b: join.other,
        };
        let table = self.end_table();
        let touch = self.pairing_touch(pairing, &table).unwrap_or_else(|| {
            Touch::refused(format!(
                "{} and {} do not meet at those ends",
                line_label(k),
                line_label(o)
            ))
        });
        self.candidate_for(pairing, touch, &table)
    }

    /// Degrees to add to `other`'s stored bearings so they read against
    /// `kept`'s north reference, when the two STORED references differ. The
    /// stored ones are compared, not the resolved ones: the combined line
    /// keeps only `kept`'s, so a line that declares its own reference and
    /// one that follows the project default differ even while they resolve
    /// alike (`Some(0.0)`: the values stay, the reference they follow changes).
    pub(crate) fn azimuth_rotation(&self, kept: &Line, other: &Line) -> Option<f64> {
        let same = match (kept.azimuth_reference, other.azimuth_reference) {
            (None, None) => true,
            (Some(x), Some(y)) => x.north == y.north && (x.correction - y.correction).abs() <= 1e-9,
            _ => false,
        };
        (!same).then(|| {
            self.azimuth_reference_for(other).correction
                - self.azimuth_reference_for(kept).correction
        })
    }

    /// How far apart the two join stations' manual offsets are, when more
    /// than the tolerance: the joined station keeps the fixed side's, so the
    /// other join station moves by this much.
    fn join_offset_change(&self, join_k: StationRef, join_o: StationRef) -> Option<f64> {
        let offset = |at| self.station_offset_for_ref(at).unwrap_or_default();
        let (k, o): ((f64, f64, f64), (f64, f64, f64)) = (offset(join_k), offset(join_o));
        let apart = ((k.0 - o.0).powi(2) + (k.1 - o.1).powi(2) + (k.2 - o.2).powi(2)).sqrt();
        (apart > COMBINE_TOLERANCE_M).then_some(apart)
    }

    /// The uuids shots of both `a` and `b` carry. The combine gives the other
    /// line's copies new ones: one uuid twice in a line would put the
    /// same shot in it twice, and every `AfterShot` anchor names the first.
    pub(crate) fn shared_shot_uuids(a: &Line, b: &Line) -> Vec<String> {
        let in_a: HashSet<&str> = a
            .shots
            .iter()
            .filter_map(|s| s.uuid.as_deref())
            .filter(|u| !u.is_empty())
            .collect();
        let mut shared: Vec<String> = b
            .shots
            .iter()
            .filter_map(|s| s.uuid.as_deref())
            .filter(|u| in_a.contains(u))
            .map(str::to_string)
            .collect();
        shared.sort_unstable();
        shared.dedup();
        shared
    }

    /// The join `pairing` names, with `kept` kept, as a [`JoinPoint`], and the
    /// translation that brings the other join station onto the kept one.
    pub(crate) fn join_point(
        &self,
        pairing: Pairing,
        touch: &Touch,
        kept: LineId,
        table: &EndTable,
    ) -> Option<(JoinPoint, (f64, f64))> {
        let line_a = self.get_line(pairing.a)?;
        let line_b = self.get_line(pairing.b)?;
        let at_a = StationRef::new(pairing.a, end_index(line_a, pairing.end_a));
        let at_b = StationRef::new(pairing.b, end_index(line_b, pairing.end_b));
        let (pa, pb) = (
            table.at(pairing.a, pairing.end_a)?,
            table.at(pairing.b, pairing.end_b)?,
        );
        let merged = touch.kind == CombineJoinKind::Merged;
        Some(if kept == pairing.a {
            (
                JoinPoint {
                    kept: pairing.a,
                    other: pairing.b,
                    join_k: at_a,
                    join_o: at_b,
                    group_k: touch.group_a.clone(),
                    group_o: touch.group_b.clone(),
                    merged,
                },
                (pa.0 - pb.0, pa.1 - pb.1),
            )
        } else {
            (
                JoinPoint {
                    kept: pairing.b,
                    other: pairing.a,
                    join_k: at_b,
                    join_o: at_a,
                    group_k: touch.group_b.clone(),
                    group_o: touch.group_a.clone(),
                    merged,
                },
                (pb.0 - pa.0, pb.1 - pa.1),
            )
        })
    }

    /// The plan the dialog shows for keeping `kept`, and the full plan
    /// `combine_lines` applies. An empty plan for a blocked pairing.
    pub(crate) fn plan_keeping(
        &self,
        pairing: Pairing,
        touch: &Touch,
        kept: LineId,
        table: &EndTable,
    ) -> (CombinePlan, Option<(JoinPoint, TiePlan)>) {
        let empty = CombinePlan {
            kept,
            moving_line: None,
            kept_moves_because: None,
            moved_lines: Vec::new(),
            shift_m: 0.0,
            torn_ties: Vec::new(),
            azimuth_rotation_deg: None,
            join_offset_change_m: None,
            closures_removed_across_one_shot: None,
            closures_replaced: None,
            moved_with_joined_station: None,
        };
        if touch.blocked.is_some() {
            return (empty, None);
        }
        let Some((join, gap)) = self.join_point(pairing, touch, kept, table) else {
            return (empty, None);
        };
        // The same gap as shown: each join station with its manual offset
        // (#755). The two differ only where the hand moves differ.
        let hand = |at| {
            value_naming(self, &self.station_offsets, at)
                .copied()
                .unwrap_or_default()
        };
        let (hand_k, hand_o): ((f64, f64, f64), (f64, f64, f64)) =
            (hand(join.join_k), hand(join.join_o));
        let shown = if (hand_k.0, hand_k.1) == (hand_o.0, hand_o.1) {
            gap
        } else {
            (gap.0 + hand_k.0 - hand_o.0, gap.1 + hand_k.1 - hand_o.1)
        };
        let rigid = table.ties.get_or_init(|| self.rigid_ties());
        let Ok(ties) = self.plan_ties(&join, (gap, shown), rigid, &table.closure_facts) else {
            return (empty, None);
        };
        let (Some(k), Some(o)) = (self.get_line(join.kept), self.get_line(join.other)) else {
            return (empty, None);
        };
        // A merged join's stations are already one point: offsets that
        // differ there show up as the shift, not as a moved join station.
        let offset_change = if join.merged {
            None
        } else {
            self.join_offset_change(join.join_k, join.join_o)
        };
        let shown = Self::plan_for_display(&ties, kept, self.azimuth_rotation(k, o), offset_change);
        (shown, Some((join, ties)))
    }

    /// The depth gap between two join stations that are NOT merged, each
    /// read on its own (`compute_station_depth`); `None` for a merged join,
    /// whose stations are already one depth.
    fn join_depth_gap(&self, at_a: StationRef, at_b: StationRef, merged: bool) -> Option<f64> {
        (!merged).then(|| {
            (self.compute_station_depth(&[at_a]) - self.compute_station_depth(&[at_b])).abs()
        })
    }

    /// The answer for one touching pairing, `a` the lower line id.
    fn candidate_for(
        &self,
        pairing: Pairing,
        touch: Touch,
        table: &EndTable,
    ) -> Option<CombineCandidate> {
        let (mut pairing, touch) = if pairing.a <= pairing.b {
            (pairing, touch)
        } else {
            (
                Pairing {
                    a: pairing.b,
                    end_a: pairing.end_b,
                    b: pairing.a,
                    end_b: pairing.end_a,
                },
                Touch {
                    group_a: touch.group_b,
                    group_b: touch.group_a,
                    ..touch
                },
            )
        };
        let a = self.get_line(pairing.a)?;
        let b = self.get_line(pairing.b)?;
        // A line with no shots has one station, both of its ends: name the
        // end that joins without reversing the other line.
        if a.shots.is_empty() {
            pairing.end_a = opposite(pairing.end_b);
        } else if b.shots.is_empty() {
            pairing.end_b = opposite(pairing.end_a);
        }
        let station_a = StationRef::new(a.id, end_index(a, pairing.end_a));
        let station_b = StationRef::new(b.id, end_index(b, pairing.end_b));
        let differing = |map: &HashMap<StationKey, String>| match (
            value_naming(self, map, station_a),
            value_naming(self, map, station_b),
        ) {
            (Some(x), Some(y)) if x != y => (Some(x.clone()), Some(y.clone())),
            _ => (None, None),
        };
        let (comment_a, comment_b) = differing(&self.station_comments);
        let (station_name_a, station_name_b) = differing(&self.station_names);
        let merged = touch.kind == CombineJoinKind::Merged;
        let (plan_keeping_a, _) = self.plan_keeping(pairing, &touch, a.id, table);
        let (plan_keeping_b, _) = self.plan_keeping(pairing, &touch, b.id, table);
        Some(CombineCandidate {
            line_a: a.id,
            line_b: b.id,
            end_a: pairing.end_a,
            end_b: pairing.end_b,
            station_a,
            station_b,
            merged,
            join_kind: touch.kind,
            reverses: pairing.end_a == pairing.end_b,
            blocked_reason: touch.blocked,
            blocked_keeping_a: Self::shot_id_room_refusal(a, b),
            blocked_keeping_b: Self::shot_id_room_refusal(b, a),
            shot_count_a: a.shots.len() as u32,
            shot_count_b: b.shots.len() as u32,
            comment_a,
            comment_b,
            station_name_a,
            station_name_b,
            depth_difference_m: self
                .join_depth_gap(station_a, station_b, merged)
                .filter(|gap| *gap > COMBINE_DEPTH_NOTICE_M),
            branches: touch.branches,
            loose_ends: touch.loose_ends,
            passing_through: touch.passing_through,
            repeated_shot_ids: Self::shared_shot_uuids(a, b).len() as u32,
            plan_keeping_a,
            plan_keeping_b,
        })
    }

    /// The ends of `line` a join can sit on: one for a line with no
    /// shots, whose single station is both.
    fn joinable_ends(line: &Line) -> &'static [LineEnd] {
        if line.shots.is_empty() {
            &[LineEnd::Start]
        } else {
            &[LineEnd::Start, LineEnd::End]
        }
    }

    /// Every pairing of an end of `s1` with an end of `s2` that touches.
    fn touching_pairings(&self, s1: &Line, s2: &Line, table: &EndTable) -> Vec<(Pairing, Touch)> {
        Self::joinable_ends(s1)
            .iter()
            .flat_map(|&end_a| {
                Self::joinable_ends(s2)
                    .iter()
                    .map(move |&end_b| (end_a, end_b))
            })
            .filter_map(|(end_a, end_b)| {
                let pairing = Pairing {
                    a: s1.id,
                    end_a,
                    b: s2.id,
                    end_b,
                };
                self.pairing_touch(pairing, table)
                    .map(|touch| (pairing, touch))
            })
            .collect()
    }

    /// Can lines `s1` and `s2` be combined, and where? Tries all four end
    /// pairings; when they meet at more than one, the join is a merge first,
    /// then co-located ends, then a closure, then the smaller gap, then one
    /// that needs no reverse. The ties at the other meeting points tear into
    /// loop closures (the plan lists them); the station menu joins at the end
    /// that was clicked instead. `None` when either line is missing or
    /// they are the same line.
    pub fn combine_candidate_for_lines(&self, s1: LineId, s2: LineId) -> Option<CombineCandidate> {
        self.candidate_for_lines_with(s1, s2, &self.end_table())
    }

    /// [`Self::combine_candidate_for_lines`] against an end table the
    /// caller already built.
    fn candidate_for_lines_with(
        &self,
        s1: LineId,
        s2: LineId,
        table: &EndTable,
    ) -> Option<CombineCandidate> {
        if s1 == s2 {
            return None;
        }
        // Lower id first, so the answer (and every reason's wording) is the
        // same whichever line the caller names first or keeps.
        let (s1, s2) = (s1.min(s2), s1.max(s2));
        let (a, b) = (self.get_line(s1)?, self.get_line(s2)?);
        let gap = |p: &Pairing| match (table.at(p.a, p.end_a), table.at(p.b, p.end_b)) {
            (Some(x), Some(y)) => distance(x, y),
            _ => f64::INFINITY,
        };
        let best = self
            .touching_pairings(a, b, table)
            .into_iter()
            .min_by(|(p1, t1), (p2, t2)| {
                t1.rank()
                    .cmp(&t2.rank())
                    .then_with(|| gap(p1).total_cmp(&gap(p2)))
                    .then_with(|| (p1.end_a == p1.end_b).cmp(&(p2.end_a == p2.end_b)))
            });
        let (pairing, touch) = best.unwrap_or_else(|| {
            (
                Pairing {
                    a: s1,
                    end_a: LineEnd::End,
                    b: s2,
                    end_b: LineEnd::Start,
                },
                Touch::refused(format!(
                    "{} and {} do not meet at an end station: merge or close a loop between \
                     two of their ends first",
                    line_label(a),
                    line_label(b)
                )),
            )
        });
        self.candidate_for(pairing, touch, table)
    }

    /// The first of [`Self::combine_candidates`]: what a station with one
    /// partner offers.
    pub fn combine_candidate(&self, station: StationRef) -> Option<CombineCandidate> {
        self.combine_candidates(station).into_iter().next()
    }

    /// Every other line end tied to `group` by a loop closure: the ends in
    /// the merge group of each closure's far station.
    fn closure_partner_ends(&self, group: &HashSet<StationRef>) -> Vec<(LineId, LineEnd)> {
        let mut out = Vec::new();
        for closure in &self.loop_closures {
            let (Some(x), Some(y)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) else {
                continue;
            };
            let far = match (group.contains(&x), group.contains(&y)) {
                (true, false) => y,
                (false, true) => x,
                _ => continue,
            };
            for member in self.get_merge_group(far) {
                if let Some(line) = self.get_line(member.line_id) {
                    out.extend(Self::end_of(line, member.station_index));
                }
            }
        }
        out
    }

    /// The end `index` is of `line`, if it is one (`Start` for a line
    /// with no shots, whose one station is both).
    fn end_of(line: &Line, index: u32) -> Option<(LineId, LineEnd)> {
        if index == 0 {
            Some((line.id, LineEnd::Start))
        } else if index as usize == line.shots.len() {
            Some((line.id, LineEnd::End))
        } else {
            None
        }
    }

    /// What the line that `station` ends can be combined with: one
    /// candidate per other line with an end at the same point (merged to
    /// it, co-located with it, or tied to it by a loop closure), in line
    /// order, each joined at `station` and with its own reason when it is
    /// blocked. At a junction the user picks one; the others stay attached
    /// to the joined station as branches (or, unmerged, untouched). Empty
    /// when `station` is not an end of its line or nothing else ends there.
    ///
    /// A station merged only to the middle of other lines gets one blocked
    /// candidate saying so; a pile of more than [`MAX_COMBINE_PARTNERS`] other
    /// ends gets one blocked candidate pointing at the Sections panel.
    pub fn combine_candidates(&self, station: StationRef) -> Vec<CombineCandidate> {
        self.station_candidates(station, false)
    }

    /// Every pair of lines that can be combined at the point `station`
    /// ends at, for the station menu: first [`Self::combine_candidates`] (the
    /// clicked line with each partner), then, at a junction, every pair of
    /// the other lines ending there, each joined at this point. A pair
    /// that meets here only through the clicked station (two ends that are
    /// neither merged, nor side by side, nor tied by a closure) is left out.
    pub fn combine_candidates_at_point(&self, station: StationRef) -> Vec<CombineCandidate> {
        self.station_candidates(station, true)
    }

    fn station_candidates(&self, station: StationRef, all_pairs: bool) -> Vec<CombineCandidate> {
        let Some(line) = self.get_line(station.line_id) else {
            return Vec::new();
        };
        let Some((_, end)) = Self::end_of(line, station.station_index) else {
            return Vec::new();
        };
        // Every other line with an end at this point: merged to it,
        // co-located with it, or across a loop closure from it.
        let table = self.end_table();
        let Some(at) = table.at(line.id, end) else {
            return Vec::new();
        };
        let group = self.get_merge_group(station);
        let mut other_ends: Vec<(LineId, LineEnd)> = table
            .near(at)
            .filter_map(|(id, end)| {
                let other = self.get_line(id)?;
                Some((
                    id,
                    if other.shots.is_empty() {
                        LineEnd::Start
                    } else {
                        end
                    },
                ))
            })
            .collect();
        for member in &group {
            if let Some(other) = self.get_line(member.line_id) {
                other_ends.extend(Self::end_of(other, member.station_index));
            }
        }
        // The ends AT this point (merged or side by side), as opposed to the
        // ones across a loop closure from it.
        let here: HashSet<(LineId, LineEnd)> = other_ends.iter().copied().collect();
        other_ends.extend(self.closure_partner_ends(&group));
        other_ends.retain(|(id, _)| *id != line.id);
        other_ends.sort_by_key(|(id, end)| (*id, *end == LineEnd::End));
        other_ends.dedup();
        let pairing_with = |(id, other_end): (LineId, LineEnd)| Pairing {
            a: line.id,
            end_a: end,
            b: id,
            end_b: other_end,
        };
        if other_ends.len() > MAX_COMBINE_PARTNERS {
            let count = other_ends.len() + 1;
            let Some((pairing, mut touch)) = other_ends.into_iter().find_map(|other| {
                let pairing = pairing_with(other);
                self.pairing_touch(pairing, &table)
                    .map(|touch| (pairing, touch))
            }) else {
                return Vec::new();
            };
            touch.blocked = Some(format!(
                "{count} line ends meet here: select the two to combine in the Sections \
                 panel"
            ));
            return self
                .candidate_for(pairing, touch, &table)
                .into_iter()
                .collect();
        }
        if other_ends.is_empty() {
            if group.len() <= 1 {
                return Vec::new();
            }
            // Merged only to stations that are not line ends: a line
            // passes through here, and there is no other end to join.
            let Some(other) = group.iter().find(|s| **s != station) else {
                return Vec::new();
            };
            let pairing = Pairing {
                a: line.id,
                end_a: end,
                b: other.line_id,
                end_b: LineEnd::Start,
            };
            let mut touch = Touch::refused("Merged to the middle of another line".to_string());
            touch.kind = CombineJoinKind::Merged;
            return self
                .candidate_for(pairing, touch, &table)
                .into_iter()
                .collect();
        }
        // Every touching pairing, the clicked line's first; then, for the
        // station menu, every pairing of two others ending here.
        let mut touching: Vec<(Pairing, Touch)> = other_ends
            .iter()
            .filter_map(|other| {
                let pairing = pairing_with(*other);
                Some((pairing, self.pairing_touch(pairing, &table)?))
            })
            .collect();
        if all_pairs {
            for (i, x) in other_ends.iter().enumerate() {
                for y in &other_ends[i + 1..] {
                    // Two ends both across a closure meet over there, not here.
                    if x.0 == y.0 || !(here.contains(x) || here.contains(y)) {
                        continue;
                    }
                    let pairing = Pairing {
                        a: x.0,
                        end_a: x.1,
                        b: y.0,
                        end_b: y.1,
                    };
                    if let Some(touch) = self.pairing_touch(pairing, &table) {
                        touching.push((pairing, touch));
                    }
                }
            }
        }
        // One candidate per pair of lines: a line can end at this point
        // with both ends (one merged or side by side, the other across a
        // closure), and the menu offers the pair once, by its best join at
        // this point, ranked as the two-line query ranks the joins it
        // compares (merged, then side by side, then across a closure; then
        // the smaller gap; then no reverse).
        let gap = |p: &Pairing| match (table.at(p.a, p.end_a), table.at(p.b, p.end_b)) {
            (Some(x), Some(y)) => distance(x, y),
            _ => f64::INFINITY,
        };
        let mut best: Vec<(Pairing, Touch)> = Vec::new();
        for (pairing, touch) in touching {
            let key = (pairing.a.min(pairing.b), pairing.a.max(pairing.b));
            let better = |(p, t): &(Pairing, Touch)| {
                touch
                    .rank()
                    .cmp(&t.rank())
                    .then_with(|| gap(&pairing).total_cmp(&gap(p)))
                    .then_with(|| (pairing.end_a == pairing.end_b).cmp(&(p.end_a == p.end_b)))
                    .is_lt()
            };
            match best
                .iter_mut()
                .find(|(p, _)| (p.a.min(p.b), p.a.max(p.b)) == key)
            {
                Some(slot) => {
                    if better(slot) {
                        *slot = (pairing, touch);
                    }
                }
                None => best.push((pairing, touch)),
            }
        }
        best.into_iter()
            .filter_map(|(pairing, touch)| self.candidate_for(pairing, touch, &table))
            .collect()
    }
}

/// The other end.
fn opposite(end: LineEnd) -> LineEnd {
    match end {
        LineEnd::Start => LineEnd::End,
        LineEnd::End => LineEnd::Start,
    }
}
