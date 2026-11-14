use chrono::Utc;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    SectionId, Shot, ShotData, ShotId, ShotKind, ShotUpdates, StationAnchor, StationKey,
    StationMerge, StationRef, FEATURE_ID_STRIDE,
};

use super::offsets::MintedMerge;
use super::{
    ClosurePair, ClosureUpdate, DeleteShotImpact, DeleteShotRangeImpact, Project, RangeDeleteKind,
    StationPair,
};

/// Minimum length (meters) either child of a split may end up with (issue
/// #59). A split is only accepted when both `distance` and
/// `shot.length - distance` are at least this floor, so a shot must be at
/// least `2 * MIN_SPLIT_CHILD_LENGTH_M` long to be splittable at all. Mirrored
/// on the frontend in `src/utils/shotSplit.ts`.
pub const MIN_SPLIT_CHILD_LENGTH_M: f64 = 0.01;

impl Project {
    /// Stations adjacent to `station_ref` through shots (skipping `excluded_shot`)
    /// and merges, plus loop closures when `include_closures` is true. Pass
    /// `false` to ask "are these connected by the *survey itself*" - used to tell
    /// a true loop from a detached pair when deciding closure-vs-merge.
    fn station_neighbors_excluding_shot(
        &self,
        station_ref: StationRef,
        excluded_shot: Option<(SectionId, usize)>,
        include_closures: bool,
    ) -> Vec<StationRef> {
        let mut neighbors = Vec::new();
        let Some(section) = self.get_section(station_ref.section_id) else {
            return neighbors;
        };
        let idx = station_ref.station_index as usize;
        if idx > section.shots.len() {
            return neighbors;
        }

        // Adjacent stations via shots (skip the excluded shot)
        if idx > 0 {
            let shot_idx = idx - 1;
            if excluded_shot != Some((station_ref.section_id, shot_idx)) {
                neighbors.push(StationRef::new(station_ref.section_id, (idx - 1) as u32));
            }
        }
        if idx < section.shots.len() {
            let shot_idx = idx;
            if excluded_shot != Some((station_ref.section_id, shot_idx)) {
                neighbors.push(StationRef::new(station_ref.section_id, (idx + 1) as u32));
            }
        }

        // Merges (resolve stored keys to positional refs)
        for merge in &self.station_merges {
            if self.key_is_ref(&merge.station_a, station_ref) {
                neighbors.extend(self.resolve_key(&merge.station_b));
            } else if self.key_is_ref(&merge.station_b, station_ref) {
                neighbors.extend(self.resolve_key(&merge.station_a));
            }
        }

        // Closures (all strategies)
        if include_closures {
            for closure in &self.loop_closures {
                if self.key_is_ref(&closure.station_a, station_ref) {
                    neighbors.extend(self.resolve_key(&closure.station_b));
                } else if self.key_is_ref(&closure.station_b, station_ref) {
                    neighbors.extend(self.resolve_key(&closure.station_a));
                }
            }
        }

        neighbors
    }

    pub(super) fn are_stations_connected_excluding_shot(
        &self,
        station_a: StationRef,
        station_b: StationRef,
        excluded_shot: Option<(SectionId, usize)>,
        include_closures: bool,
    ) -> bool {
        if station_a == station_b {
            return true;
        }
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(station_a);
        queue.push_back(station_a);

        while let Some(current) = queue.pop_front() {
            for neighbor in
                self.station_neighbors_excluding_shot(current, excluded_shot, include_closures)
            {
                if visited.insert(neighbor) {
                    if neighbor == station_b {
                        return true;
                    }
                    queue.push_back(neighbor);
                }
            }
        }
        false
    }

    fn station_reaches_zero_datum_excluding_shot(
        &self,
        station_ref: StationRef,
        excluded_shot: Option<(SectionId, usize)>,
    ) -> bool {
        let zero_datum_set: HashSet<StationRef> = self
            .zero_datums
            .iter()
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .collect();
        if zero_datum_set.contains(&station_ref) {
            return true;
        }
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(station_ref);
        queue.push_back(station_ref);

        while let Some(current) = queue.pop_front() {
            for neighbor in self.station_neighbors_excluding_shot(current, excluded_shot, true) {
                if visited.insert(neighbor) {
                    if zero_datum_set.contains(&neighbor) {
                        return true;
                    }
                    queue.push_back(neighbor);
                }
            }
        }
        false
    }

    fn max_shots_per_section() -> usize {
        FEATURE_ID_STRIDE.saturating_sub(1) as usize
    }

    /// Why one more shot cannot go into `section_id`, if it cannot: the
    /// section is missing, holds the most shots a section may, or has no shot
    /// id left in this session ([`crate::Section::shot_ids_left`]). Read-only, so a
    /// caller can refuse before changing anything.
    pub fn shot_insert_refusal(&self, section_id: SectionId) -> Option<String> {
        let Some(section) = self.get_section(section_id) else {
            return Some(format!("Section {section_id} not found"));
        };
        if section.shots.len() >= Self::max_shots_per_section() {
            return Some(format!(
                "Section {section_id} has reached the maximum of {} shots",
                Self::max_shots_per_section()
            ));
        }
        (section.shot_ids_left() == 0).then(|| {
            format!(
                "Section {section_id} has used up its shot numbers for this session: save \
                 and reopen to add a station to it"
            )
        })
    }

    /// Add a shot to a section
    pub fn add_shot(&mut self, section_id: SectionId, data: ShotData) -> Option<&Shot> {
        let section = self.get_section_mut(section_id)?;
        if section.shots.len() >= Self::max_shots_per_section() {
            return None;
        }
        let shot = section.add_shot(data);
        let shot_id = shot.id;
        self.touch();
        self.get_section(section_id)?.get_shot(shot_id)
    }

    /// Can `start` reach a still-resolvable zero datum through RIGID MERGES
    /// only, without passing through `excluded`?
    ///
    /// The merge-only twin of [`Self::has_datum_path_excluding`], which also
    /// follows aligned loop closures. Both exist deliberately: the
    /// closure-inclusive question is the right one for "is this partner pinned
    /// to the world at all" (the import placement heuristic asks that), and this
    /// one is the right question for "will the offset cascade refuse to move
    /// this partner", because [`Self::cascade_connection_offsets`]'s first phase
    /// - and the merge-alignment invariant the app asserts - are merge-only.
    fn has_merge_datum_path_excluding(&self, start: SectionId, excluded: SectionId) -> bool {
        let mut visited: HashSet<SectionId> = HashSet::new();
        let mut queue = vec![start];
        while let Some(current) = queue.pop() {
            if current == excluded || !visited.insert(current) {
                continue;
            }
            if self.zero_datums.iter().any(|zd| {
                // A dangling datum (key no longer resolves) does not anchor (#174).
                zd.station_ref.section_id == current && self.resolve_key(&zd.station_ref).is_some()
            }) {
                return true;
            }
            for m in &self.station_merges {
                if m.station_a.section_id == current {
                    queue.push(m.station_b.section_id);
                }
                if m.station_b.section_id == current {
                    queue.push(m.station_a.section_id);
                }
            }
        }
        false
    }

