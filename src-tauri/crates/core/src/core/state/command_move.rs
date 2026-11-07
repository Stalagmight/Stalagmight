use super::AppState;
use crate::core::CommandResult;
use crate::models::{normalize_azimuth, Coordinates, SectionId, StationRef};

impl AppState {
    pub(super) fn move_section(&self, section_id: SectionId, offset: Coordinates) -> CommandResult {
        // "Adjust Position" on a section moves the WHOLE rigid group it belongs to
        // (its merge partners and aligned-closure-connected sections), not just this
        // one section. A merged section is rigidly bound to its partner: moving it
        // alone would shove its merged station off the partner's, and the
        // post-command geometry reconcile would then either snap it back (silently
        // discarding the move when this isn't the group's min-id anchor) or drag the
        // partner along. Moving the rigid group as one body keeps every merge aligned,
        // so the move sticks. For a standalone section the rigid group is just itself,
        // so this is identical to the previous single-section move. Delegating to
        // `move_segment` also upgrades the datum guard from "this section holds a
        // datum" to "the group holds a datum" (a partner's datum now blocks too).
        self.move_segment(section_id, offset)
    }

    pub(super) fn move_segment(&self, section_id: SectionId, offset: Coordinates) -> CommandResult {
        self.with_project_mut(|project| {
            if !offset.is_finite() {
                return CommandResult::failure(
                    "Move offset must be finite (no NaN/inf)".to_string(),
                );
            }
            // Validate section exists
            if !project.sections.iter().any(|s| s.id == section_id) {
                return CommandResult::failure("Section not found".to_string());
            }

            // Get all connected section IDs (merges + closures), filtered to existing sections
            let existing: std::collections::HashSet<SectionId> =
                project.sections.iter().map(|s| s.id).collect();
            let connected: Vec<SectionId> = project
                .get_rigid_connected_section_ids(section_id)
                .into_iter()
                .filter(|id| existing.contains(id))
                .collect();

            // Guard the set actually being offset, not just this section's merge
            // group. `connected` follows merges AND aligned closures, while
            // `rigid_group_has_datum` walks merges only, so in principle a datum
            // reachable over a closure-only edge could slip through.
            //
            // HARDENING, not a live bug fix: that state is currently unreachable.
            // `convert_orphan_closures_to_merges` runs after every command and
            // promotes any sole-bridge closure to a merge, skipping only the case
            // where both endpoint groups are already zero-datum-anchored - and in
            // THAT case the dragged section's own merge group holds a datum, so the
            // narrow check catches it too. The point of widening it is to stop this
            // guard depending on an invariant maintained in another module.
            //
            // Resolved once against `connected` rather than per section: calling
            // rigid_group_has_datum for each member rebuilds a whole merge component
            // and rescans the datum list every time, which is quadratic on a large
            // merge-connected survey. Equivalent, because a merge edge is also a
            // rigid edge, so every member's merge component is a subset of
            // `connected`.
            let connected_set: std::collections::HashSet<SectionId> =
                connected.iter().copied().collect();
            let group_has_datum = project
                .zero_datums
                .iter()
                .filter_map(|zd| project.resolve_key(&zd.station_ref))
                .any(|sr| connected_set.contains(&sr.section_id));
            if (offset.x.abs() > 1e-9 || offset.y.abs() > 1e-9 || offset.z.abs() > 1e-9)
                && group_has_datum
            {
                return CommandResult::failure(
                    "Cannot move a segment whose rigid group holds a zero datum".to_string(),
                );
            }

            // Apply offset to every section in the connected set
            // (uses the pre-filtered connected set, not adjust_rigid_group_offsets,
            // because move_segment follows rigid + aligned-closure connectivity)
            for sid in &connected {
                project.adjust_section_offset(*sid, offset.x, offset.y);
            }
            project.touch();
            tracing::info!(
                ?section_id,
                ?offset,
                section_count = connected.len(),
                purpose = "move connected segment as rigid group",
                "Segment moved"
            );
            CommandResult::success()
        })
    }

