use std::collections::{HashMap, HashSet, VecDeque};

use crate::{SectionId, StationRef};

use super::Project;

impl Project {
    /// Get all section IDs connected to the given section through merges and closures.
    /// Both merges and closures are treated as bidirectional edges.
    /// Returns a set containing the given section and all transitively connected sections.
    pub fn get_connected_section_ids(&self, section_id: SectionId) -> HashSet<SectionId> {
        self.connected_section_ids_skipping(section_id, &HashSet::new())
    }

    /// [`Self::get_connected_section_ids`] from `a`'s section, leaving out every
    /// closure that joins exactly `a` and `b` (either way round): the sections
    /// a walk between the two can reach with their own closure edge lifted out
    /// (#576 RV19a-9, the path-length fallback).
    pub(crate) fn connected_section_ids_without_pair(
        &self,
        a: StationRef,
        b: StationRef,
    ) -> HashSet<SectionId> {
        let pair: HashSet<usize> = self
            .loop_closures
            .iter()
            .enumerate()
            .filter(|(_, closure)| {
                let (x, y) = (
                    self.resolve_key(&closure.station_a),
                    self.resolve_key(&closure.station_b),
                );
                (x == Some(a) && y == Some(b)) || (x == Some(b) && y == Some(a))
            })
            .map(|(index, _)| index)
            .collect();
        self.connected_section_ids_skipping(a.section_id, &pair)
    }

    /// The BFS behind both: merges and closures as edges, the closures at the
    /// `skipped` positions of `loop_closures` left out (positions, not ids: a
    /// legacy project can hold several closures with id 0).
    fn connected_section_ids_skipping(
        &self,
        section_id: SectionId,
        skipped: &HashSet<usize>,
    ) -> HashSet<SectionId> {
        let mut connected = HashSet::new();
        let mut queue = vec![section_id];

        while let Some(current) = queue.pop() {
            if connected.insert(current) {
                // Follow merges as edges
                for merge in &self.station_merges {
                    if merge.station_a.section_id == current {
                        if !connected.contains(&merge.station_b.section_id) {
                            queue.push(merge.station_b.section_id);
                        }
                    } else if merge.station_b.section_id == current
                        && !connected.contains(&merge.station_a.section_id)
                    {
                        queue.push(merge.station_a.section_id);
                    }
                }
                // Follow closures as edges (bidirectional, same as merges)
                for (index, closure) in self.loop_closures.iter().enumerate() {
                    if skipped.contains(&index) {
                        continue;
                    }
                    if closure.station_a.section_id == current {
                        if !connected.contains(&closure.station_b.section_id) {
                            queue.push(closure.station_b.section_id);
                        }
                    } else if closure.station_b.section_id == current
                        && !connected.contains(&closure.station_a.section_id)
                    {
                        queue.push(closure.station_a.section_id);
                    }
                }
            }
        }
        connected
    }

    /// Station-level merge adjacency: each [`StationRef`] → the stations it is
    /// directly merged with. Resolves every `station_merges` endpoint once
    /// (O(merges)), so a BFS over it is O(merges + group) instead of the
    /// O(group × merges) "rescan every merge per node" walk. Used by the merge
    /// connectivity / colocation queries on the import hot path.
    pub(crate) fn build_station_merge_adjacency(&self) -> HashMap<StationRef, Vec<StationRef>> {
        let mut adjacency: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        for merge in &self.station_merges {
            if let (Some(sa), Some(sb)) = (
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                adjacency.entry(sa).or_default().push(sb);
                adjacency.entry(sb).or_default().push(sa);
            }
        }
        adjacency
    }

    /// Get all section IDs connected through merges and aligned closures (rigid groups).
    /// Unaligned closures are ignored so sections can move independently before adjustment.
    pub fn get_rigid_connected_section_ids(&self, section_id: SectionId) -> HashSet<SectionId> {
        let mut connected = HashSet::new();
        let mut queue = vec![section_id];

        while let Some(current) = queue.pop() {
            if connected.insert(current) {
                // Follow merges as edges
                for merge in &self.station_merges {
                    if merge.station_a.section_id == current {
                        if !connected.contains(&merge.station_b.section_id) {
                            queue.push(merge.station_b.section_id);
                        }
                    } else if merge.station_b.section_id == current
                        && !connected.contains(&merge.station_a.section_id)
                    {
                        queue.push(merge.station_a.section_id);
                    }
                }
                // Follow aligned closures as edges (bidirectional, same as merges)
                for closure in &self.loop_closures {
                    if !closure.is_aligned {
                        continue;
                    }
                    if closure.station_a.section_id == current {
                        if !connected.contains(&closure.station_b.section_id) {
                            queue.push(closure.station_b.section_id);
                        }
                    } else if closure.station_b.section_id == current
                        && !connected.contains(&closure.station_a.section_id)
                    {
                        queue.push(closure.station_a.section_id);
                    }
                }
            }
        }
        connected
    }

