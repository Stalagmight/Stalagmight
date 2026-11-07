use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::{Coordinates, Section, SectionId, Shot, Station, StationRef};

use super::{HeapItem, PathEdge, PathStep, Project, StationPath};

#[cfg(test)]
mod path_length_tests;

impl Project {
    /// Calculate the path length between two stations through shots, merges, and closures.
    /// Both merges and closures are zero-distance edges.
    pub(crate) fn calculate_path_length_between(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> f64 {
        self.calculate_path_length_impl(station_a, station_b, true)
    }

    /// Calculate path length through shots and merges only (no closures).
    /// Use this when only rigid-group survey path length is needed.
    #[cfg(test)]
    pub(crate) fn calculate_merge_only_path_length(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> f64 {
        self.calculate_path_length_impl(station_a, station_b, false)
    }

    /// Internal Dijkstra path length implementation.
    /// When `include_closures` is true, loop closures are treated as zero-distance edges.
    fn calculate_path_length_impl(
        &self,
        station_a: StationRef,
        station_b: StationRef,
        include_closures: bool,
    ) -> f64 {
        if !self.station_exists(station_a) || !self.station_exists(station_b) {
            return 0.0;
        }

        // Section access is O(1) via the cached `get_section` (no need to build a
        // full section_lookup map per call); merge adjacency is the station-level
        // map (identical to `build_merge_adjacency(&full_section_lookup)` since the
        // lookup would contain every section).
        let merge_adj = self.build_station_merge_adjacency();

        let closure_adj = include_closures.then(|| self.build_station_closure_adjacency());
        self.path_length_dijkstra(station_a, station_b, &merge_adj, closure_adj.as_ref())
    }

    /// Every loop closure as a zero-cost edge both ways, in `loop_closures`
    /// order: the closure half of the graph [`Self::path_length_dijkstra`] walks.
    pub(super) fn build_station_closure_adjacency(&self) -> HashMap<StationRef, Vec<StationRef>> {
        let mut adj: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for closure in &self.loop_closures {
            let (Some(a), Some(b)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) else {
                continue;
            };
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        adj
    }

    /// Shortest survey-distance from `station_a` to `station_b`: shots are
    /// length-weighted edges; merges (and closures, when `closure_adj` is `Some`)
    /// are zero-cost edges. Takes PREBUILT adjacency so a batch caller can build
    /// it once and reuse it across many closures instead of rebuilding per call.
    /// Falls back to the reachable section lengths if no path is found (connected
    /// stations always find one). Dijkstra naturally stays within the endpoints'
    /// connected component, so disjoint components cost nothing.
    pub(crate) fn path_length_dijkstra(
        &self,
        station_a: StationRef,
        station_b: StationRef,
        merge_adj: &HashMap<StationRef, Vec<StationRef>>,
        closure_adj: Option<&HashMap<StationRef, Vec<StationRef>>>,
    ) -> f64 {
        let mut dist: HashMap<StationRef, f64> = HashMap::new();
        let mut heap = BinaryHeap::new();
        dist.insert(station_a, 0.0);
        heap.push(HeapItem {
            cost: 0.0,
            node: station_a,
        });

        while let Some(HeapItem { cost, node }) = heap.pop() {
            if let Some(&known) = dist.get(&node) {
                if cost > known + 1e-9 {
                    continue;
                }
            }

            if node == station_b {
                return cost;
            }

            let Some(section) = self.get_section(node.section_id) else {
                continue;
            };

            // Previous station (shot index = station_index - 1)
            if node.station_index > 0 {
                let prev_index = node.station_index - 1;
                if (prev_index as usize) <= section.shots.len() {
                    let shot_len = section
                        .shots
                        .get(prev_index as usize)
                        .map_or(0.0, |s| s.length.max(0.0));
                    let neighbor = StationRef::new(node.section_id, prev_index);
                    let next_cost = cost + shot_len;
                    if dist.get(&neighbor).is_none_or(|&d| next_cost < d - 1e-9) {
                        dist.insert(neighbor, next_cost);
                        heap.push(HeapItem {
                            cost: next_cost,
                            node: neighbor,
                        });
                    }
                }
            }

            // Next station (shot index = station_index)
            if (node.station_index as usize) < section.shots.len() {
                let next_index = node.station_index + 1;
                let shot_len = section
                    .shots
                    .get(node.station_index as usize)
                    .map_or(0.0, |s| s.length.max(0.0));
                let neighbor = StationRef::new(node.section_id, next_index);
                let next_cost = cost + shot_len;
                if dist.get(&neighbor).is_none_or(|&d| next_cost < d - 1e-9) {
                    dist.insert(neighbor, next_cost);
                    heap.push(HeapItem {
                        cost: next_cost,
                        node: neighbor,
                    });
                }
            }

            // Merge neighbors (zero distance)
            if let Some(neighbors) = merge_adj.get(&node) {
                for &neighbor in neighbors {
                    if dist.get(&neighbor).is_none_or(|&d| cost < d - 1e-9) {
                        dist.insert(neighbor, cost);
                        heap.push(HeapItem {
                            cost,
                            node: neighbor,
                        });
                    }
                }
            }

            // Closure neighbors (zero distance)
            if let Some(cadj) = closure_adj {
                if let Some(neighbors) = cadj.get(&node) {
                    for &neighbor in neighbors {
                        if dist.get(&neighbor).is_none_or(|&d| cost < d - 1e-9) {
                            dist.insert(neighbor, cost);
                            heap.push(HeapItem {
                                cost,
                                node: neighbor,
                            });
                        }
                    }
                }
            }
        }

        // No path found (shouldn't happen if sections are connected)
        // Fall back to sum of all reachable section lengths.
        //
        // [#576 RV19a-9] Not through a closure joining exactly these two
        // stations: a walk that found no route had none in its graph (the
        // restamp lifts the measured closure's own edge out; creation measures
        // before the closure exists), so counting the sections behind one
        // summed the far side too - a restamp came out longer than creation.
        // Any other caller would have walked such an edge at zero cost and
        // never got here, so for them nothing changes.
        let fallback_sections = if closure_adj.is_some() {
            self.connected_section_ids_without_pair(station_a, station_b)
        } else {
            self.get_merged_section_ids(station_a.section_id)
        };
        // [#576 FZ-DET] Summed in PROJECT order, not the set's: float addition
        // is not associative, so a hash-ordered sum came out a last bit apart
        // from one run to the next (18.455 vs 18.455000000000002), and the
        // closure's stored path length and error percentage with it.
        self.sections
            .iter()
            .filter(|s| fallback_sections.contains(&s.id))
            .flat_map(|s| s.shots.iter())
            // Clamp negative lengths to 0, consistent with the main-loop edges (#176).
            .map(|shot| shot.length.max(0.0))
            .sum()
    }

    pub(crate) fn build_section_lookup(&self) -> HashMap<SectionId, &Section> {
        self.sections.iter().map(|s| (s.id, s)).collect()
    }

    pub(crate) fn build_merge_adjacency(
        &self,
        section_lookup: &HashMap<SectionId, &Section>,
    ) -> HashMap<StationRef, Vec<StationRef>> {
        let mut adjacency: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for merge in &self.station_merges {
            if !section_lookup.contains_key(&merge.station_a.section_id)
                || !section_lookup.contains_key(&merge.station_b.section_id)
            {
                continue;
            }
            if let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                adjacency.entry(a).or_default().push(b);
                adjacency.entry(b).or_default().push(a);
            }
        }
        adjacency
    }

    fn shortest_path_with_target<F>(
        &self,
        start: StationRef,
        is_target: F,
    ) -> Option<(StationRef, HashMap<StationRef, (StationRef, PathEdge)>)>
    where
        F: Fn(StationRef) -> bool,
    {
        let section_lookup = self.build_section_lookup();
        let merge_adj = self.build_merge_adjacency(&section_lookup);

        // Validate start station exists
        let start_section = section_lookup.get(&start.section_id)?;
        if start.station_index as usize > start_section.shots.len() {
            return None;
        }

        let mut dist: HashMap<StationRef, f64> = HashMap::new();
        let mut prev: HashMap<StationRef, (StationRef, PathEdge)> = HashMap::new();
        let mut heap = BinaryHeap::new();

        dist.insert(start, 0.0);
        heap.push(HeapItem {
            cost: 0.0,
            node: start,
        });

        while let Some(HeapItem { cost, node }) = heap.pop() {
            if let Some(&known) = dist.get(&node) {
                if cost > known + 1e-9 {
                    continue;
                }
            }

            if is_target(node) {
                return Some((node, prev));
            }

            let Some(section) = section_lookup.get(&node.section_id) else {
                continue;
            };

            // Previous station
            if node.station_index > 0 {
                let prev_index = node.station_index - 1;
                if (prev_index as usize) <= section.shots.len() {
                    let shot_len = section
                        .shots
                        .get(prev_index as usize)
                        // Clamp negative shot lengths to 0 - Dijkstra requires
                        // non-negative edges. Interactive commands reject negative
                        // lengths, but a corrupt import/parse or a deserialized file
                        // can carry them; matches the other graph builders (#176).
                        .map_or(0.0, |s| s.length.max(0.0));
                    let neighbor = StationRef::new(node.section_id, prev_index);
                    let next_cost = cost + shot_len;
                    if dist.get(&neighbor).is_none_or(|&d| next_cost < d - 1e-9) {
                        dist.insert(neighbor, next_cost);
                        prev.insert(
                            neighbor,
                            (
                                node,
                                PathEdge::Shot {
                                    section_id: node.section_id,
                                    shot_index: prev_index as usize,
                                    forward: false,
                                },
                            ),
                        );
                        heap.push(HeapItem {
                            cost: next_cost,
                            node: neighbor,
                        });
                    }
                }
            }

            // Next station
            if (node.station_index as usize) < section.shots.len() {
                let next_index = node.station_index + 1;
                let shot_len = section
                    .shots
                    .get(node.station_index as usize)
                    // Clamp negative shot lengths to 0 (see the previous-station edge).
                    .map_or(0.0, |s| s.length.max(0.0));
                let neighbor = StationRef::new(node.section_id, next_index);
                let next_cost = cost + shot_len;
                if dist.get(&neighbor).is_none_or(|&d| next_cost < d - 1e-9) {
                    dist.insert(neighbor, next_cost);
                    prev.insert(
                        neighbor,
                        (
                            node,
                            PathEdge::Shot {
                                section_id: node.section_id,
                                shot_index: node.station_index as usize,
                                forward: true,
                            },
                        ),
                    );
                    heap.push(HeapItem {
                        cost: next_cost,
                        node: neighbor,
                    });
                }
            }

            // Merge neighbors (zero distance)
            if let Some(neighbors) = merge_adj.get(&node) {
                for &neighbor in neighbors {
                    let next_cost = cost;
                    if dist.get(&neighbor).is_none_or(|&d| next_cost < d - 1e-9) {
                        dist.insert(neighbor, next_cost);
                        prev.insert(neighbor, (node, PathEdge::Merge));
                        heap.push(HeapItem {
                            cost: next_cost,
                            node: neighbor,
                        });
                    }
                }
            }
        }

        None
    }

    fn reconstruct_path(
        start: StationRef,
        goal: StationRef,
        prev: &HashMap<StationRef, (StationRef, PathEdge)>,
    ) -> Option<StationPath> {
        let mut steps_rev: Vec<PathStep> = Vec::new();
        let mut current = goal;
        while current != start {
            let (p, edge) = prev.get(&current).copied()?;
            steps_rev.push(PathStep { to: current, edge });
            current = p;
        }
        steps_rev.reverse();
        Some(StationPath {
            start,
            steps: steps_rev,
        })
    }

    pub(super) fn shortest_path_shots_merges(
        &self,
        start: StationRef,
        goal: StationRef,
    ) -> Option<StationPath> {
        let (found, prev) = self.shortest_path_with_target(start, |node| node == goal)?;
        Self::reconstruct_path(start, found, &prev)
    }

    pub(super) fn shortest_path_to_any_zero_datum(
        &self,
        start: StationRef,
    ) -> Option<(StationRef, StationPath)> {
        if self.zero_datums.is_empty() {
            return None;
        }
        let zero_set: HashSet<StationRef> = self
            .zero_datums
            .iter()
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .collect();
        let (found, prev) =
            self.shortest_path_with_target(start, |node| zero_set.contains(&node))?;
        let path = Self::reconstruct_path(start, found, &prev)?;
        Some((found, path))
    }

    pub(super) fn build_colocated_groups(&self) -> HashMap<StationRef, Vec<StationRef>> {
        let mut parent: HashMap<StationRef, StationRef> = HashMap::new();
        let mut rank: HashMap<StationRef, usize> = HashMap::new();

        for section in &self.sections {
            for idx in 0..=section.shots.len() {
                let sr = StationRef::new(section.id, idx as u32);
                parent.insert(sr, sr);
                rank.insert(sr, 0);
            }
        }

        fn find(parent: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
            let mut current = x;
            while let Some(&p) = parent.get(&current) {
                if p == current {
                    break;
                }
                current = p;
            }
            let mut node = x;
            while let Some(&p) = parent.get(&node) {
                if p == current {
                    break;
                }
                parent.insert(node, current);
                node = p;
            }
            current
        }

        fn union(
            parent: &mut HashMap<StationRef, StationRef>,
            rank: &mut HashMap<StationRef, usize>,
            a: StationRef,
            b: StationRef,
        ) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra == rb {
                return;
            }
            let rank_a = *rank.get(&ra).unwrap_or(&0);
            let rank_b = *rank.get(&rb).unwrap_or(&0);
            match rank_a.cmp(&rank_b) {
                Ordering::Less => {
                    parent.insert(ra, rb);
                }
                Ordering::Greater => {
                    parent.insert(rb, ra);
                }
                Ordering::Equal => {
                    parent.insert(rb, ra);
                    rank.insert(ra, rank_a + 1);
                }
            }
        }

        for merge in &self.station_merges {
            if let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                union(&mut parent, &mut rank, a, b);
            }
        }

        for closure in &self.loop_closures {
            if !closure.is_aligned {
                continue;
            }
            if let (Some(a), Some(b)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) {
                union(&mut parent, &mut rank, a, b);
            }
        }

        let mut groups: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for section in &self.sections {
            for idx in 0..=section.shots.len() {
                let sr = StationRef::new(section.id, idx as u32);
                let root = find(&mut parent, sr);
                groups.entry(root).or_default().push(sr);
            }
        }

        groups
    }

