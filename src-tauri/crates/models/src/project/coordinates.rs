use std::collections::{HashMap, HashSet, VecDeque};
use tracing::warn;

use crate::config::ClosureAdjustmentConfig;
use crate::{ClosureRef, ClosureStrategy, Coordinates, Section, SectionId, Station, StationRef};

use super::Project;

impl Project {
    /// Get all stations colocated with the given station (via merges and aligned closures).
    ///
    /// Returns a vec including the station itself plus all merge/closure partners,
    /// transitively connected.
    fn get_colocated_stations(&self, station_ref: StationRef) -> Vec<StationRef> {
        // Colocation adjacency = merges + ALIGNED closures, every endpoint
        // resolved once (O(edges)). BFS over it is O(edges + group) instead of
        // rescanning all merges/closures at each BFS node - the latter made
        // coordinate/closure work on a large merged component O(group × edges).
        let mut adjacency = self.build_station_merge_adjacency();
        for closure in &self.loop_closures {
            if !closure.is_aligned {
                continue;
            }
            if let (Some(sa), Some(sb)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) {
                adjacency.entry(sa).or_default().push(sb);
                adjacency.entry(sb).or_default().push(sa);
            }
        }

        let mut group = vec![station_ref];
        let mut visited = HashSet::new();
        visited.insert(station_ref);
        let mut queue = VecDeque::new();
        queue.push_back(station_ref);

        while let Some(current) = queue.pop_front() {
            let Some(neighbors) = adjacency.get(&current) else {
                continue;
            };
            for &p in neighbors {
                if visited.insert(p) {
                    group.push(p);
                    queue.push_back(p);
                }
            }
        }

        group
    }

    /// Compute a station's absolute depth from the average of surrounding shot depths.
    ///
    /// For a station and its colocated group (merge + aligned closure partners),
    /// collects depth measurements from:
    /// - **Incoming shot** (shot at index `station_index - 1`): use `depth_out`
    /// - **Outgoing shot** (shot at index `station_index`): use `depth_in`
    ///
    /// Returns the average of all collected measurements, or 0.0 if none exist.
    ///
    /// [#469] The sum runs in a CANONICAL order, not the caller's. Floating-point
    /// addition is not associative, so the same group summed in two orders can differ
    /// in the last bit - and the callers genuinely disagree about order:
    /// [`Self::compute_station_raw_coords`] hands over the BFS order out of
    /// `get_colocated_stations`, while [`Self::compute_component_raw_coords`] and
    /// `build_colocated_groups` hand over station-list order. Each was deterministic
    /// per run; between them the same station could come out one ulp apart depending on
    /// which API asked. Sorting at this single shared site makes them bit-identical.
    /// Group MEMBERSHIP is untouched - this only fixes the order it is added in.
    fn compute_station_depth(&self, colocated: &[StationRef]) -> f64 {
        // A one-station group (every unmerged station, i.e. nearly all of them on a big
        // project) has no ordering question, so it keeps the allocation-free path: this
        // runs once per group per full coordinate rebuild.
        let sorted_group: Vec<StationRef>;
        let colocated: &[StationRef] = if colocated.len() < 2 {
            colocated
        } else {
            let mut group = colocated.to_vec();
            group.sort_unstable_by_key(|sr| (sr.section_id, sr.station_index));
            sorted_group = group;
            &sorted_group
        };

        let mut depths: Vec<f64> = Vec::new();

        for &sr in colocated {
            // O(1) cache lookup; legacy linear scan was the hot path -
            // 79 k groups × O(N sections) per `compute_station_coordinates`
            // call dominated load time for large projects.
            if let Some(section) = self.get_section(sr.section_id) {
                let idx = sr.station_index as usize;

                // Incoming shot: shot at index station_index - 1 → use depth_out
                if idx > 0 && idx - 1 < section.shots.len() {
                    depths.push(section.shots[idx - 1].depth_out);
                }

                // Outgoing shot: shot at index station_index → use depth_in
                if idx < section.shots.len() {
                    depths.push(section.shots[idx].depth_in);
                }
            }
        }

        if depths.is_empty() {
            0.0
        } else {
            depths.iter().sum::<f64>() / depths.len() as f64
        }
    }

    /// Compute raw station coordinates for a single station (without global zero datum offset)
    pub fn compute_station_raw_coords(&self, station_ref: StationRef) -> Option<Coordinates> {
        let section = self.get_section(station_ref.section_id)?;

        // Get section offset
        let (offset_x, offset_y) = self
            .section_offsets
            .get(&section.id)
            .copied()
            .unwrap_or((0.0, 0.0));
        let mut coords = Coordinates::new(offset_x, offset_y, 0.0);
        // Per-section constant (#438): resolved once, added per shot.
        let azimuth_reference = self.azimuth_reference_for(section);

        // Walk through shots to reach the station (X, Y only)
        for i in 0..station_ref.station_index as usize {
            if i >= section.shots.len() {
                return None;
            }
            let shot = &section.shots[i];
            let azimuth_rad = shot.average_azimuth_true_rad(azimuth_reference);
            let horizontal = shot.horizontal_length();
            let dx = horizontal * azimuth_rad.sin();
            let dy = horizontal * azimuth_rad.cos();
            coords.x += dx;
            coords.y += dy;
        }

        // Z = absolute depth from averaged surrounding shots
        let colocated = self.get_colocated_stations(station_ref);
        coords.z = self.compute_station_depth(&colocated);

        // Apply station offset if exists (individual station move)
        if let Some((ox, oy, oz)) = self.station_offset_for_ref(station_ref) {
            coords.x += ox;
            coords.y += oy;
            coords.z += oz;
        }

        Some(coords)
    }

    /// World `(x, y)` of a single station via the shot-walk only - the same x/y
    /// [`Self::compute_station_raw_coords`] returns, but WITHOUT the
    /// `get_colocated_stations` colocation BFS that computes Z. For callers that
    /// only need the horizontal position (e.g. merge offset alignment), this
    /// avoids an O(group × merges) traversal per call - the dominant cost when
    /// merging the thousands of stations of a large import.
    pub(crate) fn compute_station_raw_xy(&self, station_ref: StationRef) -> Option<(f64, f64)> {
        let section = self.get_section(station_ref.section_id)?;
        let (offset_x, offset_y) = self
            .section_offsets
            .get(&section.id)
            .copied()
            .unwrap_or((0.0, 0.0));
        let (mut x, mut y) = (offset_x, offset_y);
        let azimuth_reference = self.azimuth_reference_for(section);
        for i in 0..station_ref.station_index as usize {
            let shot = section.shots.get(i)?;
            let azimuth_rad = shot.average_azimuth_true_rad(azimuth_reference);
            let horizontal = shot.horizontal_length();
            x += horizontal * azimuth_rad.sin();
            y += horizontal * azimuth_rad.cos();
        }
        if let Some((ox, oy, _)) = self.station_offset_for_ref(station_ref) {
            x += ox;
            y += oy;
        }
        Some((x, y))
    }