    /// Cascade offset adjustments through drifted merge/closure connections.
    ///
    /// `seeds` are the sections whose geometry is authoritative (changed
    /// sections, datum-bearing sections, etc.).  BFS outward through merges
    /// and aligned closures: when one side is authoritative and the other is
    /// not, adjust the non-authoritative side's section offset to re-align,
    /// then mark it authoritative so further connections cascade.
    ///
    /// Zero-datum-bearing sections are always included as authoritative.
    pub fn cascade_connection_offsets(&mut self, seeds: &[SectionId]) {
        // Honor the batch-import opt-out: skip cascade during the
        // per-section import loop so the cost stays linear. The batch
        // driver in execute_import_sections runs ONE final cascade
        // (with all created sections as seeds) after the loop, which
        // is equivalent to the union of the deferred cascades for a
        // tree-shaped import.
        if self.cascade_deferred {
            return;
        }

        // Coincidence edges, kept in TWO graphs: merges (the rigid, acyclic
        // forest - loop-forming merges are rejected in merge_stations) and
        // ALIGNED loop closures (the redundant, cycle-closing edges; unaligned
        // closures are inert). Solving merges to full reach FIRST, then closures,
        // keeps a hard merge constraint winning over a stale aligned closure on
        // any section the merges can reach.
        let push_edge = |adj: &mut HashMap<SectionId, Vec<(StationRef, StationRef)>>,
                         a: StationRef,
                         b: StationRef| {
            adj.entry(a.section_id).or_default().push((a, b));
            adj.entry(b.section_id).or_default().push((b, a));
        };
        let mut merge_adj: HashMap<SectionId, Vec<(StationRef, StationRef)>> = HashMap::new();
        for m in &self.station_merges {
            if let (Some(a), Some(b)) = (
                self.resolve_key(&m.station_a),
                self.resolve_key(&m.station_b),
            ) {
                push_edge(&mut merge_adj, a, b);
            }
        }
        let mut closure_adj: HashMap<SectionId, Vec<(StationRef, StationRef)>> = HashMap::new();
        for c in &self.loop_closures {
            if c.is_aligned {
                if let (Some(a), Some(b)) = (
                    self.resolve_key(&c.station_a),
                    self.resolve_key(&c.station_b),
                ) {
                    push_edge(&mut closure_adj, a, b);
                }
            }
        }
        if merge_adj.is_empty() && closure_adj.is_empty() {
            return;
        }

        // O(1) station coordinates via the per-section local-coord precompute,
        // so each visit is constant work instead of re-walking the section's
        // shots. Local coords are offset-independent, so this stays valid as
        // offsets are mutated.
        let local = self.section_local_coords();
        let station_offsets = self.resolved_station_offsets();

        // Authoritative anchors: the seeds + every datum-bearing section.
        let mut authoritative: HashSet<SectionId> = HashSet::new();
        for &sid in seeds {
            authoritative.insert(sid);
        }
        for zd in &self.zero_datums {
            // Skip dangling datums (anchor key no longer resolves) - they are not
            // real anchors, mirroring `section_has_zero_datum`/`component_datum_refs`
            // (#174). Otherwise an unanchored section gets frozen as authoritative.
            if self.resolve_key(&zd.station_ref).is_some() {
                authoritative.insert(zd.station_ref.section_id);
            }
        }

        let sorted = |auth: &HashSet<SectionId>| {
            let mut v: Vec<SectionId> = auth.iter().copied().collect();
            v.sort_unstable();
            v
        };

        // Phase 1: BFS over merges only, to full reachability.
        let seed1 = sorted(&authoritative);
        self.cascade_bfs(
            &merge_adj,
            &local,
            &station_offsets,
            &mut authoritative,
            &seed1,
        );

        // Phase 2: aligned closures bridge to any component merges couldn't
        // reach; merges within that component then continue. A redundant closure
        // (both ends already authoritative) is skipped, leaving its residual -
        // the loop misclosure - intact.
        if !closure_adj.is_empty() {
            let mut combined = merge_adj;
            for (section, edges) in closure_adj {
                combined.entry(section).or_default().extend(edges);
            }
            let seed2 = sorted(&authoritative);
            self.cascade_bfs(
                &combined,
                &local,
                &station_offsets,
                &mut authoritative,
                &seed2,
            );
        }
    }