    pub(super) fn build_merge_groups(&self) -> HashMap<StationRef, Vec<StationRef>> {
        let mut parent: HashMap<StationRef, StationRef> = HashMap::new();
        let mut rank: HashMap<StationRef, usize> = HashMap::new();

        for section in &self.sections {
            for idx in 0..=section.shots.len() {
                let sr = StationRef::new(section.id, idx as u32);
                parent.insert(sr, sr);
                rank.insert(sr, 0);
            }
        }

        fn find(parent: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
            let mut current = x;
            while let Some(&p) = parent.get(&current) {
                if p == current {
                    break;
                }
                current = p;
            }
            let mut node = x;
            while let Some(&p) = parent.get(&node) {
                if p == current {
                    break;
                }
                parent.insert(node, current);
                node = p;
            }
            current
        }

        fn union(
            parent: &mut HashMap<StationRef, StationRef>,
            rank: &mut HashMap<StationRef, usize>,
            a: StationRef,
            b: StationRef,
        ) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra == rb {
                return;
            }
            let rank_a = *rank.get(&ra).unwrap_or(&0);
            let rank_b = *rank.get(&rb).unwrap_or(&0);
            match rank_a.cmp(&rank_b) {
                Ordering::Less => {
                    parent.insert(ra, rb);
                }
                Ordering::Greater => {
                    parent.insert(rb, ra);
                }
                Ordering::Equal => {
                    parent.insert(rb, ra);
                    rank.insert(ra, rank_a + 1);
                }
            }
        }