    /// Coordinate-only, component-scoped batch of raw (measured) station
    /// coordinates - the same values [`Self::compute_station_raw_coords`] returns,
    /// computed for every station in `component` in one linear pass.
    ///
    /// The closure solve runs this once per connected component, so it deliberately
    /// avoids whole-project [`Self::compute_station_coordinates`], which would be
    /// (a) `O(component_count × total_stations)` across the per-component loop and
    /// (b) `O(closures × stations)` from rebuilding the `merged_with`/`closures_with`
    /// metadata the solver never reads. Merges and aligned closures only connect
    /// stations within one connected component (see
    /// [`Self::get_connected_section_ids`]), so scoping the colocated depth grouping
    /// to `component` is exact.
    pub(super) fn compute_component_raw_coords(
        &self,
        component: &HashSet<SectionId>,
    ) -> HashMap<StationRef, Coordinates> {
        let station_list: Vec<StationRef> = self
            .sections
            .iter()
            .filter(|s| component.contains(&s.id))
            .flat_map(|s| (0..=s.shots.len()).map(move |idx| StationRef::new(s.id, idx as u32)))
            .collect();

        // Colocated depth groups (merges + aligned closures), restricted to the
        // component. Simple union-find with path compression.
        fn find(parent: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
            let mut cur = x;
            while let Some(&p) = parent.get(&cur) {
                if p == cur {
                    break;
                }
                cur = p;
            }
            let mut node = x;
            while let Some(&p) = parent.get(&node) {
                if p == cur {
                    break;
                }
                parent.insert(node, cur);
                node = p;
            }
            cur
        }
        let mut parent: HashMap<StationRef, StationRef> =
            station_list.iter().map(|&sr| (sr, sr)).collect();
        let unite = |parent: &mut HashMap<StationRef, StationRef>, a, b| {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        };
        for merge in &self.station_merges {
            if component.contains(&merge.station_a.section_id)
                && component.contains(&merge.station_b.section_id)
            {
                if let (Some(a), Some(b)) = (
                    self.resolve_key(&merge.station_a),
                    self.resolve_key(&merge.station_b),
                ) {
                    unite(&mut parent, a, b);
                }
            }
        }
        for closure in &self.loop_closures {
            if !closure.is_aligned {
                continue;
            }
            if component.contains(&closure.station_a.section_id)
                && component.contains(&closure.station_b.section_id)
            {
                if let (Some(a), Some(b)) = (
                    self.resolve_key(&closure.station_a),
                    self.resolve_key(&closure.station_b),
                ) {
                    unite(&mut parent, a, b);
                }
            }
        }
        let mut groups: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for &sr in &station_list {
            let root = find(&mut parent, sr);
            groups.entry(root).or_default().push(sr);
        }
        let mut depth: HashMap<StationRef, f64> = HashMap::with_capacity(station_list.len());
        for members in groups.values() {
            let d = self.compute_station_depth(members);
            for &sr in members {
                depth.insert(sr, d);
            }
        }

        // Per-section XY walk + individual station offsets.
        let mut out: HashMap<StationRef, Coordinates> = HashMap::with_capacity(station_list.len());
        for section in self.sections.iter().filter(|s| component.contains(&s.id)) {
            let (offset_x, offset_y) = self
                .section_offsets
                .get(&section.id)
                .copied()
                .unwrap_or((0.0, 0.0));
            let mut cur = (offset_x, offset_y);
            let azimuth_reference = self.azimuth_reference_for(section);
            for idx in 0..=section.shots.len() {
                if idx > 0 {
                    let shot = &section.shots[idx - 1];
                    let az = shot.average_azimuth_true_rad(azimuth_reference);
                    let h = shot.horizontal_length();
                    cur.0 += h * az.sin();
                    cur.1 += h * az.cos();
                }
                let sr = StationRef::new(section.id, idx as u32);
                let z = depth.get(&sr).copied().unwrap_or(0.0);
                let mut coords = Coordinates::new(cur.0, cur.1, z);
                if let Some((ox, oy, oz)) = self.station_offset_for_ref(sr) {
                    coords.x += ox;
                    coords.y += oy;
                    coords.z += oz;
                }
                out.insert(sr, coords);
            }
        }
        out
    }

    /// Compute all station coordinates from shots.
    /// Backward-compat wrapper that calls `compute_station_coordinates_with_progress(None)`.
    pub fn compute_station_coordinates(&self) -> Vec<Station> {
        self.compute_station_coordinates_with_progress(None)
    }
    /// Pick a deterministic anchor station for a component that has no zero datum.
    ///
    /// The anchor is two things at once, and only the first is a pure gauge:
    ///
    /// * it fixes the otherwise-free absolute translation of a datum-less
    ///   component, by pinning that one station at its MEASURED coordinates
    ///   (`adjust_least_squares`'s `fixed_groups` fallback);
    /// * it is also a member of the spur-prune `pinned` set in the same solve, so
    ///   moving it can change which stations count as spurs and are skipped by the
    ///   update phase. Making it a pure gauge is tracked separately.
    ///
    /// Either way the pin has to land on the same PHYSICAL station every time, or
    /// the adjusted view jumps.
    ///
    /// Picking the lex-min `(section_id, station_index)` did not do that. A
    /// section id survives every edit, but a station INDEX does not: reversing a
    /// section renumbers station `i` to `N - i`, so `(S, 0)` becomes the far
    /// physical end and the whole adjusted component translates by the adjustment
    /// residual - breaking the reverse's "every station keeps its position"
    /// guarantee exactly where closure-adjusted geometry is forced (Plan/Export).
    ///
    /// So the station is named the way the plan anchors name one: by the SHOTS it sits
    /// between, not by an index. The pair is captured once - on the first save that sees the
    /// section - and travels with the records, so it is unaffected by a reverse, which carries
    /// both shots together, and by the section being extended at either end, which moves
    /// neither relative to the other. Capturing records the station the section is ALREADY
    /// using, so it never moves anybody's coordinates.
    ///
    /// **Residual.** A single-shot section has no second shot to face away from, so its side
    /// comes from [`Section::reverse_parity`] instead - exact, because a reverse is the only
    /// thing that swaps such a section's two ends. Losing the captured neighbour degrades to
    /// the same rule, and losing the captured shot outright falls back to the legacy index,
    /// which is the pre-capture behaviour.
    ///
    /// The captured pair and the parity flag both persist, so the choice is reproducible
    /// across reloads.
    ///
    /// A station an edit held for the component ([`Self::held_gauges`]) comes
    /// first: a split, combine or delete-and-split that left the component's
    /// station outside its lowest-id section records it there, so the edit
    /// does not move the adjusted component (`project/gauge.rs`).
    pub(super) fn default_anchor_for_component(
        &self,
        component: &HashSet<SectionId>,
    ) -> Option<StationRef> {
        if let Some(at) = self
            .held_gauges
            .iter()
            .filter_map(|held| self.resolve_held_gauge(held))
            .find(|at| component.contains(&at.section_id))
        {
            return Some(at);
        }
        let section = self
            .sections
            .iter()
            .filter(|s| component.contains(&s.id))
            .min_by_key(|s| s.id)?;
        let index = Self::gauge_index(section);
        Some(StationRef::new(section.id, index as u32))
    }