    /// One BFS pass of the offset solver over a coincidence-edge `adjacency`,
    /// starting from the already-`authoritative` sections in `seeds` order. Each
    /// non-authoritative neighbour reached through a computable edge has its
    /// section offset set so its station coincides with the anchored side, then
    /// is itself marked authoritative and enqueued. A neighbour reached only
    /// through a dangling/out-of-range edge (coords return `None`) is left alone
    /// - it is not anchored or propagated, so it can't mis-position a chain
    /// behind a broken reference.
    pub(super) fn cascade_bfs(
        &mut self,
        adjacency: &HashMap<SectionId, Vec<(StationRef, StationRef)>>,
        local: &HashMap<SectionId, Vec<(f64, f64)>>,
        station_offsets: &HashMap<StationRef, (f64, f64, f64)>,
        authoritative: &mut HashSet<SectionId>,
        seeds: &[SectionId],
    ) {
        let mut queue: VecDeque<SectionId> = seeds.iter().copied().collect();
        while let Some(section) = queue.pop_front() {
            let Some(neighbors) = adjacency.get(&section).cloned() else {
                continue;
            };
            for (mine, theirs) in neighbors {
                if authoritative.contains(&theirs.section_id) {
                    continue;
                }
                if let (Some((mx, my)), Some((tx, ty))) = (
                    self.station_world_xy(mine, local, station_offsets),
                    self.station_world_xy(theirs, local, station_offsets),
                ) {
                    let dx = mx - tx;
                    let dy = my - ty;
                    // A non-finite delta means the coincidence isn't computable (a
                    // NaN/inf-poisoned section). Don't adjust, anchor, OR propagate
                    // through it: marking the unpositioned neighbour authoritative
                    // would drag ITS other partners to a bogus location, and the
                    // alignment check can't see a NaN distance to catch it later.
                    // Treat it like a dangling edge - leave the neighbour alone.
                    if dx.is_finite() && dy.is_finite() {
                        // Decline a correction that is float dust, exactly as the
                        // merge BFS in `reconcile_after_edit` does - one shared
                        // bound (#413 finding 2's dust half). A gap of a few ULPs
                        // of the coordinates being summed is the residue of
                        // summing a survey chain, not a misalignment;
                        // "correcting" it wrote a `section_offsets` key for a
                        // section that is not displaced, and the key (unlike the
                        // value) is structural: it is saved, and it makes this
                        // route differ from the route that had nothing to
                        // correct. That "few ULPs" is why the bound is where it
                        // is rather than as tight as possible - it is measured,
                        // at the largest coordinates the app can hold, on
                        // `Project::OFFSET_DUST_M` (#469).
                        if dx.abs() > Self::OFFSET_DUST_M || dy.abs() > Self::OFFSET_DUST_M {
                            self.adjust_section_offset(theirs.section_id, dx, dy);
                        }
                        // Anchor + propagate ONLY when the coincidence was computable.
                        authoritative.insert(theirs.section_id);
                        queue.push_back(theirs.section_id);
                    }
                }
            }
        }
    }

    /// Check whether `start` can reach a zero datum station via merges and
    /// aligned closures WITHOUT traversing through `excluded`.  This is used
    /// by the doubly-anchored guard: a merge/closure to a partner is only a
    /// positional constraint if that partner is independently datum-anchored.
    pub fn has_datum_path_excluding(&self, start: SectionId, excluded: SectionId) -> bool {
        let mut visited = HashSet::new();
        let mut queue = vec![start];
        while let Some(current) = queue.pop() {
            if current == excluded {
                continue;
            }
            if !visited.insert(current) {
                continue;
            }
            if self.zero_datums.iter().any(|zd| {
                // A dangling datum (key no longer resolves) does not anchor (#174).
                zd.station_ref.section_id == current && self.resolve_key(&zd.station_ref).is_some()
            }) {
                return true;
            }
            for m in &self.station_merges {
                if m.station_a.section_id == current && !visited.contains(&m.station_b.section_id) {
                    queue.push(m.station_b.section_id);
                }
                if m.station_b.section_id == current && !visited.contains(&m.station_a.section_id) {
                    queue.push(m.station_a.section_id);
                }
            }
            for c in &self.loop_closures {
                if !c.is_aligned {
                    continue;
                }
                if c.station_a.section_id == current && !visited.contains(&c.station_b.section_id) {
                    queue.push(c.station_b.section_id);
                }
                if c.station_b.section_id == current && !visited.contains(&c.station_a.section_id) {
                    queue.push(c.station_a.section_id);
                }
            }
        }
        false
    }

