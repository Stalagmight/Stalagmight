use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    Project, SectionData, SectionId, ShotData, ShotId, StationKey, StationRef, FEATURE_ID_STRIDE,
    MAX_SECTIONS_IN_ID_SPACE, MAX_SECTION_ID,
};

/// Every split path (both delete-and-split paths and Split Section Here) turns
/// one section into two, so each MINTS a section id (#555) through the same
/// `Project::add_section` door `create_section` guards, while going nowhere
/// near `create_section` itself. Refused before any path mutates anything: a
/// split that ran out of ids half-way would leave section A truncated with
/// nowhere to put its tail.
pub(super) fn section_budget_refusal(
    project: &Project,
    section_id: SectionId,
) -> Option<CommandResult> {
    if project.next_section_id <= MAX_SECTION_ID {
        return None;
    }
    Some(CommandResult::failure(format!(
        "Cannot split section {section_id}: to split it into two sections needs \
         a new section id, but the project has reached the maximum number of sections \
         (limit = {MAX_SECTIONS_IN_ID_SPACE}); shot ids encode as \
         section_id * {FEATURE_ID_STRIDE} + position in 32 bits, so section ids \
         past {MAX_SECTION_ID} cannot hold shots"
    )))
}

impl AppState {
    /// Execute a delete-shot-and-split-section operation as a single atomic undo/redo unit.
    /// Deletes a mid-section shot and splits the section into two, with user-provided names.
    /// Uses `ProjectSnapshotPair` for single-undo. Follows `execute_create_and_merge()` pattern.
    pub fn execute_delete_shot_and_split(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
        name_a: String,
        name_b: String,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete shot and split section") {
            return f;
        }
        tracing::info!(
            section_id,
            shot_id,
            purpose = "split section after deleting shot",
            "Executing delete_shot_and_split as single undo unit"
        );

        // Step 1: Take BEFORE snapshot
        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        // The frame-defining datum station before the delete: this path bypasses
        // `execute()`, so it carries its own frame check (#469, item 1). Deleting
        // the shot the primary datum is anchored to drops that datum and promotes
        // the next one, which re-defines what every stored coordinate means.
        let frame_before = self.frame_datum_key();

        let undo_section_b_name = name_b.clone();

