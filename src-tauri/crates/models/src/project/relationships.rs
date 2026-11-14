use chrono::Utc;
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::debug;

use crate::{LoopClosureWire, SectionId, StationKey, StationMerge, StationMergeWire, StationRef};

use super::{Project, SectionRelationships};

impl Project {
    /// Query relationships that reference the given section.
    /// Returns merges, closures, offset info, and zero datum count.
    pub fn get_section_relationships(&self, section_id: SectionId) -> SectionRelationships {
        let merges: Vec<StationMergeWire> = self
            .station_merges
            .iter()
            .filter(|m| {
                m.station_a.section_id == section_id || m.station_b.section_id == section_id
            })
            .filter_map(|m| self.merge_to_wire(m))
            .collect();

        let closures: Vec<LoopClosureWire> = self
            .loop_closures
            .iter()
            .filter(|lc| {
                lc.station_a.section_id == section_id || lc.station_b.section_id == section_id
            })
            .filter_map(|lc| self.closure_to_wire(lc))
            .collect();

        let has_section_offset = self.section_offsets.contains_key(&section_id);

        let station_offset_count = self
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == section_id)
            .count();

        let zero_datum_count = self
            .zero_datums
            .iter()
            .filter(|zd| zd.station_ref.section_id == section_id)
            .count();