    /// Get all section IDs connected through station merges only (rigid groups).
    /// Closures are NOT followed. Use this for rigid-group operations:
    /// datum propagation, offset application, zero-datum validation.
    pub fn get_merged_section_ids(&self, section_id: SectionId) -> HashSet<SectionId> {
        // ONE path: BFS over the merge-only adjacency, which
        // [`MergeTopology::adjacency`] builds on demand and every mutator
        // invalidates. There is deliberately no linear-scan fallback - the old
        // "empty cache means fall back" branch is what let a stale-but-non-empty
        // map answer authoritatively after `map_station_refs` moved an endpoint.
        // Each connected section is visited once with O(neighbours) work per
        // step, so this is O(group size), not O(group × merges).
        let adjacency = self.station_merges.adjacency();
        let mut connected = HashSet::new();
        let mut queue = vec![section_id];
        while let Some(current) = queue.pop() {
            if connected.insert(current) {
                if let Some(neighbours) = adjacency.get(&current) {
                    for &n in neighbours {
                        if !connected.contains(&n) {
                            queue.push(n);
                        }
                    }
                }
            }
        }
        connected
    }

    /// Capture section offsets for all sections connected to the given section.
    /// Returns a HashMap where:
    ///   - Key is the section ID
    ///   - Value is Option<(f64, f64)>: Some if offset existed, None if key didn't exist
    ///
    /// This is used by undo to restore exact previous state.
    pub fn capture_connected_section_offsets(
        &self,
        section_id: SectionId,
    ) -> HashMap<SectionId, Option<(f64, f64)>> {
        let merged_sections = self.get_merged_section_ids(section_id);
        let mut offsets = HashMap::new();
        for sid in merged_sections {
            offsets.insert(sid, self.section_offsets.get(&sid).copied());
        }
        offsets
    }

    /// Every `section_offsets` entry the geometry epilogue of an edit in
    /// `section_id` can reach - `None` for a section that has no key.
    ///
    /// [`Self::capture_connected_section_offsets`] takes the MERGE group, which is
    /// what `realign_after_shot_edit` walks. This one takes the RIGID connected
    /// set ([`Self::get_rigid_connected_section_ids`] - merges plus ALIGNED loop
    /// closures), which is what [`Self::cascade_connection_offsets`] walks, and is
    /// therefore the superset an insert or a removal can displace. Used by the
    /// undo entries that have to hand the map back rather than let their inverse
    /// re-derive it (#413, finding 27), where capturing the wider set costs a few
    /// map entries and capturing the narrower one would leave a
    /// closure-reachable section unrestored.
    pub fn capture_rigid_connected_section_offsets(
        &self,
        section_id: SectionId,
    ) -> HashMap<SectionId, Option<(f64, f64)>> {
        let mut offsets = HashMap::new();
        for sid in self.get_rigid_connected_section_ids(section_id) {
            offsets.insert(sid, self.section_offsets.get(&sid).copied());
        }
        offsets
    }

    /// EVERY section's `section_offsets` entry - `None` for a section that has no
    /// key. The whole-project sibling of the two captures above.
    ///
    /// For the one command whose effect is not bounded by any connectivity:
    /// changing WHICH zero datum is first re-defines the georeference origin, and
    /// [`Self::renormalize_frame`] answers that by translating the whole survey,
    /// free-floating components included. A merge-group or rigid-set capture
    /// cannot hand that back, so the undo entry that records it has to take the
    /// project (#469, item 1). Both narrower captures stay as they are: no
    /// per-keystroke shot edit can change the frame, and widening those would put
    /// an N-entry map on every one of them.
    pub fn capture_all_section_offsets(&self) -> HashMap<SectionId, Option<(f64, f64)>> {
        self.sections
            .iter()
            .map(|section| (section.id, self.section_offsets.get(&section.id).copied()))
            .collect()
    }