    /// Which sides of a structural edit at `boundary` are pinned to world
    /// coordinates, as `(head_anchored, tail_anchored)`. `doomed_anchors` are
    /// keys a surrounding range delete will also remove, and never decide.
    ///
    /// A section is rigid and its offset is ONE value for the whole section, so
    /// with `world = section-local + section_offset` an insert or delete at
    /// `boundary` (which shifts every station of index `> boundary` in
    /// section-local space) leaves exactly two possibilities in world space:
    /// leave the offset alone and the HEAD keeps its position while the tail
    /// moves, or absorb the shift into the offset and the TAIL keeps its
    /// position while the head moves. Whichever side carries a datum constraint
    /// is the side that must stay; the other side's merge partners are then
    /// pulled along by [`Self::cascade_connection_offsets`] - which can only
    /// move a partner that is NOT itself authoritative.
    ///
    /// Measured over the rigid merge forest plus zero datums, which is exactly
    /// the constraint set that cascade's first phase enforces and that
    /// [`Self::debug_assert_merge_alignment`] checks. An ALIGNED LOOP CLOSURE is
    /// the redundant edge of a loop: its residual IS the loop misclosure, the
    /// cascade follows it only to reach a component the merges cannot, and the
    /// alignment assert ignores it. So a datum reachable only through a closure
    /// is a tie-break for when the merge graph pins neither side, never a reason
    /// to overrule a merge-side anchor.
    ///
    /// #413 finding 14, measured: with merges `s0:3 <-> s3:0` and `s3:5 <->
    /// s4:0`, an aligned closure `s0:0 <-> s4:2` and the only datum on `s0`, an
    /// insert into `s3` used to see the TAIL as anchored (`s4 -> closure -> s0`
    /// reaches the datum) and back-shifted `s3`, which tore the HEAD merge
    /// `s0:3 <-> s3:0` 3.97 m apart. The cascade could not repair that: `s0` is
    /// authoritative because it holds the datum and `s3` is authoritative
    /// because it is the seed, so the BFS skips the edge between them. Keeping
    /// the head fixed satisfies BOTH merges and leaves the closure carrying the
    /// misclosure the insert really created - which is the state the epilogue's
    /// re-solve converged to anyway, one asserted-impossible intermediate later.
    ///
    /// Both sides anchored means the merge component holds two datums (the
    /// subtrees behind a head merge and a tail merge are disjoint in a forest),
    /// which is the over-anchoring [`Self::over_anchored_merge_components`]
    /// reports and which NO choice of section offset can satisfy. The head is
    /// kept, deterministically, rather than tearing the head merge for a tail
    /// that cannot be satisfied either.
    fn anchored_sides_of_edit(
        &self,
        section_id: SectionId,
        boundary: u32,
        doomed_anchors: &HashSet<StationKey>,
    ) -> (bool, bool) {
        // `Some(true)` = this key sits on the moving tail, `Some(false)` = on the
        // stationary head, `None` = not ours, doomed, or no longer resolvable.
        let side = |key: &StationKey| -> Option<bool> {
            if key.section_id != section_id || doomed_anchors.contains(key) {
                return None;
            }
            self.resolve_key(key).map(|sr| sr.station_index > boundary)
        };
        let (mut head, mut tail) = (false, false);
        for zd in &self.zero_datums {
            if let Some(is_tail) = side(&zd.station_ref) {
                *(if is_tail { &mut tail } else { &mut head }) = true;
            }
        }
        for m in &self.station_merges {
            let (ours, theirs) = if m.station_a.section_id == section_id {
                (&m.station_a, &m.station_b)
            } else if m.station_b.section_id == section_id {
                (&m.station_b, &m.station_a)
            } else {
                continue;
            };
            let Some(is_tail) = side(ours) else { continue };
            if theirs.section_id != section_id
                && self.has_merge_datum_path_excluding(theirs.section_id, section_id)
            {
                *(if is_tail { &mut tail } else { &mut head }) = true;
            }
        }
        if head || tail {
            return (head, tail);
        }
        // Nothing rigid pins either side: an aligned closure is then the only
        // constraint there is, and honoring it keeps a closed loop closed.
        for c in &self.loop_closures {
            if !c.is_aligned {
                continue;
            }
            let (ours, theirs) = if c.station_a.section_id == section_id {
                (&c.station_a, &c.station_b)
            } else if c.station_b.section_id == section_id {
                (&c.station_b, &c.station_a)
            } else {
                continue;
            };
            let Some(is_tail) = side(ours) else { continue };
            if theirs.section_id != section_id
                && self.has_datum_path_excluding(theirs.section_id, section_id)
            {
                *(if is_tail { &mut tail } else { &mut head }) = true;
            }
        }
        (head, tail)
    }

    /// Insert a shot after a specific shot in a section. `forced_id` preserves
    /// the original id and `forced_uuid` the original persistent shot UUID when
    /// redo re-runs this exact path (see [`AppState::redo`]); pass `None`/`None`
    /// for normal inserts (a fresh UUID is minted).
    pub fn insert_shot_after(
        &mut self,
        section_id: SectionId,
        after_shot_id: ShotId,
        data: ShotData,
        forced_id: Option<ShotId>,
        forced_uuid: Option<String>,
    ) -> Option<&Shot> {
        // Look up position before mutable borrow
        let section = self.get_section(section_id)?;
        let pos = section.shots.iter().position(|s| s.id == after_shot_id)?;
        if section.shots.len() >= Self::max_shots_per_section() {
            return None;
        }
        // Compute displacement before data is consumed - the new shot shifts
        // all stations after the insertion point. In the section's own frame
        // (#438), matching the walk this compensates.
        let (dx, dy) = Self::shot_displacement(&data, self.azimuth_reference_for_id(section_id));
        let section = self.get_section_mut(section_id)?;
        let shot =
            section.insert_shot_after_with_id(after_shot_id, data, forced_id, forced_uuid)?;
        let shot_id = shot.id;
        // Station keys are position-independent: no ref-shifting on insert (the
        // junction the new shot extends from keeps its anchor; shots beyond keep
        // theirs). The new shot's displacement still shifts stations > pos+1 in
        // space - if the tail side is the anchored one, back-shift the offset so
        // the tail stays put and the head moves instead. Positions are resolved
        // from the stored keys. See `anchored_sides_of_edit` for why the HEAD
        // side has to be consulted too (#413 finding 14).
        if dx != 0.0 || dy != 0.0 {
            let (head_anchored, tail_anchored) =
                self.anchored_sides_of_edit(section_id, (pos + 1) as u32, &HashSet::new());
            if tail_anchored && !head_anchored {
                self.adjust_section_offset(section_id, -dx, -dy);
            }
        }
        self.cascade_connection_offsets(&[section_id]);
        self.debug_assert_merge_alignment("insert_shot_after");
        self.touch();
        self.get_section(section_id)?.get_shot(shot_id)
    }

    /// Insert a shot before a specific shot in a section. `forced_id` preserves
    /// the original id and `forced_uuid` the original persistent shot UUID when
    /// redo re-runs this exact path; pass `None`/`None` for normal inserts.
    pub fn insert_shot_before(
        &mut self,
        section_id: SectionId,
        before_shot_id: ShotId,
        data: ShotData,
        forced_id: Option<ShotId>,
        forced_uuid: Option<String>,
    ) -> Option<&Shot> {
        // Look up position before mutable borrow
        let section = self.get_section(section_id)?;
        let pos = section.shots.iter().position(|s| s.id == before_shot_id)?;
        if section.shots.len() >= Self::max_shots_per_section() {
            return None;
        }

        // Inserting before shot[pos] (pos > 0) is the SAME physical insertion as
        // inserting after shot[pos-1]: same array slot, same station split. Route
        // it through the one code path so the ref-shift / offset / cascade can't
        // diverge between the two entry points (the off-by-one that re-pointed a
        // boundary merge onto the new station and silently displaced its
        // partner). Only the prepend case (pos == 0, no preceding shot) is
        // genuinely distinct and handled below.
        if pos > 0 {
            let prev_id = section.shots[pos - 1].id;
            return self.insert_shot_after(section_id, prev_id, data, forced_id, forced_uuid);
        }

        // Prepend (pos == 0): the new shot becomes the section's first shot and
        // every existing station shifts up by one; keep them in place by
        // back-shifting the offset.
        let (dx, dy) = Self::shot_displacement(&data, self.azimuth_reference_for_id(section_id));
        let section = self.get_section_mut(section_id)?;
        let shot =
            section.insert_shot_before_with_id(before_shot_id, data, forced_id, forced_uuid)?;
        let shot_id = shot.id;
        // Prepend is the one insert that shifts identity: the old section-start
        // station is now *after* the new first shot, so re-anchor any ref on this
        // section that was `SectionStart` to `AfterShot(new_shot)`. Every other
        // anchor is position-independent and needs no change.
        let new_uuid = self
            .get_section(section_id)
            .and_then(|s| s.shots.first()?.uuid.clone());
        if let Some(new_uuid) = new_uuid {
            self.map_station_refs(|key| {
                if key.section_id == section_id && key.anchor == StationAnchor::SectionStart {
                    StationKey::new(section_id, StationAnchor::AfterShot(new_uuid.clone()))
                } else {
                    key
                }
            });
        }
        if dx != 0.0 || dy != 0.0 {
            self.adjust_section_offset(section_id, -dx, -dy);
        }
        self.cascade_connection_offsets(&[section_id]);
        self.debug_assert_merge_alignment("insert_shot_before");
        self.touch();
        self.get_section(section_id)?.get_shot(shot_id)
    }

