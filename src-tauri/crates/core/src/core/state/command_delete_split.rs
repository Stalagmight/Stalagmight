use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    Project, LineData, LineId, ShotData, ShotId, StationKey, StationRef, FEATURE_ID_STRIDE,
    MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
};

/// Every split path (both delete-and-split paths and Split Line Here) turns
/// one line into two, so each MINTS a line id (#555) through the same
/// `Project::add_line` door `create_line` guards, while going nowhere
/// near `create_line` itself. Refused before any path mutates anything: a
/// split that ran out of ids half-way would leave line A truncated with
/// nowhere to put its tail.
pub(super) fn line_budget_refusal(
    project: &Project,
    line_id: LineId,
) -> Option<CommandResult> {
    if project.next_line_id <= MAX_LINE_ID {
        return None;
    }
    Some(CommandResult::failure(format!(
        "Cannot split line {line_id}: to split it into two lines needs \
         a new line id, but the project has reached the maximum number of lines \
         (limit = {MAX_LINES_IN_ID_SPACE}); shot ids encode as \
         line_id * {FEATURE_ID_STRIDE} + position in 32 bits, so line ids \
         past {MAX_LINE_ID} cannot hold shots"
    )))
}

impl AppState {
    /// Execute a delete-shot-and-split-line operation as a single atomic undo/redo unit.
    /// Deletes a mid-line shot and splits the line into two, with user-provided names.
    /// Uses `ProjectSnapshotPair` for single-undo. Follows `execute_create_and_merge()` pattern.
    pub fn execute_delete_shot_and_split(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        name_a: String,
        name_b: String,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete shot and split line") {
            return f;
        }
        tracing::info!(
            line_id,
            shot_id,
            purpose = "split line after deleting shot",
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
        let datums_before = self.datum_layout();

        let undo_line_b_name = name_b.clone();

        // Step 2: Get split data and perform the operation
        let (new_line_id, swept_warnings) = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };

            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            // Before anything changes, to name what the orphan sweep removes (#738).
            let closures_before = project.closures_before_sweep();

            let Some(line) = project.get_line(line_id) else {
                return CommandResult::failure(format!("Line {line_id} not found"));
            };

            // Pre-check: distinguish "shot not found" from "edge shot"
            let shot_pos = line.shots.iter().position(|s| s.id == shot_id);
            match shot_pos {
                None => {
                    return CommandResult::failure(format!(
                        "Shot {shot_id} not found in line {line_id}"
                    ))
                }
                Some(p) if p == 0 || p == line.shots.len() - 1 => {
                    return CommandResult::failure(
                        "Cannot split at edge shot (first or last). Use simple delete instead."
                            .to_string(),
                    );
                }
                _ => {}
            }

            // Frozen-geometry guard (mirrors delete_shot and the range-split
            // path): this path deletes `shot_id`, so refuse if it is part of an
            // adjusted survey network -- otherwise a frozen mid-line shot can
            // be deleted through the split path, silently breaking the LSQ
            // solution's frozen geometry. (#241)
            if project.is_shot_frozen_by_id(line_id, shot_id) {
                return CommandResult::failure(
                    "Cannot delete a frozen shot. This shot is part of an adjusted survey network."
                        .to_string(),
                );
            }

            // Line-id budget (#555), with the other pre-checks: the split
            // mints line B below, and the truncation of line A that
            // precedes it has already destroyed the shots B is supposed to
            // carry.
            if let Some(refusal) = line_budget_refusal(project, line_id) {
                return refusal;
            }

            // Get split data (safe after pre-checks above)
            let Some((pos, shots_before, shots_after)) = line.split_shots_at(shot_id) else {
                return CommandResult::failure(format!("Failed to split at shot {shot_id}"));
            };

            // Capture deleted shot data for offset calculation
            let deleted_shot_data = line.shots[pos].to_shot_data();

