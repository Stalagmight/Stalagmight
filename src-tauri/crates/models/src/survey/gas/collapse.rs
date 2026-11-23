//! One station, one entry across a loop closure (RS 6, plan 2026-11-23).
//!
//! A loop closure that archives one end shows both of its ends as one station
//! (the kept end's identity), yet they stay two graph nodes joined by a
//! zero-cost tie. A route that crosses such a tie would list that station
//! twice in a row, with no metres between the two entries, as a merge hop did
//! before the station graph (D18). So Plan mode's lists collapse every run of
//! consecutive route nodes joined by closure ties whose nodes show one
//! identity; the turn frontier lists each shown station once; and a drop
//! pinned on another node of its default station is the same place.
//!
//! The collapse is presentation only. The engine walks, prices, measures and
//! places drops on the node route first; every number it reports stays the
//! node route's (a tie costs no gas, time or distance, so a run's nodes carry
//! the same cumulative readouts). Only the lists and the indices into them
//! change. A project whose closures archive nothing takes none of this.

use crate::StationId;

use super::profile::GasProfilePoint;
use super::GasReach;

/// The runs of a node route: `runs[r] = (first, last)` node indices, each a
/// maximal stretch whose every step is a tie between nodes of one shown
/// identity; `of[i]` is the run of node `i`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Runs {
    runs: Vec<(usize, usize)>,
    of: Vec<usize>,
}

impl Runs {
    /// The runs of a route of `len` nodes, where `one_station(i)` says that
    /// the step from node `i` to node `i + 1` is a closure tie between two
    /// nodes that show one identity.
    pub(super) fn of(len: usize, one_station: impl Fn(usize) -> bool) -> Self {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut of = Vec::with_capacity(len);
        for i in 0..len {
            match runs.last_mut() {
                Some(run) if i > 0 && one_station(i - 1) => run.1 = i,
                _ => runs.push((i, i)),
            }
            of.push(runs.len() - 1);
        }
        Self { runs, of }
    }

    /// Whether no run holds more than one node (nothing to collapse).
    pub(super) fn is_trivial(&self) -> bool {
        self.runs.len() == self.of.len()
    }

    /// The node indices of the run holding node `i`.
    pub(super) fn run_holding(&self, i: usize) -> Option<(usize, usize)> {
        self.of.get(i).and_then(|&r| self.runs.get(r)).copied()
    }

    /// The collapsed index of node index `i`.
    fn collapsed(&self, i: usize) -> usize {
        self.of.get(i).copied().unwrap_or(i)
    }
}

/// The station a run stands for: its first node (where the route arrives),
/// except the last run, which stands for its last node, the route target, so
/// the route still ends on `final_turn`.
fn representative(route: &[StationId], run: (usize, usize), last_run: bool) -> StationId {
    route[if last_run { run.1 } else { run.0 }]
}

/// The turn frontier with each shown station once (D18 under RS 6): of the
/// frontier nodes that show one identity, the node holding it when it is one
/// of them, else the lowest Station ID. Ascending, like the input.
pub(super) fn one_per_shown(
    frontier: &[StationId],
    shown: impl Fn(StationId) -> StationId,
) -> Vec<StationId> {
    let mut chosen: std::collections::BTreeMap<StationId, StationId> =
        std::collections::BTreeMap::new();
    for &node in frontier {
        let identity = shown(node);
        chosen
            .entry(identity)
            .and_modify(|kept| {
                if node == identity || (*kept != identity && node < *kept) {
                    *kept = node;
                }
            })
            .or_insert(node);
    }
    let mut out: Vec<StationId> = chosen.into_values().collect();
    out.sort_unstable();
    out
}

impl GasReach {
    /// Collapse the node route's `runs` (RS 6) in place: one route entry per
    /// run, the tie legs inside a run dropped, and every route index (drops,
    /// way points, the profile) moved to the collapsed route. A profile point
    /// per run keeps the run's deepest depth (both nodes are surveyed points
    /// the diver passes; the deeper is the conservative reading, as
    /// `route_max_depth_m` already takes it), and the turnaround stays one
    /// point.
    pub(super) fn collapse_route(&mut self, runs: &Runs) {
        if runs.is_trivial() {
            return;
        }
        let node_route = std::mem::take(&mut self.route);
        let count = runs.runs.len();
        let mut route = Vec::with_capacity(count);
        let mut route_ends = Vec::with_capacity(count);
        let mut route_lines = Vec::with_capacity(count);
        for (r, &run) in runs.runs.iter().enumerate() {
            let last_run = r + 1 == count;
            let rep = representative(&node_route, run, last_run);
            route.push(rep);
            route_ends.push([node_route[run.0], node_route[run.1]]);
            // The line naming the entry: a leg that touches its station (the
            // arriving one at the run's first node, the departing one at its
            // last), else what the node route said for that node.
            let at = if last_run { run.1 } else { run.0 };
            route_lines.push(self.route_lines.get(at).copied().flatten());
        }
        // The legs between runs: the node leg from each run's last node.
        let legs: Vec<usize> = runs.runs.iter().take(count - 1).map(|run| run.1).collect();
        self.route_shots = legs
            .iter()
            .map(|&i| self.route_shots.get(i).copied().flatten())
            .collect();
        self.route_leg_kinds = legs
            .iter()
            .map(|&i| self.route_leg_kinds.get(i).copied().flatten())
            .collect();
        for j in &mut self.stage_drop_route_indices {
            *j = runs.collapsed(*j);
        }
        for w in self.waypoint_route_indices.iter_mut().flatten() {
            *w = runs.collapsed(*w);
        }
        self.route_profile = collapse_profile(&self.route_profile, runs, &route);
        self.route = route;
        self.route_ends = route_ends;
        self.route_lines = route_lines;
    }
}

/// The profile on the collapsed route: consecutive points of one run merge
/// into the first (a tie adds no distance, time or gas), keeping the run's
/// deepest depth; the return leg's points on the turnaround's run go, since
/// the turnaround is emitted once.
fn collapse_profile(
    points: &[GasProfilePoint],
    runs: &Runs,
    route: &[StationId],
) -> Vec<GasProfilePoint> {
    let turn_run = runs.runs.len().saturating_sub(1);
    let mut out: Vec<GasProfilePoint> = Vec::with_capacity(points.len());
    for p in points {
        let run = runs.collapsed(p.route_index);
        if !p.outbound && run == turn_run {
            if let Some(turn) = out
                .iter_mut()
                .rev()
                .find(|q| q.outbound && q.route_index == run)
            {
                turn.depth_m = turn.depth_m.max(p.depth_m);
            }
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.route_index == run && last.outbound == p.outbound {
                last.depth_m = last.depth_m.max(p.depth_m);
                continue;
            }
        }
        out.push(GasProfilePoint {
            route_index: run,
            station: route.get(run).copied().unwrap_or(p.station),
            ..*p
        });
    }
    // A return point of a run takes the depth its outbound point settled on.
    let depth_of: Vec<Option<f64>> = (0..runs.runs.len())
        .map(|r| {
            out.iter()
                .find(|q| q.outbound && q.route_index == r)
                .map(|q| q.depth_m)
        })
        .collect();
    for q in out.iter_mut().filter(|q| !q.outbound) {
        if let Some(Some(depth)) = depth_of.get(q.route_index) {
            q.depth_m = *depth;
        }
    }
    out
}
