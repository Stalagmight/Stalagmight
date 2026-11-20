//! The survey graph (plan 3.8): one node per station (a junction is one node),
//! one weighted edge per shot, and one zero-cost edge per loop closure, aligned
//! or not (a closed loop ties two stations to one physical point, as today).
//!
//! Ported from `models/src/project/pathfinding.rs` (the graph builder and the
//! Dijkstra with riders, copied verbatim apart from the node space) and
//! `models/src/project/distance.rs` (distances, penetration, the distance
//! field and the along-line ruler). Merge edges are gone: merged slots are one
//! node. Everything is indexed by the dense station index of
//! [`super::Topology::stations`], so a result is per station; the `slots`
//! helpers expand it to the wire's per-slot lists.
//!
//! Determinism: shots are emitted with lines in id order and closures in id
//! order, so reordering the line or closure list changes no tie-break and no
//! bit.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};

use crate::{LineId, PathShot, RulerEndpoint, StationId, StationRef};

use super::{Line, Project, RawPositions, Topology};

/// Horizontal and vertical position of a station, for the coordinate-weighted
/// graph consumers (distance field, ruler, gas). Implemented by the raw and the
/// closure-adjusted positions.
pub trait StationPositions {
    /// The station's horizontal position, local metres.
    fn station_xy(&self, station: StationId) -> Option<[f64; 2]>;
    /// The station's depth, without any line's hand-move z.
    fn station_z(&self, station: StationId) -> Option<f64>;
}

impl StationPositions for RawPositions {
    fn station_xy(&self, station: StationId) -> Option<[f64; 2]> {
        self.xy(station)
    }

    fn station_z(&self, station: StationId) -> Option<f64> {
        self.z(station)
    }
}

/// Positions per dense station (`[x, y, z]`, `z` without hand moves); a
/// station without a position reads as the origin.
pub(crate) fn dense_coords(
    topology: &Topology,
    positions: &impl StationPositions,
) -> Vec<[f64; 3]> {
    topology
        .stations()
        .iter()
        .map(|&s| {
            let [x, y] = positions.station_xy(s).unwrap_or([0.0, 0.0]);
            [x, y, positions.station_z(s).unwrap_or(0.0)]
        })
        .collect()
}

/// The position of `station` as shown on `line`: its coordinates plus the
/// line's hand-move z (slot z, plan 3.5).
pub(crate) fn slot_coords(
    coords: &[[f64; 3]],
    dense: usize,
    line: &Line,
    station: StationId,
) -> [f64; 3] {
    let [x, y, z] = coords.get(dense).copied().unwrap_or([0.0, 0.0, 0.0]);
    match line.hand_moves.get(&station) {
        Some(hand) => [x, y, z + hand[2]],
        None => [x, y, z],
    }
}

/// One shot as the graph builder hands it to a weight function.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShotEdge<'a> {
    pub(crate) line: &'a Line,
    /// Shot index on the line (the shot from chain station `index` to `index + 1`).
    pub(crate) index: usize,
    /// Dense index of the shot's start station.
    pub(crate) from: usize,
    /// Dense index of the shot's end station.
    pub(crate) to: usize,
    /// The two stations.
    pub(crate) from_station: StationId,
    pub(crate) to_station: StationId,
}

/// Distances per station from some roots; `INFINITY` for an unreachable one.
#[derive(Debug, Clone, PartialEq)]
pub struct StationDistances {
    stations: Vec<StationId>,
    distance: Vec<f64>,
}

impl StationDistances {
    pub(crate) fn new(topology: &Topology, distance: Vec<f64>) -> Self {
        Self {
            stations: topology.stations().to_vec(),
            distance,
        }
    }

    /// The distance to `station`; `None` when unreachable or unknown.
    pub fn get(&self, station: StationId) -> Option<f64> {
        let d = *self
            .distance
            .get(self.stations.binary_search(&station).ok()?)?;
        d.is_finite().then_some(d)
    }

