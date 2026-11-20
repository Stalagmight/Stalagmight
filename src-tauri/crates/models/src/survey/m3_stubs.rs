//! STUBS standing in for stream M3's derived views until M3 merges.
//!
//! Stream M2 (topology, closures, datums, move tools, Add) needs a handful of
//! derived values that plan section 12 assigns to M3: closure metrics (3.6),
//! the across-one-shot knot rule (#737, plan 2 item 7), datum roles, GPS gaps
//! and entrance defaults (3.8). They live here, small and self-contained, so
//! the integrator can delete this file and point the callers at M3's versions:
//!
//! - [`closure_metrics`] -> M3's closure metrics;
//! - [`closure_spans_one_shot`] -> M3's skip set / creation check;
//! - [`datum_roles`], [`datum_gap`], [`datum_is_entrance`] -> M3's roles,
//!   gaps and entrances.
//!
//! Each follows today's rule as plan 3.6 / 3.8 restates it on the station
//! graph; none is tuned for speed.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::{
    datum_gap_threshold_m, gps_to_local, ClosurePreview, DatumGap, DatumRole, GpsCoordinates,
    LineId, StationId,
};

use super::{ClosureStrategy, Project, RawPositions, Topology};

/// M3 STUB. The closure figures between stations `a` and `b` (plan 3.6):
/// misclosures from raw positions, `path_length` the shortest path along
/// shots (a junction is one station, closures are not traversable; `0.0` when
/// the ends are in different pieces), `error_percentage` of it. `None` when a
/// station has no position.
pub(super) fn closure_metrics(
    project: &Project,
    a: StationId,
    b: StationId,
) -> Option<ClosurePreview> {
    let positions = project.raw_positions();
    let ([ax, ay], [bx, by]) = (positions.xy(a)?, positions.xy(b)?);
    let (az, bz) = (positions.z(a)?, positions.z(b)?);
    let horizontal_misclosure = (ax - bx).hypot(ay - by);
    let vertical_misclosure = (az - bz).abs();
    let misclosure_distance = horizontal_misclosure.hypot(az - bz);
    let topology = project.topology();
    let path_length = match (topology.dense(a), topology.dense(b)) {
        (Some(da), Some(db)) => {
            let distance = shortest_paths(project, topology, &[da]);
            if distance[db].is_finite() {
                distance[db]
            } else {
                0.0
            }
        }
        _ => 0.0,
    };
    let error_percentage = if path_length > 0.0 {
        misclosure_distance / path_length * 100.0
    } else {
        0.0
    };
    Some(ClosurePreview {
        path_length,
        misclosure_distance,
        error_percentage,
        horizontal_misclosure,
        vertical_misclosure,
    })
}

/// M3 STUB. Whether a NEW closure between `a` and `b` would tie the two ends
/// of one shot together (#737), and which shot `(line, shot index)` (the
/// lowest): today's knot rule on the station graph. Knots start as single
/// stations (a junction is already one); every closure the solve ties
/// (`Batch`, `LeastSquaresWeighted`, `Tie`, an aligned snap) joins two knots
/// in list order unless that would put both ends of a shot in one knot, in
/// which case it is skipped. `None` for a closure that closes a real loop or
/// whose ends are already one knot.
pub(super) fn closure_spans_one_shot(
    project: &Project,
    a: StationId,
    b: StationId,
) -> Option<(LineId, usize)> {
    let topology = project.topology();
    let positions = project.raw_positions();
    let mut knots = Knots::new(project, topology);
    for closure in &project.closures {
        if !ties_in_solve(project, &positions, closure) {
            continue;
        }
        let (Some(ca), Some(cb)) = (topology.dense(closure.a), topology.dense(closure.b)) else {
            continue;
        };
        if knots.collapsed_shot(ca, cb).is_none() {
            knots.join(ca, cb);
        }
    }
    knots.collapsed_shot(topology.dense(a)?, topology.dense(b)?)
}

fn ties_in_solve(
    project: &Project,
    positions: &RawPositions,
    closure: &super::LoopClosure,
) -> bool {
    match closure.strategy {
        ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted | ClosureStrategy::Tie => {
            true
        }
        ClosureStrategy::Move => project.closure_is_aligned(positions, closure),
        ClosureStrategy::ProportionalByLength | ClosureStrategy::EqualDistribution => false,
    }
}

/// Union-find over stations with each knot's members, and every station's
/// shot neighbours with the shot between them.
struct Knots {
    parent: Vec<usize>,
    members: Vec<Vec<usize>>,
    neighbours: Vec<Vec<(usize, (LineId, usize))>>,
}