            // Capture metadata from original line (color, direction -- NOT
            // comment). The survey credit needs no copy: it rides on the shots.
            let color = line.color.clone();
            let direction = line.direction;
            let quality = line.quality;
            // Line B is the same survey, so it keeps the same north reference
            // (#438) - `LineData` cannot carry it, so it is copied explicitly
            // below, exactly like the metadata above.
            let azimuth_reference = line.azimuth_reference;
            // Line B must also keep the reverse parity and the gauge's
            // physical station (`settle_cut_halves` below), or a reversed
            // line's single-neighbour plan pins and gauge fallback land one
            // station off in it.
            let orientation = project.cut_orientation(line_id);
            // The component's least-squares anchor, kept on its station below.
            let component_gauges = project.hold_component_gauges(&[line_id]);
            let resolved_reference = project.azimuth_reference_for_id(line_id);

            // Stable-identity remap snapshot: resolve every stored cross-reference
            // to its CURRENT positional ref BEFORE we mutate the line.
            //
            // Shot UUIDs now survive the split (#423 - line B is rebuilt from
            // `to_shot_data`, which carries them), but the remap is still
            // required: a `StationKey` is `(line_id, anchor)`, so an anchor
            // cannot follow a shot ACROSS a line boundary. A ref into the B
            // half would keep pointing at line A, where its shot no longer
            // exists, and dangle. Positional-then-reanchor rewrites the
            // line_id; carrying the uuid is what makes the reanchored key
            // come back identical for the A half and stable for the B half.
            let split_station = (pos + 1) as u32;
            let original_station_count = line.shots.len() as u32 + 1;
            let old_refs: Vec<Option<StationRef>> = {
                let p: &Project = project;
                let mut out = Vec::new();
                p.for_each_station_ref(|_label, key| out.push(p.resolve_key(&key)));
                out
            };

            // Step 3: Modify original line -> line A
            // Truncate in-place to preserve existing Shot IDs and next_shot_id
            let Some(line_mut) = project.get_line_mut(line_id) else {
                return CommandResult::failure(format!("Line {line_id} not found"));
            };
            line_mut.name = Some(name_a);
            line_mut.shots.truncate(pos); // keep shots[0..pos], drop deleted + after

            // Step 4: Calculate offset for line B
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

            // Get existing line offset to add to computed B offset
            let (line_ox, line_oy) = project.line_offset(line_id);

            // Capture counts before moving data
            let shots_after_len = shots_after.len();

            // Step 5: Create line B with inherited metadata
            let line_b_data = LineData {
                name: Some(name_b),
                surveyor: None,
                date: None,
                color,
                direction: Some(direction),
                quality,
                shots: shots_after,
            };

            let line_b = project.add_line(line_b_data);
            let new_id = line_b.id;
            if let Some(line_b) = project.get_line_mut(new_id) {
                line_b.azimuth_reference = azimuth_reference;
            }
            // A keeps the original stations 0..=pos, B starts at pos + 1.
            if let Some(orientation) = &orientation {
                project.settle_cut_halves(line_id, new_id, orientation, pos, pos + 1);
            }

            // Set line B offset
            project.set_line_offset(new_id, (line_ox + bx, line_oy + by));

            // Step 6: Remap every cross-reference across the split.
            // Stations whose positional index moved into line B (index in
            // [split_station, original_station_count)) become line B refs with
            // index -= split_station; everything else stays put. We re-derive each
            // ref's stable key from its NEW positional home against the now-final
            // structure - which, since B's shots kept their UUIDs, differs from the
            // old key only in the line id. `old_refs` was
            // captured before truncation, in `for_each_station_ref` order; the
            // `new_keys` Vec is consumed by `map_station_refs` in that same order.
            let _ = shots_after_len;
            let new_keys: Vec<Option<StationKey>> = old_refs
                .iter()
                .map(|old_ref| {
                    let sr = (*old_ref)?;
                    let new_ref = if sr.line_id == line_id
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
                "split-line remap: map_station_refs visited a different number of \
                 refs than were captured in old_refs - traversal order diverged"
            );

            // The split can strand a line so its only attachment is a loop
            // closure - that is a merge, not a loop. Convert it now (issue #8)
            // so it's fixed immediately, not only on the next reload.
            // Once every cross-reference names its new line (components are
            // read off them), hold each component at the station the
            // adjustment held before; before the orphan sweep, which only
            // turns a closure edge into a merge edge.
            project.restore_component_gauges(&component_gauges);
            let sweep = project.sweep_orphan_closures();
            let swept_warnings = super::command_shot::report_swept_closures(
                project,
                &closures_before,
                &sweep,
                "delete shot and split",
                super::command_shot::OnePointCause::Promotion,
            );

            project.touch();
            (new_id, swept_warnings)
        };

