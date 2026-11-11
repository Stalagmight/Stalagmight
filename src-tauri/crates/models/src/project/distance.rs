use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

use crate::{
    Coordinates, FrozenShotRef, Section, SectionId, ShotData, ShotId, Station, StationKey,
    StationRef,
};

use super::{AlongLineResult, DistanceField, PathShot, Project, RulerEndpoint};

impl Project {
    /// Per-station along-cave distance from the given `roots` (indices into
    /// `stations`), summing each shot's measured `length` along the shortest
    /// path. Merges and all loop closures are zero-cost (a closed loop ties two
    /// stations to the same physical point). Unreachable stations - and every
    /// station when `roots`/`stations` is empty - get `f64::INFINITY`.
    ///
    /// This is the reusable core shared by penetration distance (roots = zero
    /// datums), the distance-gradient overlay (roots = a chosen station), and a
    /// future gas-planning mode. All graph/geometry work stays here in the
    /// domain layer; callers (IPC, UI) only consume the resulting distances.
    pub fn compute_distances_from(&self, stations: &[Station], roots: &[usize]) -> Vec<f64> {
        self.compute_distances_from_with(stations, roots, false)
    }

    /// [`compute_distances_from`](Self::compute_distances_from) with the #27
    /// `gas.include_reference_legs` opt-in threaded in. `include_reference =
    /// true` restores the pre-#27 behaviour of routing through reference ties.
    pub fn compute_distances_from_with(
        &self,
        stations: &[Station],
        roots: &[usize],
        include_reference: bool,
    ) -> Vec<f64> {
        let n = stations.len();
        if n == 0 || roots.is_empty() {
            return vec![f64::INFINITY; n];
        }
        let station_index = Self::station_index_map(stations);
        let adj = self.build_survey_graph(n, &station_index, None, include_reference);
        Self::dijkstra(&adj, roots)
    }

    /// Per-station along-cave distance to the nearest zero datum: the
    /// shortest path through the survey graph summing each shot's measured
    /// length. Edges are permanent shots and jumps (both swum) plus zero-cost
    /// edges for `station_merges` and all `loop_closures` (a closed loop ties
    /// two stations to the same physical point). Reference ties are excluded
    /// since #27: they are not swimmable, so a dive route cannot use them.
    /// Unreachable stations get `f64::INFINITY`. Empty projects / no zero
    /// datums → all `INFINITY`.
    ///
    /// Equivalent to [`compute_distances_from`] seeded with the zero-datum
    /// station indices - the default origin for the distance-gradient overlay.
    pub fn compute_penetration_distances(&self, stations: &[Station]) -> Vec<f64> {
        self.compute_penetration_distances_with(stations, false)
    }

    /// [`compute_penetration_distances`](Self::compute_penetration_distances)
    /// with the #27 `gas.include_reference_legs` opt-in threaded in.
    pub fn compute_penetration_distances_with(
        &self,
        stations: &[Station],
        include_reference: bool,
    ) -> Vec<f64> {
        let n = stations.len();
        if n == 0 || self.zero_datums.is_empty() {
            return vec![f64::INFINITY; n];
        }
        let station_index = Self::station_index_map(stations);
        let roots = self.zero_datum_roots(&station_index);
        let adj = self.build_survey_graph(n, &station_index, None, include_reference);
        Self::dijkstra(&adj, &roots)
    }

    /// Maximum along-cave penetration distance - the longest shortest-path
    /// distance from any zero datum to any station through the survey
    /// graph. Zero if there are no zero datums or nothing is reachable.
    pub fn calculate_penetration(&self, stations: &[Station]) -> f64 {
        self.calculate_penetration_with(stations, false)
    }

    /// [`calculate_penetration`](Self::calculate_penetration) with the #27
    /// `gas.include_reference_legs` opt-in threaded in.
    pub fn calculate_penetration_with(&self, stations: &[Station], include_reference: bool) -> f64 {
        self.compute_penetration_distances_with(stations, include_reference)
            .into_iter()
            .filter(|d| d.is_finite())
            .fold(0.0_f64, f64::max)
    }

    /// Per-station distance field from a chosen origin `from` station, or from the
    /// zero datum(s) when `from` is `None` (the gradient's entrance default).
    ///
    /// Edge weights come from each station's **coordinates** in `stations` - the
    /// caller passes MEASURED coordinates (surveyed lengths), so distances reflect
    /// the physical guideline length (#34). Merges/closures stay zero-cost.
    /// Unreachable stations come back as `None`; the returned `station_refs` echo
    /// the input, so the field is self-describing.
    pub fn distance_field_from(
        &self,
        stations: &[Station],
        from: Option<&StationRef>,
    ) -> DistanceField {
        self.distance_field_from_with(stations, from, false)
    }