impl Knots {
    fn new(project: &Project, topology: &Topology) -> Self {
        let n = topology.stations().len();
        let mut neighbours: Vec<Vec<(usize, (LineId, usize))>> = vec![Vec::new(); n];
        for (position, line) in project.lines().iter().enumerate() {
            let chain = topology.chain_at(position).unwrap_or(&[]);
            for (index, pair) in chain.windows(2).enumerate() {
                if let (Some(from), Some(to)) = (topology.dense(pair[0]), topology.dense(pair[1])) {
                    neighbours[from].push((to, (line.id, index)));
                    neighbours[to].push((from, (line.id, index)));
                }
            }
        }
        Self {
            parent: (0..n).collect(),
            members: (0..n).map(|d| vec![d]).collect(),
            neighbours,
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    /// The lowest shot with one end in each of the two knots, if they differ.
    fn collapsed_shot(&mut self, a: usize, b: usize) -> Option<(LineId, usize)> {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return None;
        }
        let (small, other) = if self.members[ra].len() <= self.members[rb].len() {
            (ra, rb)
        } else {
            (rb, ra)
        };
        let mut best: Option<(LineId, usize)> = None;
        for member in self.members[small].clone() {
            for (neighbour, shot) in self.neighbours[member].clone() {
                if self.find(neighbour) == other {
                    best = Some(best.map_or(shot, |b| b.min(shot)));
                }
            }
        }
        best
    }

    fn join(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (keep, gone) = if self.members[ra].len() >= self.members[rb].len() {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[gone] = keep;
        let moved = std::mem::take(&mut self.members[gone]);
        self.members[keep].extend(moved);
    }
}

/// M3 STUB. Every datum's role, parallel to `project.datums`: the first datum
/// of a piece (list order) places it, later ones are GPS fixes; `None` for a
/// datum whose station is on no line.
pub(super) fn datum_roles(project: &Project) -> Vec<Option<DatumRole>> {
    let topology = project.topology();
    let placement = project.placement_datums();
    project
        .datums
        .iter()
        .enumerate()
        .map(|(index, datum)| {
            let c = topology.component_of(datum.station)?;
            Some(if placement[c] == Some(index) {
                DatumRole::Placement
            } else {
                DatumRole::GpsFix
            })
        })
        .collect()
}

/// M3 STUB. The gap between `gps` and where the raw survey puts `station`, in
/// the current frame, with today's threshold (15 m + 2% of the survey distance
/// along shots from the piece's placement datum). `None` without a frame or
/// for a station on no line.
pub(super) fn datum_gap(
    project: &Project,
    station: StationId,
    gps: &GpsCoordinates,
) -> Option<DatumGap> {
    let frame = project.datums.get(project.frame_datum()?)?.gps;
    let [x, y] = project.raw_positions().xy(station)?;
    let topology = project.topology();
    let target = topology.dense(station)?;
    let component = topology.component_of(station)?;
    let survey_distance_m = project.placement_datums()[component]
        .and_then(|datum| topology.dense(project.datums[datum].station))
        .and_then(|root| {
            let distance = shortest_paths(project, topology, &[root])[target];
            distance.is_finite().then_some(distance)
        });
    let (gx, gy) = gps_to_local(gps, &frame);
    let (dx, dy) = (gx - x, gy - y);
    let gap_m = dx.hypot(dy);
    let threshold_m = datum_gap_threshold_m(survey_distance_m.unwrap_or(f64::INFINITY));
    Some(DatumGap {
        gap_m,
        bearing_deg: dx.atan2(dy).to_degrees().rem_euclid(360.0),
        survey_distance_m,
        threshold_m,
        exceeds_threshold: gap_m > threshold_m,
    })
}

/// M3 STUB. Whether a datum with `role` and `entrance` (its override) on
/// `station`, set via `via`, is an entrance (plan 3.8): a placement datum
/// always; a GPS fix by its override, else when the station is the first or
/// last station of its `via` line (of the station's first line when `via` is
/// unset).
pub(super) fn datum_is_entrance(
    project: &Project,
    station: StationId,
    via: Option<LineId>,
    role: DatumRole,
    entrance: Option<bool>,
) -> bool {
    match role {
        DatumRole::Placement => true,
        DatumRole::GpsFix => entrance.unwrap_or_else(|| {
            let topology = project.topology();
            let line = via.or_else(|| topology.first_slot(station).map(|slot| slot.line_id));
            line.and_then(|line| topology.chain(line))
                .is_some_and(|chain| {
                    chain.first() == Some(&station) || chain.last() == Some(&station)
                })
        }),
    }
}

/// Shortest distance along shots (slope length; a non-finite length is never
/// crossed) from `roots` to every station, indexed by dense station.
fn shortest_paths(project: &Project, topology: &Topology, roots: &[usize]) -> Vec<f64> {
    let n = topology.stations().len();
    let mut adjacency: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for (position, line) in project.lines().iter().enumerate() {
        let chain = topology.chain_at(position).unwrap_or(&[]);
        for (shot, pair) in line.shots.iter().zip(chain.windows(2)) {
            if let (Some(from), Some(to)) = (topology.dense(pair[0]), topology.dense(pair[1])) {
                if shot.length.is_finite() {
                    let w = shot.length.max(0.0);
                    adjacency[from].push((to, w));
                    adjacency[to].push((from, w));
                }
            }
        }
    }
    let mut distance = vec![f64::INFINITY; n];
    let mut heap = BinaryHeap::new();
    for &root in roots {
        if root < n {
            distance[root] = 0.0;
            heap.push(Item {
                cost: 0.0,
                node: root,
            });
        }
    }
    while let Some(Item { cost, node }) = heap.pop() {
        if cost > distance[node] {
            continue;
        }
        for &(next, w) in &adjacency[node] {
            let candidate = cost + w;
            if candidate < distance[next] {
                distance[next] = candidate;
                heap.push(Item {
                    cost: candidate,
                    node: next,
                });
            }
        }
    }
    distance
}

/// A min-heap entry (reversed order on cost, then node).
#[derive(Debug, Clone, Copy)]
struct Item {
    cost: f64,
    node: usize,
}

impl PartialEq for Item {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Item {}

impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Item {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.node.cmp(&self.node))
    }
}