        for merge in &self.station_merges {
            if let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                union(&mut parent, &mut rank, a, b);
            }
        }

        let mut groups: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for section in &self.sections {
            for idx in 0..=section.shots.len() {
                let sr = StationRef::new(section.id, idx as u32);
                let root = find(&mut parent, sr);
                groups.entry(root).or_default().push(sr);
            }
        }

        groups
    }

    /// Build a `StationRef -> index` map for `stations` (index = position in the
    /// slice) so graph builders can resolve shot/merge/closure endpoints to
    /// nodes without a linear search.
    pub(super) fn station_index_map(stations: &[Station]) -> HashMap<StationRef, usize> {
        let mut m: HashMap<StationRef, usize> = HashMap::with_capacity(stations.len());
        for (i, s) in stations.iter().enumerate() {
            m.insert(s.station_ref, i);
        }
        m
    }

    /// Build the survey distance graph as an adjacency list over the `n` stations
    /// (nodes indexed by position; `station_index` maps refs to those indices).
    /// Edges are **every real topological connection**:
    /// - every shot - permanent AND jump/reference - weighted by its measured
    ///   `length` (reference shots are real measured legs / jumps a diver swims);
    /// - `station_merges` (zero cost - the same physical point);
    /// - every `loop_closures` entry (zero cost - a closed loop means the two
    ///   stations are the same physical point), **regardless of `is_aligned`**;
    ///   alignment is only a geometry-reconciliation strategy (`Move` vs `Batch`),
    ///   so the topological connection exists either way.
    ///
    /// We deliberately include everything. Distances are shortest-path (Dijkstra
    /// always takes the minimum), so an extra real edge can only ever yield a
    /// shorter, more correct route - never inflate one. Excluding real
    /// connections is what hides reachable areas or routes "the long way".
    ///
    /// #77 is now CLOSED by #27: a [`ShotKind::Reference`] is a non-swimmable
    /// survey tie, so `include_reference = false` (the default everywhere but the
    /// `gas.include_reference_legs` opt-in) drops it from the route. That removes
    /// the false-shortcut risk this doc used to carry. Jumps stay in: they are
    /// swum. Excluding an edge can only lengthen a route or shrink reach, which
    /// is the safe direction; the 88ccbaff lesson was about excluding *real* legs,
    /// and a reference tie is precisely the leg that is not real to swim.
    pub(super) fn build_survey_graph(
        &self,
        n: usize,
        station_index: &HashMap<StationRef, usize>,
        coords: Option<&[Coordinates]>,
        include_reference: bool,
    ) -> Vec<Vec<(usize, f64)>> {
        // Thin wrapper over `build_survey_graph_weighted` preserving the original two
        // weightings: straight-line between `coords` (closure-adjusted geometry, for the
        // distance gradient) when supplied, else the measured `shot.length`. The gas engine
        // (`models/project/gas`) calls the weighted form directly with a gas-cost closure.
        // An unusable leg gets an INFINITE weight rather than being skipped:
        // `along_line_between` keys a parallel shot-edge map on this function's
        // edge-emission order, so the edge SET must not change. Dijkstra never
        // relaxes across an infinite edge, so the leg is unreachable either way.
        match coords {
            Some(c) => self.build_survey_graph_weighted(n, station_index, |shot, fi, ti| {
                if !shot.kind.is_swum() && !include_reference {
                    return f64::INFINITY;
                }
                let (a, b) = (&c[fi], &c[ti]);
                let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
                (dx * dx + dy * dy + dz * dz).sqrt()
            }),
            None => self.build_survey_graph_weighted(n, station_index, |shot, _fi, _ti| {
                if !shot.kind.is_swum() && !include_reference {
                    return f64::INFINITY;
                }
                shot.length.max(0.0)
            }),
        }
    }

    /// Generalisation of [`build_survey_graph`](Self::build_survey_graph): each shot edge's
    /// weight is supplied by `shot_weight(shot, from_idx, to_idx)`. Shot edges (permanent +
    /// jump/reference) take the closure's weight; station merges and loop closures stay
    /// **zero-cost** (same physical point). The section/shot iteration order is byte-identical
    /// to `build_survey_graph` - `along_line_between` keys a parallel shot-edge map on that
    /// edge-emission order, so it must not change here.
    pub(super) fn build_survey_graph_weighted<F>(
        &self,
        n: usize,
        station_index: &HashMap<StationRef, usize>,
        shot_weight: F,
    ) -> Vec<Vec<(usize, f64)>>
    where
        F: Fn(&Shot, usize, usize) -> f64,
    {
        let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];

        // Shots (permanent and jump/reference).
        for section in &self.sections {
            for (i, shot) in section.shots.iter().enumerate() {
                let from = StationRef::new(section.id, i as u32);
                let to = StationRef::new(section.id, (i + 1) as u32);
                if let (Some(&fi), Some(&ti)) = (station_index.get(&from), station_index.get(&to)) {
                    let w = shot_weight(shot, fi, ti);
                    adj[fi].push((ti, w));
                    adj[ti].push((fi, w));
                }
            }
        }
        // Station merges: zero-cost (same physical point).
        for merge in &self.station_merges {
            if let (Some(a_sr), Some(b_sr)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                    adj[a].push((b, 0.0));
                    adj[b].push((a, 0.0));
                }
            }
        }
        // Loop closures: zero-cost, INCLUDING non-aligned ones - a closed loop is
        // a real physical connection no matter the reconciliation strategy.
        for closure in &self.loop_closures {
            if let (Some(a_sr), Some(b_sr)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) {
                if let (Some(&a), Some(&b)) = (station_index.get(&a_sr), station_index.get(&b_sr)) {
                    adj[a].push((b, 0.0));
                    adj[b].push((a, 0.0));
                }
            }
        }

        adj
    }

    /// Multi-source Dijkstra over `adj` from the given `roots` (node indices,
    /// each seeded at distance 0). Returns the shortest-path distance to every
    /// node; unreachable nodes - and every node when `roots` is empty - stay
    /// `f64::INFINITY`. Thin wrapper over [`dijkstra_with_preds`](Self::dijkstra_with_preds)
    /// that drops the predecessor tree (the `dist` half is identical).
    pub(super) fn dijkstra(adj: &[Vec<(usize, f64)>], roots: &[usize]) -> Vec<f64> {
        Self::dijkstra_with_preds(adj, roots).0
    }

    /// Multi-source Dijkstra that also returns the **predecessor tree**: `pred[v]` is the node
    /// `v` was last relaxed from on its shortest path, or `usize::MAX` for a root or an
    /// unreachable node. Strict `<` relaxation means the first/least-cost path wins and equal-cost
    /// ties never overwrite, so `pred` always corresponds to the path that set `dist[v]` (and
    /// `dist` is identical to [`dijkstra`](Self::dijkstra)). Walk `pred` back from any node until
    /// `usize::MAX` to reconstruct its entry path to the nearest root.
    pub(super) fn dijkstra_with_preds(
        adj: &[Vec<(usize, f64)>],
        roots: &[usize],
    ) -> (Vec<f64>, Vec<usize>) {
        // Riders scaled to 0 accumulate nothing, so passing `adj` as its own rider graph is
        // CORRECT - but it is not free. Every plain Dijkstra now allocates two extra
        // `vec![f64::INFINITY; n]` and does two multiply-adds per successful relaxation, and this is
        // the entry point the distance-gradient overlay runs over the whole survey graph.
        //
        // The trade is still right: one heap loop, maintained and tested once, against two
        // near-identical copies that would drift. The costs are O(n) memory and a constant factor on
        // the relaxation - not another graph walk - so they stay well under the heap operations that
        // dominate. If a profile ever says otherwise, the fix is a rider-count generic, not a fork.
        let (dist, _, pred) = Self::dijkstra_scaled_with_riders(adj, adj, roots, 1.0, [0.0, 0.0]);
        (dist, pred)
    }

    /// [`dijkstra_with_preds`](Self::dijkstra_with_preds) over a **scaled** cost graph that also
    /// carries two positionally-aligned **rider** quantities along each shortest path.
    ///
    /// Edge `k` of node `u` costs `cost[u][k].1 x cost_scale` and contributes
    /// `rider[u][k].1 x rider_scales[j]` to rider `j`. `cost` and `rider` must therefore describe
    /// the same edges in the same order - guaranteed when both come from
    /// [`build_survey_graph_weighted`](Self::build_survey_graph_weighted), whose edge-emission
    /// order is fixed by contract. Aligning by POSITION rather than by node pair is deliberate:
    /// `adj` can hold parallel edges between one pair (a shot plus a merge/closure tie), so a
    /// pair-keyed lookup would have to guess which one Dijkstra actually relaxed.
    ///
    /// The scaling exists so one graph build serves many weightings: the gas engine prices a leg as
    /// `basis x (rmv / speed)` and varies only that scalar per plan segment (#317).
    ///
    /// ⚠ **A rider is the second quantity along the COST-optimal path, not its own minimum.** The
    /// caller wants what the chosen route costs in the rider's units, not the different route that
    /// would minimise the rider. Scooter distance along the gas-optimal path is by definition `>=`
    /// the achievable minimum, so a cap on it can only ever under-state reach. Where the gas engine
    /// needs more - the cheapest gas among the routes a scooter's range covers, which round a loop
    /// is not the gas-optimal path when that runs out of range - it does its own two-criteria search,
    /// in `gas/range_route.rs` (#576 NF-W11GAS-1, NF-W12GAS-1), and reads this function's riders only
    /// along the gas-optimal path it also needs. Keep this one single-criterion.
    ///
    /// `cost_scale` must be finite and positive, or an infinite edge could stop reading as
    /// unreachable; callers validate their speeds at the config boundary.
    pub(super) fn dijkstra_scaled_with_riders(
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

        let mut heap: BinaryHeap<std::cmp::Reverse<Node>> = BinaryHeap::new();
        for &r in roots {
            // Bounds-guard only; a duplicate root just gets re-popped once and
            // filtered by the `d > dist[u]` check below - no need to dedup here.
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
            // `u` is only ever popped after it was relaxed (or seeded as a root), so both riders are
            // finite here.
            let (ru0, ru1) = (riders[0][u], riders[1][u]);
            // `rider[u]` is aligned with `cost[u]` edge-for-edge, so the same index `k` names the
            // same edge in both. A shorter rider row would mean the two graphs came from different
            // builds; charge nothing for the unmatched edge rather than panicking.
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

    /// Resolve the project's zero-datum stations to their indices in `stations`.
    pub(super) fn zero_datum_roots(
        &self,
        station_index: &HashMap<StationRef, usize>,
    ) -> Vec<usize> {
        self.zero_datums
            .iter()
            .filter_map(|zd| {
                self.resolve_key(&zd.station_ref)
                    .and_then(|sr| station_index.get(&sr).copied())
            })
            .collect()
    }
}