    /// [`distance_field_from`](Self::distance_field_from) with the #27
    /// `gas.include_reference_legs` opt-in threaded in.
    pub fn distance_field_from_with(
        &self,
        stations: &[Station],
        from: Option<&StationRef>,
        include_reference: bool,
    ) -> DistanceField {
        let n = stations.len();
        let station_index = Self::station_index_map(stations);
        let coords: Vec<Coordinates> = stations.iter().map(|s| s.coordinates).collect();
        let roots: Vec<usize> = match from {
            Some(r) => {
                let key: StationKey = (*r).into();
                self.resolve_key(&key)
                    .and_then(|sr| station_index.get(&sr).copied())
                    .into_iter()
                    .collect()
            }
            None => self.zero_datum_roots(&station_index),
        };
        let raw = if n == 0 || roots.is_empty() {
            vec![f64::INFINITY; n]
        } else {
            let adj = self.build_survey_graph(n, &station_index, Some(&coords), include_reference);
            Self::dijkstra(&adj, &roots)
        };
        let mut max_reachable = 0.0_f64;
        let distances: Vec<Option<f64>> = raw
            .iter()
            .map(|&d| {
                if d.is_finite() {
                    if d > max_reachable {
                        max_reachable = d;
                    }
                    Some(d)
                } else {
                    None
                }
            })
            .collect();
        DistanceField {
            distances,
            station_refs: stations.iter().map(|s| s.station_ref).collect(),
            max_reachable,
        }
    }

    /// Shortest along-cave-line distance between two ruler endpoints (#26), plus
    /// the ordered shots traversed (for tinting). The distance sums the lengths of
    /// the shots on the path - each shot weighted by the euclidean distance between
    /// its endpoints in `stations` (the *placed* shot length), so passing the
    /// closure-ADJUSTED coordinates (see `AppState::along_line_between`) yields the
    /// closure-adjusted shot lengths the user asked for. For a continuous survey
    /// this placed length equals the surveyed tape length; at depth discontinuities
    /// the depth-averaged station placement can differ slightly, matching what is
    /// drawn rather than the raw tape figure. An
    /// endpoint may be a station or an exact point on a shot at fraction `t` (which
    /// splits that shot's edge; a mid-shot leg contributes `t` of the adjusted shot
    /// length). Returns `along_line: None` when the endpoints are in different
    /// connected components. `stations` supplies both the `StationRef -> index` map
    /// and the per-node coordinates used for the weights.
    pub fn along_line_between(
        &self,
        stations: &[Station],
        a: RulerEndpoint,
        b: RulerEndpoint,
    ) -> AlongLineResult {
        self.along_line_between_with(stations, a, b, false)
    }