    /// Every station with its distance (`INFINITY` when unreachable),
    /// ascending by id.
    pub fn iter(&self) -> impl Iterator<Item = (StationId, f64)> + '_ {
        self.stations
            .iter()
            .copied()
            .zip(self.distance.iter().copied())
    }

    /// The largest finite distance, `0.0` when nothing is reachable.
    pub fn max_reachable(&self) -> f64 {
        self.distance
            .iter()
            .filter(|d| d.is_finite())
            .fold(0.0_f64, |a, &d| a.max(d))
    }

    /// Per dense station (raw vector).
    pub(crate) fn dense(&self) -> &[f64] {
        &self.distance
    }

    /// The wire's per-slot field (today's `DistanceField`): one entry per slot
    /// in slot order, `None` when unreachable.
    pub fn to_distance_field(&self, project: &Project) -> crate::DistanceField {
        let mut distances = Vec::new();
        let mut station_refs = Vec::new();
        for line in project.lines() {
            for (index, station) in line.stations().enumerate() {
                station_refs.push(StationRef::new(line.id, index as u32));
                distances.push(self.get(station));
            }
        }
        crate::DistanceField {
            distances,
            station_refs,
            max_reachable: self.max_reachable(),
        }
    }
}

/// One end of a ruler measurement on the station graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RulerPoint {
    /// A station.
    Station(StationId),
    /// The point at fraction `t` (0 = start, 1 = end) along shot `index` of
    /// line `line`.
    OnShot { line: LineId, index: usize, t: f64 },
}

impl RulerPoint {
    /// Resolve the wire's ruler endpoint (a slot, or a session shot id and a
    /// fraction). `None` when it names nothing.
    pub fn from_endpoint(project: &Project, endpoint: &RulerEndpoint) -> Option<Self> {
        if let (Some(shot_id), Some(t)) = (endpoint.shot_id, endpoint.t) {
            let line = project.line(endpoint.line_id)?;
            let index = line.shots.iter().position(|shot| shot.id == shot_id)?;
            return Some(Self::OnShot {
                line: endpoint.line_id,
                index,
                t,
            });
        }
        let index = endpoint.station_index?;
        project
            .station_at(StationRef::new(endpoint.line_id, index))
            .map(Self::Station)
    }
}

/// Result of an along-line ruler query on the station graph (today's
/// `AlongLineResult`, with stations by id).
#[derive(Debug, Clone, PartialEq)]
pub struct AlongLine {
    /// The along-line distance, `None` when there is no path.
    pub along_line: Option<f64>,
    /// The shots traversed, whole, in order; closure ties are skipped.
    pub path_shots: Vec<PathShot>,
    /// The stations on the path, in order (consecutive repeats removed).
    pub path_stations: Vec<StationId>,
    /// The slots on the path, for the wire: the two ends of each traversed
    /// shot on that shot's line, in order, consecutive repeats removed (the
    /// first slot of each station when no shot is traversed).
    pub path_slots: Vec<StationRef>,
}

impl AlongLine {
    fn none() -> Self {
        Self {
            along_line: None,
            path_shots: Vec::new(),
            path_stations: Vec::new(),
            path_slots: Vec::new(),
        }
    }

    /// The wire result.
    pub fn to_wire(&self) -> crate::AlongLineResult {
        crate::AlongLineResult {
            along_line: self.along_line,
            path_shots: self.path_shots.clone(),
            path_stations: self.path_slots.clone(),
        }
    }
}

impl Project {
    /// The line list positions in line-id order (ties by list position), the
    /// order every graph builder emits shots in.
    pub(crate) fn line_positions_by_id(&self) -> Vec<usize> {
        let mut positions: Vec<usize> = (0..self.lines().len()).collect();
        positions.sort_by_key(|&p| (self.lines()[p].id, p));
        positions
    }