    pub(super) fn move_station(
        &self,
        station_ref: StationRef,
        offset: Coordinates,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            // Reject a non-finite offset: move_station recomputes and writes raw
            // shot geometry (length/azimuth/depth) from it, and the only numeric
            // guard below is `new_len <= 0.0`, which is false for NaN and inf.
            if !offset.x.is_finite() || !offset.y.is_finite() || !offset.z.is_finite() {
                return CommandResult::failure(
                    "Move offset has a non-finite (NaN/inf) value".to_string(),
                );
            }
            if project
                .zero_datums
                .iter()
                .any(|zd| project.key_is_ref(&zd.station_ref, station_ref))
                && (offset.x.abs() > 1e-9 || offset.y.abs() > 1e-9 || offset.z.abs() > 1e-9)
            {
                return CommandResult::failure("Cannot move a zero datum station".to_string());
            }

            // Incorporate any existing station offset
            let existing = project
                .key_for_ref(station_ref)
                .and_then(|key| project.station_offsets.get(&key).copied())
                .unwrap_or((0.0, 0.0, 0.0));
            let total_dx = existing.0 + offset.x;
            let total_dy = existing.1 + offset.y;
            let total_dz = existing.2 + offset.z;

            // Resolved before the `&mut` borrow below: the geometry here is in
            // world space, so every read adds this and every write-back removes
            // it again (#438).
            let azimuth_reference = project.azimuth_reference_for_id(station_ref.section_id);

            // Find the section containing this station (mutable)
            let Some(section) = project
                .sections
                .iter_mut()
                .find(|s| s.id == station_ref.section_id)
            else {
                return CommandResult::failure("Section not found".to_string());
            };

            let idx = station_ref.station_index as usize;

            // Validate station index (N shots → N+1 stations, indices 0..=N)
            if idx > section.shots.len() {
                return CommandResult::failure("Invalid station index".to_string());
            }

            // Pre-compute updates to avoid partial mutations on failure.
            let mut prev_update: Option<(f64, f64, f64, f64)> = None; // len, azi, depth_out, pitch
            if idx > 0 {
                let shot = &section.shots[idx - 1];
                let azi_rad = shot.average_azimuth_true_rad(azimuth_reference);
                let old_horizontal = shot.horizontal_length();
                let old_dx = old_horizontal * azi_rad.sin();
                let old_dy = old_horizontal * azi_rad.cos();
                let new_dx = old_dx + total_dx;
                let new_dy = old_dy + total_dy;
                let new_horizontal = (new_dx * new_dx + new_dy * new_dy).sqrt();
                let new_depth_out = shot.depth_out + total_dz;
                let new_dz = new_depth_out - shot.depth_in;
                let new_len = (new_horizontal * new_horizontal + new_dz * new_dz).sqrt();
                if new_len <= 0.0 || !new_len.is_finite() {
                    return CommandResult::failure(
                        "Move would create zero-length shot".to_string(),
                    );
                }
                let new_azi = if new_horizontal > 1e-9 {
                    // Solved in world space; stored as a measured reading (#438).
                    azimuth_reference
                        .to_measured(normalize_azimuth(new_dx.atan2(new_dy).to_degrees()))
                } else {
                    shot.average_azimuth_measured()
                };
                let pitch = new_dz.atan2(new_horizontal).to_degrees();
                prev_update = Some((new_len, new_azi, new_depth_out, pitch));
            }

            let mut next_update: Option<(f64, f64, f64, f64)> = None; // len, azi, depth_in, pitch
            if let Some(shot) = section.shots.get(idx) {
                let azi_rad = shot.average_azimuth_true_rad(azimuth_reference);
                let old_horizontal = shot.horizontal_length();
                let old_dx = old_horizontal * azi_rad.sin();
                let old_dy = old_horizontal * azi_rad.cos();
                let new_dx = old_dx - total_dx;
                let new_dy = old_dy - total_dy;
                let new_horizontal = (new_dx * new_dx + new_dy * new_dy).sqrt();
                let new_depth_in = shot.depth_in + total_dz;
                let new_dz = shot.depth_out - new_depth_in;
                let new_len = (new_horizontal * new_horizontal + new_dz * new_dz).sqrt();
                if new_len <= 0.0 || !new_len.is_finite() {
                    return CommandResult::failure(
                        "Move would create zero-length shot".to_string(),
                    );
                }
                let new_azi = if new_horizontal > 1e-9 {
                    // Solved in world space; stored as a measured reading (#438).
                    azimuth_reference
                        .to_measured(normalize_azimuth(new_dx.atan2(new_dy).to_degrees()))
                } else {
                    shot.average_azimuth_measured()
                };
                let pitch = new_dz.atan2(new_horizontal).to_degrees();
                next_update = Some((new_len, new_azi, new_depth_in, pitch));
            }

            // Apply updates after validation
            if let Some((new_len, new_azi, new_depth_out, pitch)) = prev_update {
                if let Some(shot) = section.shots.get_mut(idx - 1) {
                    shot.length = new_len;
                    shot.azimuth_in = new_azi;
                    shot.azimuth_out = new_azi;
                    shot.depth_out = new_depth_out;
                    shot.pitch_in = Some(pitch);
                    shot.pitch_out = Some(pitch);
                }
            }

            if let Some((new_len, new_azi, new_depth_in, pitch)) = next_update {
                if let Some(shot) = section.shots.get_mut(idx) {
                    shot.length = new_len;
                    shot.azimuth_in = new_azi;
                    shot.azimuth_out = new_azi;
                    shot.depth_in = new_depth_in;
                    shot.pitch_in = Some(pitch);
                    shot.pitch_out = Some(pitch);
                }
            }

            // Special case: moving station 0 moves the section offset so the rest
            // of the geometry stays in place (shot 0 was adjusted above to
            // compensate). Applied once the `section` borrow has ended, through
            // `adjust_section_offset` - i.e. `write_section_offset`, which drops
            // the key when the move lands the offset back within dust of the
            // origin instead of leaving an inert entry behind (#413 finding 2).
            // Independent of the shot edits above, so the order is immaterial.
            if idx == 0 && (total_dx != 0.0 || total_dy != 0.0) {
                project.adjust_section_offset(station_ref.section_id, total_dx, total_dy);
            }

            // Clear station offset (now incorporated into shot data)
            if let Some(key) = project.key_for_ref(station_ref) {
                project.station_offsets.remove(&key);
            }
            project.touch();

            tracing::info!(
                ?station_ref,
                ?offset,
                purpose = "adjust station position",
                "Station moved (shot data updated)"
            );
            CommandResult::success()
        })
    }
}