    /// [`along_line_between`](Self::along_line_between) with the #27
    /// `gas.include_reference_legs` opt-in threaded in. The ruler measures a
    /// route a diver could follow, so a non-swimmable reference tie is excluded
    /// by default; turning the opt-in on restores the pre-#27 behaviour.
    pub fn along_line_between_with(
        &self,
        stations: &[Station],
        a: RulerEndpoint,
        b: RulerEndpoint,
        include_reference: bool,
    ) -> AlongLineResult {
        const EPS: f64 = 1e-9;
        let none = || AlongLineResult {
            along_line: None,
            path_shots: Vec::new(),
            path_stations: Vec::new(),
        };

        let n = stations.len();
        if n == 0 {
            return none();
        }
        let idx = Self::station_index_map(stations);
        let section_lookup = self.build_section_lookup();

        // Shot lengths come from the euclidean distance between the (adjusted)
        // station coordinates, so the along-line distance sums the closure-adjusted
        // shot lengths (#26). `shot_len` is only ever called with real station node
        // indices (a shot's from/to), which are always < `coords.len()`.
        let coords: Vec<Coordinates> = stations.iter().map(|s| s.coordinates).collect();
        let shot_len = |from: usize, to: usize| -> f64 {
            let (p, q) = (&coords[from], &coords[to]);
            ((q.x - p.x).powi(2) + (q.y - p.y).powi(2) + (q.z - p.z).powi(2)).sqrt()
        };

        // How an endpoint resolves onto the graph: an existing station node, or a
        // virtual split of a shot edge at fraction `t`.
        enum Resolved {
            Station(usize),
            Shot {
                from: usize,
                to: usize,
                t: f64,
                len: f64,
                shot: PathShot,
                /// Whether a diver can actually swim this leg. `build_survey_graph`
                /// blocks a non-swum reference tie by weighting its from<->to edge
                /// INFINITY, but the split edges an endpoint adds below bypass that
                /// edge entirely - so without this the ruler routes across up to one
                /// partial reference tie per endpoint, contradicting the exclusion
                /// this function documents (#27).
                swimmable: bool,
            },
        }

        let resolve = |ep: &RulerEndpoint| -> Option<Resolved> {
            // Shot-point endpoint.
            if let (Some(shot_id), Some(t)) = (ep.shot_id, ep.t) {
                let section = section_lookup.get(&ep.section_id)?;
                let pos = section.shots.iter().position(|s| s.id == shot_id)?;
                let from = *idx.get(&StationRef::new(ep.section_id, pos as u32))?;
                let to = *idx.get(&StationRef::new(ep.section_id, (pos + 1) as u32))?;
                let len = shot_len(from, to);
                let tt = t.clamp(0.0, 1.0);
                if tt <= EPS {
                    return Some(Resolved::Station(from));
                }
                if tt >= 1.0 - EPS {
                    return Some(Resolved::Station(to));
                }
                return Some(Resolved::Shot {
                    from,
                    to,
                    t: tt,
                    len,
                    shot: PathShot {
                        section_id: ep.section_id,
                        shot_id,
                    },
                    swimmable: include_reference || section.shots[pos].kind.is_swum(),
                });
            }
            // Station endpoint.
            let si = ep.station_index?;
            let node = *idx.get(&StationRef::new(ep.section_id, si))?;
            Some(Resolved::Station(node))
        };

        let ra = match resolve(&a) {
            Some(r) => r,
            None => return none(),
        };
        let rb = match resolve(&b) {
            Some(r) => r,
            None => return none(),
        };

        // Both endpoints on the SAME shot: the direct sub-segment is |Δt|·L. We do
        // NOT short-circuit to it - a merge/loop closure between the shot's stations
        // can make a route *around* shorter than the direct segment, and Dijkstra
        // must stay free to pick the genuine shortest along-cave-line path. Instead
        // we remember it and add it as a direct edge between the two virtual nodes
        // after materializing, so it simply competes with the around routes.
        let same_shot_direct: Option<(f64, PathShot)> = match (&ra, &rb) {
            (
                Resolved::Shot {
                    t: ta,
                    len,
                    shot: sa,
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
            )),
            _ => None,
        };

        // Graph + a parallel (node_a, node_b) -> shot map, built in the SAME loop
        // order as `build_survey_graph` so reconstructed node pairs map back to a
        // shot. Zero-cost merge/closure edges are intentionally absent here ⇒ a
        // reconstructed pair not in the map is a tie to skip (never tinted).
        let mut adj = self.build_survey_graph(n, &idx, Some(&coords), include_reference);
        let mut shot_edge: HashMap<(usize, usize), PathShot> = HashMap::new();
        for section in &self.sections {
            for (i, shot) in section.shots.iter().enumerate() {
                let from = StationRef::new(section.id, i as u32);
                let to = StationRef::new(section.id, (i + 1) as u32);
                if let (Some(&fi), Some(&ti)) = (idx.get(&from), idx.get(&to)) {
                    let ps = PathShot {
                        section_id: section.id,
                        shot_id: shot.id,
                    };
                    shot_edge.insert((fi, ti), ps);
                    shot_edge.insert((ti, fi), ps);
                }
            }
        }

        // Materialize an endpoint into a node index, appending a virtual node that
        // splits the shot edge into `t·L` / `(1-t)·L` when it's a shot-point. Both
        // partial legs map to the WHOLE shot so the partial end shot still tints.
        fn materialize(
            adj: &mut Vec<Vec<(usize, f64)>>,
            shot_edge: &mut HashMap<(usize, usize), PathShot>,
            r: Resolved,
        ) -> usize {
            match r {
                Resolved::Station(i) => i,
                Resolved::Shot {
                    from,
                    to,
                    t,
                    len,
                    shot,
                    swimmable,
                } => {
                    let v = adj.len();
                    // Mirror build_survey_graph: an unswimmable leg's halves cost
                    // INFINITY rather than being omitted, so the node still exists
                    // (and still tints) but no route can pass through it.
                    let (w_from, w_to) = if swimmable {
                        (t * len, (1.0 - t) * len)
                    } else {
                        (f64::INFINITY, f64::INFINITY)
                    };
                    adj.push(vec![(from, w_from), (to, w_to)]);
                    adj[from].push((v, w_from));
                    adj[to].push((v, w_to));
                    shot_edge.insert((v, from), shot);
                    shot_edge.insert((from, v), shot);
                    shot_edge.insert((v, to), shot);
                    shot_edge.insert((to, v), shot);
                    v
                }
            }
        }

        let start = materialize(&mut adj, &mut shot_edge, ra);
        let goal = materialize(&mut adj, &mut shot_edge, rb);

        // Same-shot direct sub-segment (see above): connect the two virtual nodes
        // directly with weight |Δt|·L so it competes with routing around via the
        // shared stations / any closure. Tagged with the shot so a direct pick still
        // tints it.
        if let Some((w, ps)) = same_shot_direct {
            adj[start].push((goal, w));
            adj[goal].push((start, w));
            shot_edge.insert((start, goal), ps);
            shot_edge.insert((goal, start), ps);
        }

        // Predecessor-tracking Dijkstra (the shared `dijkstra` returns distances
        // only). Same min-heap ordering as `dijkstra`.
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

        if !dist[goal].is_finite() {
            return none();
        }

        // Reconstruct the node chain start..goal and map consecutive pairs to
        // shots, skipping zero-cost merge/closure ties and deduping repeats.
        let mut chain = vec![goal];
        let mut cur = goal;
        while cur != start {
            let p = pred[cur];
            if p == usize::MAX {
                return none();
            }
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        let mut path_shots: Vec<PathShot> = Vec::new();
        for w in chain.windows(2) {
            // Skip a step that actually traversed a zero-cost merge/closure tie. When
            // a closure runs PARALLEL to a real shot (same station pair) `shot_edge`
            // maps the pair to the shot, but a ~0-cost step means Dijkstra took the
            // tie, not the shot - tinting it would be wrong. This also drops a
            // zero-length same-shot measurement (ta≈tb), whose direct edge has weight
            // 0, leaving an empty path like identical station endpoints (#26, Codex).
            // `w[0]` is the predecessor of `w[1]`, so the step weight is exactly the
            // distance delta.
            if dist[w[1]] - dist[w[0]] <= EPS {
                continue;
            }
            if let Some(ps) = shot_edge.get(&(w[0], w[1])) {
                if path_shots.last() != Some(ps) {
                    path_shots.push(*ps);
                }
            }
        }
        // Stations on the path = chain nodes that are real stations (virtual
        // shot-split nodes have index >= n and carry no StationRef).
        let mut path_stations: Vec<StationRef> = Vec::new();
        for &node in &chain {
            if node < n {
                let sr = stations[node].station_ref;
                if path_stations.last() != Some(&sr) {
                    path_stations.push(sr);
                }
            }
        }

        AlongLineResult {
            along_line: Some(dist[goal]),
            path_shots,
            path_stations,
        }
    }

    /// Build adjacency from aligned loop closures (bidirectional).
    pub(crate) fn build_aligned_closure_adjacency(
        &self,
        section_lookup: &HashMap<SectionId, &Section>,
    ) -> HashMap<StationRef, Vec<StationRef>> {
        let mut adjacency: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for closure in &self.loop_closures {
            if !closure.is_aligned {
                continue;
            }
            if !section_lookup.contains_key(&closure.station_a.section_id)
                || !section_lookup.contains_key(&closure.station_b.section_id)
            {
                continue;
            }
            if let (Some(a), Some(b)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) {
                adjacency.entry(a).or_default().push(b);
                adjacency.entry(b).or_default().push(a);
            }
        }
        adjacency
    }

    /// BFS reachability check: can `start` reach any zero datum station,
    /// traversing shots + merges + aligned closures, but excluding one specific shot edge?
    fn can_reach_datum_excluding_shot(
        &self,
        start: StationRef,
        exclude_section_id: SectionId,
        exclude_shot_index: usize,
        section_lookup: &HashMap<SectionId, &Section>,
        merge_adj: &HashMap<StationRef, Vec<StationRef>>,
        closure_adj: &HashMap<StationRef, Vec<StationRef>>,
        datum_stations: &HashSet<StationRef>,
    ) -> bool {
        if datum_stations.contains(&start) {
            return true;
        }

        let mut visited: HashSet<StationRef> = HashSet::new();
        let mut queue: VecDeque<StationRef> = VecDeque::new();
        visited.insert(start);
        queue.push_back(start);

        while let Some(node) = queue.pop_front() {
            let Some(section) = section_lookup.get(&node.section_id) else {
                continue;
            };

            // Adjacent stations via shots (backward: station_index - 1 via shot index-1)
            if node.station_index > 0 {
                let shot_idx = (node.station_index - 1) as usize;
                let is_excluded =
                    node.section_id == exclude_section_id && shot_idx == exclude_shot_index;
                if !is_excluded && shot_idx < section.shots.len() {
                    let neighbor = StationRef::new(node.section_id, node.station_index - 1);
                    if visited.insert(neighbor) {
                        if datum_stations.contains(&neighbor) {
                            return true;
                        }
                        queue.push_back(neighbor);
                    }
                }
            }

            // Adjacent stations via shots (forward: station_index + 1 via shot at station_index)
            if (node.station_index as usize) < section.shots.len() {
                let shot_idx = node.station_index as usize;
                let is_excluded =
                    node.section_id == exclude_section_id && shot_idx == exclude_shot_index;
                if !is_excluded {
                    let neighbor = StationRef::new(node.section_id, node.station_index + 1);
                    if visited.insert(neighbor) {
                        if datum_stations.contains(&neighbor) {
                            return true;
                        }
                        queue.push_back(neighbor);
                    }
                }
            }

            // Merge neighbors
            if let Some(neighbors) = merge_adj.get(&node) {
                for &neighbor in neighbors {
                    if visited.insert(neighbor) {
                        if datum_stations.contains(&neighbor) {
                            return true;
                        }
                        queue.push_back(neighbor);
                    }
                }
            }

            // Aligned closure neighbors
            if let Some(neighbors) = closure_adj.get(&node) {
                for &neighbor in neighbors {
                    if visited.insert(neighbor) {
                        if datum_stations.contains(&neighbor) {
                            return true;
                        }
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        false
    }

    /// Check if a specific shot is frozen.
    /// A shot is frozen when both its endpoints can reach a zero datum station
    /// (excluding the shot itself) via shots, merges, and aligned closures.
    pub(crate) fn is_shot_frozen(
        &self,
        section_id: SectionId,
        shot_index: usize,
        section_lookup: &HashMap<SectionId, &Section>,
        merge_adj: &HashMap<StationRef, Vec<StationRef>>,
        closure_adj: &HashMap<StationRef, Vec<StationRef>>,
        datum_stations: &HashSet<StationRef>,
    ) -> bool {
        if datum_stations.is_empty() {
            return false;
        }

        let station_a = StationRef::new(section_id, shot_index as u32);
        let station_b = StationRef::new(section_id, (shot_index + 1) as u32);

        self.can_reach_datum_excluding_shot(
            station_a,
            section_id,
            shot_index,
            section_lookup,
            merge_adj,
            closure_adj,
            datum_stations,
        ) && self.can_reach_datum_excluding_shot(
            station_b,
            section_id,
            shot_index,
            section_lookup,
            merge_adj,
            closure_adj,
            datum_stations,
        )
    }

    /// Convenience wrapper around [`Self::is_shot_frozen`] that resolves the
    /// shot's index from its id and builds the section lookup / adjacency /
    /// datum-station structures internally. Returns `false` if the section or
    /// shot id cannot be found. Call sites that need to check a single shot
    /// should prefer this over hand-rolling the adjacency setup.
    pub fn is_shot_frozen_by_id(&self, section_id: SectionId, shot_id: ShotId) -> bool {
        let Some(section) = self.sections.iter().find(|s| s.id == section_id) else {
            return false;
        };
        let Some(shot_index) = section.shots.iter().position(|s| s.id == shot_id) else {
            return false;
        };
        let section_lookup = self.build_section_lookup();
        let merge_adj = self.build_merge_adjacency(&section_lookup);
        let closure_adj = self.build_aligned_closure_adjacency(&section_lookup);
        let datum_stations: HashSet<StationRef> = self
            .zero_datums
            .iter()
            .filter_map(|d| self.resolve_key(&d.station_ref))
            .collect();
        self.is_shot_frozen(
            section_id,
            shot_index,
            &section_lookup,
            &merge_adj,
            &closure_adj,
            &datum_stations,
        )
    }

    /// Compute all frozen shots for the project.
    ///
    /// A shot is frozen when both its endpoints can reach a zero datum station
    /// (excluding the shot itself) via shots, merges, and aligned closures.
    ///
    /// Uses Tarjan's bridge-finding algorithm for O(V+E) total complexity instead
    /// of the naive O(shots × BFS) approach.
    pub fn compute_frozen_shots(&self) -> Vec<FrozenShotRef> {
        if self.zero_datums.is_empty() {
            return Vec::new();
        }

        let section_lookup = self.build_section_lookup();
        let merge_adj = self.build_merge_adjacency(&section_lookup);
        let closure_adj = self.build_aligned_closure_adjacency(&section_lookup);
        let datum_stations: HashSet<StationRef> = self
            .zero_datums
            .iter()
            .filter_map(|d| self.resolve_key(&d.station_ref))
            .collect();

        // Assign each station a compact integer index
        let mut station_to_idx: HashMap<StationRef, usize> = HashMap::new();
        let mut idx_to_station: Vec<StationRef> = Vec::new();
        for section in &self.sections {
            for i in 0..=section.shots.len() {
                let sr = StationRef::new(section.id, i as u32);
                let idx = idx_to_station.len();
                station_to_idx.insert(sr, idx);
                idx_to_station.push(sr);
            }
        }
        let v = idx_to_station.len();
        if v == 0 {
            return Vec::new();
        }

        // Edge descriptor: (neighbor_idx, is_shot, section_id, shot_index)
        // is_shot=true means this edge corresponds to a shot that could be frozen.
        let mut adj: Vec<Vec<(usize, bool, SectionId, u32)>> = vec![Vec::new(); v];

        // Add shot edges
        for section in &self.sections {
            for shot_index in 0..section.shots.len() {
                let a = StationRef::new(section.id, shot_index as u32);
                let b = StationRef::new(section.id, (shot_index + 1) as u32);
                if let (Some(&ai), Some(&bi)) = (station_to_idx.get(&a), station_to_idx.get(&b)) {
                    adj[ai].push((bi, true, section.id, shot_index as u32));
                    adj[bi].push((ai, true, section.id, shot_index as u32));
                }
            }
        }

        // Add merge edges
        for (station, neighbors) in &merge_adj {
            if let Some(&si) = station_to_idx.get(station) {
                for neighbor in neighbors {
                    if let Some(&ni) = station_to_idx.get(neighbor) {
                        adj[si].push((ni, false, 0, 0));
                    }
                }
            }
        }

        // Add aligned closure edges
        for (station, neighbors) in &closure_adj {
            if let Some(&si) = station_to_idx.get(station) {
                for neighbor in neighbors {
                    if let Some(&ni) = station_to_idx.get(neighbor) {
                        adj[si].push((ni, false, 0, 0));
                    }
                }
            }
        }

        // Tarjan's bridge-finding + datum counting in subtrees.
        // disc[u] = discovery time, low[u] = lowest disc reachable via back-edges.
        // An edge (u, v) with disc[u] < disc[v] is a bridge iff low[v] > disc[u].
        // We also count datums in each DFS subtree to decide bridge-frozen status.
        let mut disc = vec![0u32; v];
        let mut low = vec![0u32; v];
        let mut visited = vec![false; v];
        let mut subtree_datums = vec![0u32; v]; // datums in DFS subtree rooted at node
        let mut dfs_parent = vec![usize::MAX; v];
        let mut is_bridge = vec![false; v]; // indexed by child node of the bridge edge
        let mut component_datums = vec![0u32; v]; // total datums in the component
        let mut component_root = vec![usize::MAX; v];

        let is_datum: Vec<bool> = idx_to_station
            .iter()
            .map(|sr| datum_stations.contains(sr))
            .collect();

        let mut timer: u32 = 1;

        // Iterative Tarjan's DFS to avoid stack overflow on large graphs.
        // We track whether the tree edge to the parent has been skipped
        // (parent_edge_skipped) to handle multigraph edges correctly:
        // if A-B are connected by both a shot and a merge, only one edge
        // is the tree edge; the others are back edges that prevent bridging.
        for start in 0..v {
            if visited[start] {
                continue;
            }

            // DFS stack: (node, adj_index, parent_edge_skipped)
            let mut stack: Vec<(usize, usize, bool)> = Vec::new();
            visited[start] = true;
            disc[start] = timer;
            low[start] = timer;
            timer += 1;
            subtree_datums[start] = if is_datum[start] { 1 } else { 0 };
            component_root[start] = start;
            stack.push((start, 0, true)); // root has no parent edge to skip

            while let Some((u, ei, parent_skipped)) = stack.last_mut() {
                let u = *u;
                if *ei < adj[u].len() {
                    let (w, _, _, _) = adj[u][*ei];
                    *ei += 1;
                    if !visited[w] {
                        visited[w] = true;
                        disc[w] = timer;
                        low[w] = timer;
                        timer += 1;
                        subtree_datums[w] = if is_datum[w] { 1 } else { 0 };
                        dfs_parent[w] = u;
                        component_root[w] = start;
                        stack.push((w, 0, false));
                    } else if w == dfs_parent[u] && !*parent_skipped {
                        // First edge back to parent = tree edge, skip it once
                        *parent_skipped = true;
                    } else {
                        // Back/cross edge (or second+ edge to parent): update low
                        low[u] = low[u].min(disc[w]);
                    }
                } else {
                    // Done with u, propagate to parent
                    let u_low = low[u];
                    let u_datums = subtree_datums[u];
                    stack.pop();
                    if let Some((parent, _, _)) = stack.last() {
                        let parent = *parent;
                        low[parent] = low[parent].min(u_low);
                        subtree_datums[parent] += u_datums;
                        // Check if edge (parent, u) is a bridge
                        if u_low > disc[parent] {
                            is_bridge[u] = true;
                        }
                    }
                }
            }

            // Record total datums for this component
            component_datums[start] = subtree_datums[start];
        }

        // Now determine which shots are frozen:
        // - Non-bridge shot: frozen if its component has ≥1 datum
        // - Bridge shot (parent→child): frozen if both sides have datums.
        //   child-side has subtree_datums[child], other side has
        //   component_datums[root] - subtree_datums[child].
        let mut frozen = Vec::new();
        for section in &self.sections {
            for shot_index in 0..section.shots.len() {
                let a = StationRef::new(section.id, shot_index as u32);
                let b = StationRef::new(section.id, (shot_index + 1) as u32);
                let (Some(&ai), Some(&bi)) = (station_to_idx.get(&a), station_to_idx.get(&b))
                else {
                    continue;
                };

                let root = component_root[ai];
                let total = component_datums[root];
                if total == 0 {
                    continue;
                }

                // Determine if this shot edge is a bridge
                let (child, is_bridge_edge) = if dfs_parent[bi] == ai && is_bridge[bi] {
                    (bi, true)
                } else if dfs_parent[ai] == bi && is_bridge[ai] {
                    (ai, true)
                } else {
                    (0, false)
                };

                if is_bridge_edge {
                    // Bridge: frozen only if both sides have datums
                    let child_datums = subtree_datums[child];
                    let other_datums = total - child_datums;
                    if child_datums > 0 && other_datums > 0 {
                        frozen.push(FrozenShotRef {
                            section_id: section.id,
                            shot_index: shot_index as u32,
                        });
                    }
                } else {
                    // Non-bridge in a component with datums: frozen
                    frozen.push(FrozenShotRef {
                        section_id: section.id,
                        shot_index: shot_index as u32,
                    });
                }
            }
        }
        frozen
    }

    /// Find orphaned connected components and suggest placements for their leaf
    /// ends against datum-anchored sections.
    ///
    /// An orphaned component is a set of sections connected via merges that has
    /// no zero datum. Leaf ends are terminal stations with at least 3 free shots
    /// (shots not adjacent to a merge or aligned closure).
    ///
    /// Which anchored sections an orphan may be placed onto depends on whether
    /// THIS import touched it:
    /// - an orphan containing a newly imported section may go onto ANY anchored
    ///   section, pre-existing ones included - adding a datum-less `.stalx`
    ///   into an anchored project must be offered a spot on the host (#674);
    /// - a pre-existing orphan may only go onto a newly imported anchored
    ///   section, so an unrelated later import never re-raises a pairing the
    ///   user already had their chance to act on (#446).
    pub fn suggest_orphan_placements(
        &self,
        newly_imported_section_ids: &[SectionId],
        ps_config: &crate::config::PlacementSuggestionConfig,
    ) -> Vec<super::placement_suggest::OrphanPlacementSuggestion> {
        use super::placement_suggest::{
            suggest_placements, ExistingSection, OrphanPlacementSuggestion,
        };

        if newly_imported_section_ids.is_empty() {
            return Vec::new();
        }

        // Cheap early-out: nothing in the project resolves to a zero datum at
        // all, so no component can ever be a valid suggestion TARGET (targets
        // must themselves be datum-anchored - see the per-component filter
        // below). Without this, a datum-less project makes EVERY component an
        // "orphan" and the O(components x sections) walk below runs for
        // nothing (#446).
        if !self
            .zero_datums
            .iter()
            .any(|zd| self.resolve_key(&zd.station_ref).is_some())
        {
            return Vec::new();
        }

        // 1. Build all connected components (via merges only), over the WHOLE
        // post-import project - not just pre-existing sections - so a first
        // import into an empty project can suggest an unanchored component
        // against an anchored one from the SAME import (#446).
        let components = self.connected_components();

        // 2. Split them into orphans (no datum) and anchored components, one
        // datum check per component. An empty component - impossible here -
        // is neither. The two sets are disjoint by construction, which is
        // what keeps a component from being suggested onto itself: every
        // target below comes from an anchored component, never an orphan one.
        let mut orphan_components: Vec<&HashSet<SectionId>> = Vec::new();
        let mut anchored_section_ids: HashSet<SectionId> = HashSet::new();
        for comp in &components {
            let Some(&any_member) = comp.iter().next() else {
                continue;
            };
            if self.rigid_group_has_datum(any_member) {
                anchored_section_ids.extend(comp.iter().copied());
            } else {
                orphan_components.push(comp);
            }
        }

        if orphan_components.is_empty() || anchored_section_ids.is_empty() {
            return Vec::new();
        }

        // 3. Build the two target pools. Targets must be datum-anchored:
        // without that, two distinct floating components can be suggested onto
        // each other - a "placement" that is circular and anchors nothing
        // (#446).
        let new_ids: HashSet<SectionId> = newly_imported_section_ids.iter().copied().collect();
        let to_target = |s: &Section| ExistingSection {
            id: s.id,
            name: s
                .name
                .clone()
                .unwrap_or_else(|| format!("Section {}", s.id)),
            shots: s.shots.clone(),
            azimuth_reference: self.azimuth_reference_for(s),
        };
        // Anchored sections from THIS import - the only targets a
        // pre-existing orphan may use (#446).
        let new_anchored_targets: Vec<ExistingSection> = newly_imported_section_ids
            .iter()
            .filter(|id| anchored_section_ids.contains(id))
            .filter_map(|&id| self.get_section(id))
            .map(to_target)
            .collect();
        // Every anchored section in the project - the targets for an orphan
        // this import touched (#674). Only built when such an orphan exists,
        // so an import that leaves nothing floating never copies the host.
        let import_left_an_orphan = orphan_components
            .iter()
            .any(|comp| comp.iter().any(|id| new_ids.contains(id)));
        let all_anchored_targets: Vec<ExistingSection> = if import_left_an_orphan {
            self.sections
                .iter()
                .filter(|s| anchored_section_ids.contains(&s.id))
                .map(to_target)
                .collect()
        } else {
            Vec::new()
        };

        // Helper: check if a station has any merge
        let station_has_merge = |sr: StationRef| -> bool {
            self.station_merges
                .iter()
                .any(|m| self.key_is_ref(&m.station_a, sr) || self.key_is_ref(&m.station_b, sr))
        };

        // Helper: check if a station has any aligned closure
        let station_has_aligned_closure = |sr: StationRef| -> bool {
            self.loop_closures.iter().any(|c| {
                c.is_aligned
                    && (self.key_is_ref(&c.station_a, sr) || self.key_is_ref(&c.station_b, sr))
            })
        };

        // Helper: check if a station is constrained (merge or aligned closure)
        let station_is_constrained =
            |sr: StationRef| -> bool { station_has_merge(sr) || station_has_aligned_closure(sr) };

        let mut suggestions: Vec<OrphanPlacementSuggestion> = Vec::new();

        // 4. For each orphan component, pick its target pool and find leaf
        // candidates.
        for component in &orphan_components {
            let touched_by_import = component.iter().any(|id| new_ids.contains(id));
            let component_targets: &[ExistingSection] = if touched_by_import {
                &all_anchored_targets
            } else {
                &new_anchored_targets
            };
            if component_targets.is_empty() {
                continue;
            }
            for &section_id in *component {
                let section = match self.get_section(section_id) {
                    Some(s) => s,
                    None => continue,
                };
                if section.shots.is_empty() {
                    continue;
                }

                let section_name = section
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Section {}", section_id));
                let n_shots = section.shots.len();

                // Try both ends: station 0 and station N
                let ends: [(u32, bool); 2] = [(0, true), (n_shots as u32, false)];

                for &(terminal_station_idx, is_start) in &ends {
                    let terminal = StationRef {
                        section_id,
                        station_index: terminal_station_idx,
                    };

                    // Skip if this terminal station has a merge (not a leaf)
                    if station_has_merge(terminal) {
                        continue;
                    }

                    // Walk inward counting free shots until hitting a constraint
                    let mut free_count = 0usize;
                    if is_start {
                        // Station 0: walk shots 0, 1, 2, ...
                        for shot_idx in 0..n_shots {
                            // Check the far station of this shot
                            let far_station = StationRef {
                                section_id,
                                station_index: (shot_idx + 1) as u32,
                            };
                            if station_is_constrained(far_station) {
                                break;
                            }
                            free_count += 1;
                        }
                    } else {
                        // Station N: walk shots N-1, N-2, ...
                        for shot_idx in (0..n_shots).rev() {
                            // Check the far station of this shot (going inward)
                            let far_station = StationRef {
                                section_id,
                                station_index: shot_idx as u32,
                            };
                            if station_is_constrained(far_station) {
                                break;
                            }
                            free_count += 1;
                        }
                    }

                    if free_count < 3 {
                        continue;
                    }

                    let max_shots = (free_count - 1).min(5);

                    // Collect the shots as ShotData
                    let leaf_shots: Vec<ShotData> = if is_start {
                        section.shots[..max_shots]
                            .iter()
                            .map(|s| s.to_shot_data())
                            .collect()
                    } else {
                        section.shots[n_shots - max_shots..]
                            .iter()
                            .rev()
                            .map(|s| {
                                let mut sd = s.to_shot_data();
                                // Reverse shot: flip azimuth 180°, swap depths
                                sd.azimuth_in = (sd.azimuth_in + 180.0) % 360.0;
                                sd.azimuth_out = (sd.azimuth_out + 180.0) % 360.0;
                                std::mem::swap(&mut sd.depth_in, &mut sd.depth_out);
                                sd
                            })
                            .collect()
                    };

                    // 5. Run suggest_placements for this leaf against the
                    // component's targets. The "import" side here is an existing
                    // ORPHAN section, so its own reference is on record (#438) -
                    // unlike the wizard path.
                    let placement_suggestions = suggest_placements(
                        &leaf_shots,
                        self.azimuth_reference_for(section),
                        component_targets,
                        ps_config,
                    );

                    for ps in placement_suggestions {
                        // Only keep matches from the terminal end (import station 0).
                        // suggest_placements tries both ends of the leaf, but for orphan
                        // placement only the terminal end is meaningful - matches from
                        // the interior end would produce wrong orphan station indices.
                        if ps.import_station_index != 0 {
                            continue;
                        }
                        suggestions.push(OrphanPlacementSuggestion {
                            orphan_section_id: section_id,
                            orphan_section_name: section_name.clone(),
                            orphan_station_index: terminal_station_idx as usize,
                            target_section_id: ps.existing_section_id,
                            target_section_name: ps.existing_section_name,
                            target_station_index: ps.existing_station_index,
                            score: ps.score,
                            matched_shots: ps.matched_shots,
                            existing_matched_shots: ps.existing_matched_shots,
                            preferred_merge_offset: ps.preferred_merge_offset,
                            direction: ps.direction,
                            is_partial: ps.is_partial,
                            confidence: ps.confidence,
                        });
                    }
                }
            }
        }

        // Sort by score descending
        suggestions.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));

        suggestions
    }

    /// Build all connected components of the project's sections (via merges
    /// only), covering every section exactly once. Shared by
    /// [`Self::suggest_orphan_placements`] and [`Self::count_unanchored_sections`]
    /// so the two walks cannot drift apart.
    fn connected_components(&self) -> Vec<HashSet<SectionId>> {
        let mut visited: HashSet<SectionId> = HashSet::new();
        let mut components: Vec<HashSet<SectionId>> = Vec::new();

        for section in &self.sections {
            if visited.contains(&section.id) {
                continue;
            }
            let component = self.get_merged_section_ids(section.id);
            for &sid in &component {
                visited.insert(sid);
            }
            components.push(component);
        }

        components
    }

    /// Count sections not connected to a zero datum (directly or through
    /// merges), restricted to components that contain at least one of
    /// `newly_imported_section_ids`. Used to add an info line to the import
    /// receipt when an import leaves unanchored data and the
    /// placement-suggestion pass could not offer anything to fix it - scoped
    /// to THIS import so a clean import into a project with unrelated,
    /// pre-existing orphans doesn't blame this receipt for orphans it didn't
    /// create (#446).
    pub fn count_unanchored_sections(&self, newly_imported_section_ids: &[SectionId]) -> u32 {
        let new_set: HashSet<SectionId> = newly_imported_section_ids.iter().copied().collect();

        self.connected_components()
            .iter()
            .filter(|comp| comp.iter().any(|sid| new_set.contains(sid)))
            .filter(|comp| {
                let Some(&any_member) = comp.iter().next() else {
                    return false;
                };
                !self.rigid_group_has_datum(any_member)
            })
            .map(|comp| comp.len() as u32)
            .sum()
    }
}