        SectionRelationships {
            merges,
            closures,
            has_section_offset,
            station_offset_count,
            zero_datum_count,
        }
    }

    /// Preserve merge-group connectivity and rewire closures before removing a section.
    ///
    /// For every station on the deleted section that participates in merges with
    /// stations on OTHER sections, we find the merge group, remove the deleted
    /// station, compute connected components among the remaining stations, and
    /// create replacement merges so that the group stays connected.
    ///
    /// For closures that reference a station on the deleted section, we rewire
    /// them to a merge-group partner on another section if one exists.
    pub fn preserve_connections_for_section_deletion(&mut self, section_id: SectionId) {
        let station_count = self
            .get_section(section_id)
            .map(|s| s.shots.len() + 1)
            .unwrap_or(0);

        // Collect all stations on this section
        let section_stations: HashSet<StationRef> = (0..station_count as u32)
            .map(|i| StationRef::new(section_id, i))
            .collect();

        // --- Merge preservation ---
        // Track which merge groups we've already processed (by a canonical member)
        let mut processed_groups: HashSet<StationRef> = HashSet::new();
        let mut replacement_merges: Vec<(StationRef, StationRef)> = Vec::new();

        for &station in &section_stations {
            let group = self.get_merge_group(station);

            // Only process groups that touch other sections
            let external_members: HashSet<StationRef> = group
                .iter()
                .filter(|s| s.section_id != section_id)
                .copied()
                .collect();

            if external_members.is_empty() {
                continue;
            }

            // Skip if we already processed this group via another station
            if external_members
                .iter()
                .any(|s| processed_groups.contains(s))
            {
                continue;
            }
            for &s in &external_members {
                processed_groups.insert(s);
            }

            // Build components among the external members after removing ALL
            // section stations from the graph
            let components =
                self.build_merge_components_excluding_section(&external_members, section_id);

            if components.len() > 1 {
                // Connect each disconnected component to the first one
                let base = &components[0];
                for component in components.iter().skip(1) {
                    // Find a cross-section pair (prefer different sections)
                    let mut found = None;
                    for a in base {
                        for b in component {
                            if a.section_id != b.section_id {
                                found = Some((*a, *b));
                                break;
                            }
                        }
                        if found.is_some() {
                            break;
                        }
                    }
                    // Fall back to same-section pair if needed
                    if found.is_none() {
                        if let (Some(a), Some(b)) = (base.first(), component.first()) {
                            found = Some((*a, *b));
                        }
                    }
                    if let Some((a, b)) = found {
                        if self.replacement_merge_allowed(
                            a,
                            b,
                            &section_stations,
                            &replacement_merges,
                        ) && !replacement_merges
                            .iter()
                            .any(|(ra, rb)| (*ra == a && *rb == b) || (*ra == b && *rb == a))
                        {
                            replacement_merges.push((a, b));
                        }
                    }
                }
            }
        }

        // Add replacement merges (via the helper, which keeps the adjacency memo
        // warm for the BFS calls later in this transaction).
        for (a, b) in &replacement_merges {
            let (Some(ka), Some(kb)) = (self.key_for_ref(*a), self.key_for_ref(*b)) else {
                continue;
            };
            self.record_station_merge(StationMerge {
                station_a: ka,
                station_b: kb,
                merged_at: Utc::now(),
            });
            debug!(
                a_section = a.section_id,
                a_station = a.station_index,
                b_section = b.section_id,
                b_station = b.station_index,
                "Added replacement merge during section deletion"
            );
        }

        // --- Closure rewiring ---
        let closures_to_rewire: Vec<_> = self
            .loop_closures
            .iter()
            .filter(|lc| {
                lc.station_a.section_id == section_id || lc.station_b.section_id == section_id
            })
            .map(|lc| (lc.id, lc.station_a.clone(), lc.station_b.clone()))
            .collect();

        for (closure_id, station_a, station_b) in closures_to_rewire {
            let (removed_station, other_station) = if station_a.section_id == section_id {
                (station_a, station_b)
            } else {
                (station_b, station_a)
            };

            // If both sides are on the deleted section, it will just be removed
            if other_station.section_id == section_id {
                continue;
            }
            // Resolve the keys to positional refs for the merge-group walk / checks.
            let (Some(removed_ref), Some(other_ref)) = (
                self.resolve_key(&removed_station),
                self.resolve_key(&other_station),
            ) else {
                continue;
            };

            // Find a merge-group partner on another section to rewire to
            let group = self.get_merge_group(removed_ref);
            let mut candidates: Vec<StationRef> = group
                .into_iter()
                .filter(|s| s.section_id != section_id)
                .collect();
            candidates.sort_by_key(Self::station_ref_key);

            if let Some(replacement) = candidates.first().copied() {
                // Don't rewire if it would create a self-referencing or duplicate closure
                if replacement == other_ref
                    || self.are_stations_merged(replacement, other_ref)
                    || self.are_stations_in_closure(replacement, other_ref)
                {
                    continue;
                }
                let Some(replacement_key) = self.key_for_ref(replacement) else {
                    continue;
                };

                // Rewire the closure in place
                if let Some(lc) = self.loop_closures.iter_mut().find(|lc| lc.id == closure_id) {
                    if lc.station_a.section_id == section_id {
                        lc.station_a = replacement_key;
                    } else {
                        lc.station_b = replacement_key;
                    }
                    debug!(
                        closure_id,
                        replacement_section = replacement.section_id,
                        replacement_station = replacement.station_index,
                        "Rewired closure during section deletion"
                    );
                }
            }
        }
    }

    /// May a deletion auto-invent a replacement merge between `a` and `b`?
    ///
    /// Every "replacement merge" site stitches a torn merge group back together
    /// through an ARBITRARILY picked surviving pair. Left unguarded that mints
    /// exactly the states the rest of the integrity machinery exists to repair:
    /// a merge sharing a station pair with a loop closure (what
    /// [`Self::create_loop_closure`] refuses to create), a redundant merge across
    /// sections an aligned closure already holds rigid (what
    /// [`Self::merge_stations`] refuses interactively), or a merge between two
    /// stations that are not in the same place.
    ///
    /// Re-stitching two halves that each hold a zero datum is ALLOWED (#741).
    /// They were one merge component before the deletion (a placement datum and
    /// a GPS fix), the pair is coincident (guard 4), so nothing moves, and the
    /// roles re-derive exactly as they were. Refusing it would split a cave the
    /// user merged on purpose into two pieces.
    ///
    /// `doomed` is the set of stations the deletion is about to remove, and
    /// `planned` the replacement merges already accepted in this batch. Guard 3
    /// asks about the graph that REMAINS: every site runs BEFORE the doomed
    /// stations' own merges are dropped, so reading the live graph would see the
    /// very connection the deletion is about to sever and answer "already
    /// connected" for every candidate - and a batch that adds pair after pair
    /// against an unchanging `self` would weld two anchored components together
    /// in two hops.
    ///
    /// `false` means SKIP THIS PAIR: the two components simply stay separate. No
    /// alternative pair is hunted for, and nothing softer is invented in its
    /// place - the pair is arbitrary, so a closure here would fabricate a survey
    /// relationship nobody measured.
    pub(crate) fn replacement_merge_allowed(
        &self,
        a: StationRef,
        b: StationRef,
        doomed: &HashSet<StationRef>,
        planned: &[(StationRef, StationRef)],
    ) -> bool {
        // 1. The merge is already there.
        if self.has_direct_merge(a, b) {
            return false;
        }
        // 2. A closure already describes this pair. Merge-and-closure on one
        //    pair is the state `create_loop_closure` refuses and the demotion
        //    has to normalize; the surviving closure keeps the soft topology,
        //    and `convert_orphan_closures_to_merges` is the designed path to
        //    promote it later if that ever becomes valid.
        if self.are_stations_in_closure(a, b) {
            return false;
        }
        // 3. The sections are already rigidly connected without the doomed
        //    stations - merges plus ALIGNED closures, the same graph
        //    `merge_stations` tests before rejecting a redundant merge. A
        //    same-section pair lands here too: a section's own shots already
        //    hold its stations rigid.
        if self
            .connected_sections_after_removal(a.section_id, doomed, planned, true)
            .contains(&b.section_id)
        {
            return false;
        }
        // 4. The two stations are not in the same place. A merge ASSERTS they
        //    are (`debug_assert_merge_alignment`), and unlike a closure promotion
        //    - which converts a relationship the surveyor recorded, and so pays
        //    for the coincidence by translating the movable side
        //    (`plan_merge_promotion`) - this pair is ARBITRARY: it is whichever
        //    two survivors the component walk happened to reach first. Moving a
        //    survey group onto an arbitrary partner would invent geometry, so a
        //    pair that is not already coincident is simply not stitched. In
        //    practice every genuine repair passes: the pair comes from ONE merge
        //    group, whose members are coincident by the very invariant this
        //    guards.
        //
        //    The THRESHOLD is the invariant's; the FRAME is not the assert's.
        //    This measures `compute_station_raw_xy` on the LIVE, pre-removal
        //    graph, while `debug_assert_merge_alignment` measures after the
        //    removal has shifted station indices and the gap-close has
        //    compensated the offsets. Reading the same number in two frames makes
        //    the parity APPROXIMATE - good enough that a pair the assert would
        //    accept is in practice accepted here, not a proof that it must be.
        //
        //    A refusal is a stitch the deletion genuinely loses: a merge group
        //    already carrying a beyond-invariant residual (the `RealignFixpoint`
        //    irreducible case) fragments into two components instead of being
        //    re-stitched, so this logs at WARN with the measured gap rather than
        //    at debug. It deliberately does not set
        //    `DeleteShotImpact::merge_rewire_skipped`: that flag's user-facing
        //    copy names a different cause ("only same-section candidates"), this
        //    function answers with a bool so no caller can tell the two refusals
        //    apart, and two of its three call sites have no impact struct at all.
        //    Telling the user which one happened needs an IPC field of its own.
        let gap = match (
            self.compute_station_raw_xy(a),
            self.compute_station_raw_xy(b),
        ) {
            (Some((ax, ay)), Some((bx, by))) => {
                Some(((ax - bx).powi(2) + (ay - by).powi(2)).sqrt())
            }
            // An endpoint with no computable position cannot be shown coincident,
            // and a merge onto it would be a constraint nothing can check.
            _ => None,
        };
        let coincident = gap.is_some_and(|gap| gap < Self::MERGE_ALIGNMENT_INVARIANT_M);
        if !coincident {
            tracing::warn!(
                a = ?a,
                b = ?b,
                gap_m = ?gap,
                threshold_m = Self::MERGE_ALIGNMENT_INVARIANT_M,
                purpose = "never mint a merge that asserts a coincidence the geometry lacks",
                "Skipped a replacement merge between two stations that are not co-located"
            );
        }
        coincident
    }

    /// Sections reachable from `from` through merges - and, when
    /// `follow_aligned_closures`, aligned loop closures - once every edge
    /// anchored on a `doomed` station is gone and the `planned` replacement
    /// merges are in place.
    ///
    /// Post-deletion twin of [`Self::get_rigid_connected_section_ids`] /
    /// [`Self::get_merged_section_ids`], which read the live graph (the latter
    /// through a memo that must keep describing the merges that actually exist,
    /// so a "what if these were gone" question must never go through it).
    fn connected_sections_after_removal(
        &self,
        from: SectionId,
        doomed: &HashSet<StationRef>,
        planned: &[(StationRef, StationRef)],
        follow_aligned_closures: bool,
    ) -> HashSet<SectionId> {
        // An endpoint that no longer resolves cannot carry connectivity either
        // (matching `build_merge_components_excluding_section`).
        let survives = |key: &StationKey| -> Option<SectionId> {
            let station = self.resolve_key(key)?;
            (!doomed.contains(&station)).then_some(station.section_id)
        };
        let mut edges: Vec<(SectionId, SectionId)> = Vec::new();
        for merge in &self.station_merges {
            if let (Some(x), Some(y)) = (survives(&merge.station_a), survives(&merge.station_b)) {
                edges.push((x, y));
            }
        }
        for (x, y) in planned {
            edges.push((x.section_id, y.section_id));
        }
        if follow_aligned_closures {
            for closure in self.loop_closures.iter().filter(|lc| lc.is_aligned) {
                if let (Some(x), Some(y)) =
                    (survives(&closure.station_a), survives(&closure.station_b))
                {
                    edges.push((x, y));
                }
            }
        }

        let mut connected: HashSet<SectionId> = HashSet::new();
        let mut queue = vec![from];
        while let Some(current) = queue.pop() {
            if !connected.insert(current) {
                continue;
            }
            for &(x, y) in &edges {
                if x == current && !connected.contains(&y) {
                    queue.push(y);
                } else if y == current && !connected.contains(&x) {
                    queue.push(x);
                }
            }
        }
        connected
    }

    /// Would merging sections `a` and `b` put two zero datums in one rigid merge
    /// group, once the `doomed` stations (and any datum they carry) are gone and
    /// the `planned` replacement merges are in place?
    ///
    /// Post-deletion twin of [`Self::merge_would_double_anchor`], down to the
    /// merges-ONLY component definition. Asked before a deletion PROMOTES a
    /// closure to a merge, which (like every automatic promotion) must not fuse
    /// two placed groups.
    pub(crate) fn would_double_anchor_after_removal(
        &self,
        a: SectionId,
        b: SectionId,
        doomed: &HashSet<StationRef>,
        planned: &[(StationRef, StationRef)],
    ) -> bool {
        let component_a = self.connected_sections_after_removal(a, doomed, planned, false);
        if component_a.contains(&b) {
            // Already one merge component - a redundant merge adds no anchor.
            return false;
        }
        let anchored = |component: &HashSet<SectionId>| {
            self.zero_datums
                .iter()
                .filter_map(|zd| self.resolve_key(&zd.station_ref))
                .filter(|sr| !doomed.contains(sr))
                .any(|sr| component.contains(&sr.section_id))
        };
        anchored(&component_a)
            && anchored(&self.connected_sections_after_removal(b, doomed, planned, false))
    }

    /// Like `build_merge_components` but excludes ALL stations from a given section
    /// (not just a single station). Used during section deletion.
    fn build_merge_components_excluding_section(
        &self,
        stations: &HashSet<StationRef>,
        excluded_section: SectionId,
    ) -> Vec<Vec<StationRef>> {
        let mut adjacency: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for station in stations {
            adjacency.entry(*station).or_default();
        }

        for merge in &self.station_merges {
            if merge.station_a.section_id == excluded_section
                || merge.station_b.section_id == excluded_section
            {
                continue;
            }
            let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) else {
                continue;
            };
            if stations.contains(&a) && stations.contains(&b) {
                adjacency.entry(a).or_default().push(b);
                adjacency.entry(b).or_default().push(a);
            }
        }

        let mut components = Vec::new();
        let mut visited = HashSet::new();

        let mut station_list: Vec<StationRef> = stations.iter().copied().collect();
        station_list.sort_by_key(Self::station_ref_key);

        for station in station_list {
            if visited.contains(&station) {
                continue;
            }
            let mut component = Vec::new();
            let mut queue = VecDeque::new();
            visited.insert(station);
            queue.push_back(station);

            while let Some(current) = queue.pop_front() {
                component.push(current);
                if let Some(neighbors) = adjacency.get(&current) {
                    for neighbor in neighbors {
                        if visited.insert(*neighbor) {
                            queue.push_back(*neighbor);
                        }
                    }
                }
            }

            component.sort_by_key(Self::station_ref_key);
            components.push(component);
        }

        components
    }

    /// Like [`Self::preserve_connections_for_section_deletion`] but for an
    /// arbitrary SET of stations being removed (e.g. the interior stations
    /// dropped by a contiguous range delete) rather than a whole section. Adds
    /// replacement merges so external merge groups stay connected after the
    /// stations vanish, and rewires closures off the removed stations to a
    /// surviving merge-group partner. Call this BEFORE dropping the removed
    /// stations' own refs - it reads the live merge groups. Computing the
    /// replacement merges over the ENTIRE removed set at once (rather than
    /// per-station) guarantees every replacement joins two survivors.
    pub fn preserve_connections_for_removed_stations(&mut self, removed: &HashSet<StationRef>) {
        // --- Merge preservation ---
        let mut processed_groups: HashSet<StationRef> = HashSet::new();
        let mut replacement_merges: Vec<(StationRef, StationRef)> = Vec::new();

        for &station in removed {
            let group = self.get_merge_group(station);
            let external_members: HashSet<StationRef> = group
                .iter()
                .filter(|s| !removed.contains(s))
                .copied()
                .collect();
            if external_members.is_empty() {
                continue;
            }
            if external_members
                .iter()
                .any(|s| processed_groups.contains(s))
            {
                continue;
            }
            for &s in &external_members {
                processed_groups.insert(s);
            }

            let components =
                self.build_merge_components_excluding_stations(&external_members, removed);
            if components.len() > 1 {
                let base = &components[0];
                for component in components.iter().skip(1) {
                    let mut found = None;
                    for a in base {
                        for b in component {
                            if a.section_id != b.section_id {
                                found = Some((*a, *b));
                                break;
                            }
                        }
                        if found.is_some() {
                            break;
                        }
                    }
                    if found.is_none() {
                        if let (Some(a), Some(b)) = (base.first(), component.first()) {
                            found = Some((*a, *b));
                        }
                    }
                    if let Some((a, b)) = found {
                        if self.replacement_merge_allowed(a, b, removed, &replacement_merges)
                            && !replacement_merges
                                .iter()
                                .any(|(ra, rb)| (*ra == a && *rb == b) || (*ra == b && *rb == a))
                        {
                            replacement_merges.push((a, b));
                        }
                    }
                }
            }
        }

        for (a, b) in &replacement_merges {
            let (Some(ka), Some(kb)) = (self.key_for_ref(*a), self.key_for_ref(*b)) else {
                continue;
            };
            self.record_station_merge(StationMerge {
                station_a: ka,
                station_b: kb,
                merged_at: Utc::now(),
            });
        }

        // --- Closure rewiring ---
        let closures_to_rewire: Vec<(u64, StationRef, StationRef)> = self
            .loop_closures
            .iter()
            .filter_map(|lc| {
                let a = self.resolve_key(&lc.station_a)?;
                let b = self.resolve_key(&lc.station_b)?;
                if removed.contains(&a) || removed.contains(&b) {
                    Some((lc.id, a, b))
                } else {
                    None
                }
            })
            .collect();

        for (closure_id, ref_a, ref_b) in closures_to_rewire {
            let replace_a = removed.contains(&ref_a);
            let (removed_ref, other_ref) = if replace_a {
                (ref_a, ref_b)
            } else {
                (ref_b, ref_a)
            };
            // If both sides are removed, the closure drops with its stations.
            if removed.contains(&other_ref) {
                continue;
            }
            let group = self.get_merge_group(removed_ref);
            let mut candidates: Vec<StationRef> =
                group.into_iter().filter(|s| !removed.contains(s)).collect();
            candidates.sort_by_key(Self::station_ref_key);
            // Prefer a survivor on a DIFFERENT section than the surviving
            // endpoint so the rewired closure stays cross-section. A same-section
            // closure would be degenerate (a self-loop) and is silently skipped
            // by convert_orphan_closures_to_merges and the integrity checks, so
            // it would persist undetected. If no cross-section survivor exists
            // the loop has genuinely collapsed into one section - leave the
            // closure for cleanup to drop rather than rewire it into a self-loop.
            let replacement = candidates
                .iter()
                .find(|s| s.section_id != other_ref.section_id)
                .copied();
            if let Some(replacement) = replacement {
                if replacement == other_ref
                    || self.are_stations_merged(replacement, other_ref)
                    || self.are_stations_in_closure(replacement, other_ref)
                {
                    continue;
                }
                let Some(replacement_key) = self.key_for_ref(replacement) else {
                    continue;
                };
                if let Some(lc) = self.loop_closures.iter_mut().find(|lc| lc.id == closure_id) {
                    if replace_a {
                        lc.station_a = replacement_key;
                    } else {
                        lc.station_b = replacement_key;
                    }
                }
            }
        }
    }

    /// Like [`Self::build_merge_components_excluding_section`] but excludes any
    /// merge edge whose endpoints resolve into the given `removed` station set.
    fn build_merge_components_excluding_stations(
        &self,
        stations: &HashSet<StationRef>,
        removed: &HashSet<StationRef>,
    ) -> Vec<Vec<StationRef>> {
        let mut adjacency: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for station in stations {
            adjacency.entry(*station).or_default();
        }

        for merge in &self.station_merges {
            let (Some(a), Some(b)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) else {
                continue;
            };
            if removed.contains(&a) || removed.contains(&b) {
                continue;
            }
            if stations.contains(&a) && stations.contains(&b) {
                adjacency.entry(a).or_default().push(b);
                adjacency.entry(b).or_default().push(a);
            }
        }

        let mut components = Vec::new();
        let mut visited = HashSet::new();
        let mut station_list: Vec<StationRef> = stations.iter().copied().collect();
        station_list.sort_by_key(Self::station_ref_key);

        for station in station_list {
            if visited.contains(&station) {
                continue;
            }
            let mut component = Vec::new();
            let mut queue = VecDeque::new();
            visited.insert(station);
            queue.push_back(station);
            while let Some(current) = queue.pop_front() {
                component.push(current);
                if let Some(neighbors) = adjacency.get(&current) {
                    for neighbor in neighbors {
                        if visited.insert(*neighbor) {
                            queue.push_back(*neighbor);
                        }
                    }
                }
            }
            component.sort_by_key(Self::station_ref_key);
            components.push(component);
        }

        components
    }

    /// Drop every project-level reference (merge, closure, station offset,
    /// station comment, zero datum) anchored on a station in `removed`. Call
    /// AFTER [`Self::preserve_connections_for_removed_stations`] and BEFORE
    /// re-deriving geometry, when removing a sub-range of a section's stations.
    pub fn cleanup_removed_station_references(&mut self, removed: &HashSet<StationRef>) {
        let is_removed = |this: &Self, key: &StationKey| -> bool {
            this.resolve_key(key)
                .is_some_and(|sr| removed.contains(&sr))
        };
        // `mem::take` so the predicate can borrow `self` immutably (resolve_key
        // reads section shots, not these fields) while we rebuild each list.
        let merges = std::mem::take(&mut self.station_merges);
        self.station_merges = merges
            .into_iter()
            .filter(|m| !is_removed(self, &m.station_a) && !is_removed(self, &m.station_b))
            .collect();

        let closures = std::mem::take(&mut self.loop_closures);
        self.loop_closures = closures
            .into_iter()
            .filter(|lc| !is_removed(self, &lc.station_a) && !is_removed(self, &lc.station_b))
            .collect();

        let datums = std::mem::take(&mut self.zero_datums);
        self.zero_datums = datums
            .into_iter()
            .filter(|zd| !is_removed(self, &zd.station_ref))
            .collect();

        let removed_keys: Vec<StationKey> = self
            .station_offsets
            .keys()
            .chain(self.station_comments.keys())
            .chain(self.station_names.keys())
            .filter(|&k| is_removed(self, k))
            .cloned()
            .collect();
        for k in removed_keys {
            self.station_offsets.remove(&k);
            self.station_comments.remove(&k);
            self.station_names.remove(&k);
        }
    }

    /// Remove all merges, closures, section_offsets, station_offsets, and zero_datums
    /// that reference the given section. Call before `remove_section()` to avoid orphans.
    pub fn cleanup_section_references(&mut self, section_id: SectionId) {
        // Remove merges referencing this section.
        self.station_merges.retain(|m| {
            m.station_a.section_id != section_id && m.station_b.section_id != section_id
        });

        // Remove closures referencing this section
        self.loop_closures.retain(|lc| {
            lc.station_a.section_id != section_id && lc.station_b.section_id != section_id
        });

        // Remove section offset
        self.remove_section_offset(section_id);

        // Remove station offsets referencing this section
        self.station_offsets
            .retain(|sr, _| sr.section_id != section_id);

        // Remove station comments referencing this section
        self.station_comments
            .retain(|sr, _| sr.section_id != section_id);

        // Remove station names referencing this section
        self.station_names
            .retain(|sr, _| sr.section_id != section_id);

        // Remove zero datums referencing this section
        self.zero_datums
            .retain(|zd| zd.station_ref.section_id != section_id);
    }
}
