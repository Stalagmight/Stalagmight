use super::Project;
use crate::config::ClosureAdjustmentConfig;
use crate::{Coordinates, LoopClosure, SectionId, StationRef};
use cave_closure::horizontal::{HorizontalGroup, HorizontalObservation, HorizontalProblem};
use cave_closure::SolveControl;
use std::collections::{BTreeMap, HashMap, HashSet};

impl Project {
    pub(super) fn adjust_least_squares(
        &mut self,
        component: &HashSet<SectionId>,
        closures: &[LoopClosure],
        anchor_station: StationRef,
        config: &ClosureAdjustmentConfig,
    ) -> Result<(), String> {
        // Collect all stations in component
        let mut stations: Vec<StationRef> = Vec::new();
        for section in self.sections.iter().filter(|s| component.contains(&s.id)) {
            for idx in 0..=section.shots.len() {
                stations.push(StationRef::new(section.id, idx as u32));
            }
        }

        // Precompute every component station's raw (measured) coordinates in one
        // linear, component-scoped pass. The warm-start, group-Z and section-delta
        // phases below each need a station's measured position; calling
        // `compute_station_raw_coords` per station re-walks the section from its
        // start (O(index)), which made the solve O(n^2) on long degree-2 chains
        // (#81). The map is owned (no borrow of `self`), so it stays valid across
        // the later `iter_mut` update phase.
        let raw_coords = self.compute_component_raw_coords(component);

        let mut parent: HashMap<StationRef, StationRef> = HashMap::new();
        for station in &stations {
            parent.insert(*station, *station);
        }

        let find = |x: StationRef, parent: &mut HashMap<StationRef, StationRef>| -> StationRef {
            let mut current = x;
            while let Some(p) = parent.get(&current) {
                if *p == current {
                    break;
                }
                current = *p;
            }
            let root = current;
            // Path compression
            let mut current = x;
            while let Some(p) = parent.get(&current) {
                if *p == root {
                    break;
                }
                let next = *p;
                parent.insert(current, root);
                current = next;
            }
            root
        };

        let union = |a: StationRef, b: StationRef, parent: &mut HashMap<StationRef, StationRef>| {
            let ra = find(a, parent);
            let rb = find(b, parent);
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
                    union(a, b, &mut parent);
                }
            }
        }

        for closure in closures {
            if component.contains(&closure.station_a.section_id)
                && component.contains(&closure.station_b.section_id)
            {
                if let (Some(a), Some(b)) = (
                    self.resolve_key(&closure.station_a),
                    self.resolve_key(&closure.station_b),
                ) {
                    union(a, b, &mut parent);
                }
            }
        }

        let mut groups: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
        let mut station_root: HashMap<StationRef, StationRef> = HashMap::new();
        for station in &stations {
            let root = find(*station, &mut parent);
            station_root.insert(*station, root);
            groups.entry(root).or_default().push(*station);
        }

        let zero_datum_set: HashSet<StationRef> = self
            .zero_datums
            .iter()
            .filter(|zd| component.contains(&zd.station_ref.section_id))
            .filter_map(|zd| self.resolve_key(&zd.station_ref))
            .collect();

        let mut fixed_groups: HashSet<StationRef> = HashSet::new();
        let mut fixed_coords: HashMap<StationRef, Coordinates> = HashMap::new();

        for (root, members) in &groups {
            let mut datum_members: Vec<StationRef> = members
                .iter()
                .filter(|m| zero_datum_set.contains(m))
                .copied()
                .collect();
            if datum_members.len() > 1 {
                return Err("Multiple zero datums in a single constraint group".to_string());
            }
            if let Some(datum) = datum_members.pop() {
                let coords = raw_coords
                    .get(&datum)
                    .copied()
                    .ok_or_else(|| "Failed to compute zero datum coordinates".to_string())?;
                fixed_groups.insert(*root);
                fixed_coords.insert(*root, coords);
            }
        }

        if fixed_groups.is_empty() {
            let anchor_root = *station_root
                .get(&anchor_station)
                .ok_or_else(|| "Anchor station not found".to_string())?;
            let coords = raw_coords
                .get(&anchor_station)
                .copied()
                .ok_or_else(|| "Failed to compute anchor coordinates".to_string())?;
            fixed_groups.insert(anchor_root);
            fixed_coords.insert(anchor_root, coords);
        }

        // Preserve source group ordering and representative warm starts. These
        // select the translation retained by an unconstrained numerical axis.
        let mut roots: Vec<StationRef> = groups.keys().copied().collect();
        roots.sort_unstable();
        if roots.iter().all(|root| fixed_groups.contains(root)) {
            return Ok(());
        }
        let group_index: HashMap<StationRef, usize> = roots
            .iter()
            .enumerate()
            .map(|(i, root)| (*root, i))
            .collect();
        let mut problem = HorizontalProblem {
            groups: roots
                .iter()
                .map(|root| {
                    let coords = groups[root]
                        .first()
                        .and_then(|m| raw_coords.get(m))
                        .copied()
                        .unwrap_or_else(|| Coordinates::new(0.0, 0.0, 0.0));
                    HorizontalGroup {
                        initial: [coords.x, coords.y],
                        fixed: fixed_coords.get(root).map(|c| [c.x, c.y]),
                    }
                })
                .collect(),
            observations: Vec::new(),
            sigma_length_m: config.base_sigma_length_m,
            sigma_azimuth_deg: config.base_sigma_azimuth_deg,
        };
        for section in self.sections.iter().filter(|s| component.contains(&s.id)) {
            let reference = self.azimuth_reference_for(section);
            for (idx, shot) in section.shots.iter().enumerate() {
                let from = station_root[&StationRef::new(section.id, idx as u32)];
                let to = station_root[&StationRef::new(section.id, (idx + 1) as u32)];
                if from == to {
                    continue;
                }
                let vector = Self::shot_vector_along(shot, true, reference);
                problem.observations.push(HorizontalObservation {
                    from: group_index[&from],
                    to: group_index[&to],
                    displacement: [vector.x, vector.y],
                    length_m: shot.length,
                    azimuth_rad: shot.average_azimuth_true_rad(reference),
                    quality: section.quality,
                });
            }
        }
        let solution = problem
            .solve(&SolveControl::default())
            .map_err(|e| e.to_string())?;
        // Retain the original-system optimality assertion in every application
        // test that exercises the real adapter, not just the library's unit tests.
        #[cfg(test)]
        assert!(
            solution.axes.iter().all(|a| a.residual_infinity < 1e-6),
            "closure solve is not at the LSQ optimum: {:?}",
            solution.axes
        );

        // Collect original Z coordinates for each group root (depths are not adjusted)
        let mut group_raw_z: HashMap<StationRef, f64> = HashMap::new();
        for (root, members) in &groups {
            // Use the first member's raw Z as representative for the group
            if let Some(member) = members.first() {
                if let Some(coords) = raw_coords.get(member).copied() {
                    group_raw_z.insert(*root, coords.z);
                }
            }
        }

        let mut group_coords: HashMap<StationRef, Coordinates> = HashMap::new();
        for root in &roots {
            let raw_z = group_raw_z.get(root).copied().unwrap_or(0.0);
            if let Some(coords) = fixed_coords.get(root) {
                // Fixed groups keep their original coords but Z is always original
                group_coords.insert(*root, Coordinates::new(coords.x, coords.y, raw_z));
                continue;
            }
            let [x, y] = solution.coordinates[group_index[root]];
            group_coords.insert(*root, Coordinates::new(x, y, raw_z));
        }

        let station_offsets = self.resolved_station_offsets();

        // Identify spur stations (not on any cycle) by iterative leaf pruning.
        // Spur shots have no redundancy - there's nothing to adjust them against -
        // so they must be skipped in the update phase.
        let spur_stations: HashSet<StationRef> = {
            let mut adj_deg: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
            for section in self.sections.iter().filter(|s| component.contains(&s.id)) {
                for idx in 0..section.shots.len() {
                    let si = StationRef::new(section.id, idx as u32);
                    let sj = StationRef::new(section.id, (idx + 1) as u32);
                    adj_deg.entry(si).or_default().push(sj);
                    adj_deg.entry(sj).or_default().push(si);
                }
            }
            for merge in &self.station_merges {
                if component.contains(&merge.station_a.section_id)
                    && component.contains(&merge.station_b.section_id)
                {
                    if let (Some(a), Some(b)) = (
                        self.resolve_key(&merge.station_a),
                        self.resolve_key(&merge.station_b),
                    ) {
                        adj_deg.entry(a).or_default().push(b);
                        adj_deg.entry(b).or_default().push(a);
                    }
                }
            }
            for closure in closures {
                if let (Some(a), Some(b)) = (
                    self.resolve_key(&closure.station_a),
                    self.resolve_key(&closure.station_b),
                ) {
                    adj_deg.entry(a).or_default().push(b);
                    adj_deg.entry(b).or_default().push(a);
                }
            }

            // Fixed stations must not be pruned - they anchor the network
            // even if they sit at a graph leaf.  The anchor station is only
            // fixed when no zero datums exist (mirrors the solver logic).
            let mut pinned: HashSet<StationRef> = zero_datum_set.clone();
            if pinned.is_empty() {
                pinned.insert(anchor_station);
            }

            let mut degree: HashMap<StationRef, usize> =
                adj_deg.iter().map(|(s, nbrs)| (*s, nbrs.len())).collect();

            let mut stack: Vec<StationRef> = degree
                .iter()
                .filter(|(&s, &d)| d == 1 && !pinned.contains(&s))
                .map(|(&s, _)| s)
                .collect();

            let mut pruned: HashSet<StationRef> = HashSet::new();
            while let Some(station) = stack.pop() {
                pruned.insert(station);
                if let Some(neighbors) = adj_deg.get(&station) {
                    for neighbor in neighbors {
                        if !pruned.contains(neighbor) {
                            if let Some(d) = degree.get_mut(neighbor) {
                                *d = d.saturating_sub(1);
                                if *d == 1 && !pinned.contains(neighbor) {
                                    stack.push(*neighbor);
                                }
                            }
                        }
                    }
                }
            }
            pruned
        };

        // Update shots based on solved coordinates
        let azimuth_references = self.azimuth_reference_map();
        for section in self
            .sections
            .iter_mut()
            .filter(|s| component.contains(&s.id))
        {
            let azimuth_reference = azimuth_references
                .get(&section.id)
                .copied()
                .unwrap_or_default();
            for idx in 0..section.shots.len() {
                let station_i = StationRef::new(section.id, idx as u32);
                let station_j = StationRef::new(section.id, (idx + 1) as u32);

                // Skip spur shots - they have no loop redundancy and must
                // not be modified by the adjustment.
                if spur_stations.contains(&station_i) || spur_stations.contains(&station_j) {
                    continue;
                }

                let root_i = *station_root
                    .get(&station_i)
                    .ok_or_else(|| "Missing station root".to_string())?;
                let root_j = *station_root
                    .get(&station_j)
                    .ok_or_else(|| "Missing station root".to_string())?;

                // Skip shots within the same constraint group - both endpoints
                // share the same solved position, so the shot displacement would
                // collapse to zero. These shots are internal to a rigid group
                // (connected via merges/closures) and should not be modified.
                if root_i == root_j {
                    continue;
                }

                let coords_i = group_coords
                    .get(&root_i)
                    .ok_or_else(|| "Missing solved coordinates".to_string())?;
                let coords_j = group_coords
                    .get(&root_j)
                    .ok_or_else(|| "Missing solved coordinates".to_string())?;

                let offset_i = station_offsets
                    .get(&station_i)
                    .copied()
                    .unwrap_or((0.0, 0.0, 0.0));
                let offset_j = station_offsets
                    .get(&station_j)
                    .copied()
                    .unwrap_or((0.0, 0.0, 0.0));

                let base_i = Coordinates::new(
                    coords_i.x - offset_i.0,
                    coords_i.y - offset_i.1,
                    coords_i.z - offset_i.2,
                );
                let base_j = Coordinates::new(
                    coords_j.x - offset_j.0,
                    coords_j.y - offset_j.1,
                    coords_j.z - offset_j.2,
                );

                let shot = section
                    .shots
                    .get_mut(idx)
                    .ok_or_else(|| "Shot not found for LSQ update".to_string())?;
                // Only adjust horizontal geometry (azimuth + length); depths stay untouched.
                Self::update_shot_horizontal_only(
                    shot,
                    base_j.x - base_i.x,
                    base_j.y - base_i.y,
                    azimuth_reference,
                );
            }
        }

        // Recompute coordinates now that the shots above were rewritten. The
        // section-alignment phase below compares each station's *post-update*
        // position against its solved target, so it must not reuse `raw_coords`
        // (the pre-update measured snapshot). Same component-scoped O(n) batch.
        let post_update_coords = self.compute_component_raw_coords(component);

        // Align each section's absolute position to solved coordinates (rigid XY translation).
        // This prevents merged stations in different sections from drifting apart.
        // Only X and Y are compared/applied; Z (depth) is not adjusted.
        // Keyed order, so the offsets are applied in section order rather than in
        // hash order. Each entry targets a distinct section today, but pinning the
        // order keeps that from being load-bearing (issue #413, finding 21).
        let mut section_deltas: BTreeMap<SectionId, (f64, f64)> = BTreeMap::new();
        for section in self.sections.iter().filter(|s| component.contains(&s.id)) {
            let mut delta_xy: Option<(f64, f64)> = None;
            for idx in 0..=section.shots.len() {
                let station = StationRef::new(section.id, idx as u32);
                // Skip spur stations - their shots were not updated, so their
                // current positions don't match the solved positions.
                if spur_stations.contains(&station) {
                    continue;
                }
                let root = *station_root.get(&station).unwrap_or(&station);
                let target = group_coords
                    .get(&root)
                    .ok_or_else(|| "Missing solved coordinates".to_string())?;
                let current = post_update_coords
                    .get(&station)
                    .copied()
                    .ok_or_else(|| "Failed to compute station coordinates".to_string())?;
                let dx = target.x - current.x;
                let dy = target.y - current.y;
                if let Some((ex, ey)) = delta_xy {
                    if (ex - dx).abs() > 1e-6 || (ey - dy).abs() > 1e-6 {
                        return Err(
                            "Least squares adjustment produced inconsistent section translation"
                                .to_string(),
                        );
                    }
                } else {
                    delta_xy = Some((dx, dy));
                }
            }
            // For all-spur sections (no non-spur station set a delta), derive the
            // translation from the merge attachment point.  That station's group
            // is shared with a loop station, so its solved position is exact.
            if delta_xy.is_none() {
                for merge in &self.station_merges {
                    let local_key = if merge.station_a.section_id == section.id
                        && component.contains(&merge.station_b.section_id)
                    {
                        &merge.station_a
                    } else if merge.station_b.section_id == section.id
                        && component.contains(&merge.station_a.section_id)
                    {
                        &merge.station_b
                    } else {
                        continue;
                    };
                    let Some(local) = self.resolve_key(local_key) else {
                        continue;
                    };
                    let root = *station_root.get(&local).unwrap_or(&local);
                    if let (Some(target), Some(current)) = (
                        group_coords.get(&root),
                        post_update_coords.get(&local).copied(),
                    ) {
                        delta_xy = Some((target.x - current.x, target.y - current.y));
                        break;
                    }
                }
            }
            if let Some((dx, dy)) = delta_xy {
                if dx.abs() > 1e-9 || dy.abs() > 1e-9 {
                    section_deltas.insert(section.id, (dx, dy));
                }
            }
        }

        for (section_id, (dx, dy)) in section_deltas {
            self.adjust_section_offset(section_id, dx, dy);
        }

        self.debug_assert_merge_alignment("adjust_loop_closures");
        Ok(())
    }
}