    /// Visit every shot, lines in id order, shots in chain order.
    pub(crate) fn for_each_shot_edge(&self, mut visit: impl FnMut(ShotEdge<'_>)) {
        let topology = self.topology();
        for position in self.line_positions_by_id() {
            let line = &self.lines()[position];
            let Some(chain) = topology.chain_at(position) else {
                continue;
            };
            for index in 0..line.shots.len() {
                let (Some(&a), Some(&b)) = (chain.get(index), chain.get(index + 1)) else {
                    continue;
                };
                let (Some(from), Some(to)) = (topology.dense(a), topology.dense(b)) else {
                    continue;
                };
                visit(ShotEdge {
                    line,
                    index,
                    from,
                    to,
                    from_station: a,
                    to_station: b,
                });
            }
        }
    }

    /// The closures as dense end pairs, in closure-id order (ties by list
    /// position), self-ties left out.
    pub(crate) fn closure_edges_by_id(&self) -> Vec<(usize, usize)> {
        let topology = self.topology();
        let mut order: Vec<usize> = (0..self.closures.len()).collect();
        order.sort_by_key(|&i| (self.closures[i].id, i));
        order
            .into_iter()
            .filter_map(|i| {
                let c = &self.closures[i];
                let (a, b) = (topology.dense(c.a)?, topology.dense(c.b)?);
                (a != b).then_some((a, b))
            })
            .collect()
    }

    /// The survey graph with each shot weighted by `shot_weight` and every
    /// closure at zero cost (today's `build_survey_graph_weighted`). The edge
    /// emission order is fixed (shots by line id then index, then closures by
    /// id), so two graphs built with different weights are aligned edge for
    /// edge, which [`dijkstra_scaled_with_riders`] relies on. An unusable leg
    /// gets an infinite weight rather than being left out, so the edge set is
    /// the same for every weighting.
    pub(crate) fn survey_graph_weighted(
        &self,
        mut shot_weight: impl FnMut(ShotEdge<'_>) -> f64,
    ) -> Vec<Vec<(usize, f64)>> {
        let n = self.topology().stations().len();
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        self.for_each_shot_edge(|edge| {
            let w = shot_weight(edge);
            adj[edge.from].push((edge.to, w));
            adj[edge.to].push((edge.from, w));
        });
        for (a, b) in self.closure_edges_by_id() {
            adj[a].push((b, 0.0));
            adj[b].push((a, 0.0));
        }
        adj
    }

    /// The survey graph weighted by measured shot length (today's
    /// `build_survey_graph(.., None, ..)`): a reference tie is infinite unless
    /// `include_reference` (#27).
    pub(crate) fn survey_graph_by_length(&self, include_reference: bool) -> Vec<Vec<(usize, f64)>> {
        self.survey_graph_weighted(|edge| {
            let shot = &edge.line.shots[edge.index];
            if !shot.kind.is_swum() && !include_reference {
                return f64::INFINITY;
            }
            shot.length.max(0.0)
        })
    }

    /// The survey graph weighted by the 3-D distance between the shot's two
    /// stations as placed (slot coordinates on the shot's line), today's
    /// `build_survey_graph(.., Some(coords), ..)`.
    pub(crate) fn survey_graph_by_coords(
        &self,
        coords: &[[f64; 3]],
        include_reference: bool,
    ) -> Vec<Vec<(usize, f64)>> {
        self.survey_graph_weighted(|edge| {
            let shot = &edge.line.shots[edge.index];
            if !shot.kind.is_swum() && !include_reference {
                return f64::INFINITY;
            }
            placed_length(coords, edge)
        })
    }

    /// Shot lookup by dense end pair: `(min, max) -> (line, shot index)`. In a
    /// valid model (a forest of simple chains) at most one shot joins two
    /// stations; with an invalid one the lowest `(line id, index)` wins.
    pub(crate) fn shot_between_map(&self) -> BTreeMap<(usize, usize), (LineId, usize)> {
        let mut map: BTreeMap<(usize, usize), (LineId, usize)> = BTreeMap::new();
        self.for_each_shot_edge(|edge| {
            let key = (edge.from.min(edge.to), edge.from.max(edge.to));
            map.entry(key).or_insert((edge.line.id, edge.index));
        });
        map
    }

    /// Along-cave distance (measured shot lengths) from `roots` to every
    /// station: today's `compute_distances_from_with`. Closures are zero-cost.
    pub fn survey_distances_from(
        &self,
        roots: &[StationId],
        include_reference: bool,
    ) -> StationDistances {
        let topology = self.topology();
        let n = topology.stations().len();
        let roots: Vec<usize> = roots.iter().filter_map(|&s| topology.dense(s)).collect();
        let distance = if n == 0 || roots.is_empty() {
            vec![f64::INFINITY; n]
        } else {
            dijkstra(&self.survey_graph_by_length(include_reference), &roots)
        };
        StationDistances::new(topology, distance)
    }

    /// Along-cave distance from the nearest entrance datum (today's
    /// `compute_penetration_distances_with`).
    pub fn penetration_distances(&self, include_reference: bool) -> StationDistances {
        self.survey_distances_from(&self.entrance_stations(), include_reference)
    }

    /// The longest along-cave distance from an entrance to any station (today's
    /// `calculate_penetration_with`); `0.0` with no entrance.
    pub fn penetration(&self, include_reference: bool) -> f64 {
        self.penetration_distances(include_reference)
            .max_reachable()
    }

    /// The distance field from `from`, or from the entrance datums when `None`
    /// (today's `distance_field_from_with`): each shot weighted by the 3-D
    /// distance between its stations in `positions` (pass measured positions,
    /// #34), closures zero-cost. A `from` that is not a station on a line
    /// leaves everything unreachable.
    pub fn distance_field(
        &self,
        positions: &impl StationPositions,
        from: Option<StationId>,
        include_reference: bool,
    ) -> StationDistances {
        let topology = self.topology();
        let n = topology.stations().len();
        let roots: Vec<usize> = match from {
            Some(station) => topology.dense(station).into_iter().collect(),
            None => self
                .entrance_stations()
                .into_iter()
                .filter_map(|s| topology.dense(s))
                .collect(),
        };
        let distance = if n == 0 || roots.is_empty() {
            vec![f64::INFINITY; n]
        } else {
            let coords = dense_coords(topology, positions);
            dijkstra(
                &self.survey_graph_by_coords(&coords, include_reference),
                &roots,
            )
        };
        StationDistances::new(topology, distance)
    }

    /// The shortest along-cave-line distance between two ruler points, and the
    /// shots and stations on the way (today's `along_line_between_with`, #26).
    /// Each shot weighs the 3-D distance between its stations in `positions`
    /// (the caller passes the closure-adjusted positions for the adjusted
    /// shot lengths); a point on a shot splits it at `t`. Closures are
    /// zero-cost; a non-swum reference tie is impassable unless
    /// `include_reference`.
    pub fn along_line_between(
        &self,
        positions: &impl StationPositions,
        a: RulerPoint,
        b: RulerPoint,
        include_reference: bool,
    ) -> AlongLine {
        const EPS: f64 = 1e-9;
        let topology = self.topology();
        let n = topology.stations().len();
        if n == 0 {
            return AlongLine::none();
        }
        let coords = dense_coords(topology, positions);

        enum Resolved {
            Station(usize),
            Shot {
                from: usize,
                to: usize,
                t: f64,
                len: f64,
                shot: PathShot,
                edge: (LineId, usize),
                swimmable: bool,
            },
        }

        let resolve = |point: RulerPoint| -> Option<Resolved> {
            match point {
                RulerPoint::Station(station) => topology.dense(station).map(Resolved::Station),
                RulerPoint::OnShot { line, index, t } => {
                    let position = self.line_position(line)?;
                    let line_ref = &self.lines()[position];
                    let shot = line_ref.shots.get(index)?;
                    let chain = topology.chain_at(position)?;
                    let (sa, sb) = (*chain.get(index)?, *chain.get(index + 1)?);
                    let (from, to) = (topology.dense(sa)?, topology.dense(sb)?);
                    let len = placed_length(
                        &coords,
                        ShotEdge {
                            line: line_ref,
                            index,
                            from,
                            to,
                            from_station: sa,
                            to_station: sb,
                        },
                    );
                    let tt = t.clamp(0.0, 1.0);
                    if tt <= EPS {
                        return Some(Resolved::Station(from));
                    }
                    if tt >= 1.0 - EPS {
                        return Some(Resolved::Station(to));
                    }
                    Some(Resolved::Shot {
                        from,
                        to,
                        t: tt,
                        len,
                        shot: PathShot {
                            line_id: line,
                            shot_id: shot.id,
                        },
                        edge: (line, index),
                        swimmable: include_reference || shot.kind.is_swum(),
                    })
                }
            }
        };
        let (Some(ra), Some(rb)) = (resolve(a), resolve(b)) else {
            return AlongLine::none();
        };

        // Both points on the same shot: the direct sub-segment competes with
        // routes around it (a closure between the shot's stations can make a
        // route around shorter), exactly as today.
        let same_shot_direct: Option<(f64, PathShot, (LineId, usize))> = match (&ra, &rb) {
            (
                Resolved::Shot {
                    t: ta,
                    len,
                    shot: sa,
                    edge,
                    swimmable,
                    ..
                },
                Resolved::Shot {
                    t: tb, shot: sb, ..
                },
            ) if sa == sb => Some((
                if *swimmable {
                    (ta - tb).abs() * len
                } else {
                    f64::INFINITY
                },
                *sa,
                *edge,
            )),
            _ => None,
        };

        let mut adj = self.survey_graph_by_coords(&coords, include_reference);
        // Parallel (node, node) -> shot map for the tint, built from the same
        // edges; closure ties are absent, so a step not in it is skipped.
        let mut shot_edge: BTreeMap<(usize, usize), (PathShot, (LineId, usize))> = BTreeMap::new();
        self.for_each_shot_edge(|edge| {
            let ps = PathShot {
                line_id: edge.line.id,
                shot_id: edge.line.shots[edge.index].id,
            };
            let at = (edge.line.id, edge.index);
            shot_edge.entry((edge.from, edge.to)).or_insert((ps, at));
            shot_edge.entry((edge.to, edge.from)).or_insert((ps, at));
        });

        let mut materialize = |adj: &mut Vec<Vec<(usize, f64)>>, r: Resolved| -> usize {
            match r {
                Resolved::Station(i) => i,
                Resolved::Shot {
                    from,
                    to,
                    t,
                    len,
                    shot,
                    edge,
                    swimmable,
                } => {
                    let v = adj.len();
                    let (w_from, w_to) = if swimmable {
                        (t * len, (1.0 - t) * len)
                    } else {
                        (f64::INFINITY, f64::INFINITY)
                    };
                    adj.push(vec![(from, w_from), (to, w_to)]);
                    adj[from].push((v, w_from));
                    adj[to].push((v, w_to));
                    for key in [(v, from), (from, v), (v, to), (to, v)] {
                        shot_edge.insert(key, (shot, edge));
                    }
                    v
                }
            }
        };
        let start = materialize(&mut adj, ra);
        let goal = materialize(&mut adj, rb);
        if let Some((w, ps, at)) = same_shot_direct {
            adj[start].push((goal, w));
            adj[goal].push((start, w));
            shot_edge.insert((start, goal), (ps, at));
            shot_edge.insert((goal, start), (ps, at));
        }

        let (dist, pred) = dijkstra_to_goal(&adj, start, goal);
        if !dist[goal].is_finite() {
            return AlongLine::none();
        }
        let mut chain = vec![goal];
        let mut cur = goal;
        while cur != start {
            let p = pred[cur];
            if p == usize::MAX {
                return AlongLine::none();
            }
            chain.push(p);
            cur = p;
        }
        chain.reverse();

        let stations = topology.stations();
        let mut path_shots: Vec<PathShot> = Vec::new();
        let mut path_slots: Vec<StationRef> = Vec::new();
        let push_slot = |slot: StationRef, slots: &mut Vec<StationRef>| {
            if slots.last() != Some(&slot) {
                slots.push(slot);
            }
        };
        for w in chain.windows(2) {
            // A ~0-cost step took a closure tie (or a zero-length shot piece),
            // never tinted (as today).
            if dist[w[1]] - dist[w[0]] <= EPS {
                continue;
            }
            if let Some(&(ps, (line, _))) = shot_edge.get(&(w[0], w[1])) {
                if path_shots.last() != Some(&ps) {
                    path_shots.push(ps);
                }
                // The real stations of this step, as slots on the shot's line.
                for node in [w[0], w[1]] {
                    if node >= n {
                        continue;
                    }
                    if let Some(slot) = topology.slot_on(stations[node], line) {
                        push_slot(slot, &mut path_slots);
                    }
                }
            }
        }
        let mut path_stations: Vec<StationId> = Vec::new();
        for &node in &chain {
            if node < n && path_stations.last() != Some(&stations[node]) {
                path_stations.push(stations[node]);
            }
        }
        if path_slots.is_empty() {
            for &station in &path_stations {
                if let Some(slot) = topology.first_slot(station) {
                    push_slot(slot, &mut path_slots);
                }
            }
        }
        AlongLine {
            along_line: Some(dist[goal]),
            path_shots,
            path_stations,
            path_slots,
        }
    }
}

/// The 3-D distance between a shot's two stations as placed (slot coordinates
/// on the shot's line).
pub(crate) fn placed_length(coords: &[[f64; 3]], edge: ShotEdge<'_>) -> f64 {
    let p = slot_coords(coords, edge.from, edge.line, edge.from_station);
    let q = slot_coords(coords, edge.to, edge.line, edge.to_station);
    ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2)).sqrt()
}