        // Step 2: Get split data and perform the operation
        let new_section_id = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };

            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };

            let Some(section) = project.get_section(section_id) else {
                return CommandResult::failure(format!("Section {section_id} not found"));
            };

            // Pre-check: distinguish "shot not found" from "edge shot"
            let shot_pos = section.shots.iter().position(|s| s.id == shot_id);
            match shot_pos {
                None => {
                    return CommandResult::failure(format!(
                        "Shot {shot_id} not found in section {section_id}"
                    ))
                }
                Some(p) if p == 0 || p == section.shots.len() - 1 => {
                    return CommandResult::failure(
                        "Cannot split at edge shot (first or last). Use simple delete instead."
                            .to_string(),
                    );
                }
                _ => {}
            }

            // Frozen-geometry guard (mirrors delete_shot and the range-split
            // path): this path deletes `shot_id`, so refuse if it is part of an
            // adjusted survey network -- otherwise a frozen mid-section shot can
            // be deleted through the split path, silently breaking the LSQ
            // solution's frozen geometry. (#241)
            if project.is_shot_frozen_by_id(section_id, shot_id) {
                return CommandResult::failure(
                    "Cannot delete a frozen shot. This shot is part of an adjusted survey network."
                        .to_string(),
                );
            }

            // Section-id budget (#555), with the other pre-checks: the split
            // mints section B below, and the truncation of section A that
            // precedes it has already destroyed the shots B is supposed to
            // carry.
            if let Some(refusal) = section_budget_refusal(project, section_id) {
                return refusal;
            }

            // Get split data (safe after pre-checks above)
            let Some((pos, shots_before, shots_after)) = section.split_shots_at(shot_id) else {
                return CommandResult::failure(format!("Failed to split at shot {shot_id}"));
            };

            // Capture deleted shot data for offset calculation
            let deleted_shot_data = section.shots[pos].to_shot_data();

            // Capture metadata from original section (color, direction -- NOT
            // comment). The survey credit needs no copy: it rides on the shots.
            let color = section.color.clone();
            let direction = section.direction;
            let quality = section.quality;
            // Section B is the same survey, so it keeps the same north reference
            // (#438) - `SectionData` cannot carry it, so it is copied explicitly
            // below, exactly like the metadata above.
            let azimuth_reference = section.azimuth_reference;
            let resolved_reference = project.azimuth_reference_for_id(section_id);

            // Stable-identity remap snapshot: resolve every stored cross-reference
            // to its CURRENT positional ref BEFORE we mutate the section.
            //
            // Shot UUIDs now survive the split (#423 - section B is rebuilt from
            // `to_shot_data`, which carries them), but the remap is still
            // required: a `StationKey` is `(section_id, anchor)`, so an anchor
            // cannot follow a shot ACROSS a section boundary. A ref into the B
            // half would keep pointing at section A, where its shot no longer
            // exists, and dangle. Positional-then-reanchor rewrites the
            // section_id; carrying the uuid is what makes the reanchored key
            // come back identical for the A half and stable for the B half.
            let split_station = (pos + 1) as u32;
            let original_station_count = section.shots.len() as u32 + 1;
            let old_refs: Vec<Option<StationRef>> = {
                let p: &Project = project;
                let mut out = Vec::new();
                p.for_each_station_ref(|_label, key| out.push(p.resolve_key(&key)));
                out
            };

            // Step 3: Modify original section -> section A
            // Truncate in-place to preserve existing Shot IDs and next_shot_id
            let Some(section_mut) = project.get_section_mut(section_id) else {
                return CommandResult::failure(format!("Section {section_id} not found"));
            };
            section_mut.name = Some(name_a);
            section_mut.shots.truncate(pos); // keep shots[0..pos], drop deleted + after

            // Step 4: Calculate offset for section B
            // Walk shots_before + deleted shot to accumulate (bx, by)
            let mut bx: f64 = 0.0;
            let mut by: f64 = 0.0;
            for shot_data in &shots_before {
                let temp_shot = crate::models::Shot::new(0, shot_data.clone());
                let azimuth_rad = temp_shot.average_azimuth_true_rad(resolved_reference);
                let horizontal = temp_shot.horizontal_length();
                bx += horizontal * azimuth_rad.sin();
                by += horizontal * azimuth_rad.cos();
            }
            // Add deleted shot
            {
                let deleted_shot = crate::models::Shot::new(0, deleted_shot_data);
                let azimuth_rad = deleted_shot.average_azimuth_true_rad(resolved_reference);
                let horizontal = deleted_shot.horizontal_length();
                bx += horizontal * azimuth_rad.sin();
                by += horizontal * azimuth_rad.cos();
            }

            // Get existing section offset to add to computed B offset
            let (section_ox, section_oy) = project.section_offset(section_id);

            // Capture counts before moving data
            let shots_after_len = shots_after.len();

            // Step 5: Create section B with inherited metadata
            let section_b_data = SectionData {
                name: Some(name_b),
                surveyor: None,
                date: None,
                color,
                direction: Some(direction),
                quality,
                shots: shots_after,
            };

            let section_b = project.add_section(section_b_data);
            let new_id = section_b.id;
            if let Some(section_b) = project.get_section_mut(new_id) {
                section_b.azimuth_reference = azimuth_reference;
            }

            // Set section B offset
            project.set_section_offset(new_id, (section_ox + bx, section_oy + by));

            // Step 6: Remap every cross-reference across the split.
            // Stations whose positional index moved into section B (index in
            // [split_station, original_station_count)) become section B refs with
            // index -= split_station; everything else stays put. We re-derive each
            // ref's stable key from its NEW positional home against the now-final
            // structure - which, since B's shots kept their UUIDs, differs from the
            // old key only in the section id. `old_refs` was
            // captured before truncation, in `for_each_station_ref` order; the
            // `new_keys` Vec is consumed by `map_station_refs` in that same order.
            let _ = shots_after_len;
            let new_keys: Vec<Option<StationKey>> = old_refs
                .iter()
                .map(|old_ref| {
                    let sr = (*old_ref)?;
                    let new_ref = if sr.section_id == section_id
                        && sr.station_index >= split_station
                        && sr.station_index < original_station_count
                    {
                        StationRef::new(new_id, sr.station_index - split_station)
                    } else {
                        sr
                    };
                    project.key_for_ref(new_ref)
                })
                .collect();
            let mut remap_i = 0usize;
            project.map_station_refs(|old_key| {
                let next = new_keys.get(remap_i).cloned().flatten();
                remap_i += 1;
                next.unwrap_or(old_key)
            });
            // `new_keys` was built 1:1 from `old_refs` (a for_each_station_ref pass)
            // and is consumed positionally here; the alignment is only valid if
            // map_station_refs visits the same refs in the same order. Assert it so
            // a future divergence fails loudly in debug rather than silently
            // re-anchoring a cross-reference onto the wrong station.
            debug_assert_eq!(
                remap_i,
                new_keys.len(),
                "split-section remap: map_station_refs visited a different number of \
                 refs than were captured in old_refs - traversal order diverged"
            );

            // The split can strand a section so its only attachment is a loop
            // closure - that is a merge, not a loop. Convert it now (issue #8)
            // so it's fixed immediately, not only on the next reload.
            project.convert_orphan_closures_to_merges();

            project.touch();
            new_id
        };

        // This custom path bypasses execute()'s reconcile; re-derive offsets here
        // (after the orphan-closure conversion above) and before the after-snapshot
        // so undo/redo and the saved file reflect the normalized topology. The
        // frame check runs first, in the same order as execute()'s epilogue.
        self.renormalize_frame_if_changed(frame_before);
        self.reconcile_geometry();

        // Step 7: Take AFTER snapshot
        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Step 8: Push single undo entry with ProjectSnapshotPair
        // Use a Batch with both DeleteShot + CreateSection to accurately describe the compound op
        let batch_cmd = Command::Batch {
            commands: vec![
                Command::DeleteShot {
                    section_id,
                    shot_id,
                },
                Command::CreateSection {
                    name: Some(undo_section_b_name),
                    surveyor: None,
                    date: None,
                    color: String::new(),
                    direction: None,
                    shots: Vec::new(),
                },
            ],
        };

        self.push_snapshot_undo(batch_cmd, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): a delete/split mutated a remote project.
        // Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        tracing::info!(
            section_id,
            shot_id,
            new_section_id,
            purpose = "split section after deleting shot",
            "Delete shot and split section completed"
        );

        CommandResult::success().with_section_id(new_section_id)
    }

    /// Delete a contiguous *interior* range of shots and split the section into
    /// two: section A keeps the shots before the range (in place), section B is a
    /// new section holding the shots after the range (offset so it keeps its
    /// absolute position). Generalizes [`Self::execute_delete_shot_and_split`]
    /// from a single shot to `[start_pos..=end_pos]`. Commits as ONE
    /// `ProjectSnapshotPair` undo entry. The range must be interior (shots remain
    /// before AND after); prefix/suffix/whole ranges use the shift or
    /// section-delete paths.
    pub fn execute_delete_shot_range_split(
        &self,
        section_id: SectionId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
        name_a: String,
        name_b: String,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete shot range and split section") {
            return f;
        }
        tracing::info!(
            section_id,
            start_shot_id,
            end_shot_id,
            purpose = "split section after deleting shot range",
            "Executing delete_shot_range_split as single undo unit"
        );

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // Same frame check as the single-shot split above (#469, item 1).
        let frame_before = self.frame_datum_key();
        let undo_section_b_name = name_b.clone();

        let new_section_id = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            let Some(section) = project.get_section(section_id) else {
                return CommandResult::failure(format!("Section {section_id} not found"));
            };

            let n = section.shots.len();
            let Some(mut start_pos) = section.shots.iter().position(|s| s.id == start_shot_id)
            else {
                return CommandResult::failure(format!(
                    "Shot {start_shot_id} not found in section {section_id}"
                ));
            };
            let Some(mut end_pos) = section.shots.iter().position(|s| s.id == end_shot_id) else {
                return CommandResult::failure(format!(
                    "Shot {end_shot_id} not found in section {section_id}"
                ));
            };
            if start_pos > end_pos {
                std::mem::swap(&mut start_pos, &mut end_pos);
            }
            // Split requires shots both before and after the range.
            if start_pos == 0 || end_pos >= n - 1 {
                return CommandResult::failure(
                    "Range is not interior to the section (split needs shots before and after). \
                     Use shift or delete the section instead."
                        .to_string(),
                );
            }

            // Capture owned data; afterwards the `section` borrow ends so we can mutate.
            let shots_before: Vec<ShotData> = section.shots[..start_pos]
                .iter()
                .map(|s| s.to_shot_data())
                .collect();
            let shots_after: Vec<ShotData> = section.shots[end_pos + 1..]
                .iter()
                .map(|s| s.to_shot_data())
                .collect();
            let deleted_shots_data: Vec<ShotData> = section.shots[start_pos..=end_pos]
                .iter()
                .map(|s| s.to_shot_data())
                .collect();
            let range_shot_ids: Vec<ShotId> = section.shots[start_pos..=end_pos]
                .iter()
                .map(|s| s.id)
                .collect();
            // The survey credit rides on the shots; nothing to copy.
            let color = section.color.clone();
            let direction = section.direction;
            let quality = section.quality;
            // Same survey, same north reference (#438); see the single-shot path.
            let azimuth_reference = section.azimuth_reference;

            // Frozen-geometry guard (mirrors delete_shot).
            for &sid in &range_shot_ids {
                if project.is_shot_frozen_by_id(section_id, sid) {
                    return CommandResult::failure(
                        "Cannot delete a frozen shot. This range includes shots that are part of \
                         an adjusted survey network."
                            .to_string(),
                    );
                }
            }

            // Section-id budget (#555), before the connectivity rewrites below:
            // this path mutates the project (preserve/cleanup) well before it
            // mints section B, so a refusal has to happen here to stay free.
            if let Some(refusal) = section_budget_refusal(project, section_id) {
                return refusal;
            }

            // Interior stations dropped by the split (strictly between the kept
            // boundary stations `start_pos` (→A) and `end_pos+1` (→B:0)).
            let interior: std::collections::HashSet<StationRef> = ((start_pos as u32 + 1)
                ..=(end_pos as u32))
                .map(|i| StationRef::new(section_id, i))
                .collect();

            // Preserve connectivity across the dropped stations, then drop their
            // own refs - BEFORE we capture old_refs, so the stored-ref set is
            // final and the positional remap below stays 1:1 aligned. Both run
            // while the section is structurally intact (indices still resolve).
            project.preserve_connections_for_removed_stations(&interior);
            project.cleanup_removed_station_references(&interior);

            // Stable-identity remap snapshot (see execute_delete_shot_and_split):
            // resolve every surviving cross-ref to its current positional ref.
            let split_station = (end_pos + 1) as u32;
            let original_station_count = n as u32 + 1;
            let old_refs: Vec<Option<StationRef>> = {
                let p: &Project = project;
                let mut out = Vec::new();
                p.for_each_station_ref(|_label, key| out.push(p.resolve_key(&key)));
                out
            };

            // Section A = before (truncate in place to preserve shot IDs).
            let Some(section_mut) = project.get_section_mut(section_id) else {
                // preserve/cleanup above already mutated the project; roll back
                // to the before-snapshot so a failure here leaves no half-applied
                // state (mirrors the shift path's rollback on remove failure).
                let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                return CommandResult::failure(format!("Section {section_id} not found"));
            };
            section_mut.name = Some(name_a);
            section_mut.shots.truncate(start_pos);

            // Section B offset = section offset + displacement of (before ++ deleted).
            let mut bx: f64 = 0.0;
            let mut by: f64 = 0.0;
            let resolved_reference = project.azimuth_reference_for_id(section_id);
            for shot_data in shots_before.iter().chain(deleted_shots_data.iter()) {
                let temp_shot = crate::models::Shot::new(0, shot_data.clone());
                let azimuth_rad = temp_shot.average_azimuth_true_rad(resolved_reference);
                let horizontal = temp_shot.horizontal_length();
                bx += horizontal * azimuth_rad.sin();
                by += horizontal * azimuth_rad.cos();
            }
            let (section_ox, section_oy) = project.section_offset(section_id);

            let section_b_data = SectionData {
                name: Some(name_b),
                surveyor: None,
                date: None,
                color,
                direction: Some(direction),
                quality,
                shots: shots_after,
            };
            let section_b = project.add_section(section_b_data);
            let new_id = section_b.id;
            if let Some(section_b) = project.get_section_mut(new_id) {
                section_b.azimuth_reference = azimuth_reference;
            }
            project.set_section_offset(new_id, (section_ox + bx, section_oy + by));

            // Remap cross-refs across the split: stations at index >= split_station
            // (= end_pos+1) move to B with index -= split_station; everything else
            // (index <= start_pos) stays on A. Interior refs were already dropped,
            // so no surviving ref resolves into the gap.
            let new_keys: Vec<Option<StationKey>> = old_refs
                .iter()
                .map(|old_ref| {
                    let sr = (*old_ref)?;
                    let new_ref = if sr.section_id == section_id
                        && sr.station_index >= split_station
                        && sr.station_index < original_station_count
                    {
                        StationRef::new(new_id, sr.station_index - split_station)
                    } else {
                        sr
                    };
                    project.key_for_ref(new_ref)
                })
                .collect();
            let mut remap_i = 0usize;
            project.map_station_refs(|old_key| {
                let next = new_keys.get(remap_i).cloned().flatten();
                remap_i += 1;
                next.unwrap_or(old_key)
            });
            debug_assert_eq!(
                remap_i,
                new_keys.len(),
                "split-range remap: map_station_refs visited a different number of refs \
                 than were captured in old_refs - traversal order diverged"
            );

            project.convert_orphan_closures_to_merges();
            project.touch();
            new_id
        };

        // Custom path bypasses execute()'s reconcile; re-derive offsets here.
        self.renormalize_frame_if_changed(frame_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        let batch_cmd = Command::Batch {
            commands: vec![
                Command::DeleteShot {
                    section_id,
                    shot_id: start_shot_id,
                },
                Command::CreateSection {
                    name: Some(undo_section_b_name),
                    surveyor: None,
                    date: None,
                    color: String::new(),
                    direction: None,
                    shots: Vec::new(),
                },
            ],
        };
        self.push_snapshot_undo(batch_cmd, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): a delete/split mutated a remote project.
        // Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        tracing::info!(
            section_id,
            new_section_id,
            purpose = "split section after deleting shot range",
            "Delete shot range and split section completed"
        );
        CommandResult::success().with_section_id(new_section_id)
    }

    /// Delete a contiguous range of shots by gap-closing (shifting) - the
    /// survivors rejoin and the section stays single. ATOMIC: the range's shots
    /// go in forward order inside ONE project lock and ONE `ProjectSnapshotPair`
    /// undo unit, with the geometry cascade DEFERRED to a single reconcile at the
    /// end (each step is a valid single-shot gap-close: head/before stays for
    /// interior & suffix ranges, tail/after stays for prefix ranges). Refuses
    /// when the range is shift-blocked or would empty the section (use split /
    /// section delete respectively).
    ///
    /// # Not a composition of `n` `Command::DeleteShot`s, on purpose
    ///
    /// This comment used to say the method "generalizes" the single-shot
    /// `delete_shot` by deleting the range's shots in forward order, which reads
    /// as a promise that the range is the same edit as the `n` singles. It is
    /// not, and the promise was the defect (issue #413 finding 22, resolved as
    /// intended behaviour). What holds:
    ///
    /// * for a range that removes NO station a merge is anchored to, the two are
    ///   identical - measured over prefix and interior ranges, they agree to the
    ///   last bit;
    /// * for a range whose removed stations DO include one a merged partner
    ///   section is anchored to (the doomed-tail-anchor shape), they deliberately
    ///   DIVERGE. `AppState::execute` runs the geometry epilogue after every
    ///   command, so the single-delete sequence re-anchors the partner while the
    ///   merge is still live, drags it along as the section shrinks, and then
    ///   deletes the merge - leaving the partner displaced with no edge to pull
    ///   it back. Deferring to one final cascade leaves it where the surveyor put
    ///   it, which is what this method does.
    ///
    /// The atomic behaviour is the reviewed intent, pinned by
    /// `test_delete_range_shift_does_not_displace_free_floating_merged_section`
    /// and `test_delete_range_suffix_shift_keeps_head_with_doomed_tail_anchor`
    /// (PR #95, `core/state/tests.rs`), and asserted on random topology by the
    /// metamorphic pair `range-shift-is-n-single-deletes`
    /// (`core/state/metamorphic_undo_tests.rs`), which requires equality on every
    /// other shape.
    pub fn execute_delete_shot_range_shift(
        &self,
        section_id: SectionId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete shot range and shift") {
            return f;
        }
        tracing::info!(
            section_id,
            start_shot_id,
            end_shot_id,
            purpose = "gap-close a deleted shot range",
            "Executing delete_shot_range_shift as single undo unit"
        );

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Same frame check as the split paths above (#469, item 1).
        let frame_before = self.frame_datum_key();

        let result_section = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            let Some(section) = project.get_section(section_id) else {
                return CommandResult::failure(format!("Section {section_id} not found"));
            };

            let n = section.shots.len();
            let Some(mut start_pos) = section.shots.iter().position(|s| s.id == start_shot_id)
            else {
                return CommandResult::failure(format!(
                    "Shot {start_shot_id} not found in section {section_id}"
                ));
            };
            let Some(mut end_pos) = section.shots.iter().position(|s| s.id == end_shot_id) else {
                return CommandResult::failure(format!(
                    "Shot {end_shot_id} not found in section {section_id}"
                ));
            };
            if start_pos > end_pos {
                std::mem::swap(&mut start_pos, &mut end_pos);
            }
            // Deleting the whole section this way would leave it empty - route
            // that through delete_section instead.
            if start_pos == 0 && end_pos == n - 1 {
                return CommandResult::failure(
                    "Cannot shift-delete an entire section; delete the section instead."
                        .to_string(),
                );
            }

            // Delete in forward (ascending position) order by stable shot id.
            let range_shot_ids: Vec<ShotId> = section.shots[start_pos..=end_pos]
                .iter()
                .map(|s| s.id)
                .collect();

            // Frozen-geometry + shift-blocked prechecks (no mutation yet).
            for &sid in &range_shot_ids {
                if project.is_shot_frozen_by_id(section_id, sid) {
                    return CommandResult::failure(
                        "Cannot delete a frozen shot. This range includes shots that are part of \
                         an adjusted survey network."
                            .to_string(),
                    );
                }
            }
            if let Ok(impact) =
                project.preview_delete_shot_range(section_id, start_shot_id, end_shot_id)
            {
                if impact.shift_blocked {
                    let reasons = if impact.shift_blocked_reasons.is_empty() {
                        "Shift geometry is not allowed for this range.".to_string()
                    } else {
                        impact.shift_blocked_reasons.join(" ")
                    };
                    return CommandResult::failure(format!("{reasons} Use split instead."));
                }
            }

            // Stations the WHOLE range will remove, captured up front as
            // canonical (position-independent) keys so renumbering during the
            // per-shot deletes can't invalidate them. Each per-shot gap-close
            // ignores anchors on these when deciding which side keeps its
            // position - otherwise an anchor on a later, also-doomed range shot
            // would displace the survivors (e.g. a suffix shift would keep the
            // doomed tail fixed and shift the head). Whole-section was refused
            // above, so start_pos==0 here means prefix.
            let doomed_indices: Vec<u32> = if start_pos == 0 {
                (0..=(end_pos as u32)).collect() // prefix
            } else if end_pos == n - 1 {
                ((start_pos as u32 + 1)..=(n as u32)).collect() // suffix
            } else {
                ((start_pos as u32 + 1)..=(end_pos as u32 + 1)).collect() // interior
            };
            let doomed_anchors: std::collections::HashSet<_> = doomed_indices
                .iter()
                .filter_map(|&i| project.key_for_ref(StationRef::new(section_id, i)))
                .collect();

            for &sid in &range_shot_ids {
                if let Err(e) = project.remove_shot_with_rewire_excluding_anchors(
                    section_id,
                    sid,
                    &doomed_anchors,
                    /* defer_cascade */ true,
                ) {
                    // Already mutated - roll back to the before snapshot.
                    let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                    return CommandResult::failure(e);
                }
            }
            // Single cascade now that every range shot (and any doomed merge) is
            // gone, so a doomed station's intermediate shift never propagates into
            // a connected free-floating section - the per-shot cascades were
            // deferred for exactly this reason. Assert alignment here, since each
            // per-shot delete skipped its own alignment check.
            project.cascade_connection_offsets(&[section_id]);
            // That cascade is seeded with the edited section plus the datum
            // sections, which is the right frame for THIS edit and is not a set
            // that reaches every merge the assert below covers: a range can cut a
            // whole component loose, and the BFS never enters one with no
            // authoritative section in it. `realign_merges` closes that hole with
            // one anchor per connected component - and it is not an extra state
            // change, because the epilogue's `reconcile_geometry` (just below this
            // scope, before the `after` snapshot is taken) runs the very same heal
            // a moment later, under the very same condition. All it moves is WHEN,
            // so the assert is backed by a solve that can see what it asserts on
            // (#464 defect B). It early-returns when nothing is beyond the
            // invariant, which is the ordinary case, and it still cannot invent an
            // alignment that is genuinely unreachable - the assert keeps its teeth.
            //
            // The `cascade_deferred` gate is what makes "the very same heal" true.
            // `reconcile_geometry` returns early during a deferred batch, and so
            // does `debug_assert_merge_alignment` - intermediate states inside one
            // are misaligned BY DESIGN. `realign_merges` has no such guard and
            // clears the flag internally (`resolve_offsets_tracked`), so calling it
            // unconditionally would resolve a batch's geometry mid-batch, ahead of
            // an assert that is not even looking. No caller is known to reach this
            // path inside a deferred window; the gate makes that irrelevant.
            if !project.cascade_deferred {
                project.realign_merges();
            }
            project.debug_assert_merge_alignment("delete_shot_range_shift");

            // Closures-are-loops invariant (issue #413 finding 10): a loop closure
            // exists only for a TRUE loop, so every delete entry point converts the
            // orphans it creates - memory and disk then agree and the loader's
            // unconditional pass (`redrive_offsets_after_load`) is a no-op safety
            // net rather than a silent reclassification on reopen. This path needs
            // no call of its own: `remove_shot_with_rewire_excluding_anchors` runs
            // `convert_orphan_closures_to_merges()` for EVERY shot it removes
            // (`models/project/shots.rs`, "the deletion may have removed the OTHER
            // path of a loop"), including the last one of the range, so the
            // invariant already holds when this scope ends.
            //
            // Undo restores from the `ProjectSnapshotPair`, which legitimately puts
            // the PRE-conversion state back: the closure was a true loop before the
            // delete, so restoring it as a closure is the correct inverse.
            section_id
        };

        self.renormalize_frame_if_changed(frame_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        let batch_cmd = Command::Batch {
            commands: vec![
                Command::DeleteShot {
                    section_id,
                    shot_id: start_shot_id,
                },
                Command::DeleteShot {
                    section_id,
                    shot_id: end_shot_id,
                },
            ],
        };
        self.push_snapshot_undo(batch_cmd, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): a delete/split mutated a remote project.
        // Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        tracing::info!(
            section_id,
            purpose = "gap-close a deleted shot range",
            "Delete shot range (shift) completed"
        );
        CommandResult::success().with_section_id(result_section)
    }

    /// Execute a delete-last-shot-and-section operation as a single atomic undo/redo unit.
    /// When a section has exactly 1 shot remaining, this removes the section (and its
    /// only shot) along with all section references (merges, closures, section_offsets,
    /// station_offsets, zero_datums). Uses `ProjectSnapshotPair` for single-undo.
    pub fn execute_delete_last_shot_and_section(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete last shot and section") {
            return f;
        }
        tracing::info!(
            section_id,
            shot_id,
            purpose = "remove single-shot section",
            "Executing delete_last_shot_and_section as single undo unit"
        );

        // Step 1: Take BEFORE snapshot
        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Same frame check as the split paths above (#469, item 1). This path
        // removes a whole SECTION, so it can drop the primary datum outright.
        let frame_before = self.frame_datum_key();

        // Step 2: Validate and mutate in a single lock scope. On any failure after
        // mutation we roll back from before_snapshot. `command_lock` is held for the
        // whole call, so no other command can observe or interleave with the window
        // between this scope and the after-snapshot below.
        {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };

            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };

            // Validate section exists
            let Some(section) = project.get_section(section_id) else {
                return CommandResult::failure(format!("Section {section_id} not found"));
            };

            // Verify section has exactly 1 shot
            if section.shots.len() != 1 {
                return CommandResult::failure(format!(
                    "Section {} has {} shots, expected exactly 1",
                    section_id,
                    section.shots.len()
                ));
            }

            // Verify the shot matches
            if section.shots[0].id != shot_id {
                return CommandResult::failure(format!(
                    "Shot {} not found in section {} (section has shot {})",
                    shot_id, section_id, section.shots[0].id
                ));
            }

            // Step 3: Preserve merge/closure connectivity, then clean up section references
            project.preserve_connections_for_section_deletion(section_id);

            project.station_merges.retain(|m| {
                m.station_a.section_id != section_id && m.station_b.section_id != section_id
            });

            project.loop_closures.retain(|lc| {
                lc.station_a.section_id != section_id && lc.station_b.section_id != section_id
            });

            // Remove section offsets
            project.remove_section_offset(section_id);

            // Remove station offsets referencing this section
            project
                .station_offsets
                .retain(|key, _| key.section_id != section_id);

            // Remove station comments and names referencing this section
            project
                .station_comments
                .retain(|key, _| key.section_id != section_id);
            project
                .station_names
                .retain(|key, _| key.section_id != section_id);

            // Remove zero datums referencing this section
            project
                .zero_datums
                .retain(|zd| zd.station_ref.section_id != section_id);

            // Step 4: Remove the section (and its only shot)
            if project.remove_section(section_id).is_none() {
                // Section vanished between validation and removal (should not happen).
                // Rollback to before_snapshot to undo reference cleanup.
                tracing::error!(
                    section_id,
                    purpose = "remove single-shot section",
                    "remove_section returned None after validation; rolling back"
                );
                let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                return CommandResult::failure(format!(
                    "Section {section_id} not found during removal"
                ));
            }

            // Step 5: Enforce the closures-are-loops invariant, like the two split
            // siblings do (`execute_delete_shot_and_split`,
            // `execute_delete_shot_range_split`) and like `execute()`'s epilogue does
            // for every `can_orphan_closures` command. A loop closure exists only for
            // a TRUE loop; removing the section that carried the loop's other path
            // leaves the closure as the sole connection, which is a merge. Every
            // delete entry point converts its orphans here so memory and disk agree
            // and the loader's unconditional pass (`redrive_offsets_after_load`)
            // is a no-op safety net rather than a silent reclassification on reopen
            // (issue #413 finding 10).
            //
            // Undo restores this delete from its `ProjectSnapshotPair`, which
            // legitimately puts the PRE-conversion state back: the closure existed
            // as a true loop before the delete, so restoring it as a closure is the
            // correct inverse, not a missed conversion.
            let Some(project) = proj_guard.as_mut() else {
                // Project became None -- restore.
                tracing::error!(
                    purpose = "restore after mutation failure",
                    "Project became None after mutation; rolling back"
                );
                let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                return CommandResult::failure("No project after mutation".to_string());
            };
            project.convert_orphan_closures_to_merges();

            tracing::info!(
                section_id,
                shot_id,
                purpose = "remove single-shot section",
                "Deleted last shot and removed section"
            );
        }

        // This custom path bypasses execute()'s epilogue; re-derive offsets here
        // (after the orphan-closure conversion above) so a closure that just became
        // a rigid merge is solved immediately, and before the after-snapshot so
        // undo/redo and the saved file reflect the normalized topology. The frame
        // check runs first, in the same order as execute()'s epilogue.
        self.renormalize_frame_if_changed(frame_before);
        self.reconcile_geometry();

        // Step 6: Take the AFTER snapshot.
        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Step 7: Push single undo entry with ProjectSnapshotPair
        let batch_cmd = Command::Batch {
            commands: vec![
                Command::DeleteShot {
                    section_id,
                    shot_id,
                },
                Command::DeleteSection { section_id },
            ],
        };

        self.push_snapshot_undo(batch_cmd, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): a delete/split mutated a remote project.
        // Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        CommandResult::success()
    }
}