        // This custom path bypasses execute()'s reconcile; re-derive offsets here
        // (after the orphan-closure conversion above) and before the after-snapshot
        // so undo/redo and the saved file reflect the normalized topology. The
        // frame check runs first, in the same order as execute()'s epilogue.
        self.settle_datum_layout(&datums_before);
        self.reconcile_geometry();

        // Step 7: Take AFTER snapshot
        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Step 8: Push single undo entry with ProjectSnapshotPair
        // Use a Batch with both DeleteShot + CreateLine to accurately describe the compound op
        let batch_cmd = Command::Batch {
            commands: vec![
                Command::DeleteShot {
                    line_id,
                    shot_id,
                },
                Command::CreateLine {
                    name: Some(undo_line_b_name),
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
            line_id,
            shot_id,
            new_line_id,
            purpose = "split line after deleting shot",
            "Delete shot and split line completed"
        );

        CommandResult::success()
            .with_line_id(new_line_id)
            .with_warnings(swept_warnings)
    }

    /// Delete a contiguous *interior* range of shots and split the line into
    /// two: line A keeps the shots before the range (in place), line B is a
    /// new line holding the shots after the range (offset so it keeps its
    /// absolute position). Generalizes [`Self::execute_delete_shot_and_split`]
    /// from a single shot to `[start_pos..=end_pos]`. Commits as ONE
    /// `ProjectSnapshotPair` undo entry. The range must be interior (shots remain
    /// before AND after); prefix/suffix/whole ranges use the shift or
    /// line-delete paths.
    pub fn execute_delete_shot_range_split(
        &self,
        line_id: LineId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
        name_a: String,
        name_b: String,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete shot range and split line") {
            return f;
        }
        tracing::info!(
            line_id,
            start_shot_id,
            end_shot_id,
            purpose = "split line after deleting shot range",
            "Executing delete_shot_range_split as single undo unit"
        );

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // Same frame check as the single-shot split above (#469, item 1).
        let datums_before = self.datum_layout();
        let undo_line_b_name = name_b.clone();

        let (new_line_id, swept_warnings) = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            // Before anything changes, to name what the orphan sweep removes (#738).
            let closures_before = project.closures_before_sweep();
            let Some(line) = project.get_line(line_id) else {
                return CommandResult::failure(format!("Line {line_id} not found"));
            };

            let n = line.shots.len();
            let Some(mut start_pos) = line.shots.iter().position(|s| s.id == start_shot_id)
            else {
                return CommandResult::failure(format!(
                    "Shot {start_shot_id} not found in line {line_id}"
                ));
            };
            let Some(mut end_pos) = line.shots.iter().position(|s| s.id == end_shot_id) else {
                return CommandResult::failure(format!(
                    "Shot {end_shot_id} not found in line {line_id}"
                ));
            };
            if start_pos > end_pos {
                std::mem::swap(&mut start_pos, &mut end_pos);
            }
            // Split requires shots both before and after the range.
            if start_pos == 0 || end_pos >= n - 1 {
                return CommandResult::failure(
                    "Range is not interior to the line (split needs shots before and after). \
                     Use shift or delete the line instead."
                        .to_string(),
                );
            }

            // Capture owned data; afterwards the `line` borrow ends so we can mutate.
            let shots_before: Vec<ShotData> = line.shots[..start_pos]
                .iter()
                .map(|s| s.to_shot_data())
                .collect();
            let shots_after: Vec<ShotData> = line.shots[end_pos + 1..]
                .iter()
                .map(|s| s.to_shot_data())
                .collect();
            let deleted_shots_data: Vec<ShotData> = line.shots[start_pos..=end_pos]
                .iter()
                .map(|s| s.to_shot_data())
                .collect();
            let range_shot_ids: Vec<ShotId> = line.shots[start_pos..=end_pos]
                .iter()
                .map(|s| s.id)
                .collect();
            // The survey credit rides on the shots; nothing to copy.
            let color = line.color.clone();
            let direction = line.direction;
            let quality = line.quality;
            // Same survey, same north reference (#438); see the single-shot path.
            let azimuth_reference = line.azimuth_reference;
            // And the same reverse parity and gauge station; see the single-shot path.
            let orientation = project.cut_orientation(line_id);
            // The component's least-squares anchor, kept on its station below.
            let component_gauges = project.hold_component_gauges(&[line_id]);

            // Frozen-geometry guard (mirrors delete_shot).
            for &sid in &range_shot_ids {
                if project.is_shot_frozen_by_id(line_id, sid) {
                    return CommandResult::failure(
                        "Cannot delete a frozen shot. This range includes shots that are part of \
                         an adjusted survey network."
                            .to_string(),
                    );
                }
            }

            // Line-id budget (#555), before the connectivity rewrites below:
            // this path mutates the project (preserve/cleanup) well before it
            // mints line B, so a refusal has to happen here to stay free.
            if let Some(refusal) = line_budget_refusal(project, line_id) {
                return refusal;
            }

            // Interior stations dropped by the split (strictly between the kept
            // boundary stations `start_pos` (→A) and `end_pos+1` (→B:0)).
            let interior: std::collections::HashSet<StationRef> = ((start_pos as u32 + 1)
                ..=(end_pos as u32))
                .map(|i| StationRef::new(line_id, i))
                .collect();

            // Preserve connectivity across the dropped stations, then drop their
            // own refs - BEFORE we capture old_refs, so the stored-ref set is
            // final and the positional remap below stays 1:1 aligned. Both run
            // while the line is structurally intact (indices still resolve).
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

            // Line A = before (truncate in place to preserve shot IDs).
            let Some(line_mut) = project.get_line_mut(line_id) else {
                // preserve/cleanup above already mutated the project; roll back
                // to the before-snapshot so a failure here leaves no half-applied
                // state (mirrors the shift path's rollback on remove failure).
                let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                return CommandResult::failure(format!("Line {line_id} not found"));
            };
            line_mut.name = Some(name_a);
            line_mut.shots.truncate(start_pos);

            // Line B offset = line offset + displacement of (before ++ deleted).
            let mut bx: f64 = 0.0;
            let mut by: f64 = 0.0;
            let resolved_reference = project.azimuth_reference_for_id(line_id);
            for shot_data in shots_before.iter().chain(deleted_shots_data.iter()) {
                let temp_shot = crate::models::Shot::new(0, shot_data.clone());
                let azimuth_rad = temp_shot.average_azimuth_true_rad(resolved_reference);
                let horizontal = temp_shot.horizontal_length();
                bx += horizontal * azimuth_rad.sin();
                by += horizontal * azimuth_rad.cos();
            }
            let (line_ox, line_oy) = project.line_offset(line_id);

            let line_b_data = LineData {
                name: Some(name_b),
                surveyor: None,
                date: None,
                color,
                direction: Some(direction),
                quality,
                shots: shots_after,
            };
            let line_b = project.add_line(line_b_data);
            let new_id = line_b.id;
            if let Some(line_b) = project.get_line_mut(new_id) {
                line_b.azimuth_reference = azimuth_reference;
            }
            // A keeps the original stations 0..=start_pos, B starts after
            // the deleted range, at end_pos + 1.
            if let Some(orientation) = &orientation {
                project.settle_cut_halves(line_id, new_id, orientation, start_pos, end_pos + 1);
            }
            project.set_line_offset(new_id, (line_ox + bx, line_oy + by));

            // Remap cross-refs across the split: stations at index >= split_station
            // (= end_pos+1) move to B with index -= split_station; everything else
            // (index <= start_pos) stays on A. Interior refs were already dropped,
            // so no surviving ref resolves into the gap.
            let new_keys: Vec<Option<StationKey>> = old_refs
                .iter()
                .map(|old_ref| {
                    let sr = (*old_ref)?;
                    let new_ref = if sr.line_id == line_id
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

            // Once every cross-reference names its new line (components are
            // read off them), hold each component at the station the
            // adjustment held before; before the orphan sweep, which only
            // turns a closure edge into a merge edge.
            project.restore_component_gauges(&component_gauges);
            let sweep = project.sweep_orphan_closures();
            let swept_warnings = super::command_shot::report_swept_closures(
                project,
                &closures_before,
                &sweep,
                "delete shot range and split",
                super::command_shot::OnePointCause::Promotion,
            );
            project.touch();
            (new_id, swept_warnings)
        };

        // Custom path bypasses execute()'s reconcile; re-derive offsets here.
        self.settle_datum_layout(&datums_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        let batch_cmd = Command::Batch {
            commands: vec![
                Command::DeleteShot {
                    line_id,
                    shot_id: start_shot_id,
                },
                Command::CreateLine {
                    name: Some(undo_line_b_name),
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
            line_id,
            new_line_id,
            purpose = "split line after deleting shot range",
            "Delete shot range and split line completed"
        );
        CommandResult::success()
            .with_line_id(new_line_id)
            .with_warnings(swept_warnings)
    }

    /// Delete a contiguous range of shots by gap-closing (shifting) - the
    /// survivors rejoin and the line stays single. ATOMIC: the range's shots
    /// go in forward order inside ONE project lock and ONE `ProjectSnapshotPair`
    /// undo unit, with the geometry cascade DEFERRED to a single reconcile at the
    /// end (each step is a valid single-shot gap-close: head/before stays for
    /// interior & suffix ranges, tail/after stays for prefix ranges). Refuses
    /// when the range is shift-blocked or would empty the line (use split /
    /// line delete respectively).
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
    ///   line is anchored to (the doomed-tail-anchor shape), they deliberately
    ///   DIVERGE. `AppState::execute` runs the geometry epilogue after every
    ///   command, so the single-delete sequence re-anchors the partner while the
    ///   merge is still live, drags it along as the line shrinks, and then
    ///   deletes the merge - leaving the partner displaced with no edge to pull
    ///   it back. Deferring to one final cascade leaves it where the surveyor put
    ///   it, which is what this method does.
    ///
    /// The atomic behaviour is the reviewed intent, pinned by
    /// `test_delete_range_shift_does_not_displace_free_floating_merged_line`
    /// and `test_delete_range_suffix_shift_keeps_head_with_doomed_tail_anchor`
    /// (PR #95, `core/state/tests.rs`), and asserted on random topology by the
    /// metamorphic pair `range-shift-is-n-single-deletes`
    /// (`core/state/metamorphic_undo_tests.rs`), which requires equality on every
    /// other shape.
    pub fn execute_delete_shot_range_shift(
        &self,
        line_id: LineId,
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
            line_id,
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
        let datums_before = self.datum_layout();

        let (result_line, removed_closures) = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            let Some(line) = project.get_line(line_id) else {
                return CommandResult::failure(format!("Line {line_id} not found"));
            };

            let n = line.shots.len();
            let Some(mut start_pos) = line.shots.iter().position(|s| s.id == start_shot_id)
            else {
                return CommandResult::failure(format!(
                    "Shot {start_shot_id} not found in line {line_id}"
                ));
            };
            let Some(mut end_pos) = line.shots.iter().position(|s| s.id == end_shot_id) else {
                return CommandResult::failure(format!(
                    "Shot {end_shot_id} not found in line {line_id}"
                ));
            };
            if start_pos > end_pos {
                std::mem::swap(&mut start_pos, &mut end_pos);
            }
            // Deleting the whole line this way would leave it empty - route
            // that through delete_line instead.
            if start_pos == 0 && end_pos == n - 1 {
                return CommandResult::failure(
                    "Cannot shift-delete an entire line; delete the line instead."
                        .to_string(),
                );
            }

            // Delete in forward (ascending position) order by stable shot id.
            let range_shot_ids: Vec<ShotId> = line.shots[start_pos..=end_pos]
                .iter()
                .map(|s| s.id)
                .collect();

            // Frozen-geometry + shift-blocked prechecks (no mutation yet).
            for &sid in &range_shot_ids {
                if project.is_shot_frozen_by_id(line_id, sid) {
                    return CommandResult::failure(
                        "Cannot delete a frozen shot. This range includes shots that are part of \
                         an adjusted survey network."
                            .to_string(),
                    );
                }
            }
            // Without the gap-close closure prediction: this command is about
            // to run that very gap-close for real.
            if let Ok(impact) = project.preview_delete_shot_range_relationships(
                line_id,
                start_shot_id,
                end_shot_id,
            ) {
                if impact.shift_blocked {
                    let reasons = if impact.shift_blocked_reasons.is_empty() {
                        "Shift geometry is not allowed for this range.".to_string()
                    } else {
                        impact.shift_blocked_reasons.join(" ")
                    };
                    return CommandResult::failure(format!("{reasons} Use split instead."));
                }
            }

            // Closures already across one shot, and every closure's stations,
            // before the range goes (#737): the removal below names a closure by
            // the ids the user created it with, not the renumbered ones.
            let closures_before = project.closures_before_edit();

            // The gap-close itself, shared with the preview that predicts its
            // closure removals (`Project::gap_close_shot_range`, which also
            // keeps the doomed-anchor rule this path used to spell out here).
            if let Err(e) = project.gap_close_shot_range(line_id, start_pos, end_pos) {
                // Already mutated - roll back to the before snapshot.
                let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                return CommandResult::failure(e);
            }
            // An interior gap-close brings the head boundary one shot from the
            // first surviving tail station, so a closure between them (A:1-A:4
            // with shots 1..2 deleted) now spans one shot and closes nothing.
            // Unlike the single-shot delete, which refuses to shift a shot that
            // is part of any loop, the range only refuses merge loops - so this
            // is reachable here. Removed now, inside the one undo unit, before
            // the cascade sees the final topology (#737).
            let removed_closures = super::command_shot::remove_closures_newly_across_one_shot(
                project,
                &closures_before,
            );
            // Single cascade now that every range shot (and any doomed merge) is
            // gone, so a doomed station's intermediate shift never propagates into
            // a connected free-floating line - the per-shot cascades were
            // deferred for exactly this reason. Assert alignment here, since each
            // per-shot delete skipped its own alignment check.
            project.cascade_connection_offsets(&[line_id]);
            // That cascade is seeded with the edited line plus the datum
            // lines, which is the right frame for THIS edit and is not a set
            // that reaches every merge the assert below covers: a range can cut a
            // whole component loose, and the BFS never enters one with no
            // authoritative line in it. `realign_merges` closes that hole with
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
            (line_id, removed_closures)
        };

        self.settle_datum_layout(&datums_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        let batch_cmd = Command::Batch {
            commands: vec![
                Command::DeleteShot {
                    line_id,
                    shot_id: start_shot_id,
                },
                Command::DeleteShot {
                    line_id,
                    shot_id: end_shot_id,
                },
            ],
        };
        self.push_snapshot_undo(batch_cmd, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): a delete/split mutated a remote project.
        // Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        tracing::info!(
            line_id,
            purpose = "gap-close a deleted shot range",
            "Delete shot range (shift) completed"
        );
        CommandResult::success()
            .with_line_id(result_line)
            .with_warnings(removed_closures)
    }

    /// Execute a delete-last-shot-and-line operation as a single atomic undo/redo unit.
    /// When a line has exactly 1 shot remaining, this removes the line (and its
    /// only shot) along with all line references (merges, closures, line_offsets,
    /// station_offsets, zero_datums). Uses `ProjectSnapshotPair` for single-undo.
    pub fn execute_delete_last_shot_and_line(
        &self,
        line_id: LineId,
        shot_id: ShotId,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete last shot and line") {
            return f;
        }
        tracing::info!(
            line_id,
            shot_id,
            purpose = "remove single-shot line",
            "Executing delete_last_shot_and_line as single undo unit"
        );

        // Step 1: Take BEFORE snapshot
        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Same frame check as the split paths above (#469, item 1). This path
        // removes a whole LINE, so it can drop the primary datum outright.
        let datums_before = self.datum_layout();

        // Step 2: Validate and mutate in a single lock scope. On any failure after
        // mutation we roll back from before_snapshot. `command_lock` is held for the
        // whole call, so no other command can observe or interleave with the window
        // between this scope and the after-snapshot below.
        let swept_warnings = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };

            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            // Before anything changes, to name what the orphan sweep removes (#738).
            let closures_before = project.closures_before_sweep();

            // Validate line exists
            let Some(line) = project.get_line(line_id) else {
                return CommandResult::failure(format!("Line {line_id} not found"));
            };

            // Verify line has exactly 1 shot
            if line.shots.len() != 1 {
                return CommandResult::failure(format!(
                    "Line {} has {} shots, expected exactly 1",
                    line_id,
                    line.shots.len()
                ));
            }

            // Verify the shot matches
            if line.shots[0].id != shot_id {
                return CommandResult::failure(format!(
                    "Shot {} not found in line {} (line has shot {})",
                    shot_id, line_id, line.shots[0].id
                ));
            }

            // Step 3: Preserve merge/closure connectivity, then clean up line references
            project.preserve_connections_for_line_deletion(line_id);

            project.station_merges.retain(|m| {
                m.station_a.line_id != line_id && m.station_b.line_id != line_id
            });

            project.loop_closures.retain(|lc| {
                lc.station_a.line_id != line_id && lc.station_b.line_id != line_id
            });

            // Remove line offsets
            project.remove_line_offset(line_id);

            // Remove station offsets referencing this line
            project
                .station_offsets
                .retain(|key, _| key.line_id != line_id);

            // Remove station comments and names referencing this line
            project
                .station_comments
                .retain(|key, _| key.line_id != line_id);
            project
                .station_names
                .retain(|key, _| key.line_id != line_id);

            // Remove zero datums referencing this line
            project
                .zero_datums
                .retain(|zd| zd.station_ref.line_id != line_id);

            // Step 4: Remove the line (and its only shot)
            if project.remove_line(line_id).is_none() {
                // Line vanished between validation and removal (should not happen).
                // Rollback to before_snapshot to undo reference cleanup.
                tracing::error!(
                    line_id,
                    purpose = "remove single-shot line",
                    "remove_line returned None after validation; rolling back"
                );
                let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                return CommandResult::failure(format!(
                    "Line {line_id} not found during removal"
                ));
            }

            // Step 5: Enforce the closures-are-loops invariant, like the two split
            // siblings do (`execute_delete_shot_and_split`,
            // `execute_delete_shot_range_split`) and like `execute()`'s epilogue does
            // for every `can_orphan_closures` command. A loop closure exists only for
            // a TRUE loop; removing the line that carried the loop's other path
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
            let sweep = project.sweep_orphan_closures();
            let swept_warnings = super::command_shot::report_swept_closures(
                project,
                &closures_before,
                &sweep,
                "delete last shot and line",
                super::command_shot::OnePointCause::Promotion,
            );

            tracing::info!(
                line_id,
                shot_id,
                purpose = "remove single-shot line",
                "Deleted last shot and removed line"
            );
            swept_warnings
        };

        // This custom path bypasses execute()'s epilogue; re-derive offsets here
        // (after the orphan-closure conversion above) so a closure that just became
        // a rigid merge is solved immediately, and before the after-snapshot so
        // undo/redo and the saved file reflect the normalized topology. The frame
        // check runs first, in the same order as execute()'s epilogue.
        self.settle_datum_layout(&datums_before);
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
                    line_id,
                    shot_id,
                },
                Command::DeleteLine { line_id },
            ],
        };

        self.push_snapshot_undo(batch_cmd, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): a delete/split mutated a remote project.
        // Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        CommandResult::success().with_warnings(swept_warnings)
    }
}