    /// Insert a station mid-shot by splitting `shot_id` at `distance` meters
    /// from its start into two collinear child shots (issue #59). The new
    /// "head" shot (fresh id/uuid) is inserted before `shot_id`; `shot_id`
    /// itself becomes the shrunken "tail" shot, MUTATED IN PLACE so it keeps
    /// its original id, uuid, comment, and marker_index. The new station sits
    /// at index `pos + 1` (the boundary between the two children).
    ///
    /// Every cross-reference (merge, closure, zero datum, comment) is stored
    /// as a position-independent [`StationKey`] (`SectionStart` or
    /// `AfterShot(uuid)`); since the original shot's uuid stays on the tail
    /// child, every existing anchor keeps resolving to the SAME station it
    /// always did (now one position further along), and NO remap is needed.
    /// This only holds because the split never re-anchors `SectionStart` the
    /// way [`Self::insert_shot_before`]'s prepend path does -- this method
    /// therefore goes through the section-level insert directly rather than
    /// `Self::insert_shot_before`/`insert_shot_after`.
    ///
    /// The split is collinear (same azimuth/pitch on both children, depth
    /// linearly interpolated), so `horizontal_length_from_slope` is exact
    /// under the split and every downstream station's absolute position is
    /// preserved exactly -- no section-offset adjustment is needed.
    ///
    /// Both children inherit the parent shot's LRUD unchanged: an interior
    /// point of the measured shot is better estimated by the parent's own
    /// LRUD than by null, and no renderer currently consumes per-shot LRUD.
    /// This is a deliberate mid-station estimate, not a bug.
    ///
    /// Returns the new head shot's `ShotId` plus the pieces the split was
    /// computed from -- the head shot's [`ShotData`] and the [`ShotUpdates`]
    /// applied to the tail. The command layer reuses these to build its
    /// human-readable undo label, so [`Self::compute_split`] runs exactly once
    /// per split (see [`crate::core::AppState::execute_split_shot_at_distance`]).
    pub fn split_shot_at_distance(
        &mut self,
        section_id: SectionId,
        shot_id: ShotId,
        distance: f64,
    ) -> Result<(ShotId, ShotData, ShotUpdates), String> {
        let section = self
            .get_section(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        if section.shots.len() >= Self::max_shots_per_section() {
            return Err(format!(
                "Section {section_id} has reached the maximum of {} shots",
                Self::max_shots_per_section()
            ));
        }
        let shot = section
            .get_shot(shot_id)
            .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?;
        let (head_data, tail_updates) = Self::compute_split(shot, distance)?;

        let section = self
            .get_section_mut(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        let tail = section
            .get_shot_mut(shot_id)
            .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?;
        tail.apply_updates(&tail_updates);

        let new_shot = section
            .insert_shot_before_with_id(shot_id, head_data.clone(), None, None)
            .ok_or_else(|| format!("Failed to insert split shot before {shot_id}"))?;
        let new_shot_id = new_shot.id;

        self.debug_assert_merge_alignment("split_shot_at_distance");
        self.touch();
        Ok((new_shot_id, head_data, tail_updates))
    }

    /// Pure split math for [`Self::split_shot_at_distance`]: given the shot
    /// being split and a distance from its start, validates the distance and
    /// returns the new "head" shot's data (to insert before the original)
    /// plus the [`ShotUpdates`] the original ("tail") shot needs applied. No
    /// mutation happens here -- this is the one place the split math and its
    /// error messages live, shared by the mutating split itself and by the
    /// command layer, which needs the identical computation to build a
    /// human-readable undo label without re-deriving it.
    pub(crate) fn compute_split(
        shot: &Shot,
        distance: f64,
    ) -> Result<(ShotData, ShotUpdates), String> {
        if !distance.is_finite() {
            return Err("Distance must be finite".to_string());
        }
        if distance <= 0.0 || distance >= shot.length {
            return Err(format!(
                "Distance must be strictly between 0 and the shot's length ({:.2}m)",
                shot.length
            ));
        }
        let tail_length = shot.length - distance;
        if distance < MIN_SPLIT_CHILD_LENGTH_M || tail_length < MIN_SPLIT_CHILD_LENGTH_M {
            return Err(format!(
                "Both sides of the split must be at least {MIN_SPLIT_CHILD_LENGTH_M:.2}m long"
            ));
        }

        let t = distance / shot.length;
        let mid_depth = shot.depth_in + t * (shot.depth_out - shot.depth_in);
        // Every field of the parent that is not geometry or identity travels to
        // the head piece, including any per-shot field added later: the struct
        // update copies them all, and only the fields a split must change are
        // named here.
        let head_data = ShotData {
            // The head child is a NEW shot; only the tail keeps the parent's
            // identity (see this method's doc comment - it is what makes every
            // existing `AfterShot` anchor keep resolving without a remap).
            // Copying the parent's uuid here would make the section hold two
            // shots with one id, and the FIRST-wins repair would then re-mint
            // the tail - moving every anchor. See [`ShotData::uuid`].
            uuid: None,
            length: distance,
            depth_out: mid_depth,
            // A comment and a marker annotate one physical spot, so they stay on
            // the tail (the parent's identity) and are not duplicated.
            comment: None,
            marker_index: None,
            // Everything else travels, credit included: both pieces were
            // surveyed together, by the same people.
            ..shot.to_shot_data()
        };
        let tail_updates = ShotUpdates {
            length: Some(tail_length),
            depth_in: Some(mid_depth),
            ..Default::default()
        };
        Ok((head_data, tail_updates))
    }

    /// The in-session id of the shot `uuid` names in `section_id`, if it is
    /// still there.
    ///
    /// [`Shot::id`] is regenerated on every load (`section_id *
    /// FEATURE_ID_STRIDE + sequence_order`), so it cannot address a shot across
    /// a save; [`Shot::uuid`] exists precisely because it can. Anything holding
    /// a shot address longer than one session - a persisted undo entry, a
    /// station anchor - resolves through here.
    pub fn shot_id_by_uuid(&self, section_id: SectionId, uuid: &str) -> Option<ShotId> {
        self.get_section(section_id)?
            .shots
            .iter()
            .find(|shot| shot.uuid.as_deref() == Some(uuid))
            .map(|shot| shot.id)
    }

    /// Update a shot
    pub fn update_shot(
        &mut self,
        section_id: SectionId,
        shot_id: ShotId,
        updates: &ShotUpdates,
    ) -> Option<&Shot> {
        let section = self.get_section_mut(section_id)?;
        let shot = section.get_shot_mut(shot_id)?;
        shot.apply_updates(updates);
        self.touch();
        self.get_section(section_id)?.get_shot(shot_id)
    }

    pub fn preview_delete_shot(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
    ) -> Result<DeleteShotImpact, String> {
        let section = self
            .get_section(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        let pos = section
            .shots
            .iter()
            .position(|s| s.id == shot_id)
            .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?;

        let removed_station = if pos == 0 {
            StationRef::new(section_id, 0)
        } else {
            StationRef::new(section_id, (pos + 1) as u32)
        };

        let mut impact = DeleteShotImpact {
            removed_station,
            removed_merges: Vec::new(),
            removed_closures: Vec::new(),
            replacement_merges: Vec::new(),
            replacement_closure_updates: Vec::new(),
            closure_to_merge: Vec::new(),
            merge_rewire_skipped: false,
            closure_rewire_skipped: false,
            shift_blocked: false,
            shift_blocked_reasons: Vec::new(),
        };

        for merge in &self.station_merges {
            if self.key_is_ref(&merge.station_a, removed_station)
                || self.key_is_ref(&merge.station_b, removed_station)
            {
                impact.removed_merges.push(StationPair {
                    station_a: self.key_to_ref_lossy(&merge.station_a),
                    station_b: self.key_to_ref_lossy(&merge.station_b),
                });
            }
        }

        for closure in &self.loop_closures {
            if self.key_is_ref(&closure.station_a, removed_station)
                || self.key_is_ref(&closure.station_b, removed_station)
            {
                impact.removed_closures.push(ClosurePair {
                    closure_id: closure.id,
                    station_a: self.key_to_ref_lossy(&closure.station_a),
                    station_b: self.key_to_ref_lossy(&closure.station_b),
                });
            }
        }

        let merge_group = self.get_merge_group(removed_station);
        let mut merge_group_without = merge_group;
        merge_group_without.remove(&removed_station);

        if merge_group_without.len() > 1 {
            let components = self.build_merge_components(&merge_group_without, removed_station);
            if components.len() > 1 {
                let base_component = &components[0];

                let mut planned_pairs: Vec<StationPair> = Vec::new();
                for component in components.iter().skip(1) {
                    let mut found_pair: Option<StationPair> = None;
                    for station_a in base_component {
                        for station_b in component {
                            if station_a.section_id != station_b.section_id {
                                found_pair = Some(StationPair {
                                    station_a: *station_a,
                                    station_b: *station_b,
                                });
                                break;
                            }
                        }
                        if found_pair.is_some() {
                            break;
                        }
                    }

                    if let Some(pair) = found_pair {
                        // Same guard the section/range deletion paths apply, so
                        // a single-shot delete cannot mint the merge those refuse
                        // to. This list is BOTH the dialog's preview and the list
                        // `remove_shot_with_rewire_excluding_anchors` replays, so
                        // guarding here keeps promise and execution identical.
                        let doomed = HashSet::from([removed_station]);
                        let planned: Vec<(StationRef, StationRef)> = planned_pairs
                            .iter()
                            .map(|p| (p.station_a, p.station_b))
                            .collect();
                        if self.replacement_merge_allowed(
                            pair.station_a,
                            pair.station_b,
                            &doomed,
                            &planned,
                        ) && !planned_pairs.iter().any(|p| {
                            (p.station_a == pair.station_a && p.station_b == pair.station_b)
                                || (p.station_a == pair.station_b && p.station_b == pair.station_a)
                        }) {
                            planned_pairs.push(pair);
                        }
                    } else {
                        impact.merge_rewire_skipped = true;
                    }
                }

                impact.replacement_merges = planned_pairs;
            }
        }

        if !impact.removed_closures.is_empty() {
            let doomed = HashSet::from([removed_station]);
            let mut candidates: Vec<StationRef> = merge_group_without.into_iter().collect();
            candidates.sort_by_key(Self::station_ref_key);
            let replacement = candidates.first().copied();

            for closure in &impact.removed_closures {
                if let Some(replacement_station) = replacement {
                    // The hypothesis ACCUMULATES, exactly as the
                    // `replacement_merges` arm above rebuilds `planned` inside
                    // its own loop: a promotion already decided on in this loop
                    // is a merge `remove_shot_with_rewire` will record, so the
                    // next closure's guard has to be asked about the graph that
                    // INCLUDES it. Hoisted out of the loop, two sole-bridge
                    // closures on one doomed station each passed the guard alone
                    // and together welded two independently anchored components -
                    // defect A again, one hop further out (#464).
                    let planned: Vec<(StationRef, StationRef)> = impact
                        .replacement_merges
                        .iter()
                        .chain(impact.closure_to_merge.iter())
                        .map(|p| (p.station_a, p.station_b))
                        .collect();
                    let other = if closure.station_a == removed_station {
                        closure.station_b
                    } else {
                        closure.station_a
                    };

                    if replacement_station == other
                        || self.are_stations_merged(replacement_station, other)
                        || self.are_stations_in_closure(replacement_station, other)
                    {
                        impact.closure_rewire_skipped = true;
                    } else if self.are_stations_connected_excluding_shot(
                        replacement_station,
                        other,
                        Some((section_id, pos)),
                        false,
                    ) || self.would_double_anchor_after_removal(
                        replacement_station.section_id,
                        other.section_id,
                        &doomed,
                        &planned,
                    ) {
                        // Two reasons to keep this a CLOSURE, re-anchored off the
                        // deleted station:
                        //
                        // * the two ends still reach each other through the survey
                        //   (shots + merges), so it remains a genuine loop;
                        // * or promoting it would weld two components that each
                        //   survive independently zero-datum-anchored. That edge
                        //   puts two GPS anchors in one rigid group -
                        //   `cascade_bfs` treats both ends as authoritative and
                        //   never traverses between them - so the gap is PERMANENT
                        //   and Repair cannot reduce it (#464 defect A; the sibling
                        //   `replacement_merges` arm above has always guarded this,
                        //   and `convert_orphan_closures_to_merges` refuses the same
                        //   promotion for the same reason, so the refusal survives
                        //   the epilogue sweep and the next load).
                        //
                        // Refusing here rather than dropping the closure keeps the
                        // relationship the surveyor recorded, and refusing in the
                        // PREVIEW keeps the delete dialog's promise identical to
                        // what `remove_shot_with_rewire` replays.
                        impact.replacement_closure_updates.push(ClosureUpdate {
                            closure_id: closure.closure_id,
                            old_station: removed_station,
                            new_station: replacement_station,
                        });
                    } else {
                        // The loop is broken: the surviving segment is detached,
                        // so the closure becomes a plain merge (issue #8).
                        impact.closure_to_merge.push(StationPair {
                            station_a: replacement_station,
                            station_b: other,
                        });
                    }
                } else {
                    impact.closure_rewire_skipped = true;
                }
            }
        }

        let is_mid_shot = pos > 0 && pos < section.shots.len() - 1;
        if is_mid_shot {
            let station_before = StationRef::new(section_id, pos as u32);
            let station_after = StationRef::new(section_id, (pos + 1) as u32);
            let excluded = Some((section_id, pos));

            if self.are_stations_connected_excluding_shot(
                station_before,
                station_after,
                excluded,
                true,
            ) {
                impact.shift_blocked_reasons.push(
                    "This shot is part of a loop (the two sides are already connected)."
                        .to_string(),
                );
            }

            let anchored_before =
                self.station_reaches_zero_datum_excluding_shot(station_before, excluded);
            let anchored_after =
                self.station_reaches_zero_datum_excluding_shot(station_after, excluded);
            if anchored_before && anchored_after {
                impact
                    .shift_blocked_reasons
                    .push("Both sides are anchored to a zero datum.".to_string());
            }

            impact.shift_blocked = !impact.shift_blocked_reasons.is_empty();
        }

        Ok(impact)
    }

    /// Aggregate impact of deleting a contiguous range of shots
    /// `[start_shot_id .. end_shot_id]` (by their current positions) from one
    /// section. Used by the bulk-delete dialog to show a range summary, the
    /// removed station relationships, and whether the gap-close (shift) path is
    /// available. `start`/`end` may be given in any order - they are normalized.
    pub fn preview_delete_shot_range(
        &self,
        section_id: SectionId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
    ) -> Result<DeleteShotRangeImpact, String> {
        let mut impact =
            self.preview_delete_shot_range_relationships(section_id, start_shot_id, end_shot_id)?;
        if Self::gap_close_is_offered(&impact) {
            impact.closures_removed_across_one_shot = self
                .preview_gap_close_closure_removals(
                    section_id,
                    impact.start_pos as usize,
                    impact.end_pos as usize,
                )
                .ok();
        }
        Ok(impact)
    }

    /// [`Self::preview_delete_shot_range`] for a caller that owns a copy of
    /// the project it can give up (the IPC command clones the open project
    /// anyway): the gap-close prediction runs on that copy instead of cloning
    /// it again.
    pub fn into_preview_delete_shot_range(
        self,
        section_id: SectionId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
    ) -> Result<DeleteShotRangeImpact, String> {
        let mut impact =
            self.preview_delete_shot_range_relationships(section_id, start_shot_id, end_shot_id)?;
        if Self::gap_close_is_offered(&impact) {
            impact.closures_removed_across_one_shot = self
                .into_gap_close_closure_removals(
                    section_id,
                    impact.start_pos as usize,
                    impact.end_pos as usize,
                )
                .ok();
        }
        Ok(impact)
    }

    /// Only a gap-close that is actually offered gets a prediction (#737).
    fn gap_close_is_offered(impact: &DeleteShotRangeImpact) -> bool {
        !impact.shift_blocked && impact.kind != RangeDeleteKind::WholeSection
    }

    /// [`Self::preview_delete_shot_range`] without the gap-close closure
    /// prediction, which simulates the whole gap-close on a clone. The range
    /// delete itself only needs the shift-blocked verdict before it runs that
    /// same gap-close for real, so it asks this and does not pay twice.
    pub fn preview_delete_shot_range_relationships(
        &self,
        section_id: SectionId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
    ) -> Result<DeleteShotRangeImpact, String> {
        let section = self
            .get_section(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        let n = section.shots.len();
        let mut start_pos = section
            .shots
            .iter()
            .position(|s| s.id == start_shot_id)
            .ok_or_else(|| format!("Shot {start_shot_id} not found in section {section_id}"))?;
        let mut end_pos = section
            .shots
            .iter()
            .position(|s| s.id == end_shot_id)
            .ok_or_else(|| format!("Shot {end_shot_id} not found in section {section_id}"))?;
        if start_pos > end_pos {
            std::mem::swap(&mut start_pos, &mut end_pos);
        }

        let kind = match (start_pos == 0, end_pos == n - 1) {
            (true, true) => RangeDeleteKind::WholeSection,
            (true, false) => RangeDeleteKind::Prefix,
            (false, true) => RangeDeleteKind::Suffix,
            (false, false) => RangeDeleteKind::Interior,
        };

        // Stations any offered action removes. The gap-close (shift) removes the
        // interior PLUS the tail boundary station `end_pos+1` (which an interior
        // split keeps as B:0). Use that superset for the relationship panel so
        // closing the gap never hides what it touches. (Prefix/suffix/whole
        // remove exactly this set; only an interior split keeps `end_pos+1`.)
        let removed_indices: Vec<u32> = match kind {
            RangeDeleteKind::Interior => ((start_pos as u32 + 1)..=(end_pos as u32 + 1)).collect(),
            RangeDeleteKind::Prefix => (0..=(end_pos as u32)).collect(),
            RangeDeleteKind::Suffix => ((start_pos as u32 + 1)..=(n as u32)).collect(),
            RangeDeleteKind::WholeSection => (0..=(n as u32)).collect(),
        };
        let removed_set: HashSet<StationRef> = removed_indices
            .iter()
            .map(|&i| StationRef::new(section_id, i))
            .collect();
        let in_removed = |key: &StationKey| -> bool {
            self.resolve_key(key)
                .is_some_and(|sr| removed_set.contains(&sr))
        };

        // Stations the SPLIT drops (interior only - it keeps both boundaries).
        // Datums/comments anchored here are silently lost by the split, so the
        // dialog warns about them; the gap-close instead BLOCKS on datum loss,
        // so these loss sets are intentionally split-only.
        let split_removed_set: HashSet<StationRef> = match kind {
            RangeDeleteKind::Interior => ((start_pos as u32 + 1)..=(end_pos as u32))
                .map(|i| StationRef::new(section_id, i))
                .collect(),
            // No split path exists for these - their loss set is the removed set.
            _ => removed_set.clone(),
        };
        let split_in_removed = |key: &StationKey| -> bool {
            self.resolve_key(key)
                .is_some_and(|sr| split_removed_set.contains(&sr))
        };

        let removed_datums: Vec<StationRef> = self
            .zero_datums
            .iter()
            .filter(|zd| split_in_removed(&zd.station_ref))
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .collect();
        // Simulate the connectivity the gap-close preserves: each per-shot
        // `remove_shot_with_rewire` re-bridges any external merge group that was
        // joined only through a removed station, so two boundary stations can be
        // reconnected AFTER the deletion by a replacement merge. The blocker and
        // the replacement-merge display both read this POST-preservation topology
        // (`sim`) so a loop / both-anchored case that only re-forms via a
        // replacement is not missed (and a path that exists only through a
        // to-be-removed station is correctly dropped).
        let mut sim = self.clone();
        sim.preserve_connections_for_removed_stations(&removed_set);

        // Gap-close (shift) availability.
        let mut shift_blocked_reasons: Vec<String> = Vec::new();
        // Loop / both-anchored ambiguity applies only to an INTERIOR gap-close,
        // which removes stations `start_pos+1 ..= end_pos+1` (= `removed_set`) and
        // rejoins the head boundary `start_pos` with the first SURVIVING tail
        // station `end_pos+2`. Evaluate connectivity / anchoring between those two
        // survivors on `sim` (post-preservation), treating every removed station
        // (incl. the tail boundary `end_pos+1`) and the range's shots as gone.
        if kind == RangeDeleteKind::Interior {
            let head = StationRef::new(section_id, start_pos as u32);
            let tail = StationRef::new(section_id, end_pos as u32 + 2);
            // Only a MERGE-based loop blocks: merges are hard coincidence
            // constraints that survive the delete, so gap-closing two
            // merge-connected sides is over-constrained. A loop closure is a
            // soft/redundant constraint that the delete removes (or converts), so
            // the resulting geometry is still valid - closures must NOT block
            // (include_closures = false).
            if sim.stations_connected_excluding_range(
                head,
                tail,
                section_id,
                start_pos,
                end_pos,
                &removed_set,
                false,
            ) {
                shift_blocked_reasons.push(
                    "This range is part of a loop (the two sides are already connected)."
                        .to_string(),
                );
            }
            let head_anchored = sim.station_reaches_datum_excluding_range(
                head,
                section_id,
                start_pos,
                end_pos,
                &removed_set,
            );
            let tail_anchored = sim.station_reaches_datum_excluding_range(
                tail,
                section_id,
                start_pos,
                end_pos,
                &removed_set,
            );
            if head_anchored && tail_anchored {
                shift_blocked_reasons.push("Both sides are anchored to a zero datum.".to_string());
            }
        }
        // The gap-close removes every station in `removed_set`; if any carries a
        // zero datum the survey loses an anchor, so block (use split - which
        // keeps the boundary datums - or remove the datum first).
        let shift_drops_datum = self
            .zero_datums
            .iter()
            .any(|zd| in_removed(&zd.station_ref));
        if shift_drops_datum {
            let reason = "Deleting this range would remove a zero datum.".to_string();
            if !shift_blocked_reasons.contains(&reason) {
                shift_blocked_reasons.push(reason);
            }
        }

        // Station comments on every station the gap-close removes (the shift
        // superset, incl. the tail boundary `end_pos+1`). The dialog frames these
        // per action - a SPLIT keeps `end_pos+1` and its comment - using the
        // `start_pos`/`end_pos` it carries.
        let mut removed_comments: Vec<StationRef> = self
            .station_comments
            .keys()
            .filter_map(|k| self.resolve_key(k))
            .filter(|sr| removed_set.contains(sr))
            .collect();
        // [#469] Sorted like its sibling `removed_stations` below, and for the same
        // reason: this is built by iterating a `HashMap`, so without a sort the dialog
        // lists the same comments in a different order on every run of the same preview.
        // Nothing downstream re-sorts (`BulkDeleteDialog.tsx` renders the vector as
        // given), so this is the only place the order can be pinned.
        removed_comments.sort_by_key(|sr| (sr.section_id, sr.station_index));

        // The relationship panel (`removed_stations`/`removed_merges`/
        // `removed_closures`) and the preserved-connectivity replacements describe
        // the action actually OFFERED. When the gap-close is blocked an interior
        // range can only SPLIT, which keeps both boundary stations, so describe
        // the split's smaller removed set and its own preservation; otherwise
        // describe the gap-close superset (the help text notes a split keeps the
        // boundaries). Prefix/suffix have no split, so this is always the gap-close
        // set for them.
        let shift_blocked = !shift_blocked_reasons.is_empty();
        let interior_split_only = shift_blocked && kind == RangeDeleteKind::Interior;
        let split_sim;
        let (impact_removed_set, impact_sim): (&HashSet<StationRef>, &Project) =
            if interior_split_only {
                let mut s = self.clone();
                s.preserve_connections_for_removed_stations(&split_removed_set);
                split_sim = s;
                (&split_removed_set, &split_sim)
            } else {
                (&removed_set, &sim)
            };
        let in_impact = |key: &StationKey| -> bool {
            self.resolve_key(key)
                .is_some_and(|sr| impact_removed_set.contains(&sr))
        };

        let mut removed_stations: Vec<StationRef> = impact_removed_set.iter().copied().collect();
        removed_stations.sort_by_key(|s| (s.section_id, s.station_index));
        let mut removed_merges = Vec::new();
        for merge in &self.station_merges {
            if in_impact(&merge.station_a) || in_impact(&merge.station_b) {
                removed_merges.push(StationPair {
                    station_a: self.key_to_ref_lossy(&merge.station_a),
                    station_b: self.key_to_ref_lossy(&merge.station_b),
                });
            }
        }
        let mut removed_closures = Vec::new();
        for closure in &self.loop_closures {
            if in_impact(&closure.station_a) || in_impact(&closure.station_b) {
                removed_closures.push(ClosurePair {
                    closure_id: closure.id,
                    station_a: self.key_to_ref_lossy(&closure.station_a),
                    station_b: self.key_to_ref_lossy(&closure.station_b),
                });
            }
        }

        // Connectivity the operation PRESERVES, so the dialog doesn't overstate
        // loss (it would otherwise list a bridging merge/closure as "Removed"
        // with no replacement shown). Diff the simulated post-preservation graph
        // against the current one - see [`Self::preview_preserved_connections`].
        let (replacement_merges, replacement_closure_updates) =
            self.preview_preserved_connections(impact_sim);

        Ok(DeleteShotRangeImpact {
            section_id,
            start_pos: start_pos as u32,
            end_pos: end_pos as u32,
            kind,
            removed_stations,
            removed_merges,
            removed_closures,
            replacement_merges,
            replacement_closure_updates,
            closure_to_merge: Vec::new(),
            merge_rewire_skipped: false,
            closure_rewire_skipped: false,
            shift_blocked,
            shift_blocked_reasons,
            removed_datums,
            removed_comments,
            closures_removed_across_one_shot: None,
        })
    }

    /// Neighbors of `station_ref` for range-delete connectivity checks: like
    /// [`Self::station_neighbors_excluding_shot`] but skips ALL shot edges at
    /// positions `start_pos..=end_pos` in `section_id` (the gap-closed range).
    fn range_neighbors(
        &self,
        station_ref: StationRef,
        section_id: SectionId,
        start_pos: usize,
        end_pos: usize,
        include_closures: bool,
    ) -> Vec<StationRef> {
        let mut neighbors = Vec::new();
        let Some(section) = self.get_section(station_ref.section_id) else {
            return neighbors;
        };
        let idx = station_ref.station_index as usize;
        if idx > section.shots.len() {
            return neighbors;
        }
        let in_range = |shot_idx: usize| {
            station_ref.section_id == section_id && (start_pos..=end_pos).contains(&shot_idx)
        };
        if idx > 0 && !in_range(idx - 1) {
            neighbors.push(StationRef::new(station_ref.section_id, (idx - 1) as u32));
        }
        if idx < section.shots.len() && !in_range(idx) {
            neighbors.push(StationRef::new(station_ref.section_id, (idx + 1) as u32));
        }
        for merge in &self.station_merges {
            if self.key_is_ref(&merge.station_a, station_ref) {
                neighbors.extend(self.resolve_key(&merge.station_b));
            } else if self.key_is_ref(&merge.station_b, station_ref) {
                neighbors.extend(self.resolve_key(&merge.station_a));
            }
        }
        if include_closures {
            for closure in &self.loop_closures {
                if self.key_is_ref(&closure.station_a, station_ref) {
                    neighbors.extend(self.resolve_key(&closure.station_b));
                } else if self.key_is_ref(&closure.station_b, station_ref) {
                    neighbors.extend(self.resolve_key(&closure.station_a));
                }
            }
        }
        neighbors
    }

    /// Are `a` and `b` connected WITHOUT using a shot in `section_id`'s range
    /// `start_pos..=end_pos` and without passing through a `removed` station?
    /// Used to detect a loop around a to-be-gap-closed interior range so the
    /// check sees the post-delete topology rather than each original single shot.
    fn stations_connected_excluding_range(
        &self,
        a: StationRef,
        b: StationRef,
        section_id: SectionId,
        start_pos: usize,
        end_pos: usize,
        removed: &HashSet<StationRef>,
        include_closures: bool,
    ) -> bool {
        if a == b {
            return true;
        }
        let mut visited = HashSet::new();
        visited.insert(a);
        let mut queue = VecDeque::new();
        queue.push_back(a);
        while let Some(cur) = queue.pop_front() {
            for nb in self.range_neighbors(cur, section_id, start_pos, end_pos, include_closures) {
                if removed.contains(&nb) {
                    continue;
                }
                if nb == b {
                    return true;
                }
                if visited.insert(nb) {
                    queue.push_back(nb);
                }
            }
        }
        false
    }

    /// Does `station` reach a zero datum WITHOUT using a shot in the range or
    /// passing through a `removed` station (datums on removed stations don't
    /// count - they are themselves being deleted)?
    fn station_reaches_datum_excluding_range(
        &self,
        station: StationRef,
        section_id: SectionId,
        start_pos: usize,
        end_pos: usize,
        removed: &HashSet<StationRef>,
    ) -> bool {
        let datum_set: HashSet<StationRef> = self
            .zero_datums
            .iter()
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .filter(|sr| !removed.contains(sr))
            .collect();
        if datum_set.contains(&station) {
            return true;
        }
        let mut visited = HashSet::new();
        visited.insert(station);
        let mut queue = VecDeque::new();
        queue.push_back(station);
        while let Some(cur) = queue.pop_front() {
            for nb in self.range_neighbors(cur, section_id, start_pos, end_pos, true) {
                if removed.contains(&nb) {
                    continue;
                }
                if datum_set.contains(&nb) {
                    return true;
                }
                if visited.insert(nb) {
                    queue.push_back(nb);
                }
            }
        }
        false
    }

    /// Read-only summary of the merges/closures a range delete would ADD or
    /// REWIRE to keep external groups connected when the removed stations vanish.
    /// Diffs a caller-supplied `sim` (a clone of `self` with
    /// [`Self::preserve_connections_for_removed_stations`] already applied)
    /// against `self`, so the bulk-delete preview can never drift from what the
    /// executor does. Returns `(replacement_merges, replacement_closure_updates)`
    /// resolved to current positional refs.
    fn preview_preserved_connections(
        &self,
        sim: &Project,
    ) -> (Vec<StationPair>, Vec<ClosureUpdate>) {
        let same_pair = |a: StationRef, b: StationRef, x: StationRef, y: StationRef| {
            (a == x && b == y) || (a == y && b == x)
        };
        // Merges that exist BEFORE preservation (resolved, unordered).
        let existing: Vec<(StationRef, StationRef)> = self
            .station_merges
            .iter()
            .filter_map(|m| {
                Some((
                    self.resolve_key(&m.station_a)?,
                    self.resolve_key(&m.station_b)?,
                ))
            })
            .collect();

        let mut replacement_merges: Vec<StationPair> = Vec::new();
        for m in &sim.station_merges {
            let (Some(a), Some(b)) = (sim.resolve_key(&m.station_a), sim.resolve_key(&m.station_b))
            else {
                continue;
            };
            if existing.iter().any(|&(ea, eb)| same_pair(ea, eb, a, b)) {
                continue; // pre-existing merge, not a replacement
            }
            if replacement_merges
                .iter()
                .any(|p| same_pair(p.station_a, p.station_b, a, b))
            {
                continue; // already listed
            }
            replacement_merges.push(StationPair {
                station_a: a,
                station_b: b,
            });
        }

        // Closures whose endpoint moved off a removed station onto a survivor.
        let mut replacement_closure_updates: Vec<ClosureUpdate> = Vec::new();
        for old in &self.loop_closures {
            let Some(new) = sim.loop_closures.iter().find(|c| c.id == old.id) else {
                continue;
            };
            for (old_key, new_key) in [
                (&old.station_a, &new.station_a),
                (&old.station_b, &new.station_b),
            ] {
                let old_ref = self.resolve_key(old_key);
                let new_ref = sim.resolve_key(new_key);
                if old_ref != new_ref {
                    if let (Some(old_station), Some(new_station)) = (old_ref, new_ref) {
                        replacement_closure_updates.push(ClosureUpdate {
                            closure_id: old.id,
                            old_station,
                            new_station,
                        });
                    }
                }
            }
        }

        (replacement_merges, replacement_closure_updates)
    }

    pub fn remove_shot_with_rewire(
        &mut self,
        section_id: SectionId,
        shot_id: ShotId,
    ) -> Result<(Shot, DeleteShotImpact), String> {
        self.remove_shot_with_rewire_excluding_anchors(section_id, shot_id, &HashSet::new(), false)
    }

    /// Like [`Self::remove_shot_with_rewire`] but for use inside a bulk-range
    /// delete, with two range-specific knobs:
    /// - `doomed_anchors`: canonical keys of stations the surrounding range will
    ///   ALSO remove. The keep-side (head vs tail) decision ignores anchors on
    ///   these - deciding from an anchor that is itself about to be deleted would
    ///   displace the survivors (e.g. a suffix shift would shift the head).
    /// - `defer_cascade`: skip the per-shot `cascade_connection_offsets` and the
    ///   alignment assert. The range driver runs ONE cascade after all range
    ///   shots are gone (see `execute_delete_shot_range_shift`); a per-shot
    ///   cascade would propagate a doomed station's transient shift into a merged
    ///   free-floating section before that merge is removed.
    pub fn remove_shot_with_rewire_excluding_anchors(
        &mut self,
        section_id: SectionId,
        shot_id: ShotId,
        doomed_anchors: &HashSet<StationKey>,
        defer_cascade: bool,
    ) -> Result<(Shot, DeleteShotImpact), String> {
        let section = self
            .get_section(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        let pos = section
            .shots
            .iter()
            .position(|s| s.id == shot_id)
            .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?;

        let impact = self.preview_delete_shot(section_id, shot_id)?;
        // Which closures are orphans ALREADY, before this removal touches
        // anything - see the conversion below for what that is for. Must be taken
        // here, ahead of the first mutation (the retains just below), or a closure
        // this removal orphans would be mistaken for one that arrived orphaned.
        let preexisting_orphans = self.orphan_closure_ids();
        let removed_station = impact.removed_station;
        // The deleted station's canonical key (still resolvable - the shot is
        // removed below). Stored refs are canonical, so key equality is sound.
        let removed_key = self.key_for_ref(removed_station);

        if let Some(rk) = removed_key.clone() {
            self.station_offsets.remove(&rk);
            self.station_comments.remove(&rk);
            self.station_names.remove(&rk);
            self.zero_datums.retain(|zd| zd.station_ref != rk);
            self.station_merges
                .retain(|m| m.station_a != rk && m.station_b != rk);
        }

        // Pre-resolve replacement closure targets to keys (the drain below can't
        // borrow `self`).
        let closure_updates: HashMap<u64, StationKey> = impact
            .replacement_closure_updates
            .iter()
            .filter_map(|u| self.key_for_ref(u.new_station).map(|k| (u.closure_id, k)))
            .collect();

        let mut updated_closures = Vec::with_capacity(self.loop_closures.len());
        for mut closure in self.loop_closures.drain(..) {
            let touches_a = removed_key.as_ref() == Some(&closure.station_a);
            let touches_b = removed_key.as_ref() == Some(&closure.station_b);
            if touches_a || touches_b {
                if let Some(new_key) = closure_updates.get(&closure.id).cloned() {
                    if touches_a {
                        closure.station_a = new_key;
                    } else {
                        closure.station_b = new_key;
                    }
                    updated_closures.push(closure);
                }
                // else: closure dropped (its anchoring station is gone)
            } else {
                updated_closures.push(closure);
            }
        }
        self.loop_closures = updated_closures;

        for pair in &impact.replacement_merges {
            if !self.has_direct_merge(pair.station_a, pair.station_b) {
                if let (Some(ka), Some(kb)) = (
                    self.key_for_ref(pair.station_a),
                    self.key_for_ref(pair.station_b),
                ) {
                    self.record_station_merge(StationMerge {
                        station_a: ka,
                        station_b: kb,
                        merged_at: Utc::now(),
                    });
                }
            }
        }

        // Closures whose loop the deletion broke become plain merges: the
        // endpoints stay declared-coincident but are no longer a true loop (#8).
        // A closure TOLERATES a misclosure and a merge ASSERTS coincidence, so
        // each promotion plans the translation that makes the claim true and is
        // settled below, once this function's own geometry is final (#464).
        //
        // A `None` plan REFUSES the merge here too. `preview_delete_shot` asked
        // the same over-anchoring question through
        // `would_double_anchor_after_removal` - accumulating the promotions it
        // decided on, so its hypothesis matches this loop closure-for-closure -
        // and everything this function has mutated since (the retains that drop
        // the removed station's merges and datum, the closure rewires, the
        // replacement merges) is exactly the `doomed`/`planned` graph the preview
        // evaluated against. So this arm is UNREACHABLE, and recording anyway is
        // strictly worse than not: an unsettled promotion is a merge asserting a
        // coincidence nothing settles and no cascade, realign or Repair can heal
        // (#464 defect A - it measured 1.4 km in the test that pinned this).
        // Preview and execution stay structurally identical because the preview,
        // refusing the same promotion, routes that closure to
        // `replacement_closure_updates` instead: the closure survives, re-anchored,
        // in both halves.
        let mut minted: Vec<MintedMerge> = Vec::new();
        for pair in &impact.closure_to_merge {
            if !self.has_direct_merge(pair.station_a, pair.station_b) {
                if let (Some(ka), Some(kb)) = (
                    self.key_for_ref(pair.station_a),
                    self.key_for_ref(pair.station_b),
                ) {
                    if let Some(plan) = self.plan_merge_promotion(&ka, &kb) {
                        minted.push(plan);
                        self.record_station_merge(StationMerge {
                            station_a: ka,
                            station_b: kb,
                            merged_at: Utc::now(),
                        });
                    } else {
                        // Loud: it means the preview promised a merge on a
                        // hypothesis this function then contradicted, which is
                        // the preview/execution divergence #464 is about.
                        tracing::error!(
                            station_a = ?pair.station_a,
                            station_b = ?pair.station_b,
                            purpose = "never record a promotion nothing can settle",
                            "Refused a promised closure->merge promotion: it would double-anchor"
                        );
                        #[cfg(test)]
                        #[expect(
                            clippy::panic,
                            reason = "tripwire: unreachable once the preview's guard \
                                      accumulates, and a silent preview/execution divergence \
                                      is what #464 is about"
                        )]
                        {
                            panic!(
                                "preview promised a closure->merge promotion {:?} <-> {:?} \
                                 that `plan_merge_promotion` refuses",
                                pair.station_a, pair.station_b
                            );
                        }
                    }
                }
            }
        }

        let deleted_shot = {
            let section_mut = self
                .get_section_mut(section_id)
                .ok_or_else(|| format!("Section {section_id} not found"))?;
            section_mut
                .remove_shot(shot_id)
                .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?
        };

        // Station keys are position-independent - no ref-shifting on delete. The
        // deleted station's own refs were rewired above; every surviving ref keeps
        // its anchor (its resolved index just moves down).
        //
        // One case the `removed_station` rewire above can't catch: a ref anchored
        // directly on the DELETED shot's boundary, `AfterShot(deleted_uuid)`, that
        // is NOT the `removed_station` (this happens at `pos == 0`, where
        // `removed_station` is `SectionStart` but a prepend had re-anchored a
        // boundary ref to `AfterShot(first_shot)`). Its anchoring shot is now gone,
        // so re-anchor it to the station that absorbs the deleted boundary -
        // `SectionStart` when the first shot was removed, else `AfterShot(prev)`.
        if let Some(deleted_uuid) = deleted_shot.uuid.clone() {
            let survivor = if pos == 0 {
                Some(StationAnchor::SectionStart)
            } else {
                self.get_section(section_id)
                    .and_then(|s| s.shots.get(pos - 1))
                    .and_then(|s| s.uuid.clone())
                    .map(StationAnchor::AfterShot)
            };
            if let Some(survivor) = survivor {
                let stale = StationAnchor::AfterShot(deleted_uuid);
                self.map_station_refs(|key| {
                    if key.section_id == section_id && key.anchor == stale {
                        StationKey::new(section_id, survivor.clone())
                    } else {
                        key
                    }
                });
            }
        }

        // The deletion may have removed the OTHER path of a loop, leaving a
        // closure as the sole bridge to a section - that is no longer a loop, so
        // turn any such closure into a merge before the offsets are recomputed
        // (issue #8, beyond the closure-touched-the-deleted-station case above).
        //
        // Scoped to the orphans THIS removal created, not a whole-project sweep,
        // because this function also runs as an inverse (undo of `CreateShot`, via
        // `remove_created_shot`) and an inverse must not do more than invert
        // (#413, the orphan-closure observation in `fuzz_undo_tests.rs`'s header).
        // A closure orphaned EARLIER by a command
        // that does not convert - `removeZeroDatum` un-anchors one side and the
        // sweep's double-anchor guard stops refusing it - is part of the state the
        // history recorded; converting it here would land the undo on a state no
        // snapshot ever held. Everything this removal genuinely stranded still
        // converts, including a closure nowhere near the deleted station whose
        // loop-ness hung off a merge that was. The forward user delete loses
        // nothing: `execute()`'s `can_orphan_closures` epilogue still sweeps the
        // whole project after `Command::DeleteShot`.
        minted.extend(self.convert_closures_orphaned_since(&preexisting_orphans));

        // After removing a shot, stations after the deletion point (index > pos)
        // shift by -displacement in section-local coordinates.  We must decide
        // which side to keep at its original absolute position:
        //
        //   - tail anchored (datum reachable via post-deletion stations):
        //     adjust offset += displacement → tail stays, head shifts
        //   - head anchored or neither anchored:
        //     don't adjust offset → head stays, tail shifts
        //
        // The shift_blocked guard already prevents deletion when BOTH sides
        // are datum-anchored.
        let (dx, dy) = Self::shot_displacement(
            &deleted_shot.to_shot_data(),
            self.azimuth_reference_for_id(section_id),
        );

        if pos == 0 {
            // First shot: station 0 removed, ALL remaining stations are "tail".
            // Always adjust to keep them in place.
            if dx != 0.0 || dy != 0.0 {
                self.adjust_section_offset(section_id, dx, dy);
            }
        } else if dx != 0.0 || dy != 0.0 {
            // Mid-section: keep whichever side of the deletion point is anchored.
            // Tail stations resolve to index > pos (after the deletion renumbered
            // them). The HEAD side is consulted too, for the same reason the
            // insert consults it - see `anchored_sides_of_edit`: compensating for
            // a tail whose "anchor" runs through a soft closure while the head is
            // merged straight to the datum tears the head merge apart and the
            // cascade cannot put it back (#413 finding 14's undo-side mirror,
            // which reaches this function through undo of `CreateShot`).
            let (head_anchored, tail_anchored) =
                self.anchored_sides_of_edit(section_id, pos as u32, doomed_anchors);
            if tail_anchored && !head_anchored {
                self.adjust_section_offset(section_id, dx, dy);
            }
        }

        // Every merge this removal MINTED out of a coincidence claim now pays for
        // the coincidence it declares, by translating the movable side onto the
        // anchored one. This is the last thing that touches geometry before the
        // cascade, deliberately: the gap-close compensation above slides the whole
        // edited section, and the shot removal itself re-placed that section's
        // stations, so a translation computed when the merge was recorded would
        // already be stale. It runs even when the cascade is deferred - the range
        // driver's single cascade is seeded with the edited section and cannot
        // reach a component the range has just cut loose (#464 defects B and C).
        self.settle_minted_merges(&minted);

        // A bulk range delete removes shots one at a time and runs ONE cascade +
        // alignment pass at the end (see execute_delete_shot_range_shift). A
        // per-step cascade here would propagate a doomed station's transient
        // shift into a merged free-floating section BEFORE that merge is removed,
        // leaving the section displaced with no edge to pull it back; and an
        // intermediate step can transiently misalign a merge on a doomed station.
        // The range driver sets `defer_cascade`; standalone single-shot deletes
        // cascade + assert immediately.
        if !defer_cascade {
            // Cascade offset adjustments for any drifted connections on the
            // non-anchored side.
            self.cascade_connection_offsets(&[section_id]);
            self.debug_assert_merge_alignment("remove_shot_with_rewire");
        }

        self.touch();
        Ok((deleted_shot, impact))
    }

    /// The exact inverse of [`Self::add_shot`] into an **empty** section:
    /// remove the section's only shot and leave station 0 - and everything
    /// anchored to it - exactly as the append found them.
    ///
    /// [`Self::remove_shot_with_rewire`] is the wrong inverse for this one
    /// shape. Its `pos == 0` branch answers the USER's delete-the-first-shot
    /// question: station 0 goes away, station 1 becomes the new section start,
    /// every ref keyed to station 0 is dropped, and `adjust_section_offset`
    /// slides the section so the surviving stations keep their world positions.
    /// With exactly one shot the appended shot IS index 0, so there is no
    /// survivor to keep in place: the compensation slides the whole section by
    /// the shot's horizontal vector while the offset-neutral `add_shot` gave
    /// nothing back, and the redo re-appends offset-neutrally, so every
    /// undo/redo cycle slides it again (#413, finding 3, and finding 15 which
    /// is that leftover key observed several moves later). The same branch
    /// destroys the merges, zero datums and station comments keyed to station 0,
    /// which in an empty section is every ref it has (finding 3's data-loss
    /// half).
    ///
    /// The station this append CREATED is station 1, `AfterShot(uuid)`. That is
    /// the one that must go, with anything anchored on it; station 0 predates
    /// the append and must not move. Offset-neutral in both directions, exactly
    /// as `add_shot` is, so no cascade or alignment pass is needed either - the
    /// sole exception being a closure the removal turns into a merge (see the
    /// conversion below), which is a new rigid edge and not an offset this
    /// function chose to move.
    ///
    /// Errors (rather than silently doing something else) when `shot_id` is not
    /// the section's sole shot, so a caller that mis-detects the shape gets a
    /// refusal instead of a wrong inverse.
    pub fn remove_appended_first_shot(
        &mut self,
        section_id: SectionId,
        shot_id: ShotId,
    ) -> Result<Shot, String> {
        let section = self
            .get_section(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        if section.shots.len() != 1 || section.shots[0].id != shot_id {
            return Err(format!(
                "Shot {shot_id} is not the only shot of section {section_id}"
            ));
        }
        // Which closures are orphans ALREADY, before this inverse touches
        // anything - see the conversion below. Taken ahead of the retains, which
        // are this function's first mutation.
        let preexisting_orphans = self.orphan_closure_ids();
        // Refs on the station the append created. There are none when the undo
        // follows its own `CreateShot` (anything anchored there would have been
        // pushed above this entry and undone first), but a ref whose anchoring
        // shot is gone dangles, so drop rather than orphan.
        let created = section
            .shots
            .first()
            .and_then(|shot| shot.uuid.clone())
            .map(|uuid| StationKey::new(section_id, StationAnchor::AfterShot(uuid)));
        if let Some(key) = created {
            self.station_offsets.remove(&key);
            self.station_comments.remove(&key);
            self.station_names.remove(&key);
            self.zero_datums.retain(|zd| zd.station_ref != key);
            // `MergeTopology::retain` invalidates its own adjacency memo, so no
            // explicit cache maintenance is needed here.
            self.station_merges
                .retain(|m| m.station_a != key && m.station_b != key);
            self.loop_closures
                .retain(|c| c.station_a != key && c.station_b != key);
        }
        let removed = self
            .get_section_mut(section_id)
            .and_then(|section| section.remove_shot(shot_id))
            .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?;

        // Same duty `remove_shot_with_rewire` discharges after ITS retains
        // and its shot removal, for the same reason and in the same place: dropping
        // the merges that hung off the removed station can leave a loop closure
        // somewhere else as the SOLE connection between its two endpoints, and a
        // closure that is not a loop is a merge (#8, and #413 finding 10 - the
        // invariant is enforced at every other delete entry point, so an inverse
        // that skipped it was the one way to persist a state the app declares
        // impossible). Must run after the shot is gone: the conversion asks whether
        // the endpoints are still connected, and until then they are.
        //
        // Scoped exactly as the sibling scopes its own call, and for a reason that
        // binds harder here because this function is ONLY ever an inverse: an
        // inverse must not do more than invert (#413, the orphan-closure
        // observation in `fuzz_undo_tests.rs`'s header). A
        // closure that was already an orphan when this undo started - orphaned
        // moves earlier by a command that does not convert, `removeZeroDatum`
        // un-anchoring one side so the sweep's double-anchor guard stops refusing
        // it - is part of the state the history recorded, and converting it here
        // would land the undo on a state no snapshot ever held. What this removal
        // itself stranded still converts.
        //
        // The one thing this does NOT copy from the sibling is its unconditional
        // cascade: this function is offset-neutral by contract (see above), and
        // re-cascading here would undo that. A conversion turns a soft edge into a
        // rigid one, though, which can leave the new merge drifted, so THAT case -
        // and only that case - cascades, exactly as the sibling does. Nothing
        // converted is the overwhelmingly common case and stays offset-neutral.
        let minted = self.convert_closures_orphaned_since(&preexisting_orphans);
        if !minted.is_empty() {
            // Settled before the cascade, as the sibling does and for the same
            // reason (#464): the cascade is seeded with this section, so a merge
            // whose other end is datum-anchored has BOTH ends authoritative and
            // the BFS never traverses it. Nothing between here and the cascade
            // moves anything, so "now" is final geometry for this function.
            self.settle_minted_merges(&minted);
            self.cascade_connection_offsets(&[section_id]);
        }

        self.touch();
        Ok(removed)
    }

    /// Remove a shot
    pub fn remove_shot(&mut self, section_id: SectionId, shot_id: ShotId) -> Option<Shot> {
        self.remove_shot_with_rewire(section_id, shot_id)
            .ok()
            .map(|(shot, _)| shot)
    }

    /// Set a shot's kind (permanent / jump / reference, #27)
    pub fn set_shot_kind(
        &mut self,
        section_id: SectionId,
        shot_id: ShotId,
        kind: ShotKind,
    ) -> Option<&Shot> {
        let section = self.get_section_mut(section_id)?;
        let shot = section.get_shot_mut(shot_id)?;
        shot.kind = kind;
        self.touch();
        self.get_section(section_id)?.get_shot(shot_id)
    }
}
