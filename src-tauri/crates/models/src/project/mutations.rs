use chrono::Utc;
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::{debug, error, info, trace};

use crate::{
    gps_to_local, ClosurePreview, ClosureStrategy, Coordinates, GpsCoordinates, LoopClosure,
    SectionId, StationKey, StationMerge, StationRef, ZeroDatum,
};

use super::Project;

impl Project {
    /// Merge two stations
    /// Returns Err if validation fails, Ok(true) on success
    pub fn merge_stations(
        &mut self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> Result<bool, String> {
        debug!(
            station_a = ?station_a,
            station_b = ?station_b,
            purpose = "merge stations",
            "Merging stations"
        );
        // Check that both stations exist
        if !self.station_exists(station_a) || !self.station_exists(station_b) {
            return Err("One or both stations do not exist".to_string());
        }

        // Check not already merged
        if self.are_stations_merged(station_a, station_b) {
            return Err("These stations are already merged".to_string());
        }

        // Reject only if the two sections are already RIGIDLY connected (via merges
        // or *aligned* closures) - adding a merge there would create a redundant
        // rigid cycle, which is what a loop closure is for. An INERT (unaligned)
        // closure does not constrain the two stations yet, so a merge across one is
        // the first real connection and must be allowed; rejecting it (the old
        // get_connected_section_ids walk, which followed inert closures too) forced
        // users into the sole-bridge-closure-that-should-be-a-merge bug. This asserts
        // the same graph the cascade actually solves (catalog #8 / #integrity Root 3).
        let connected_a = self.get_rigid_connected_section_ids(station_a.section_id);
        if connected_a.contains(&station_b.section_id) {
            return Err(
                "Stations are already connected. Use Create Loop Closure instead.".to_string(),
            );
        }

        // Check zero datum status - uses transitive connections through existing merges
        // This ensures that if B is connected to a datum-holding section C through an
        // earlier merge, b_has_zd will be true and the merge will propagate datum positioning
        let a_has_zd = self.section_has_zero_datum(station_a.section_id);
        let b_has_zd = self.section_has_zero_datum(station_b.section_id);

        if a_has_zd && b_has_zd {
            return Err("Both stations are connected to zero datums. Remove one zero datum before merging, or create a loop closure if the sections are connected.".to_string());
        }

        debug!(
            station_a = ?station_a,
            station_b = ?station_b,
            a_has_zero_datum = a_has_zd,
            b_has_zero_datum = b_has_zd,
            purpose = "merge stations",
            "Resolved zero datum anchors for merge"
        );

        // Current horizontal (x/y) positions in the world coordinate system. Only
        // x/y are needed to align the merge point (the offset below is purely
        // horizontal), so use the x/y-only walk and skip the expensive Z/colocation
        // computation - this is on the per-merge hot path of a large import.
        let coords_a = self
            .compute_station_raw_xy(station_a)
            .ok_or_else(|| "Failed to compute coordinates for station A".to_string())?;
        let coords_b = self
            .compute_station_raw_xy(station_b)
            .ok_or_else(|| "Failed to compute coordinates for station B".to_string())?;

        // Datum propagation: When merging a section without datum into a datum group,
        // the non-datum section is repositioned to align with the datum coordinate system.
        // This is similar to set_zero_datum() offset calculation but triggered by merge.
        //
        // Determine which section group needs to move:
        // - If A has zero datum (direct or transitive), B's group moves to align with A
        // - If B has zero datum (direct or transitive), A's group moves to align with B
        // - If neither has zero datum, A's group moves to align with B (arbitrary choice)
        let ((anchor_x, anchor_y), (moving_x, moving_y), moving_section_id) = if a_has_zd {
            (coords_a, coords_b, station_b.section_id)
        } else {
            // B has zero datum OR neither has one (in which case B is the anchor)
            (coords_b, coords_a, station_a.section_id)
        };

        // Calculate offset to move the merge point station from its current position
        // to the anchor station's position, aligning them in world coordinates
        let offset_x = anchor_x - moving_x;
        let offset_y = anchor_y - moving_y;

        // Apply offset to all sections in the moving merge group (rigid body)
        // This repositions the entire moving group relative to the datum coordinate system
        let moving_sections = self.get_merged_section_ids(moving_section_id);
        // Resolve-aware (a dangling datum is not a real anchor - matching
        // section_has_zero_datum), so a deleted-anchor datum can't trigger a
        // spurious "merge would move a zero datum" alarm.
        let moving_has_zero_datum = self
            .zero_datums
            .iter()
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .any(|sr| moving_sections.contains(&sr.section_id));
        if moving_has_zero_datum && (offset_x.abs() > 1e-9 || offset_y.abs() > 1e-9) {
            error!(
                moving_section_id,
                offset_x,
                offset_y,
                moving_section_count = moving_sections.len(),
                purpose = "protect zero datum position",
                "Merge would move a zero datum section"
            );
        }

        debug!(
            moving_section_id,
            moving_section_count = moving_sections.len(),
            offset_x,
            offset_y,
            purpose = "merge stations",
            "Computed merge offset for moving sections"
        );
        self.adjust_rigid_group_offsets(moving_section_id, offset_x, offset_y);

        // Record the merge through the named entry point (it forwards to
        // MergeTopology::push, which carries the adjacency memo forward with an
        // O(1) delta instead of dropping it).
        let (Some(key_a), Some(key_b)) = (self.key_for_ref(station_a), self.key_for_ref(station_b))
        else {
            return Err("Cannot merge: station has no stable anchor".to_string());
        };
        self.record_station_merge(StationMerge {
            station_a: key_a,
            station_b: key_b,
            merged_at: Utc::now(),
        });

        // Cascade through any further connections that the new merge may ripple into
        self.cascade_connection_offsets(&[station_a.section_id, station_b.section_id]);
        self.debug_assert_merge_alignment("merge_stations");
        self.touch();
        Ok(true)
    }

    /// Compute closure metrics between two stations without mutation.
    /// Both stations must exist and have computable coordinates.
    pub fn compute_closure_metrics(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> Result<ClosurePreview, String> {
        trace!(
            station_a = ?station_a,
            station_b = ?station_b,
            purpose = "compute closure metrics",
            "Computing closure metrics"
        );
        let coords_a = self
            .compute_station_raw_coords(station_a)
            .ok_or_else(|| "Failed to compute coordinates for station A".to_string())?;
        let coords_b = self
            .compute_station_raw_coords(station_b)
            .ok_or_else(|| "Failed to compute coordinates for station B".to_string())?;

        let horizontal_misclosure = coords_a.horizontal_distance_to(&coords_b);
        let vertical_misclosure = (coords_a.z - coords_b.z).abs();
        let misclosure_distance = coords_a.distance_to(&coords_b);
        let path_length = self.calculate_path_length_between(station_a, station_b);

        let error_percentage = if path_length > 0.0 {
            (misclosure_distance / path_length) * 100.0
        } else {
            0.0
        };

        debug!(
            station_a = ?station_a,
            station_b = ?station_b,
            path_length,
            misclosure_distance,
            error_percentage,
            horizontal_misclosure,
            vertical_misclosure,
            purpose = "compute closure metrics",
            "Computed closure metrics"
        );

        Ok(ClosurePreview {
            path_length,
            misclosure_distance,
            error_percentage,
            horizontal_misclosure,
            vertical_misclosure,
        })
    }

    /// Lift `next_closure_id` above every id already in use, so the next closure
    /// minted from the counter gets a unique one.
    ///
    /// A legacy file can carry closures with id 0 and a counter that was never
    /// advanced past them, so the counter alone is not trustworthy on a project
    /// that came off disk. Every path that mints a closure id calls this first -
    /// [`Self::create_loop_closure`], [`Self::create_loop_closures_batch`], and
    /// the merge demotion - so they cannot disagree about the convention.
    pub(crate) fn normalize_next_closure_id(&mut self) {
        let max_existing = self.loop_closures.iter().map(|lc| lc.id).max().unwrap_or(0);
        if self.next_closure_id <= max_existing {
            self.next_closure_id = max_existing + 1;
        }
    }

    /// Create a loop closure between two connected stations
    /// Returns the created LoopClosure on success
    pub fn create_loop_closure(
        &mut self,
        station_a: StationRef,
        station_b: StationRef,
        strategy: ClosureStrategy,
    ) -> Result<LoopClosure, String> {
        debug!(
            station_a = ?station_a,
            station_b = ?station_b,
            strategy = ?strategy,
            purpose = "create loop closure",
            "Creating loop closure"
        );
        // Check that both stations exist
        if !self.station_exists(station_a) || !self.station_exists(station_b) {
            return Err("One or both stations do not exist".to_string());
        }

        // Reject already-merged station pairs
        if self.are_stations_merged(station_a, station_b) {
            return Err(
                "These stations are already merged. Closures are for unmerged station pairs."
                    .to_string(),
            );
        }

        // Check stations are connected (via merges or closures)
        let connected_a = self.get_connected_section_ids(station_a.section_id);
        let connected = connected_a.contains(&station_b.section_id);
        if !connected {
            let a_has_zd = self.section_has_zero_datum(station_a.section_id);
            let b_has_zd = self.section_has_zero_datum(station_b.section_id);
            if !(a_has_zd && b_has_zd) {
                return Err("Stations are not connected. They must be in the same connected group to create a loop closure.".to_string());
            }
        }

        // Check for duplicate closure between same station pair
        let already_has_closure = self.are_stations_in_closure(station_a, station_b);
        if already_has_closure {
            return Err("A loop closure already exists between these stations.".to_string());
        }

        // A closure needs a loop to close (#737): refuse one that would tie the
        // two ends of one shot together - the solve could only skip it.
        // Whatever the strategy: a tie across one shot means nothing for a
        // `Move` closure either. A pair other merges and closures already tie
        // together is NOT refused: it changes nothing for the solve.
        if let Some(shot) = self.closure_spans_one_shot(station_a, station_b) {
            return Err(super::closure_shots::closure_across_one_shot_refusal(shot));
        }

        // Compute closure values using shared helper
        let metrics = self.compute_closure_metrics(station_a, station_b)?;

        // Deferred: create-time distribution strategies are disabled for now.
        if matches!(
            strategy,
            ClosureStrategy::ProportionalByLength | ClosureStrategy::EqualDistribution
        ) {
            return Err("Closure distribution strategies are deferred. Use Batch.".to_string());
        }

        self.normalize_next_closure_id();
        // Assign next closure ID from monotonic counter
        let next_id = self.next_closure_id;
        self.next_closure_id += 1;

        let (Some(key_a), Some(key_b)) = (self.key_for_ref(station_a), self.key_for_ref(station_b))
        else {
            return Err("Cannot create loop closure: station has no stable anchor".to_string());
        };
        // MEASURED, not asserted (#469 item 2). A `Move` closure is born of the
        // snap-to-merge workflow, whose batch runs the moves that made the two
        // stations coincide BEFORE this command - so in that workflow the
        // measurement says `true` and nothing changes. Every other route to a
        // `Move` closure (IPC, the harness, the fuzzer) can hand this an
        // arbitrary pair metres apart, and stamping such a pair aligned made the
        // app assert a coincidence it had computed the refutation of one line
        // earlier, rigidify the edge, and report it Critical forever with no
        // path back.
        let is_aligned = self.closure_alignment_stamp_now(strategy, &key_a, &key_b);
        let closure = LoopClosure {
            id: next_id,
            station_a: key_a,
            station_b: key_b,
            path_length: metrics.path_length,
            misclosure_distance: metrics.misclosure_distance,
            error_percentage: metrics.error_percentage,
            horizontal_misclosure: metrics.horizontal_misclosure,
            vertical_misclosure: metrics.vertical_misclosure,
            strategy,
            is_aligned,
        };

        self.loop_closures.push(closure.clone());
        self.touch();

        info!(
            closure_id = closure.id,
            station_a = ?station_a,
            station_b = ?station_b,
            strategy = ?strategy,
            purpose = "create loop closure",
            "Loop closure created"
        );

        Ok(closure)
    }

    /// Batch-create `Batch`-strategy loop closures (the importer's hot path).
    ///
    /// Semantically identical to calling
    /// `create_loop_closure(a, b, ClosureStrategy::Batch)` for each pair IN
    /// ORDER, but builds the graph invariants ONCE and reuses them: station
    /// coordinates, a station-level merge-group union-find (the "already merged?"
    /// check → O(α) instead of a BFS per closure), a section-level connectivity
    /// union-find (merges + closures; grown as closures are added), the
    /// station-merge adjacency, the closure adjacency, a duplicate-pair set, and
    /// the constraint knots behind the across-one-shot refusal (#737).
    /// Sections/merges/offsets are fixed for the batch (the import's cascade ran
    /// first and Batch closures are never `is_aligned`, so they don't rigidify),
    /// so only the closure-dependent state (connectivity UF, closure adjacency,
    /// dup set, knots) updates per created closure - exactly what path-length needs to
    /// still see earlier closures. Returns one result per input pair (Ok =
    /// created, Err = skipped, with the same reason strings as the single path).
    pub fn create_loop_closures_batch(
        &mut self,
        pairs: &[(StationRef, StationRef)],
    ) -> Vec<Result<LoopClosure, String>> {
        // Order-independent pair key (StationRef is not Ord).
        fn norm(a: StationRef, b: StationRef) -> (StationRef, StationRef) {
            if (a.section_id, a.station_index) <= (b.section_id, b.station_index) {
                (a, b)
            } else {
                (b, a)
            }
        }
        fn uf_find<K: std::hash::Hash + Eq + Copy>(p: &mut HashMap<K, K>, x: K) -> K {
            let mut root = x;
            while let Some(&pr) = p.get(&root) {
                if pr == root {
                    break;
                }
                root = pr;
            }
            let mut cur = x;
            while let Some(&pr) = p.get(&cur) {
                if pr == root {
                    break;
                }
                p.insert(cur, root);
                cur = pr;
            }
            root
        }
        fn uf_union<K: std::hash::Hash + Eq + Copy>(p: &mut HashMap<K, K>, a: K, b: K) {
            p.entry(a).or_insert(a);
            p.entry(b).or_insert(b);
            let ra = uf_find(p, a);
            let rb = uf_find(p, b);
            if ra != rb {
                p.insert(ra, rb);
            }
        }

        // Raw station coordinates (same x/y/z as compute_station_raw_coords; built
        // once via the colocated-group pass).
        let coords: HashMap<StationRef, Coordinates> = self
            .compute_station_coordinates()
            .into_iter()
            .map(|s| (s.station_ref, s.coordinates))
            .collect();

        // Station-level merge-group union-find (immutable for the batch).
        let mut merge_uf: HashMap<StationRef, StationRef> = HashMap::new();
        for merge in &self.station_merges {
            if let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                uf_union(&mut merge_uf, a, b);
            }
        }

        // Section-level connectivity union-find (merges + closures); grows below.
        let mut conn_uf: HashMap<SectionId, SectionId> = HashMap::new();
        for merge in &self.station_merges {
            uf_union(
                &mut conn_uf,
                merge.station_a.section_id,
                merge.station_b.section_id,
            );
        }
        for closure in &self.loop_closures {
            uf_union(
                &mut conn_uf,
                closure.station_a.section_id,
                closure.station_b.section_id,
            );
        }

        // Path-length adjacency: merges (immutable) + closures (grows below).
        let merge_adj = self.build_station_merge_adjacency();
        let mut closure_adj: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        let mut closure_pairs: HashSet<(StationRef, StationRef)> = HashSet::new();
        for closure in &self.loop_closures {
            if let (Some(a), Some(b)) = (
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) {
                closure_adj.entry(a).or_default().push(b);
                closure_adj.entry(b).or_default().push(a);
                closure_pairs.insert(norm(a, b));
            }
        }

        // The constraint knots the single path asks `closure_spans_one_shot`
        // about (#737), built once and grown as closures are added.
        let (mut knots, _) = self.closure_knots();

        // Normalize the id counter once (legacy id=0 projects).
        self.normalize_next_closure_id();

        let mut results = Vec::with_capacity(pairs.len());
        let mut created_any = false;
        for &(station_a, station_b) in pairs {
            if !self.station_exists(station_a) || !self.station_exists(station_b) {
                results.push(Err("One or both stations do not exist".to_string()));
                continue;
            }
            merge_uf.entry(station_a).or_insert(station_a);
            merge_uf.entry(station_b).or_insert(station_b);
            if uf_find(&mut merge_uf, station_a) == uf_find(&mut merge_uf, station_b) {
                results.push(Err(
                    "These stations are already merged. Closures are for unmerged station pairs."
                        .to_string(),
                ));
                continue;
            }
            conn_uf
                .entry(station_a.section_id)
                .or_insert(station_a.section_id);
            conn_uf
                .entry(station_b.section_id)
                .or_insert(station_b.section_id);
            let connected = uf_find(&mut conn_uf, station_a.section_id)
                == uf_find(&mut conn_uf, station_b.section_id);
            if !connected
                && !(self.section_has_zero_datum(station_a.section_id)
                    && self.section_has_zero_datum(station_b.section_id))
            {
                results.push(Err("Stations are not connected. They must be in the same connected group to create a loop closure.".to_string()));
                continue;
            }
            if closure_pairs.contains(&norm(station_a, station_b)) {
                results.push(Err(
                    "A loop closure already exists between these stations.".to_string()
                ));
                continue;
            }
            if let Some(shot) = knots.collapsed_shot(station_a, station_b) {
                results.push(Err(super::closure_shots::closure_across_one_shot_refusal(
                    shot,
                )));
                continue;
            }
            let (Some(&ca), Some(&cb)) = (coords.get(&station_a), coords.get(&station_b)) else {
                results.push(Err("Failed to compute coordinates for station".to_string()));
                continue;
            };
            let horizontal_misclosure = ca.horizontal_distance_to(&cb);
            let vertical_misclosure = (ca.z - cb.z).abs();
            let misclosure_distance = ca.distance_to(&cb);
            let path_length =
                self.path_length_dijkstra(station_a, station_b, &merge_adj, Some(&closure_adj));
            let error_percentage = if path_length > 0.0 {
                (misclosure_distance / path_length) * 100.0
            } else {
                0.0
            };
            let (Some(key_a), Some(key_b)) =
                (self.key_for_ref(station_a), self.key_for_ref(station_b))
            else {
                results.push(Err(
                    "Cannot create loop closure: station has no stable anchor".to_string(),
                ));
                continue;
            };
            let id = self.next_closure_id;
            self.next_closure_id += 1;
            let closure = LoopClosure {
                id,
                station_a: key_a,
                station_b: key_b,
                path_length,
                misclosure_distance,
                error_percentage,
                horizontal_misclosure,
                vertical_misclosure,
                strategy: ClosureStrategy::Batch,
                is_aligned: false,
            };
            self.loop_closures.push(closure.clone());
            // Update the closure-dependent invariants so the NEXT closure's
            // connectivity + path-length see this one (matches the sequential path).
            closure_adj.entry(station_a).or_default().push(station_b);
            closure_adj.entry(station_b).or_default().push(station_a);
            closure_pairs.insert(norm(station_a, station_b));
            uf_union(&mut conn_uf, station_a.section_id, station_b.section_id);
            // A `Batch` closure is eligible, so the next pair sees it tied.
            knots.join(station_a, station_b);
            created_any = true;
            results.push(Ok(closure));
        }
        if created_any {
            self.touch();
        }
        results
    }

    /// Remove a loop closure by ID
    pub fn remove_loop_closure(&mut self, closure_id: u64) -> Result<LoopClosure, String> {
        if let Some(pos) = self.loop_closures.iter().position(|lc| lc.id == closure_id) {
            let closure = self.loop_closures.remove(pos);
            self.touch();
            info!(
                closure_id,
                station_a = ?closure.station_a,
                station_b = ?closure.station_b,
                purpose = "remove loop closure",
                "Loop closure removed"
            );
            Ok(closure)
        } else {
            Err("Loop closure not found".to_string())
        }
    }

    /// Unmerge stations. Also removes closures for the specific unmerged pair only.
    /// Returns (success, removed_closures) where removed_closures contains any
    /// loop closures that matched this specific station pair.
    pub fn unmerge_stations(
        &mut self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> (bool, Vec<LoopClosure>) {
        let pos = self.station_merges.iter().position(|m| {
            (self.key_is_ref(&m.station_a, station_a) && self.key_is_ref(&m.station_b, station_b))
                || (self.key_is_ref(&m.station_a, station_b)
                    && self.key_is_ref(&m.station_b, station_a))
        });

        if let Some(pos) = pos {
            self.station_merges.remove(pos);

            // Decide which closures match the unmerged pair before the retain -
            // `resolve_key` needs `&self`, but `retain` borrows `loop_closures`
            // mutably, so the resolution can't happen inside the closure.
            let remove_flags: Vec<bool> = self
                .loop_closures
                .iter()
                .map(|lc| {
                    (self.key_is_ref(&lc.station_a, station_a)
                        && self.key_is_ref(&lc.station_b, station_b))
                        || (self.key_is_ref(&lc.station_a, station_b)
                            && self.key_is_ref(&lc.station_b, station_a))
                })
                .collect();

            // Remove closures for the specific unmerged pair only
            let mut removed_closures = Vec::new();
            let mut flag_iter = remove_flags.into_iter();
            self.loop_closures.retain(|lc| {
                if flag_iter.next() == Some(true) {
                    removed_closures.push(lc.clone());
                    false
                } else {
                    true
                }
            });

            self.touch();
            (true, removed_closures)
        } else {
            (false, Vec::new())
        }
    }

    /// Check if two stations are merged
    pub fn are_stations_merged(&self, a: StationRef, b: StationRef) -> bool {
        if a == b {
            return true;
        }
        // Check transitive merge equivalence using BFS through merge graph.
        // Two stations are "merged" if there's any chain of merges connecting them
        // (i.e., they're in the same merge group).
        //
        // Resolve every merge endpoint to a station-level adjacency map ONCE
        // (resolve_key is O(1)), then BFS over that. The previous version
        // re-scanned ALL `station_merges` at every BFS node - O(group × merges)
        // per call - which made a batch import that checks this once per loop
        // closure (with thousands of merges) quadratic-to-cubic.
        let adjacency = self.build_station_merge_adjacency();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(a);
        queue.push_back(a);

        while let Some(current) = queue.pop_front() {
            let Some(neighbors) = adjacency.get(&current) else {
                continue;
            };
            for &n in neighbors {
                if n == b {
                    return true;
                }
                if visited.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        false
    }

    /// Check if two stations are in a loop closure
    pub fn are_stations_in_closure(&self, a: StationRef, b: StationRef) -> bool {
        self.loop_closures.iter().any(|lc| {
            (self.key_is_ref(&lc.station_a, a) && self.key_is_ref(&lc.station_b, b))
                || (self.key_is_ref(&lc.station_a, b) && self.key_is_ref(&lc.station_b, a))
        })
    }

    /// Check if a station reference is valid
    pub fn station_exists(&self, station_ref: StationRef) -> bool {
        if let Some(section) = self.get_section(station_ref.section_id) {
            // Station index 0 is before first shot, subsequent indices are after each shot
            station_ref.station_index as usize <= section.shots.len()
        } else {
            false
        }
    }

    /// The zero datum (other than `station_ref` itself) that already anchors the
    /// merge-connected rigid group `station_ref` belongs to, if any. Used to reject
    /// setting a second datum on an already-anchored group. Merge-only connectivity
    /// via the shared [`Self::component_datum_refs`] source of truth.
    fn find_connected_zero_datum(&self, station_ref: StationRef) -> Option<StationRef> {
        self.component_datum_refs(station_ref.section_id)
            .into_iter()
            .find(|&sr| sr != station_ref)
    }

    /// Format a station reference for user display.
    /// Uses section name if available, otherwise "Section N" (0-indexed to match canonical IDs).
    /// Station index is displayed as 0-indexed to match UI format (e.g., "Section #0").
    fn format_station_ref_for_display(&self, station_ref: StationRef) -> String {
        let section_label = self
            .sections
            .iter()
            .find(|s| s.id == station_ref.section_id)
            .and_then(|s| s.name.as_ref())
            .map_or_else(
                || format!("Section {}", station_ref.section_id),
                |name| format!("\"{name}\""),
            );

        // Display station index as 0-indexed to match canonical StationRef format
        format!("{} #{}", section_label, station_ref.station_index)
    }

    /// Set a station as the zero datum and reposition the survey around it.
    ///
    /// When a GPS reference is set:
    /// - For the FIRST datum (the survey had no resolvable one): the station ends
    ///   at (0, 0), reached by translating EVERY section rigidly
    ///   ([`Self::renormalize_frame`]) rather than only the datum's merge group.
    ///   Establishing the frame is what gives local (0, 0) a meaning, and nothing
    ///   was placed relative to that meaning yet, so the survey moves as one body
    ///   and every relative position survives.
    /// - For subsequent datums: the station is positioned at the GPS offset from
    ///   the frame datum, by moving that station's merge-connected group only.
    /// - Re-setting the frame datum's own GPS keeps it at (0, 0): it IS the origin.
    ///
    /// "First" here means the first datum that still RESOLVES, matching
    /// [`Self::frame_datum_key`] and the wire; a dangling datum anchors nothing.
    ///
    /// Z-coordinates (depths) are preserved and not affected by GPS positioning.
    ///
    /// Returns Err if the station doesn't exist, GPS is invalid, or if a connected station
    /// already has a zero datum.
    pub fn set_zero_datum(
        &mut self,
        station_ref: StationRef,
        gps: GpsCoordinates,
    ) -> Result<bool, String> {
        if !self.station_exists(station_ref) {
            return Err("Station does not exist".to_string());
        }

        // Validate GPS coordinates to prevent serialization issues
        GpsCoordinates::validate(gps.latitude, gps.longitude, gps.depth_reference)?;

        // Stable stored identity for this datum's station.
        let datum_key = self
            .key_for_ref(station_ref)
            .ok_or_else(|| "Cannot set zero datum: station has no stable anchor".to_string())?;

        // Check for connected zero datum
        if let Some(existing) = self.find_connected_zero_datum(station_ref) {
            let new_station = self.format_station_ref_for_display(station_ref);
            let existing_station = self.format_station_ref_for_display(existing);
            return Err(format!(
                "Cannot set zero datum: {new_station} is connected to {existing_station} which already has a zero datum"
            ));
        }

        info!(
            station_ref = ?station_ref,
            gps_lat = gps.latitude,
            gps_lng = gps.longitude,
            purpose = "anchor survey to GPS",
            "Setting zero datum"
        );

        // Get all sections connected through merges (rigid group for offset propagation)
        let connected_sections = self.get_merged_section_ids(station_ref.section_id);
        debug!(
            station_ref = ?station_ref,
            connected_section_count = connected_sections.len(),
            connected_sections = ?connected_sections,
            purpose = "reposition connected sections",
            "Computed connected sections for zero datum"
        );

        // Compute current raw coordinates of the datum station
        let current_coords = self
            .compute_station_raw_coords(station_ref)
            .ok_or_else(|| "Failed to compute station coordinates".to_string())?;
        debug!(
            station_ref = ?station_ref,
            coords = ?current_coords,
            purpose = "reposition connected sections",
            "Computed raw station coordinates for zero datum"
        );

        // The datum that defines the frame RIGHT NOW, if the survey has one
        // ([`Self::frame_datum`]: the first RESOLVABLE one, which is what the wire,
        // the map's origin and the renormalisation gate all read).
        //
        // Read BEFORE the store below, and that ORDER IS LOAD-BEARING: after the
        // push this datum is itself a resolvable datum, so `frame_datum()` can
        // never come back `None` again and a "simplification" that re-read it
        // afterwards would silently stop re-framing anything. What this captures is
        // "did the survey have a frame when the call began".
        let frame_datum = self.frame_datum().map(|d| (d.station_ref.clone(), d.gps));

        // Where this datum's station has to end up, or `None` when this add
        // ESTABLISHES the frame and the whole survey moves instead (below).
        let target = match &frame_datum {
            Some((frame_key, _)) if *frame_key == datum_key => {
                debug!(
                    station_ref = ?station_ref,
                    purpose = "anchor survey to GPS",
                    "Updating the frame datum; keeping at origin"
                );
                Some((0.0, 0.0))
            }
            Some((_, frame_gps)) => {
                // Subsequent datum: compute GPS offset from the frame datum
                let (east, north) = gps_to_local(&gps, frame_gps);
                debug!(
                    station_ref = ?station_ref,
                    offset_x = east,
                    offset_y = north,
                    purpose = "anchor survey to GPS",
                    "Computed GPS offset from the frame datum"
                );
                Some((east, north))
            }
            None => {
                debug!(
                    station_ref = ?station_ref,
                    purpose = "anchor survey to GPS",
                    "First datum; the whole survey will be re-framed onto it"
                );
                None
            }
        };

        // Move the datum's rigid group onto its target. Skipped entirely when this
        // add establishes the frame: there is nothing to be placed RELATIVE to yet,
        // and moving one group would move it relative to every other component.
        if let Some((target_x, target_y)) = target {
            // Compute the offset needed to move station from current position to target
            let offset_delta_x = target_x - current_coords.x;
            let offset_delta_y = target_y - current_coords.y;
            debug!(
                station_ref = ?station_ref,
                offset_delta_x,
                offset_delta_y,
                purpose = "reposition connected sections",
                "Computed offset delta for zero datum"
            );

            // Apply offset to all connected sections (rigid group)
            self.adjust_rigid_group_offsets(station_ref.section_id, offset_delta_x, offset_delta_y);
        }

        // Cascade through aligned closures that may have been affected
        self.cascade_connection_offsets(&[station_ref.section_id]);
        self.debug_assert_merge_alignment("set_zero_datum");

        // Store the GPS datum
        // Update in-place if datum exists (preserves order/position for stable center)
        let mut updated_existing = false;
        if let Some(existing) = self
            .zero_datums
            .iter_mut()
            .find(|zd| zd.station_ref == datum_key)
        {
            existing.gps = gps;
            updated_existing = true;
        } else {
            self.zero_datums.push(ZeroDatum {
                station_ref: datum_key,
                gps,
            });
        }

        // The survey had no frame and now has one, so local (0, 0) has just
        // acquired a MEANING it did not have: this datum's GPS. Re-frame onto it
        // the same way every other frame change does
        // ([`Self::renormalize_frame`]) - one rigid translation of EVERY section,
        // free-floating components included. Placing only the datum's own merge
        // group would move that group relative to the rest of the survey, which is
        // the one thing a georeference change must not do; before the frame
        // existed the absolute coordinates were arbitrary and only the relative
        // arrangement was real, and a rigid translation preserves exactly that.
        //
        // Deliberately HERE and not left to `AppState`'s epilogue: the
        // subsequent-datum arithmetic above reads "the frame datum's station sits
        // at (0, 0)" on every call, so that invariant has to hold when this
        // function returns and not merely by the end of the command. The paths
        // with no epilogue (a batch's second datum, the import placement loop,
        // redo's re-execute) are exactly the ones that would otherwise place a
        // second datum against an origin the first one does not sit on.
        // Idempotent, so the epilogue's own `renormalize_frame_if_changed` is a
        // measured no-op after this.
        if frame_datum.is_none() {
            self.renormalize_frame();
        }

        self.touch();
        info!(
            station_ref = ?station_ref,
            updated_existing,
            connected_section_count = connected_sections.len(),
            total_zero_datums = self.zero_datums.len(),
            purpose = "anchor survey to GPS",
            "Zero datum set successfully"
        );
        Ok(true)
    }

    /// Store a zero datum without triggering section repositioning.
    ///
    /// This is an internal helper for the copy shared by "Merge projects" and
    /// "Add .stalx" and for import paths that store a datum where the survey
    /// already is. Nothing moves here; a copy that has to land at the datum's
    /// GPS position is placed afterwards (`place_by_datum` in the core crate).
    ///
    /// Returns Err if validation fails (same rules as `set_zero_datum`).
    pub fn store_zero_datum_without_repositioning(
        &mut self,
        station_ref: StationRef,
        gps: GpsCoordinates,
    ) -> Result<bool, String> {
        if !self.station_exists(station_ref) {
            return Err("Station does not exist".to_string());
        }

        // Validate GPS coordinates
        GpsCoordinates::validate(gps.latitude, gps.longitude, gps.depth_reference)?;

        // Stable stored identity for this datum's station.
        let datum_key = self
            .key_for_ref(station_ref)
            .ok_or_else(|| "Cannot store zero datum: station has no stable anchor".to_string())?;

        // Check for connected zero datum (same validation as set_zero_datum)
        if let Some(existing) = self.find_connected_zero_datum(station_ref) {
            let new_station = self.format_station_ref_for_display(station_ref);
            let existing_station = self.format_station_ref_for_display(existing);
            return Err(format!(
                "Cannot store zero datum: {new_station} is connected to {existing_station} which already has a zero datum"
            ));
        }

        debug!(
            station_ref = ?station_ref,
            gps_lat = gps.latitude,
            gps_lng = gps.longitude,
            purpose = "store zero datum without repositioning",
            "Storing zero datum without repositioning"
        );

        // Store the GPS datum without computing or applying offsets
        let mut updated_existing = false;
        if let Some(existing) = self
            .zero_datums
            .iter_mut()
            .find(|zd| zd.station_ref == datum_key)
        {
            existing.gps = gps;
            updated_existing = true;
        } else {
            self.zero_datums.push(ZeroDatum {
                station_ref: datum_key,
                gps,
            });
        }

        self.touch();
        debug!(
            station_ref = ?station_ref,
            updated_existing,
            total_zero_datums = self.zero_datums.len(),
            purpose = "store zero datum without repositioning",
            "Zero datum stored without repositioning"
        );
        Ok(true)
    }

    /// Remove zero datum from a station
    pub fn remove_zero_datum(&mut self, station_ref: StationRef) -> bool {
        let len_before = self.zero_datums.len();
        let target_key = self.key_for_ref(station_ref);
        self.zero_datums
            .retain(|zd| Some(&zd.station_ref) != target_key.as_ref());
        if self.zero_datums.len() == len_before {
            debug!(
                station_ref = ?station_ref,
                purpose = "remove zero datum",
                "Zero datum not found for removal"
            );
            false
        } else {
            self.touch();
            info!(
                station_ref = ?station_ref,
                remaining_zero_datums = self.zero_datums.len(),
                purpose = "remove zero datum",
                "Zero datum removed"
            );
            true
        }
    }

    /// Get the primary zero datum by POSITION (first one, or None).
    ///
    /// Not the frame: [`Self::frame_datum`] is, and anything that reads a datum in
    /// order to interpret LOCAL coordinates has to use that one. This accessor
    /// answers a different question - "what is `zero_datums[0]`" - and has no
    /// callers today.
    pub fn primary_zero_datum(&self) -> Option<&ZeroDatum> {
        self.zero_datums.first()
    }

    /// THE datum that defines the georeference frame: the first one that still
    /// RESOLVES.
    ///
    /// "First that resolves" rather than plain `zero_datums.first()` because a
    /// dangling datum is not on the wire at all (`Project::datum_to_wire` drops
    /// it), so the frontend map and the data table take their GPS origin from the
    /// first RESOLVABLE one. Every consumer that reads a datum in order to make
    /// sense of LOCAL metres routes through here - the renormalisation gate,
    /// `set_zero_datum`'s placement of a subsequent datum, the shot-edit realign's
    /// absolute fallback and the Ariane export's `START` lat/long - so the frame
    /// rule has exactly one definition and the thing the user looks at agrees with
    /// it.
    pub fn frame_datum(&self) -> Option<&ZeroDatum> {
        self.zero_datums
            .iter()
            .find(|d| self.resolve_key(&d.station_ref).is_some())
    }

    /// The station whose local position IS the survey's georeference origin:
    /// [`Self::frame_datum`]'s anchor.
    pub fn frame_datum_key(&self) -> Option<StationKey> {
        self.frame_datum().map(|d| d.station_ref.clone())
    }

    /// Translate the WHOLE survey so the frame-defining datum's station sits at
    /// local `(0, 0)` again. Returns true when anything moved.
    ///
    /// # Why this exists
    ///
    /// Local `(0, 0)` is not a neutral coordinate: it MEANS the frame datum's GPS
    /// ([`Self::frame_datum`], the first datum that RESOLVES).
    /// [`Self::set_zero_datum`] establishes that by landing the first datum
    /// on the origin - through THIS function, so the survey re-frames as one rigid
    /// body - and every later one at `gps_to_local(its gps, frame.gps)`,
    /// and every reader of local metres relies on it - the map's `gpsOrigin`, the
    /// data table's GPS column, the Ariane export's `START` lat/long. So the frame
    /// is the first RESOLVABLE datum BY POSITION IN THAT LIST, and anything that
    /// changes WHICH datum that is re-defines what every stored coordinate means. [`Self::remove_zero_datum`] is a bare `retain`: it promotes the next
    /// datum and moves nothing, and `resolve_offsets` cannot repair that (the
    /// cascade marks datum sections authoritative and never re-derives their
    /// offsets), so the survey stays laid out against the datum that is gone while
    /// the app now reads it against the one that is left. Nothing warns, no
    /// command asked for it, and the whole cave slides by the inter-datum vector
    /// on the map and in every export (#469, item 1; #413 finding 26 is the same
    /// mechanism reached one edit later).
    ///
    /// The fix preserves the thing the user cares about - every station's
    /// REAL-WORLD position - and changes only the arbitrary part, the local
    /// coordinates: one rigid translation of every section by minus the promoted
    /// datum station's current position.
    ///
    /// # Measure, do not re-derive
    ///
    /// The delta is `-(measured position of the promoted datum station)`, not
    /// `-gps_to_local(G_new, G_old)`. For a self-consistent project the two are
    /// the same number (that identity is exactly what `set_zero_datum` wrote).
    /// For one that is already inconsistent - a project merged before #682, whose
    /// later sources' datums were copied without being placed by their GPS -
    /// only the measured form restores the app's own invariant,
    /// and only the measured form is IDEMPOTENT: a second application measures
    /// zero and does nothing. That is the same choice
    /// [`Self::datum_anchor_position`] already made for shot edits.
    ///
    /// # What is NOT rewritten
    ///
    /// * `ZeroDatum::gps` - rewriting a measured GPS fix would falsify data.
    /// * `station_offsets` - they are relative to the walked position.
    /// * shot geometry - no closure or merge metric goes stale from a translation.
    ///
    /// Datum-less, free-floating components translate too: their local
    /// coordinates are read against the same origin, so leaving them behind would
    /// move them RELATIVE to the anchored survey, which is the one thing a
    /// georeference change must not do.
    ///
    /// Every write goes through [`Self::adjust_section_offset`] and therefore
    /// [`Self::write_section_offset`], so the dust-zero invariant holds: the
    /// promoted datum's own section normally ends with no key at all.
    pub(crate) fn renormalize_frame(&mut self) -> bool {
        let Some(key) = self.frame_datum_key() else {
            return false;
        };
        let Some(current) = self
            .resolve_key(&key)
            .and_then(|sr| self.compute_station_raw_coords(sr))
        else {
            return false;
        };

        let (dx, dy) = (-current.x, -current.y);
        // Dust is not a displacement - the shared bound, see
        // `Project::OFFSET_DUST_M`. This is also what makes a second call a
        // provable no-op rather than a churn of near-zero writes.
        if dx.abs() <= Self::OFFSET_DUST_M && dy.abs() <= Self::OFFSET_DUST_M {
            return false;
        }

        let section_ids: Vec<SectionId> = self.sections.iter().map(|s| s.id).collect();
        for id in section_ids {
            self.adjust_section_offset(id, dx, dy);
        }

        info!(
            datum_section = key.section_id,
            delta_x = dx,
            delta_y = dy,
            purpose = "renormalise georeference frame",
            "The frame-defining zero datum changed; translated the survey onto the new origin"
        );
        true
    }

    /// [`Self::renormalize_frame`], but only when the frame-defining station is a
    /// DIFFERENT one than `before` named. THE gate - every trigger site routes
    /// through it so the rule lives in one place.
    ///
    /// The AFTER side must be `Some`: `Some -> None` removed the last anchor, so
    /// there is no frame left to renormalise onto and translating onto nothing
    /// would move the survey for no reason. `None -> Some` DOES fire - it
    /// establishes the frame, which is a change of what every stored coordinate
    /// means, and the answer to that is the same rigid translation as any other
    /// promotion. [`Self::set_zero_datum`] already performs it inline (it has to:
    /// its own subsequent-datum arithmetic reads the invariant), so on that route
    /// this is a measured no-op; the gate covers it anyway rather than asserting
    /// that no other route can ever establish a frame, which is the claim the
    /// derive-don't-hook rule exists to avoid having to make.
    ///
    /// `before == after` covers the ordinary command and makes a second call after
    /// a first one a no-op by identity rather than by arithmetic.
    pub fn renormalize_frame_if_changed(&mut self, before: Option<StationKey>) -> bool {
        let Some(after) = self.frame_datum_key() else {
            return false;
        };
        if before.as_ref() == Some(&after) {
            return false;
        }
        self.renormalize_frame()
    }

    /// The world position of the anchor [`Self::realign_after_shot_edit`] will
    /// hold, measured BEFORE the edit is applied.
    ///
    /// A shot edit must keep the group's datum station where it already is; it
    /// may not re-georeference the survey. Those are the same thing for a
    /// project whose datum sits where its GPS implies, and they part company for
    /// one that does not - which the app can reach without any command asking
    /// for it, because the georeference origin is a POSITION in the datum list
    /// (the first entry that resolves - [`Self::frame_datum`]) rather than a
    /// property of any one datum.
    /// Removing the first datum ([`Self::remove_zero_datum`]) promotes the next
    /// one and moves nothing, so every group placed relative to the old origin
    /// is left contradicting its own datum. Re-deriving the ABSOLUTE target on
    /// that contradiction translates the whole rigid group by the distance
    /// between the two GPS anchors, inside an `update_shot` whose
    /// `UndoData::PreviousValues` records the shot's fields and no offsets - so
    /// the translation cannot be undone (#413 finding 26).
    ///
    /// `None` when the group holds no resolvable datum - there is nothing
    /// absolute to hold, and the realign anchors on the edited section.
    pub fn datum_anchor_position(&self, section_id: SectionId) -> Option<(f64, f64)> {
        let merge_group = self.get_merged_section_ids(section_id);
        let datum = self.zero_datums.iter().find(|d| {
            merge_group.contains(&d.station_ref.section_id)
                && self.resolve_key(&d.station_ref).is_some()
        })?;
        let coords = self
            .resolve_key(&datum.station_ref)
            .and_then(|sr| self.compute_station_raw_coords(sr))?;
        Some((coords.x, coords.y))
    }

    /// After a geometry-changing shot edit in `edited_section_id`, realign the
    /// merge group's section offsets so that anchored stations (zero datums,
    /// merge points to pinned sections) remain at their correct positions.
    ///
    /// Stations upstream of the edit absorb the length change; stations downstream
    /// (including merge points and closure endpoints) remain stationary.
    ///
    /// Algorithm:
    /// 1. Pick the group's anchor: its zero datum if it has one, else the edited
    ///    section itself
    /// 2. Reanchor the datum section (adjust offset to keep the datum where
    ///    `hold_anchor_at` says the edit found it, or - with no held position -
    ///    at its GPS target). Skipped for an unanchored group: there is nothing
    ///    absolute to hold, and the edited section keeps the offset it has
    /// 3. BFS through merges from the anchor: adjust each partner section's
    ///    offset so merge points stay aligned
    ///
    /// A group with NO zero datum still has RELATIVE constraints - every merge in
    /// it must stay co-located - so it is realigned against one of its own
    /// sections rather than skipped. The edited section is that anchor: it is the
    /// one whose geometry the user just changed, so holding it still and moving
    /// its partners is the least surprising reading of the edit, and it makes the
    /// realign idempotent (a second call finds everything already aligned).
    /// Skipping it instead left the merge drifted by exactly the edit, under the
    /// app's own 5 cm reporting threshold and OVER the 1 cm its
    /// `debug_assert_merge_alignment` calls a violation - so the next command
    /// that asserted, in any section, panicked on a merge this edit broke
    /// (#413 finding 20).
    ///
    /// Idempotent: calling multiple times produces the same result.
    ///
    /// `hold_anchor_at` is the datum station's world position measured BEFORE
    /// the edit ([`Self::datum_anchor_position`]). Passing it holds the anchor
    /// exactly where the edit found it; `None` falls back to the datum's
    /// GPS-implied target, which is the same position for a project whose frame
    /// is self-consistent and a re-georeference for one whose is not (#413
    /// finding 26).
    pub fn realign_after_shot_edit(
        &mut self,
        edited_section_id: SectionId,
        hold_anchor_at: Option<(f64, f64)>,
    ) {
        // Find the merge group containing the edited section
        let merge_group = self.get_merged_section_ids(edited_section_id);

        // Find the zero datum for this merge group (if any). A dangling datum
        // (anchor key no longer resolves) is not a real anchor - skip it so the
        // realign does not pin the group to a phantom anchor (#174); the group is
        // then treated as unanchored and realigned against the edited section.
        let datum = self
            .zero_datums
            .iter()
            .find(|d| {
                merge_group.contains(&d.station_ref.section_id)
                    && self.resolve_key(&d.station_ref).is_some()
            })
            .cloned();

        // Step 1: reanchor the datum section's offset to keep the datum at its
        // target. Nothing to do for an unanchored group - it has no absolute
        // position to restore, only the relative merge constraints Step 2 solves.
        let anchor_section_id = match &datum {
            Some(datum) => {
                // Determine the datum's target position. Against the FRAME datum
                // ([`Self::frame_datum`], the first RESOLVABLE one), never against
                // `zero_datums[0]`: a dangling datum is not on the wire, is not
                // what the survey is laid out against, and re-deriving an absolute
                // target from its GPS would translate this group by the whole
                // inter-datum vector.
                let frame = self.frame_datum();
                let is_frame_datum = frame.is_some_and(|d| d.station_ref == datum.station_ref);

                let (target_x, target_y) = if let Some(held) = hold_anchor_at {
                    held
                } else if is_frame_datum {
                    (0.0, 0.0)
                } else if let Some(frame) = frame {
                    gps_to_local(&datum.gps, &frame.gps)
                } else {
                    return;
                };

                let datum_section_id = datum.station_ref.section_id;
                if let Some(current) = self
                    .resolve_key(&datum.station_ref)
                    .and_then(|sr| self.compute_station_raw_coords(sr))
                {
                    let delta_x = target_x - current.x;
                    let delta_y = target_y - current.y;
                    // Dust is not a displacement - the shared bound, see
                    // `Project::OFFSET_DUST_M` and its twin in `cascade_bfs`.
                    if delta_x.abs() > Self::OFFSET_DUST_M || delta_y.abs() > Self::OFFSET_DUST_M {
                        self.adjust_section_offset(datum_section_id, delta_x, delta_y);
                    }
                }
                datum_section_id
            }
            None => edited_section_id,
        };

        // Step 2: BFS through merges from the anchor section, adjusting each
        // partner section's offset so its merge station matches the already-
        // aligned station on the other side.
        let merges = self.station_merges.clone();
        let mut visited = HashSet::new();
        visited.insert(anchor_section_id);
        let mut queue = VecDeque::new();
        queue.push_back(anchor_section_id);

        while let Some(current_section) = queue.pop_front() {
            for merge in &merges {
                let (this_station, partner_station) = if merge.station_a.section_id
                    == current_section
                    && !visited.contains(&merge.station_b.section_id)
                {
                    (&merge.station_a, &merge.station_b)
                } else if merge.station_b.section_id == current_section
                    && !visited.contains(&merge.station_a.section_id)
                {
                    (&merge.station_b, &merge.station_a)
                } else {
                    continue;
                };

                let partner_section = partner_station.section_id;
                visited.insert(partner_section);

                // this_station's section is already aligned; partner needs adjustment
                if let (Some(this_pos), Some(partner_pos)) = (
                    self.resolve_key(this_station)
                        .and_then(|sr| self.compute_station_raw_coords(sr)),
                    self.resolve_key(partner_station)
                        .and_then(|sr| self.compute_station_raw_coords(sr)),
                ) {
                    let delta_x = this_pos.x - partner_pos.x;
                    let delta_y = this_pos.y - partner_pos.y;
                    // Same shared bound as the gate above and `cascade_bfs`.
                    if delta_x.abs() > Self::OFFSET_DUST_M || delta_y.abs() > Self::OFFSET_DUST_M {
                        self.adjust_section_offset(partner_section, delta_x, delta_y);
                    }
                }

                queue.push_back(partner_section);
            }
        }
    }
}