    /// Restore section offsets from a captured snapshot.
    ///
    /// For each entry in `offsets`:
    /// - `Some(offset)` restores the offset to that value
    /// - `None` removes the offset (it didn't exist before)
    ///
    /// This is used by undo to restore exact previous state (counterpart of
    /// `capture_connected_section_offsets`).
    ///
    /// Routed through [`Project::write_section_offset`] like every other writer,
    /// because the invariant documented there is TOTAL: a dust-zero entry does not
    /// exist, so a captured `Some((0, 0))` cannot have come from a state this app
    /// is allowed to reproduce. This used to be the deliberate exception - a
    /// restore reproducing the map as it was, all-zero entries included - and that
    /// exception is what made the present-zero key representable at all. It is
    /// gone with the state it served; a `Some` whose value is dust restores as
    /// ABSENT, which is the only spelling of what it meant.
    pub fn restore_section_offsets(&mut self, offsets: &HashMap<SectionId, Option<(f64, f64)>>) {
        for (section_id, prev_offset) in offsets {
            match prev_offset {
                Some(offset) => {
                    Project::write_section_offset(&mut self.section_offsets, *section_id, *offset);
                }
                None => {
                    self.section_offsets.remove(section_id);
                }
            }
        }
    }

    /// Every zero-datum station whose section lies in the same merge-connected
    /// component as `section_id` (resolved to current StationRefs).
    ///
    /// This is the single source of truth for "what anchors this rigid group?" -
    /// `section_has_zero_datum`, `find_connected_zero_datum`, the merge guard, and
    /// the over-anchoring detector all derive from it, so they cannot drift apart.
    ///
    /// Counts only datums that still RESOLVE: a corrupt project can be opened with a
    /// dangling zero datum (anchor shot deleted) so the user can repair it, and a
    /// dangling datum is not a real anchor. Counting it would let a merge pick the
    /// phantom-anchored section as the fixed group and shift the world coordinate
    /// system around a station that doesn't exist.
    pub(crate) fn component_datum_refs(&self, section_id: SectionId) -> Vec<StationRef> {
        let merged = self.get_merged_section_ids(section_id);
        self.zero_datums
            .iter()
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .filter(|sr| merged.contains(&sr.section_id))
            .collect()
    }

    /// Check if a section is connected to a zero datum (directly or through merges only).
    pub(super) fn section_has_zero_datum(&self, section_id: SectionId) -> bool {
        !self.component_datum_refs(section_id).is_empty()
    }

    /// Group resolved zero datums by their merge-connected component, returning
    /// only the components that hold MORE THAN ONE datum. An empty result means
    /// the "≤1 zero datum per merge component" invariant holds.
    ///
    /// Two GPS anchors in one rigid merge group over-constrain it: the offset
    /// cascade ([`Self::cascade_connection_offsets`]) marks every datum section
    /// authoritative, so the two wavefronts meet on a merge edge that cannot
    /// satisfy both anchors and that merge visibly tears apart. This is how the
    /// invariant violation surfaces to the user.
    pub fn over_anchored_merge_components(&self) -> Vec<Vec<StationRef>> {
        let resolved: Vec<StationRef> = self
            .zero_datums
            .iter()
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .collect();
        let mut groups: Vec<(HashSet<SectionId>, Vec<StationRef>)> = Vec::new();
        for sr in resolved {
            if let Some(group) = groups
                .iter_mut()
                .find(|(sections, _)| sections.contains(&sr.section_id))
            {
                group.1.push(sr);
            } else {
                groups.push((self.get_merged_section_ids(sr.section_id), vec![sr]));
            }
        }
        groups
            .into_iter()
            .map(|(_, refs)| refs)
            .filter(|refs| refs.len() > 1)
            .collect()
    }