#[derive(Copy, Clone, PartialEq)]
struct Node(f64, usize);

impl Eq for Node {}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or(Ordering::Equal)
            .then(self.1.cmp(&other.1))
    }
}

/// Multi-source Dijkstra (today's `Project::dijkstra`): the distance to every
/// node, `INFINITY` when unreachable.
pub(crate) fn dijkstra(adj: &[Vec<(usize, f64)>], roots: &[usize]) -> Vec<f64> {
    dijkstra_with_preds(adj, roots).0
}

/// Multi-source Dijkstra with the predecessor tree (today's
/// `Project::dijkstra_with_preds`): `pred[v]` is `usize::MAX` for a root or
/// an unreachable node.
pub(crate) fn dijkstra_with_preds(
    adj: &[Vec<(usize, f64)>],
    roots: &[usize],
) -> (Vec<f64>, Vec<usize>) {
    let (dist, _, pred) = dijkstra_scaled_with_riders(adj, adj, roots, 1.0, [0.0, 0.0]);
    (dist, pred)
}

/// Dijkstra over a scaled cost graph carrying two position-aligned riders
/// along each shortest path. Copied from
/// `models/src/project/pathfinding.rs` (`Project::dijkstra_scaled_with_riders`)
/// unchanged; see that function for the contract (`cost` and `rider` from the
/// same builder, edge for edge; a rider is the second quantity along the
/// COST-optimal path, not its own minimum).
pub(crate) fn dijkstra_scaled_with_riders(
    cost: &[Vec<(usize, f64)>],
    rider: &[Vec<(usize, f64)>],
    roots: &[usize],
    cost_scale: f64,
    rider_scales: [f64; 2],
) -> (Vec<f64>, [Vec<f64>; 2], Vec<usize>) {
    let n = cost.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut pred = vec![usize::MAX; n];
    let mut riders = [vec![f64::INFINITY; n], vec![f64::INFINITY; n]];
    let mut heap: BinaryHeap<std::cmp::Reverse<Node>> = BinaryHeap::new();
    for &r in roots {
        if r < n {
            dist[r] = 0.0;
            riders[0][r] = 0.0;
            riders[1][r] = 0.0;
            heap.push(std::cmp::Reverse(Node(0.0, r)));
        }
    }
    while let Some(std::cmp::Reverse(Node(d, u))) = heap.pop() {
        if d > dist[u] {
            continue;
        }
        let (ru0, ru1) = (riders[0][u], riders[1][u]);
        for (k, &(v, w)) in cost[u].iter().enumerate() {
            let nd = d + w * cost_scale;
            if nd < dist[v] {
                dist[v] = nd;
                pred[v] = u;
                let r = rider[u].get(k).map_or(0.0, |&(_, rw)| rw);
                riders[0][v] = ru0 + r * rider_scales[0];
                riders[1][v] = ru1 + r * rider_scales[1];
                heap.push(std::cmp::Reverse(Node(nd, v)));
            }
        }
    }
    (dist, riders, pred)
}

/// Single-source Dijkstra that stops at `goal` (the ruler's search).
fn dijkstra_to_goal(
    adj: &[Vec<(usize, f64)>],
    start: usize,
    goal: usize,
) -> (Vec<f64>, Vec<usize>) {
    let total = adj.len();
    let mut dist = vec![f64::INFINITY; total];
    let mut pred = vec![usize::MAX; total];
    let mut heap: BinaryHeap<std::cmp::Reverse<Node>> = BinaryHeap::new();
    dist[start] = 0.0;
    heap.push(std::cmp::Reverse(Node(0.0, start)));
    while let Some(std::cmp::Reverse(Node(d, u))) = heap.pop() {
        if d > dist[u] {
            continue;
        }
        if u == goal {
            break;
        }
        for &(v, w) in &adj[u] {
            let nd = d + w;
            if nd < dist[v] {
                dist[v] = nd;
                pred[v] = u;
                heap.push(std::cmp::Reverse(Node(nd, v)));
            }
        }
    }
    (dist, pred)
}
