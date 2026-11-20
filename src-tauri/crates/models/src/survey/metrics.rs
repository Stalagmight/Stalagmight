//! Closure metrics (plan 3.6), recomputed from raw positions every time.
//!
//! - `misclosure_distance`, `horizontal_misclosure`, `vertical_misclosure`:
//!   between the two ends' raw positions, each end's depth read on its `via`
//!   line (slot z, so a legacy hand-move z counts as today); the formulas are
//!   today's `Coordinates::distance_to` / `horizontal_distance_to`.
//! - `path_length`: the shortest path along SHOTS between the two stations
//!   (junctions are free, as one station; closures are not traversable). The
//!   shot graph is a forest (invariant 5), so that path is the unique tree
//!   path; it is summed from `a` to `b` with each shot weighing
//!   `length.max(0.0)` as today. Declared change: today's creation-time value
//!   also crossed merges and every existing closure at zero cost.
//! - `error_percentage`: `misclosure / path_length * 100`, `0` without a path.
//!   Two ends in different components (two datum-held pieces) have no path:
//!   `path_length` and `error_percentage` are `0`.

use crate::{ClosurePreview, LineId, StationId};

use super::{Project, RawPositions, Topology};

/// The shot forest rooted per component, for tree-path queries: parent,
/// the shot length to the parent, and the depth (edge count) of every dense
/// station.
pub(crate) struct ShotForest {
    parent: Vec<usize>,
    up_length: Vec<f64>,
    depth: Vec<usize>,
}

impl ShotForest {
    /// BFS from each component's lowest station over shots (lines in id
    /// order), so the forest is a function of the model alone.
    pub(crate) fn build(project: &Project) -> Self {
        let topology = project.topology();
        let n = topology.stations().len();
        let mut neighbours: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        project.for_each_shot_edge(|edge| {
            let w = edge.line.shots[edge.index].length.max(0.0);
            neighbours[edge.from].push((edge.to, w));
            neighbours[edge.to].push((edge.from, w));
        });
        let mut parent = vec![usize::MAX; n];
        let mut up_length = vec![0.0; n];
        let mut depth = vec![0; n];
        let mut seen = vec![false; n];
        for root in 0..n {
            if seen[root] {
                continue;
            }
            seen[root] = true;
            let mut queue = std::collections::VecDeque::from([root]);
            while let Some(u) = queue.pop_front() {
                for &(v, w) in &neighbours[u] {
                    if !seen[v] {
                        seen[v] = true;
                        parent[v] = u;
                        up_length[v] = w;
                        depth[v] = depth[u] + 1;
                        queue.push_back(v);
                    }
                }
            }
        }
        Self {
            parent,
            up_length,
            depth,
        }
    }

    /// The tree-path length from `a` to `b` (dense), summed from `a`; `None`
    /// when they are in different trees.
    pub(crate) fn path_length(&self, a: usize, b: usize) -> Option<f64> {
        let (mut x, mut y) = (a, b);
        let mut from_a: Vec<f64> = Vec::new();
        let mut from_b: Vec<f64> = Vec::new();
        while self.depth[x] > self.depth[y] {
            from_a.push(self.up_length[x]);
            x = self.parent[x];
        }
        while self.depth[y] > self.depth[x] {
            from_b.push(self.up_length[y]);
            y = self.parent[y];
        }
        while x != y {
            if self.parent[x] == usize::MAX || self.parent[y] == usize::MAX {
                return None;
            }
            from_a.push(self.up_length[x]);
            x = self.parent[x];
            from_b.push(self.up_length[y]);
            y = self.parent[y];
        }
        let mut total = 0.0;
        for w in from_a.iter().chain(from_b.iter().rev()) {
            total += w;
        }
        Some(total)
    }
}

impl Project {
    /// The shortest path along shots between two stations (junctions free,
    /// closures not traversable); `None` when they are not connected.
    pub fn shot_path_length(&self, a: StationId, b: StationId) -> Option<f64> {
        let topology = self.topology();
        let (da, db) = (topology.dense(a)?, topology.dense(b)?);
        ShotForest::build(self).path_length(da, db)
    }

    /// The metrics a closure between `a` and `b` would have (the closure
    /// preview, M2): raw positions, station depths. `None` when either station
    /// is on no line.
    pub fn closure_metrics(&self, a: StationId, b: StationId) -> Option<ClosurePreview> {
        let positions = self.raw_positions();
        let forest = ShotForest::build(self);
        self.closure_metrics_in(&positions, &forest, (a, None), (b, None))
    }

    /// [`Self::closure_metrics`] with each end's depth read on its `via` line
    /// (the slot the user picked).
    pub fn closure_metrics_via(
        &self,
        a: (StationId, Option<LineId>),
        b: (StationId, Option<LineId>),
    ) -> Option<ClosurePreview> {
        let positions = self.raw_positions();
        let forest = ShotForest::build(self);
        self.closure_metrics_in(&positions, &forest, a, b)
    }

    /// The metrics of every closure, in list order (one raw layout and one
    /// forest for all); `None` for a closure with an end on no line.
    pub fn closures_metrics(&self, positions: &RawPositions) -> Vec<Option<ClosurePreview>> {
        let forest = ShotForest::build(self);
        self.closures
            .iter()
            .map(|c| self.closure_metrics_in(positions, &forest, (c.a, c.a_via), (c.b, c.b_via)))
            .collect()
    }

    fn closure_metrics_in(
        &self,
        positions: &RawPositions,
        forest: &ShotForest,
        (a, a_via): (StationId, Option<LineId>),
        (b, b_via): (StationId, Option<LineId>),
    ) -> Option<ClosurePreview> {
        let topology = self.topology();
        let pa = self.end_coords(topology, positions, a, a_via)?;
        let pb = self.end_coords(topology, positions, b, b_via)?;
        let (dx, dy, dz) = (pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]);
        let horizontal_misclosure = (dx * dx + dy * dy).sqrt();
        let misclosure_distance = (dx * dx + dy * dy + dz * dz).sqrt();
        let vertical_misclosure = (pa[2] - pb[2]).abs();
        let path_length = forest
            .path_length(topology.dense(a)?, topology.dense(b)?)
            .unwrap_or(0.0);
        let error_percentage = if path_length > 0.0 {
            (misclosure_distance / path_length) * 100.0
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

    /// A closure end's raw position: the station's XY, its depth plus the
    /// hand-move z of its `via` line (else its first line).
    fn end_coords(
        &self,
        topology: &Topology,
        positions: &RawPositions,
        station: StationId,
        via: Option<LineId>,
    ) -> Option<[f64; 3]> {
        let slot = via
            .and_then(|line| topology.slot_on(station, line))
            .or_else(|| topology.first_slot(station))?;
        let [x, y, z] = positions.slot(self, slot)?;
        Some([x, y, z])
    }
}
