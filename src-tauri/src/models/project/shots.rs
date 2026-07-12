use chrono::Utc;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::models::{
    SectionId, Shot, ShotData, ShotId, ShotUpdates, StationAnchor, StationKey, StationMerge,
    StationRef, FEATURE_ID_STRIDE,
};

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
    /// `false` to ask "are these connected by the *survey itself*" — used to tell
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

    /// Insert a shot after a specific shot in a section. `forced_id` preserves
    /// the original id and `forced_uuid` the original collaboration UUID when
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
        // Compute displacement before data is consumed — the new shot shifts
        // all stations after the insertion point.
        let (dx, dy) = Self::shot_displacement(&data);
        let section = self.get_section_mut(section_id)?;
        let shot =
            section.insert_shot_after_with_id(after_shot_id, data, forced_id, forced_uuid)?;
        let shot_id = shot.id;
        // Station keys are position-independent: no ref-shifting on insert (the
        // junction the new shot extends from keeps its anchor; shots beyond keep
        // theirs). The new shot's displacement still shifts stations > pos+1 in
        // space — if the tail side is datum-anchored, back-shift the offset so the
        // tail stays put. Positions are resolved from the stored keys.
        if dx != 0.0 || dy != 0.0 {
            let boundary = (pos + 1) as u32;
            let on_tail = |key: &StationKey| -> bool {
                key.section_id == section_id
                    && self
                        .resolve_key(key)
                        .is_some_and(|sr| sr.station_index > boundary)
            };
            let tail_anchored = self.zero_datums.iter().any(|zd| on_tail(&zd.station_ref))
                || self.station_merges.iter().any(|m| {
                    let (our, their) = if m.station_a.section_id == section_id {
                        (&m.station_a, &m.station_b)
                    } else if m.station_b.section_id == section_id {
                        (&m.station_b, &m.station_a)
                    } else {
                        return false;
                    };
                    on_tail(our)
                        && their.section_id != section_id
                        && self.has_datum_path_excluding(their.section_id, section_id)
                })
                || self.loop_closures.iter().any(|c| {
                    if !c.is_aligned {
                        return false;
                    }
                    let (our, their) = if c.station_a.section_id == section_id {
                        (&c.station_a, &c.station_b)
                    } else if c.station_b.section_id == section_id {
                        (&c.station_b, &c.station_a)
                    } else {
                        return false;
                    };
                    on_tail(our)
                        && their.section_id != section_id
                        && self.has_datum_path_excluding(their.section_id, section_id)
                });

            if tail_anchored {
                self.adjust_section_offset(section_id, -dx, -dy);
            }
        }
        self.cascade_connection_offsets(&[section_id]);
        self.debug_assert_merge_alignment("insert_shot_after");
        self.touch();
        self.get_section(section_id)?.get_shot(shot_id)
    }

    /// Insert a shot before a specific shot in a section. `forced_id` preserves
    /// the original id and `forced_uuid` the original collaboration UUID when
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
        let (dx, dy) = Self::shot_displacement(&data);
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
    pub fn split_shot_at_distance(
        &mut self,
        section_id: SectionId,
        shot_id: ShotId,
        distance: f64,
    ) -> Result<ShotId, String> {
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
            .insert_shot_before_with_id(shot_id, head_data, None, None)
            .ok_or_else(|| format!("Failed to insert split shot before {shot_id}"))?;
        let new_shot_id = new_shot.id;

        self.debug_assert_merge_alignment("split_shot_at_distance");
        self.touch();
        Ok(new_shot_id)
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
        let head_data = ShotData {
            azimuth_in: shot.azimuth_in,
            azimuth_out: shot.azimuth_out,
            length: distance,
            depth_in: shot.depth_in,
            depth_out: mid_depth,
            pitch_in: shot.pitch_in,
            pitch_out: shot.pitch_out,
            left: shot.left,
            right: shot.right,
            up: shot.up,
            down: shot.down,
            comment: None,
            is_reference: shot.is_reference,
            marker_index: None,
            temperature: shot.temperature,
        };
        let tail_updates = ShotUpdates {
            length: Some(tail_length),
            depth_in: Some(mid_depth),
            ..Default::default()
        };
        Ok((head_data, tail_updates))
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
                        if !self.has_direct_merge(pair.station_a, pair.station_b)
                            && !planned_pairs.iter().any(|p| {
                                (p.station_a == pair.station_a && p.station_b == pair.station_b)
                                    || (p.station_a == pair.station_b
                                        && p.station_b == pair.station_a)
                            })
                        {
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
            let mut candidates: Vec<StationRef> = merge_group_without.into_iter().collect();
            candidates.sort_by_key(Self::station_ref_key);
            let replacement = candidates.first().copied();

            for closure in &impact.removed_closures {
                if let Some(replacement_station) = replacement {
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
                    ) {
                        // The two ends still reach each other through the survey
                        // (shots + merges), so it remains a genuine loop — just
                        // move the closure off the deleted station.
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
    /// available. `start`/`end` may be given in any order — they are normalized.
    pub fn preview_delete_shot_range(
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

        // Stations the SPLIT drops (interior only — it keeps both boundaries).
        // Datums/comments anchored here are silently lost by the split, so the
        // dialog warns about them; the gap-close instead BLOCKS on datum loss,
        // so these loss sets are intentionally split-only.
        let split_removed_set: HashSet<StationRef> = match kind {
            RangeDeleteKind::Interior => ((start_pos as u32 + 1)..=(end_pos as u32))
                .map(|i| StationRef::new(section_id, i))
                .collect(),
            // No split path exists for these — their loss set is the removed set.
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
            // the resulting geometry is still valid — closures must NOT block
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
        // zero datum the survey loses an anchor, so block (use split — which
        // keeps the boundary datums — or remove the datum first).
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
        // per action — a SPLIT keeps `end_pos+1` and its comment — using the
        // `start_pos`/`end_pos` it carries.
        let removed_comments: Vec<StationRef> = self
            .station_comments
            .keys()
            .filter_map(|k| self.resolve_key(k))
            .filter(|sr| removed_set.contains(sr))
            .collect();

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
        // against the current one — see [`Self::preview_preserved_connections`].
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
    /// count — they are themselves being deleted)?
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
    ///   these — deciding from an anchor that is itself about to be deleted would
    ///   displace the survivors (e.g. a suffix shift would shift the head).
    /// - `defer_cascade`: skip the per-shot `cascade_connection_offsets` and the
    ///   alignment assert. The range driver runs ONE cascade after all range
    ///   shots are gone (see `execute_delete_shot_range_shift`); a per-shot
    ///   cascade would propagate a doomed station's transient shift into a merged
    ///   free-floating section before that merge is removed.
    pub(crate) fn remove_shot_with_rewire_excluding_anchors(
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
        let removed_station = impact.removed_station;
        // The deleted station's canonical key (still resolvable — the shot is
        // removed below). Stored refs are canonical, so key equality is sound.
        let removed_key = self.key_for_ref(removed_station);

        if let Some(rk) = removed_key.clone() {
            self.station_offsets.remove(&rk);
            self.station_comments.remove(&rk);
            self.zero_datums.retain(|zd| zd.station_ref != rk);
            self.station_merges
                .retain(|m| m.station_a != rk && m.station_b != rk);
        }
        self.markers
            .retain(|m| !(m.section_id == section_id && m.shot_id == shot_id));
        self.merge_adjacency_cache.clear();

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
        for pair in &impact.closure_to_merge {
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

        let deleted_shot = {
            let section_mut = self
                .get_section_mut(section_id)
                .ok_or_else(|| format!("Section {section_id} not found"))?;
            section_mut
                .remove_shot(shot_id)
                .ok_or_else(|| format!("Shot {shot_id} not found in section {section_id}"))?
        };

        // Station keys are position-independent — no ref-shifting on delete. The
        // deleted station's own refs were rewired above; every surviving ref keeps
        // its anchor (its resolved index just moves down).
        //
        // One case the `removed_station` rewire above can't catch: a ref anchored
        // directly on the DELETED shot's boundary, `AfterShot(deleted_uuid)`, that
        // is NOT the `removed_station` (this happens at `pos == 0`, where
        // `removed_station` is `SectionStart` but a prepend had re-anchored a
        // boundary ref to `AfterShot(first_shot)`). Its anchoring shot is now gone,
        // so re-anchor it to the station that absorbs the deleted boundary —
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
        // closure as the sole bridge to a section — that is no longer a loop, so
        // turn any such closure into a merge before the offsets are recomputed
        // (issue #8, beyond the closure-touched-the-deleted-station case above).
        self.convert_orphan_closures_to_merges();

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
        let (dx, dy) = Self::shot_displacement(&deleted_shot.to_shot_data());

        if pos == 0 {
            // First shot: station 0 removed, ALL remaining stations are "tail".
            // Always adjust to keep them in place.
            if dx != 0.0 || dy != 0.0 {
                self.adjust_section_offset(section_id, dx, dy);
            }
        } else if dx != 0.0 || dy != 0.0 {
            // Mid-section: check if the tail side is datum-anchored. Tail stations
            // resolve to index > pos (after the deletion renumbered them).
            let on_tail = |key: &StationKey| -> bool {
                key.section_id == section_id
                    && !doomed_anchors.contains(key)
                    && self
                        .resolve_key(key)
                        .is_some_and(|sr| sr.station_index > pos as u32)
            };
            let tail_anchored = self.zero_datums.iter().any(|zd| on_tail(&zd.station_ref))
                || self.station_merges.iter().any(|m| {
                    let (our, their) = if m.station_a.section_id == section_id {
                        (&m.station_a, &m.station_b)
                    } else if m.station_b.section_id == section_id {
                        (&m.station_b, &m.station_a)
                    } else {
                        return false;
                    };
                    on_tail(our)
                        && their.section_id != section_id
                        && self.has_datum_path_excluding(their.section_id, section_id)
                })
                || self.loop_closures.iter().any(|c| {
                    if !c.is_aligned {
                        return false;
                    }
                    let (our, their) = if c.station_a.section_id == section_id {
                        (&c.station_a, &c.station_b)
                    } else if c.station_b.section_id == section_id {
                        (&c.station_b, &c.station_a)
                    } else {
                        return false;
                    };
                    on_tail(our)
                        && their.section_id != section_id
                        && self.has_datum_path_excluding(their.section_id, section_id)
                });

            if tail_anchored {
                self.adjust_section_offset(section_id, dx, dy);
            }
        }

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

    /// Remove a shot
    pub fn remove_shot(&mut self, section_id: SectionId, shot_id: ShotId) -> Option<Shot> {
        self.remove_shot_with_rewire(section_id, shot_id)
            .ok()
            .map(|(shot, _)| shot)
    }

    /// Set a shot as reference or not
    pub fn set_shot_reference(
        &mut self,
        section_id: SectionId,
        shot_id: ShotId,
        is_reference: bool,
    ) -> Option<&Shot> {
        let section = self.get_section_mut(section_id)?;
        let shot = section.get_shot_mut(shot_id)?;
        shot.is_reference = is_reference;
        self.touch();
        self.get_section(section_id)?.get_shot(shot_id)
    }
}