    /// Label every section with its merge-only component in the HYPOTHETICAL
    /// graph that drops the merges at the indices in `skip`, and record which of
    /// those components an anchor still reaches.
    ///
    /// ONE O(V + E) pass answers both questions
    /// [`Self::demote_double_anchoring_merges`] asks of every candidate - "are
    /// these two halves still connected without the torn merges?" and "is each
    /// half independently anchored?" - so the separation test and the anchoring
    /// test can never be answered against different graphs, and neither costs a
    /// BFS per candidate.
    ///
    /// Only datums that still RESOLVE anchor a component, mirroring
    /// [`Self::component_datum_refs`]: a corrupt project can be opened with a
    /// dangling zero datum (its anchoring shot was deleted) so the user can repair
    /// it, and a dangling datum is not a real anchor - counting it would make a
    /// half look anchored when nothing holds it in place.
    ///
    /// Walks `station_merges` into a throwaway local adjacency rather than going
    /// through the memoized [`Self::get_merged_section_ids`]: the shared memo
    /// describes the merges that actually exist, and answering a "what if these
    /// merges were gone" question must never mutate the project or that memo.
    pub(crate) fn merge_components_excluding(
        &self,
        skip: &HashSet<usize>,
    ) -> ExcludedMergeComponents {
        let mut adjacency: HashMap<SectionId, Vec<SectionId>> = HashMap::new();
        for (i, m) in self.station_merges.iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            adjacency
                .entry(m.station_a.section_id)
                .or_default()
                .push(m.station_b.section_id);
            adjacency
                .entry(m.station_b.section_id)
                .or_default()
                .push(m.station_a.section_id);
        }

        // Roots: every section in the project, then any section a surviving merge
        // names that the project does not hold (a dangling endpoint in a project
        // opened for repair). Nothing is left unlabelled, so a section that
        // participates in no surviving merge is its own singleton component and
        // both accessors answer for it.
        let mut component_of: HashMap<SectionId, usize> = HashMap::new();
        let mut next_component = 0usize;
        let roots: Vec<SectionId> = self
            .sections
            .iter()
            .map(|s| s.id)
            .chain(adjacency.keys().copied())
            .collect();
        for root in roots {
            if component_of.contains_key(&root) {
                continue;
            }
            let component = next_component;
            next_component += 1;
            let mut queue = vec![root];
            while let Some(current) = queue.pop() {
                if component_of.insert(current, component).is_some() {
                    continue;
                }
                if let Some(neighbours) = adjacency.get(&current) {
                    for &n in neighbours {
                        if !component_of.contains_key(&n) {
                            queue.push(n);
                        }
                    }
                }
            }
        }

        let mut anchored: HashSet<usize> = HashSet::new();
        for zd in &self.zero_datums {
            let Some(sr) = self.resolve_key(&zd.station_ref) else {
                continue;
            };
            if let Some(&component) = component_of.get(&sr.section_id) {
                anchored.insert(component);
            }
        }

        ExcludedMergeComponents {
            component_of,
            anchored,
        }
    }

    /// Would adding a merge between sections `a` and `b` place two zero datums in
    /// one merge component? True only when `a` and `b` are in DIFFERENT merge
    /// components and EACH component is already anchored - i.e. the merge would
    /// fuse two independently-anchored rigid groups. Used to keep
    /// [`Self::convert_orphan_closures_to_merges`] from silently creating the
    /// over-constraint (a closure between two anchored groups is legitimate; a
    /// rigid merge between them is not).
    ///
    /// Asked about a merge that does NOT yet exist, so it reads the real graph
    /// through the memoized [`Self::get_merged_section_ids`].
    pub(crate) fn merge_would_double_anchor(&self, a: SectionId, b: SectionId) -> bool {
        let component_a = self.get_merged_section_ids(a);
        if component_a.contains(&b) {
            // Already the same merge component - a redundant merge adds no anchor.
            return false;
        }
        let anchored = |component: &HashSet<SectionId>| {
            self.zero_datums
                .iter()
                .filter_map(|zd| self.resolve_key(&zd.station_ref))
                .any(|sr| component.contains(&sr.section_id))
        };
        anchored(&component_a) && anchored(&self.get_merged_section_ids(b))
    }
}

/// Merge-only connected components of every section in a project, computed as if
/// some set of merges did not exist, plus which components hold at least one
/// resolved zero datum. Built by [`Project::merge_components_excluding`].
pub(crate) struct ExcludedMergeComponents {
    /// section id -> the index of its component in the hypothetical graph.
    component_of: HashMap<SectionId, usize>,
    /// Component indices reached by at least one resolved zero datum.
    anchored: HashSet<usize>,
}

impl ExcludedMergeComponents {
    /// True when `a` and `b` are still merge-connected with the excluded merges
    /// gone. A section this graph does not know is connected to nothing.
    pub(crate) fn same_component(&self, a: SectionId, b: SectionId) -> bool {
        match (self.component_of.get(&a), self.component_of.get(&b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }

    /// True when `section`'s component holds at least one resolved zero datum.
    pub(crate) fn is_anchored(&self, section: SectionId) -> bool {
        self.component_of
            .get(&section)
            .is_some_and(|c| self.anchored.contains(c))
    }
}