    /// Where the gauge sits in `section` right now.
    ///
    /// Prefers the captured pair (see [`Section::gauge_shot_uuid`]): the station is the end of
    /// that shot facing AWAY from its recorded neighbour, worked out from their current
    /// positions. Neither moves relative to the other when the section is extended, and a
    /// reverse carries both, so the same junction is named throughout.
    ///
    /// Falls back to the index rule for a section whose pair was never captured (nothing has
    /// been saved yet) or whose recorded shots are gone. That fallback is what an
    /// already-existing project uses until its first save, and it deliberately reproduces the
    /// legacy answer - station 0 - so upgrading moves nobody.
    pub(super) fn gauge_index(section: &Section) -> usize {
        let position_of = |uuid: &Option<String>| -> Option<usize> {
            let uuid = uuid.as_deref()?;
            section
                .shots
                .iter()
                .position(|s| s.uuid.as_deref() == Some(uuid))
        };
        match (
            position_of(&section.gauge_shot_uuid),
            position_of(&section.gauge_neighbour_uuid),
        ) {
            // The neighbour is behind the gauge shot, so the gauge station is in front of it.
            (Some(px), Some(py)) if py < px => px + 1,
            (Some(px), Some(_)) => px,
            // The captured shot survives but has no recorded neighbour - a single-shot section,
            // or one whose neighbour was deleted. Position cannot decide the side: for a
            // one-shot section BOTH ends touch that shot, and reversing it swaps them while
            // leaving the shot at position 0. Read the side off the orientation instead, the
            // way the plan anchors do.
            //
            // The CURRENT parity is sufficient, and not because capture always happens with it
            // false - it can just as well capture a section that is already reversed. It works
            // because capture picks the shot the parity rule already names (parity false takes
            // `shots[0]`, parity true takes the last), so the pair and the flag agree at
            // capture; and a reverse moves that shot to the opposite end AND flips the flag,
            // so they keep agreeing however many times it happens.
            (Some(px), None) if section.reverse_parity => px + 1,
            (Some(px), None) => px,
            _ => {
                if section.reverse_parity {
                    section.shots.len()
                } else {
                    0
                }
            }
        }
    }

    /// Capture the gauge pair for every section that does not have one, so the choice becomes
    /// durable instead of being re-derived from an index on each solve. Called on save, where
    /// the project is already mutable.
    ///
    /// Records the pair that names the gauge station the section has RIGHT NOW, so capturing
    /// never moves it - an existing project is pinned to exactly the station it was already
    /// using.
    pub fn capture_missing_gauge_anchors(&mut self) {
        for section in &mut self.sections {
            if section.gauge_shot_uuid.is_some() || section.shots.is_empty() {
                continue;
            }
            let last = section.shots.len() - 1;
            let gauge = if section.reverse_parity {
                section.shots.len()
            } else {
                0
            };
            // The shot the gauge station touches, and the one beyond it that gives it a side.
            let (x, y) = if gauge == 0 {
                (0usize, 1usize)
            } else {
                (last, last.wrapping_sub(1))
            };
            section.gauge_shot_uuid = section.shots.get(x).and_then(|s| s.uuid.clone());
            section.gauge_neighbour_uuid = if x == y {
                None
            } else {
                section.shots.get(y).and_then(|s| s.uuid.clone())
            };
        }
    }

    /// The gauge station `section_id`'s component would use, for tests that need to assert it
    /// survives a save/reload rather than only a reverse in memory.
    #[cfg(any(test, feature = "test-support"))]
    pub fn adjustment_anchor_for_tests(&self, section_id: SectionId) -> Option<StationRef> {
        let component = self.get_connected_section_ids(section_id);
        self.default_anchor_for_component(&component)
    }

    /// Whether a loop closure participates in the least-squares adjustment.
    /// Mirrors the strategy filter inside [`Self::adjust_loop_closures`]: `Move`
    /// closures are already coincident (snap-to-merge) and the deferred
    /// proportional/equal strategies are not solved.
    fn closure_is_eligible(strategy: ClosureStrategy) -> bool {
        matches!(
            strategy,
            ClosureStrategy::Batch | ClosureStrategy::LeastSquaresWeighted
        )
    }

    /// Plan which component anchors to feed the closure-adjustment solve.
    ///
    /// Returns one anchor per connected component that contains an eligible
    /// closure: a zero datum in the component when present, otherwise a
    /// deterministic default anchor (see [`Self::default_anchor_for_component`]).
    /// Computed from the *measured* topology so it can run once up front, before
    /// any adjustment mutates the working copy.
    fn plan_adjustment_anchors(&self) -> Vec<StationRef> {
        let mut anchors = Vec::new();
        let mut visited: HashSet<SectionId> = HashSet::new();
        for section in &self.sections {
            if visited.contains(&section.id) {
                continue;
            }
            let component = self.get_connected_section_ids(section.id);
            for sid in &component {
                visited.insert(*sid);
            }

            let has_eligible = self.loop_closures.iter().any(|lc| {
                Self::closure_is_eligible(lc.strategy)
                    && component.contains(&lc.station_a.section_id)
                    && component.contains(&lc.station_b.section_id)
            });
            if !has_eligible {
                continue;
            }

            // Anchor on a zero datum in the component when present, otherwise a
            // deterministic default so the gauge is reproducible across reloads.
            let anchor = self
                .zero_datums
                .iter()
                .find(|zd| component.contains(&zd.station_ref.section_id))
                .and_then(|zd| self.resolve_key(&zd.station_ref))
                .or_else(|| self.default_anchor_for_component(&component));

            if let Some(anchor) = anchor {
                anchors.push(anchor);
            }
        }
        anchors
    }

    /// Compute station coordinates with loop-closure adjustment applied as a
    /// **non-destructive derived view**.
    ///
    /// The measured project is never mutated: the proven adjustment runs on a
    /// throwaway clone and only the resulting coordinates are returned. The output
    /// is identical to the geometry users saw after the old "Run Adjustment" action,
    /// but it is recomputed live from intact measured data - so adding/removing
    /// closures always re-solves accurately instead of compounding on already-adjusted
    /// geometry. Each disconnected component is solved independently, anchored on a
    /// zero datum when present and otherwise on a deterministic default anchor.
    pub fn compute_adjusted_station_coordinates(
        &self,
        config: &ClosureAdjustmentConfig,
    ) -> Vec<Station> {
        // Fast path: nothing to adjust → adjusted == measured (skip the clone).
        if !self
            .loop_closures
            .iter()
            .any(|lc| Self::closure_is_eligible(lc.strategy))
        {
            return self.compute_station_coordinates();
        }

        self.clone().into_adjusted_station_coordinates(config)
    }

    /// By-value counterpart of [`Self::compute_adjusted_station_coordinates`] that
    /// consumes `self` as the working copy, so a caller already holding an owned
    /// clone (the IPC path in `AppState::get_adjusted_stations`) avoids a second
    /// full-project clone.
    ///
    /// The adjustment plan (which component anchors on what) is derived up front
    /// from the pristine measured topology, before any mutation. Each component is
    /// then solved on the working copy and **rolled back to measured on failure**:
    /// `adjust_loop_closures` mutates in place and can return `Err` *after* the
    /// solve has already moved stations (e.g. the post-solve zero-datum-fixed
    /// check), so without the rollback a failed component would render
    /// partially-adjusted geometry instead of the measured fallback the warning
    /// promises.
    ///
    /// Cost: the rollback snapshots the working copy once per eligible component,
    /// so a cache-miss solve clones the project `O(eligible components)` times.
    /// In practice that is one or two clones (most surveys are a single connected
    /// component), it only happens on a cache miss, and `adjust_least_squares`
    /// rewrites `section.shots` in place - so a cheaper offsets-only snapshot would
    /// be unsound. Revisit with a scoped {sections, offsets, closures} snapshot
    /// only if profiling large surveys shows the clones matter.
    pub fn into_adjusted_station_coordinates(
        self,
        config: &ClosureAdjustmentConfig,
    ) -> Vec<Station> {
        let anchors = self.plan_adjustment_anchors();
        let mut working = self;
        for anchor in anchors {
            let snapshot = working.clone();
            if let Err(e) =
                working.adjust_loop_closures(anchor, ClosureStrategy::LeastSquaresWeighted, config)
            {
                working = snapshot;
                warn!(
                    error = %e,
                    anchor = ?anchor,
                    purpose = "derive closure-adjusted view",
                    "Skipping closure adjustment for component; showing measured geometry"
                );
            }
        }
        working.compute_station_coordinates()
    }

    /// Compute all station coordinates from shots with optional progress callback.
    ///
    /// The callback receives (current, total) station counts for progress reporting.
    /// Total stations = shots + 1 per section (first station).
    pub fn compute_station_coordinates_with_progress(
        &self,
        on_progress: Option<&mut dyn FnMut(u32, u32)>,
    ) -> Vec<Station> {
        let mut stations: Vec<Station> = self
            .compute_station_positions_with_progress(on_progress)
            .into_iter()
            .map(|(station_ref, coords)| Station::with_coordinates(station_ref, coords))
            .collect();
        self.attach_station_relationships(&mut stations);
        stations
    }

    /// Every station's measured position, and nothing else: the same `x, y, z`
    /// [`Self::compute_station_coordinates`] reports, without the merge and
    /// closure relationship lists it attaches. Near-linear in the station count
    /// whatever the merge-group sizes (#596), so it is the path for consumers
    /// that only need geometry, e.g. `stalx-reader`.
    pub fn compute_station_positions(&self) -> Vec<(StationRef, Coordinates)> {
        self.compute_station_positions_with_progress(None)
    }

    fn compute_station_positions_with_progress(
        &self,
        mut on_progress: Option<&mut dyn FnMut(u32, u32)>,
    ) -> Vec<(StationRef, Coordinates)> {
        // Calculate total stations first for progress reporting
        let total_stations: u32 = self.sections.iter().map(|s| s.shots.len() as u32 + 1).sum();

        // Pre-compute absolute station depths using colocated groups.
        // Each station's Z is the average of surrounding shot depths across
        // all merge/closure partners - no cumulative dz walk needed.
        let t_phase = std::time::Instant::now();
        let colocated_groups = self.build_colocated_groups();
        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            groups = colocated_groups.len(),
            "compute_station_coordinates: build_colocated_groups"
        );
        let t_phase = std::time::Instant::now();
        let mut station_depths: HashMap<StationRef, f64> = HashMap::new();
        for group in colocated_groups.values() {
            let depth = self.compute_station_depth(group);
            for &sr in group {
                station_depths.insert(sr, depth);
            }
        }
        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            "compute_station_coordinates: depths loop"
        );
        let t_phase = std::time::Instant::now();

        let mut stations: Vec<(StationRef, Coordinates)> = Vec::new();
        let mut station_count = 0u32;

        for section in &self.sections {
            // Get section offset (from merges)
            let (offset_x, offset_y) = self
                .section_offsets
                .get(&section.id)
                .copied()
                .unwrap_or((0.0, 0.0));
            let mut current_xy = (offset_x, offset_y);

            // First station of section
            let first_station_ref = StationRef::new(section.id, 0);
            let z = station_depths
                .get(&first_station_ref)
                .copied()
                .unwrap_or(0.0);
            let mut station_coords = Coordinates::new(current_xy.0, current_xy.1, z);
            // Apply station offset if exists
            if let Some((ox, oy, oz)) = self.station_offset_for_ref(first_station_ref) {
                station_coords = Coordinates::new(
                    station_coords.x + ox,
                    station_coords.y + oy,
                    station_coords.z + oz,
                );
            }
            stations.push((first_station_ref, station_coords));
            station_count += 1;

            // Report progress for first station
            if let Some(ref mut cb) = on_progress {
                cb(station_count, total_stations);
            }

            // Compute subsequent stations from shots (X, Y walk; Z from pre-computed depths)
            let azimuth_reference = self.azimuth_reference_for(section);
            for (i, shot) in section.shots.iter().enumerate() {
                let azimuth_rad = shot.average_azimuth_true_rad(azimuth_reference);
                let horizontal_length = shot.horizontal_length();

                // Compute new X, Y position
                let dx = horizontal_length * azimuth_rad.sin();
                let dy = horizontal_length * azimuth_rad.cos();
                current_xy.0 += dx;
                current_xy.1 += dy;

                let station_ref = StationRef::new(section.id, (i + 1) as u32);
                let z = station_depths.get(&station_ref).copied().unwrap_or(0.0);
                let mut station_coords = Coordinates::new(current_xy.0, current_xy.1, z);
                // Apply station offset if exists
                if let Some((ox, oy, oz)) = self.station_offset_for_ref(station_ref) {
                    station_coords = Coordinates::new(
                        station_coords.x + ox,
                        station_coords.y + oy,
                        station_coords.z + oz,
                    );
                }
                stations.push((station_ref, station_coords));
                station_count += 1;

                // Report progress after each station
                if let Some(ref mut cb) = on_progress {
                    cb(station_count, total_stations);
                }
            }
        }

        // NOTE: Global zero datum subtraction removed (fn-19-mmx.2).
        // Positioning is now controlled solely by section_offsets, which are
        // computed when set_zero_datum() is called.
        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            "compute_station_coordinates: xyz walk"
        );
        stations
    }

    /// Merge groups (transitive closure of `station_merges`): each station in a
    /// group maps to its group index. Union by size, so a pathological merge order
    /// stays near-linear (#596). Member order is unchanged from the pre-#596 code
    /// (`a`'s group, then `b`'s): `merged_with` is read in that order by the frontend.
    fn merge_groups(&self) -> (HashMap<StationRef, usize>, Vec<Vec<StationRef>>) {
        let mut station_to_group: HashMap<StationRef, usize> = HashMap::new();
        let mut groups: Vec<VecDeque<StationRef>> = Vec::new();

        for merge in &self.station_merges {
            let (Some(sa), Some(sb)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) else {
                continue;
            };
            let group_a = station_to_group.get(&sa).copied();
            let group_b = station_to_group.get(&sb).copied();

            match (group_a, group_b) {
                (None, None) => {
                    // Neither station is in a group, create a new group
                    let group_id = groups.len();
                    groups.push(VecDeque::from([sa, sb]));
                    station_to_group.insert(sa, group_id);
                    station_to_group.insert(sb, group_id);
                }
                (Some(gid), None) => {
                    // station_a is in a group, add station_b to it
                    groups[gid].push_back(sb);
                    station_to_group.insert(sb, gid);
                }
                (None, Some(gid)) => {
                    // station_b is in a group, add station_a to it
                    groups[gid].push_back(sa);
                    station_to_group.insert(sa, gid);
                }
                (Some(gid_a), Some(gid_b)) => {
                    if gid_a != gid_b {
                        // Both are in different groups: move the SMALLER into the
                        // larger, so each station is re-pointed O(log n) times in
                        // total rather than once per merge it is swept into (#596).
                        let (keep, absorb) = if groups[gid_a].len() >= groups[gid_b].len() {
                            (gid_a, gid_b)
                        } else {
                            (gid_b, gid_a)
                        };
                        let absorbed = std::mem::take(&mut groups[absorb]);
                        for member in &absorbed {
                            station_to_group.insert(*member, keep);
                        }
                        if keep == gid_a {
                            groups[keep].extend(absorbed);
                        } else {
                            // `a` is absorbed into `b`: prepend so `a`'s members stay first.
                            for member in absorbed.into_iter().rev() {
                                groups[keep].push_front(member);
                            }
                        }
                    }
                    // If same group, nothing to do
                }
            }
        }
        (
            station_to_group,
            groups.into_iter().map(Vec::from).collect(),
        )
    }

    /// Attach each station's merge (`merged_with`) and loop-closure (`closures_with`)
    /// relationships. `merged_with` lists every other member of the station's merge
    /// group, so its output is quadratic in group size by contract (the IPC shape the
    /// frontend reads); geometry-only consumers use [`Self::compute_station_positions`].
    fn attach_station_relationships(&self, stations_out: &mut Vec<Station>) {
        let mut stations = std::mem::take(stations_out);
        let t_phase = std::time::Instant::now();

        // Mark merged stations - build merge groups (transitive closure)
        // First, build a map of station_ref -> group_id using Union-Find logic
        let (station_to_group, groups) = self.merge_groups();

        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            groups = groups.len(),
            "compute_station_coordinates: union-find groups"
        );
        let t_phase = std::time::Instant::now();

        // Now assign merged_with for each station based on their group
        for station in &mut stations {
            if let Some(&group_id) = station_to_group.get(&station.station_ref) {
                for member in &groups[group_id] {
                    if *member != station.station_ref {
                        station.merged_with.push(*member);
                    }
                }
            }
        }
        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            "compute_station_coordinates: merged_with"
        );
        let t_phase = std::time::Instant::now();

        // Populate closures_with for each station from loop_closures
        // For each closure, both endpoint stations (and their merged equivalents)
        // get a ClosureRef entry. Transitive closures go through merged stations.
        for closure in &self.loop_closures {
            let (Some(a_ref), Some(b_ref)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) else {
                continue;
            };

            // Collect merged equivalents for each endpoint
            let a_group: Vec<StationRef> = station_to_group
                .get(&a_ref)
                .map_or_else(|| vec![a_ref], |&gid| groups[gid].clone());
            let b_group: Vec<StationRef> = station_to_group
                .get(&b_ref)
                .map_or_else(|| vec![b_ref], |&gid| groups[gid].clone());

            // Skip if both endpoints are in the same merge group (legacy data edge case)
            let a_group_id = station_to_group.get(&a_ref);
            let b_group_id = station_to_group.get(&b_ref);
            if a_group_id.is_some() && a_group_id == b_group_id {
                continue;
            }

            // For each station in A's group, add direct ref to B and transitive refs to B's merged partners
            for station in &mut stations {
                if a_group.contains(&station.station_ref) {
                    // Direct closure to station_b (skip self-references)
                    if station.station_ref != b_ref {
                        let via = if station.station_ref == a_ref {
                            None
                        } else {
                            Some(a_ref) // transitive: reached via the actual closure endpoint
                        };
                        station.closures_mut().push(ClosureRef {
                            closure_id: closure.id,
                            station_ref: b_ref,
                            via_station: via,
                            is_transitive: via.is_some(),
                        });
                    }
                    // Transitive closures to B's merged partners (skip self-references)
                    for &b_member in &b_group {
                        if b_member != b_ref && b_member != station.station_ref {
                            station.closures_mut().push(ClosureRef {
                                closure_id: closure.id,
                                station_ref: b_member,
                                via_station: Some(b_ref),
                                is_transitive: true,
                            });
                        }
                    }
                } else if b_group.contains(&station.station_ref) {
                    // Direct closure to station_a (skip self-references)
                    if station.station_ref != a_ref {
                        let via = if station.station_ref == b_ref {
                            None
                        } else {
                            Some(b_ref)
                        };
                        station.closures_mut().push(ClosureRef {
                            closure_id: closure.id,
                            station_ref: a_ref,
                            via_station: via,
                            is_transitive: via.is_some(),
                        });
                    }
                    // Transitive closures to A's merged partners (skip self-references)
                    for &a_member in &a_group {
                        if a_member != a_ref && a_member != station.station_ref {
                            station.closures_mut().push(ClosureRef {
                                closure_id: closure.id,
                                station_ref: a_member,
                                via_station: Some(a_ref),
                                is_transitive: true,
                            });
                        }
                    }
                }
            }
        }

        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            closures = self.loop_closures.len(),
            "compute_station_coordinates: closures_with"
        );
        let t_phase = std::time::Instant::now();

        // Closure-chain transitivity: when a station has direct closures to multiple
        // other stations, those other stations should see transitive closures to each
        // other via the shared station.
        //
        // Example: Station X has closures to Y and Z.
        //   Y should see: transitive closure to Z (via X)
        //   Z should see: transitive closure to Y (via X)
        //
        // The closure_id used is the one connecting the shared station to the far endpoint,
        // so the metrics displayed correspond to that specific loop closure.
        {
            // Collect direct (non-transitive) closure refs for each station
            let mut direct_closures: HashMap<StationRef, Vec<(u64, StationRef)>> = HashMap::new();
            for station in &stations {
                for cr in station.closures() {
                    if !cr.is_transitive {
                        direct_closures
                            .entry(station.station_ref)
                            .or_default()
                            .push((cr.closure_id, cr.station_ref));
                    }
                }
            }

            // For each station with 2+ direct closures, create chain-transitive refs
            // between all pairs of its closure partners
            let mut chain_transitive_refs: Vec<(StationRef, ClosureRef)> = Vec::new();
            for (shared_ref, endpoints) in &direct_closures {
                if endpoints.len() < 2 {
                    continue;
                }
                for i in 0..endpoints.len() {
                    for j in 0..endpoints.len() {
                        if i == j {
                            continue;
                        }
                        let (_, target_i) = &endpoints[i];
                        let (closure_j, target_j) = &endpoints[j];

                        // target_i should see transitive closure to target_j via shared_ref
                        // using closure_j's id (the closure connecting shared_ref to target_j)
                        if target_i != target_j {
                            chain_transitive_refs.push((
                                *target_i,
                                ClosureRef {
                                    closure_id: *closure_j,
                                    station_ref: *target_j,
                                    via_station: Some(*shared_ref),
                                    is_transitive: true,
                                },
                            ));
                        }
                    }
                }
            }

            // Apply chain-transitive refs, avoiding duplicates
            for (station_ref, new_ref) in chain_transitive_refs {
                if let Some(station) = stations.iter_mut().find(|s| s.station_ref == station_ref) {
                    let already_exists = station.closures().iter().any(|cr| {
                        cr.station_ref == new_ref.station_ref
                            && cr.via_station == new_ref.via_station
                    });
                    if !already_exists {
                        station.closures_mut().push(new_ref);
                    }
                }
            }
        }

        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            "compute_station_coordinates: closure-chain transitivity"
        );
        let t_phase = std::time::Instant::now();

        // Mark zero datums
        for zd in &self.zero_datums {
            let Some(sr) = self.resolve_key(&zd.station_ref) else {
                continue;
            };
            for station in &mut stations {
                if station.station_ref == sr {
                    station.is_zero_datum = true;
                    station.gps_coordinates = Some(zd.gps);
                }
            }
        }

        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            "compute_station_coordinates: zero datums"
        );
        let t_phase = std::time::Instant::now();

        // Assign station comments and names through one ref -> position index.
        // Names cover nearly every station of a DMP import, so the per-entry scan
        // over all stations the comments used to do would be quadratic here
        // (~8e9 comparisons on an 89k-station project). Stations are unique per
        // ref - each section contributes `0..=shots.len()` once.
        if !self.station_comments.is_empty() || !self.station_names.is_empty() {
            let position: HashMap<StationRef, usize> = stations
                .iter()
                .enumerate()
                .map(|(i, s)| (s.station_ref, i))
                .collect();
            for (key, comment) in &self.station_comments {
                if let Some(&i) = self.resolve_key(key).and_then(|sr| position.get(&sr)) {
                    stations[i].comment = Some(comment.clone());
                }
            }
            for (key, name) in &self.station_names {
                if let Some(&i) = self.resolve_key(key).and_then(|sr| position.get(&sr)) {
                    stations[i].name = Some(name.clone());
                }
            }
        }
        tracing::debug!(
            ms = t_phase.elapsed().as_millis() as u64,
            "compute_station_coordinates: station comments and names"
        );

        *stations_out = stations;
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::*;
    use crate::{LoopClosure, SectionData, ShotData, StationMerge};
    use chrono::Utc;

    fn shot(azimuth: f64, length: f64) -> ShotData {
        ShotData {
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            length,
            ..Default::default()
        }
    }

    fn add_section(project: &mut Project, shots: Vec<ShotData>) -> SectionId {
        project
            .add_section(SectionData {
                shots,
                ..Default::default()
            })
            .id
    }

    /// The station index [`Project::default_anchor_for_component`] picks for the
    /// component `section_id` belongs to.
    fn anchor_index(project: &Project, section_id: SectionId) -> u32 {
        let component = project.get_connected_section_ids(section_id);
        let anchor = project.default_anchor_for_component(&component).unwrap();
        assert_eq!(
            anchor.section_id, section_id,
            "fixture expects the anchor to come from section {section_id}"
        );
        anchor.station_index
    }

    fn adjusted_xy(stations: &[Station], sr: StationRef) -> (f64, f64) {
        stations
            .iter()
            .find(|s| s.station_ref == sr)
            .map(|s| (s.coordinates.x, s.coordinates.y))
            .unwrap()
    }

    /// One loop built from two SINGLE-shot sections: merged at their station 0,
    /// tied by an eligible closure at their station 1, 0.25 m of misclosure and
    /// no zero datum - so the component's translation is gauged by
    /// [`Project::default_anchor_for_component`] alone.
    ///
    /// A 1-shot section is the shape the shot-UUID identity cannot separate on
    /// its own (both stations sit between the same single shot and a section
    /// end), so this is the fixture the incidence tie-break exists for.
    fn project_with_a_one_shot_loop() -> (Project, SectionId, SectionId) {
        let mut project = Project::new("one-shot loop".to_string(), None);
        let a = add_section(&mut project, vec![shot(0.0, 10.0)]);
        // 0.25 m longer, so the loop does not quite close.
        let b = add_section(&mut project, vec![shot(0.0, 10.25)]);
        project.ensure_shot_uuids();
        project.station_merges.push(StationMerge {
            station_a: project.key_for_ref(StationRef::new(a, 0)).unwrap(),
            station_b: project.key_for_ref(StationRef::new(b, 0)).unwrap(),
            merged_at: Utc::now(),
        });
        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: project.key_for_ref(StationRef::new(a, 1)).unwrap(),
            station_b: project.key_for_ref(StationRef::new(b, 1)).unwrap(),
            path_length: 20.25,
            misclosure_distance: 0.25,
            error_percentage: 1.2,
            horizontal_misclosure: 0.25,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::LeastSquaresWeighted,
            is_aligned: false,
        });
        (project, a, b)
    }

    /// The reverse promises every station keeps its world position, and Plan /
    /// Export mode FORCES the closure-adjusted view - so the promise has to hold
    /// for the ADJUSTED coordinates of a section the user did not even touch.
    ///
    /// With every section in the component down to a single shot, the shot-UUID
    /// identity is the same at both ends (station 0 is `(None, u0)`; station 1 is
    /// `(Some(u0), None)`, which the sort flips onto the same pair), so the pick
    /// fell through to the lowest index - the far physical end after a reverse.
    /// The gauge then re-pinned on the other end of the loop and the untouched
    /// section slid by half the misclosure, 0.125 m.
    #[test]
    fn reverse_of_a_one_shot_anchor_section_does_not_move_the_component() {
        let (project, a, b) = project_with_a_one_shot_loop();
        let config = ClosureAdjustmentConfig::default();

        let before = project.clone().into_adjusted_station_coordinates(&config);
        let mut project = project;
        assert!(project.reverse_section(a, false));
        let after = project.into_adjusted_station_coordinates(&config);

        // Precondition: the solve actually ran and fused the closure, so the
        // assertions below are about the gauge and not about a no-op.
        let (fused_a, fused_b) = (
            adjusted_xy(&before, StationRef::new(a, 1)),
            adjusted_xy(&before, StationRef::new(b, 1)),
        );
        assert!(
            (fused_a.0 - fused_b.0).abs() < 1e-6 && (fused_a.1 - fused_b.1).abs() < 1e-6,
            "precondition: the closure should have been adjusted, got {fused_a:?} / {fused_b:?}"
        );

        // Section `b` was not touched at all, so not one of its stations may move.
        for i in 0..=1 {
            let p = adjusted_xy(&before, StationRef::new(b, i));
            let q = adjusted_xy(&after, StationRef::new(b, i));
            assert!(
                (p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9,
                "adjusted station b:{i} moved: {p:?} -> {q:?}"
            );
        }
        // And `a`'s station `i` is now station `1 - i`, in the same place.
        for i in 0..=1 {
            let p = adjusted_xy(&before, StationRef::new(a, i));
            let q = adjusted_xy(&after, StationRef::new(a, 1 - i));
            assert!(
                (p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9,
                "adjusted station a:{i} moved: {p:?} -> {q:?}"
            );
        }
    }

    /// UPGRADE REGRESSION: an existing project's gauge must not move.
    ///
    /// The gauge has always been "station 0 of the lowest-id section in the component", and the
    /// solve pins that station at its measured coordinates - so re-picking it on anything
    /// derived from current data (shot counts, UUID ordering, incidence) would shift Plan and
    /// Export output the moment someone upgraded, with no reverse performed, no action taken
    /// and nothing reported. `reverse_parity` is `false` on every file that predates it, which
    /// is what makes this hold.
    #[test]
    fn the_gauge_of_an_untouched_project_is_the_legacy_one() {
        let mut project = Project::new("mixed".to_string(), None);
        let one = add_section(&mut project, vec![shot(0.0, 10.0)]);
        let three = add_section(
            &mut project,
            vec![shot(0.0, 10.0), shot(90.0, 10.0), shot(180.0, 10.0)],
        );
        project.ensure_shot_uuids();
        project.station_merges.push(StationMerge {
            station_a: project.key_for_ref(StationRef::new(one, 1)).unwrap(),
            station_b: project.key_for_ref(StationRef::new(three, 0)).unwrap(),
            merged_at: Utc::now(),
        });
        assert!(one < three, "fixture: the 1-shot section owns the lower id");

        let component = project.get_connected_section_ids(one);
        let anchor = project.default_anchor_for_component(&component).unwrap();

        assert_eq!(
            (anchor.section_id, anchor.station_index),
            (one, 0),
            "the gauge must stay where it has always been: station 0 of the lowest-id section"
        );
    }

    /// THE reviewer's case: appending to an already-reversed section must not move the gauge.
    ///
    /// The old rule was "station 0, or `shots.len()` when reversed" - stable across the reverse
    /// itself, but `len` grows on the next append and walks the pin onto the newly created far
    /// station, translating the whole adjusted component by the adjustment residual. The
    /// captured pair is immune: neither shot moves relative to the other when the section is
    /// extended.
    #[test]
    fn appending_to_a_reversed_section_does_not_move_the_gauge() {
        let mut project = Project::new("append".to_string(), None);
        let s = add_section(
            &mut project,
            vec![shot(0.0, 10.0), shot(90.0, 10.0), shot(180.0, 10.0)],
        );
        project.ensure_shot_uuids();
        // Capture happens on save; do it directly here.
        project.capture_missing_gauge_anchors();
        assert_eq!(anchor_index(&project, s), 0);

        assert!(project.reverse_section(s, false));
        assert_eq!(anchor_index(&project, s), 3, "station 0 is now station 3");

        // Extending the passage must not disturb it.
        project.add_shot(s, shot(270.0, 10.0)).unwrap();
        assert_eq!(
            anchor_index(&project, s),
            3,
            "the gauge must stay on the same physical station when the section grows"
        );
    }

    /// A ONE-SHOT section has no gauge neighbour, so position cannot decide the side: both of
    /// its ends touch the same single shot, and reversing swaps them while leaving that shot at
    /// position 0. Reading the side off the orientation is what keeps the gauge on its physical
    /// station - reachable whenever a one-shot, lowest-id section is wired into a closure
    /// component.
    #[test]
    fn the_gauge_of_a_one_shot_section_follows_a_reverse() {
        let mut project = Project::new("one shot".to_string(), None);
        let s = add_section(&mut project, vec![shot(0.0, 10.0)]);
        project.ensure_shot_uuids();
        project.capture_missing_gauge_anchors();
        assert!(
            project.sections[0].gauge_neighbour_uuid.is_none(),
            "fixture: a one-shot section has no neighbour to face away from"
        );
        assert_eq!(anchor_index(&project, s), 0);

        assert!(project.reverse_section(s, false));
        assert_eq!(
            anchor_index(&project, s),
            1,
            "the original gauge station is now station 1, and the gauge must follow it"
        );

        assert!(project.reverse_section(s, false));
        assert_eq!(anchor_index(&project, s), 0, "and back again");
    }

    /// A project that has never been saved has no captured pair, and must keep using the legacy
    /// station-0 answer - capturing is what makes it durable, not what chooses it.
    #[test]
    fn an_uncaptured_section_still_uses_the_legacy_gauge() {
        let mut project = Project::new("legacy".to_string(), None);
        let s = add_section(&mut project, vec![shot(0.0, 10.0), shot(90.0, 10.0)]);
        project.ensure_shot_uuids();
        assert!(project.sections[0].gauge_shot_uuid.is_none());
        assert_eq!(anchor_index(&project, s), 0);

        // Capturing records exactly the station already in use.
        project.capture_missing_gauge_anchors();
        assert_eq!(anchor_index(&project, s), 0);
    }

    /// ...and it follows that physical station through a reverse, rather than staying on the
    /// index. Station 0 becomes station N, and the parity marker is what says so.
    #[test]
    fn the_gauge_follows_its_physical_station_through_a_reverse() {
        let mut project = Project::new("one section".to_string(), None);
        let s = add_section(
            &mut project,
            vec![shot(0.0, 10.0), shot(90.0, 10.0), shot(180.0, 10.0)],
        );
        project.ensure_shot_uuids();

        assert_eq!(anchor_index(&project, s), 0);
        assert!(project.reverse_section(s, false));
        assert_eq!(
            anchor_index(&project, s),
            3,
            "station 0 is now station 3, and the gauge must be there - not on the new station 0"
        );
        // Reversing back returns it, because parity is a flip and not a count.
        assert!(project.reverse_section(s, false));
        assert_eq!(anchor_index(&project, s), 0);
    }

    /// Due north end to end: `sin(0.0)` is exactly 0 but `sin(180.0)` is
    /// 1.22e-16, so a rule that ordered stations by their computed coordinates
    /// would pick a different end after the reverse turned every bearing around.
    /// A guard, not a regression - the shot-UUID identity already handles a
    /// multi-shot section - kept so nobody reintroduces a coordinate rule.
    #[test]
    fn anchor_identity_is_stable_across_a_due_north_reverse() {
        let mut project = Project::new("due north".to_string(), None);
        let s = add_section(
            &mut project,
            vec![shot(0.0, 10.0), shot(0.0, 10.0), shot(0.0, 10.0)],
        );
        project.ensure_shot_uuids();

        let before = anchor_index(&project, s);
        assert!(project.reverse_section(s, false));
        let after = anchor_index(&project, s);

        assert_eq!(
            after,
            3 - before,
            "the anchor left its physical station: {before} -> {after}"
        );
    }

    /// A lone one-shot section with no merge and no closure: its two ends are told apart by the
    /// section's ORIENTATION, which a reverse flips.
    ///
    /// The rationale this test used to give - that the ends are distinguished because a reverse
    /// swaps `depth_in`/`depth_out` bit-exactly - described a picker that no longer exists. The
    /// current one never reads depth, so the fixture's depths were inert and the assertion held
    /// for any one-shot section rather than for the reason stated. Kept because the CASE still
    /// matters; the reason is now the parity flag.
    #[test]
    fn a_lone_one_shot_sections_gauge_follows_its_orientation() {
        let mut project = Project::new("depth".to_string(), None);
        let s = add_section(
            &mut project,
            vec![ShotData {
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                length: 10.0,
                depth_in: 0.0,
                depth_out: 5.0,
                ..Default::default()
            }],
        );
        project.ensure_shot_uuids();

        let before = anchor_index(&project, s);
        assert!(project.reverse_section(s, false));
        let after = anchor_index(&project, s);

        assert_eq!(
            after,
            1 - before,
            "the anchor left its physical station: {before} -> {after}"
        );
    }

    /// `2 * pairs` one-shot sections whose start stations are merged in the order
    /// that made the old union quadratic: each new pair forms a small group, then
    /// joins the big one, and "move b into a" re-pointed the whole big group.
    fn adversarial_merge_order(pairs: usize) -> Project {
        use crate::station::{StationAnchor, StationKey};
        let mut project = Project::new("Big group".to_string(), None);
        let ids: Vec<SectionId> = (0..2 * pairs)
            .map(|_| add_section(&mut project, vec![shot(90.0, 1.0)]))
            .collect();
        let merge = |a: SectionId, b: SectionId| StationMerge {
            station_a: StationKey::new(a, StationAnchor::SectionStart),
            station_b: StationKey::new(b, StationAnchor::SectionStart),
            merged_at: Utc::now(),
        };
        project.station_merges.push(merge(ids[0], ids[1]));
        for pair in ids.chunks(2).skip(1) {
            project.station_merges.push(merge(pair[0], pair[1]));
            project.station_merges.push(merge(pair[0], ids[0]));
        }
        project
    }

    #[test]
    fn merge_groups_stay_near_linear_in_the_worst_merge_order() {
        let project = adversarial_merge_order(10_000);

        let started = std::time::Instant::now();
        let (station_to_group, groups) = project.merge_groups();
        let elapsed = started.elapsed();

        assert_eq!(station_to_group.len(), 20_000);
        assert_eq!(groups.iter().filter(|g| !g.is_empty()).count(), 1);
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 20_000);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "union by size keeps this near-linear; took {elapsed:?}"
        );
    }

    #[test]
    fn merge_groups_keep_the_first_groups_members_first() {
        use crate::station::{StationAnchor, StationKey};
        let mut project = Project::new("Order".to_string(), None);
        let ids: Vec<SectionId> = (0..5)
            .map(|_| add_section(&mut project, vec![shot(90.0, 1.0)]))
            .collect();
        let key = |i: usize| StationKey::new(ids[i], StationAnchor::SectionStart);
        let merge = |a: usize, b: usize| StationMerge {
            station_a: key(a),
            station_b: key(b),
            merged_at: Utc::now(),
        };
        // {0,1} then {2,3,4}; the last merge joins the SMALLER group (a) to the
        // larger one (b), which union by size absorbs the other way round.
        for m in [merge(0, 1), merge(2, 3), merge(2, 4), merge(0, 2)] {
            project.station_merges.push(m);
        }

        let (station_to_group, groups) = project.merge_groups();
        let expected: Vec<StationRef> = (0..5)
            .filter_map(|i| project.resolve_key(&key(i)))
            .collect();
        assert_eq!(expected.len(), 5);
        assert_eq!(groups[station_to_group[&expected[0]]], expected);
    }

    #[test]
    fn positions_are_exactly_the_full_coordinates() {
        let project = adversarial_merge_order(20);

        let full = project.compute_station_coordinates();
        let positions = project.compute_station_positions();

        assert_eq!(full.len(), positions.len());
        for (station, (station_ref, coords)) in full.iter().zip(&positions) {
            assert_eq!(station.station_ref, *station_ref);
            let bits = |c: &Coordinates| (c.x.to_bits(), c.y.to_bits(), c.z.to_bits());
            assert_eq!(bits(&station.coordinates), bits(coords));
        }
    }

    #[test]
    fn positions_skip_the_quadratic_relationship_lists() {
        // A 20,000-member merge group: `merged_with` would be 20,000^2 entries.
        let project = adversarial_merge_order(10_000);

        let started = std::time::Instant::now();
        let positions = project.compute_station_positions();
        let elapsed = started.elapsed();

        assert_eq!(positions.len(), 40_000);
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "positions are near-linear; took {elapsed:?}"
        );
    }
}
