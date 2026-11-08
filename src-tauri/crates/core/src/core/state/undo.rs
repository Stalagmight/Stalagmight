use crate::core::{
    history_step_failure, Command, CommandError, CommandErrorCode, CommandResult, ReversedSection,
    UndoData, UndoEntry,
};
use crate::models::{SectionId, SectionIdentity, SectionUpdates, Shot, ShotId, ShotUpdates};

use super::AppState;

/// Which way through the history a step is going, for the two paths that have to
/// treat the two halves of a `ProjectSnapshotPair` asymmetrically.
#[derive(Clone, Copy)]
enum HistoryMove {
    Undo,
    Redo,
}

impl HistoryMove {
    /// [#576 C4] The refusal of a step with nothing to take: nothing ran.
    fn nothing_to_step(self) -> CommandResult {
        CommandResult::refused(match self {
            Self::Undo => CommandError::new(CommandErrorCode::NothingToUndo, "Nothing to undo"),
            Self::Redo => CommandError::new(CommandErrorCode::NothingToRedo, "Nothing to redo"),
        })
    }
}

/// [#576 C4] A history step that RAN and failed: `HISTORY_STEP_FAILED`, step
/// `failed_step` of `total_steps`, with `message` as the display text. The
/// project changed (the steps before it landed, and a failing step can mutate
/// before it reports), which is what the code tells the frontend.
fn history_step_failed(message: String, failed_step: usize, total_steps: usize) -> CommandResult {
    let mut err = CommandError::new(CommandErrorCode::HistoryStepFailed, message);
    err.payload.history_step = Some(history_step_failure(failed_step, total_steps));
    CommandResult::refused(err)
}

/// [#576 C4] A single undo/redo's answer: a failure of the step it ran is step 1
/// of 1 (`HISTORY_STEP_FAILED`), its message unchanged. An empty stack found by
/// the step itself stays `NOTHING_TO_*`: nothing ran.
fn single_step_answer(result: CommandResult) -> CommandResult {
    let nothing_ran = matches!(
        result.code,
        Some(CommandErrorCode::NothingToUndo | CommandErrorCode::NothingToRedo)
    );
    if result.success || nothing_ran {
        return result;
    }
    history_step_failed(
        result.error.unwrap_or_else(|| "Unknown error".to_string()),
        1,
        1,
    )
}

impl AppState {
    // === Project operations ===

    /// Perform undo - restore previous state based on undo data
    pub fn undo(&self) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let _cmd_guard = self.command_lock.lock();
        // [#576 A11] Refused rather than recovered after an internal crash: the
        // inverse would land on what the crash left (`refuse_after_crash`).
        if let Err(refused) = self.refuse_after_crash("Undo") {
            return CommandResult::refused(refused);
        }
        // Read-only edit gate (#191, D-B6): you cannot mutate a read-only remote
        // project even to revert. Gated unconditionally.
        if let Err(f) = self.guard_mutation("Undo") {
            return CommandResult::refused(f);
        }
        self.invalidate_adjusted_cache();
        // [#576 A18(a) / NF-AN3-1] The index check `undo_to` makes, before anything can
        // change: an empty stack is the one failure that provably mutated nothing.
        if let Err(nothing) = self.history_step_available(HistoryMove::Undo) {
            return nothing;
        }
        let result = self.undo_inner();
        // Dirty-bit flip (#191, D-B16): an undo is an edit of a remote project.
        // Set-only; no-op for local-only. [#576 A18(a)] A FAILED step past the check
        // above gets it too: an arm reports its failure only after running its
        // inverse, and the inverse mutates as it goes (see `undo_to`). Over-marking
        // costs at most a no-op 304 push; a missed mark let Done skip the upload of
        // real changes and a re-pull replace the copy without asking.
        self.mark_remote_dirty();
        // Reconcile derived offsets after the restore - and after a failed step's
        // partial one, for the same reason. For snapshot-restore
        // undos (Merge/Move/Delete/Repair) the restored `before` is already a
        // valid aligned state, so realign_merges early-returns (no-op) - EXCEPT
        // undo-of-Repair, whose `before` carries the >=tolerance drift Repair
        // healed. There, reconcile re-derives those offsets: the undo reverts the
        // STRUCTURAL repair (dropped dangling refs / converted closures come back)
        // while keeping geometry valid, rather than re-introducing broken derived
        // offsets the user can't act on. Offsets are derived state (re-solved on
        // every load), so this is the intended contract, not a loss. (A
        // sub-tolerance offset change is below realign_merges' threshold, so the
        // `before` snapshot's offsets are preserved exactly there.)
        self.reconcile_geometry();
        single_step_answer(result)
    }

    /// Undo implementation (caller must hold command_lock)
    fn undo_inner(&self) -> CommandResult {
        // Pop from undo stack
        let mut entry = {
            let mut stack = self.undo_stack.lock();

            let Some(e) = stack.pop_undo() else {
                return HistoryMove::Undo.nothing_to_step();
            };
            e
        };

        // A `CreateSection` undo is about to DELETE the section, so this is the
        // last moment its identity can be read. Refresh the entry from the live
        // section first, or the redo puts back what `CreateSection` originally
        // built rather than what the undo destroyed - which loses every heal
        // recorded since, the gauge pair a save captured above all (#413,
        // finding 1's second half). A no-op for every other variant.
        self.refresh_created_section_identity(&mut entry);
        // A snapshot pair is a WALL, and the live project is the wall's `after`
        // side. Re-record it before stepping through, so the redo puts back what
        // this undo is about to destroy rather than a payload that predates every
        // undo-less heal since (#413, finding 11's re-creation residue). A no-op
        // for every other variant.
        self.refresh_snapshot_pair_counterpart(&mut entry, HistoryMove::Undo);

        // Apply undo based on undo_data type
        // IMPORTANT: No early returns after this point - all paths must flow
        // through the epilogue so the entry is either pushed to redo (success)
        // or pushed back to undo (failure).
        let mut result = match &entry.undo_data {
            UndoData::InverseCommand(inverse_cmd) => {
                // Execute the inverse command (without pushing to undo stack)
                self.execute_without_undo(inverse_cmd.clone())
            }

            UndoData::PreviousValues {
                section_id,
                shot_id,
                shot_uuid,
                values_json,
                previous_offsets,
            } => {
                // Restore previous values
                if let Some(shot_id) = shot_id {
                    // Restore shot values - onto the shot this entry was
                    // recorded against, resolved by uuid (#413, finding 12).
                    match self.resolve_undo_shot(*section_id, shot_uuid.as_deref(), *shot_id) {
                        Ok(target_id) => {
                            if let Ok(prev_updates) =
                                serde_json::from_str::<ShotUpdates>(values_json.as_str())
                            {
                                // Guard-free (`restore_shot_values`): an inverse
                                // consults no user-intent guard, and the frozen
                                // check judges the state the FORWARD command
                                // produced rather than the one it was permitted
                                // from. See that function for the single-command
                                // route that makes the two differ.
                                let restored =
                                    self.restore_shot_values(*section_id, target_id, prev_updates);
                                if restored.success {
                                    // The re-execution's `realign_after_shot_edit`
                                    // just RE-DERIVED the group's offsets. An
                                    // inverse restores rather than re-derives, and
                                    // the two part company on a key that lands
                                    // back on the origin: the adjusted write drops
                                    // it, and the pre-edit state may legitimately
                                    // have HELD it (#413, finding 27). Hand the
                                    // recorded map back verbatim, exactly as
                                    // `SegmentMoveInfo` and `ZeroDatumInfo` do.
                                    self.restore_captured_section_offsets(previous_offsets);
                                }
                                restored
                            } else {
                                CommandResult::failure("Failed to parse undo data".to_string())
                            }
                        }
                        Err(e) => CommandResult::failure(e),
                    }
                } else {
                    // Restore section values
                    if let Ok(prev_updates) =
                        serde_json::from_str::<SectionUpdates>(values_json.as_str())
                    {
                        self.update_section(*section_id, prev_updates)
                    } else {
                        CommandResult::failure("Failed to parse undo data".to_string())
                    }
                }
            }

            UndoData::SectionSnapshot {
                section_id,
                section_data,
            } => {
                // Restore deleted section. LEGACY ONLY: a live `DeleteSection`
                // captures a `ProjectSnapshotPair` (which restores the whole
                // `Section`, identity included), so this arm serves persisted
                // entries written before that change - and those carry no
                // identity, so the shots come back with fresh uuids exactly as
                // they always did.
                self.restore_section(
                    *section_id,
                    section_data.clone(),
                    &SectionIdentity::default(),
                )
            }

            UndoData::CreatedSectionSnapshot { section_id, .. } => {
                // Undo a CreateSection by deleting the section
                self.delete_section(*section_id)
            }

            UndoData::ShotSnapshot {
                section_id,
                shot_id,
                shot_data,
                position,
                uuid,
            } => {
                // Restore deleted shot at its original position with original ID
                // and original UUID. (Live DeleteShot now captures a full
                // ProjectSnapshotPair; this path serves legacy persisted entries,
                // where `uuid` is None and a fresh one is minted.)
                self.restore_shot(
                    *section_id,
                    *shot_id,
                    shot_data.clone(),
                    *position,
                    uuid.clone(),
                )
            }

            UndoData::CreatedShotSnapshot {
                section_id,
                shot_id,
                position,
                after_shot_id,
                before_shot_id,
                uuid,
                previous_offsets,
                legacy_comment_rows,
                ..
            } => {
                // Undo a CreateShot by removing the shot it created - resolved
                // by uuid, because the in-session id it was recorded with
                // addresses a DIFFERENT shot after a save/reopen (#413,
                // finding 8) or none at all (finding 13).
                //
                // Through `remove_created_shot`, NOT the user-facing
                // `delete_shot`: an inverse must not consult the guards that
                // express what a user may do, or a permitted CreateShot becomes
                // an undo that fails and wedges the history (#413, finding 6).
                // The recorded anchors and position say WHICH insert is being
                // inverted; both anchors absent at position 0 is the append
                // into an empty section whose inverse is not the general
                // delete (finding 3).
                match self.resolve_undo_shot(*section_id, uuid.as_deref(), *shot_id) {
                    Ok(target_id) => {
                        let removed = self.remove_created_shot(
                            *section_id,
                            target_id,
                            after_shot_id.is_none() && before_shot_id.is_none() && *position == 0,
                        );
                        if removed.success {
                            // The removal paid the section's offset back by the
                            // segment it took out, through the adjusted write -
                            // which DROPS a key that lands on the origin whether
                            // or not the pre-insert state held one. Restore the
                            // recorded map instead of trusting that re-derivation
                            // (#413, finding 27's second arm).
                            self.restore_captured_section_offsets(previous_offsets);
                            // [#576 RV20-6] A comment held under another key that
                            // named the station goes back under that key. In
                            // session the removal left it in place; after a save
                            // and reopen it had the station's own key and went
                            // with the shot.
                            if !legacy_comment_rows.is_empty() {
                                if let Some(project) = self.project.lock().as_mut() {
                                    for (key, text) in legacy_comment_rows {
                                        project
                                            .station_comments
                                            .entry(key.clone())
                                            .or_insert_with(|| text.clone());
                                    }
                                    project.touch();
                                }
                            }
                        }
                        removed
                    }
                    Err(e) => CommandResult::failure(e),
                }
            }

            UndoData::MergeInfo {
                station_a,
                station_b,
                previous_offsets,
                ..
            } => {
                // Undo a merge by restoring section offsets and unmerging the stations.
                {
                    let mut proj_guard = self.project.lock();
                    match proj_guard.as_mut() {
                        Some(project) => {
                            // Restore section offsets to pre-merge positions
                            project.restore_section_offsets(previous_offsets);

                            // Then unmerge the stations
                            project.unmerge_stations(*station_a, *station_b);
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    }
                }
            }

            UndoData::UnmergeInfo {
                station_a,
                station_b,
                removed_closures,
            } => {
                // Re-merge the stations to undo the unmerge
                let merge_result = self.merge_stations(*station_a, *station_b);
                if merge_result.success {
                    // Merge succeeded, now restore removed closures
                    if !removed_closures.is_empty() {
                        {
                            let mut proj_guard = self.project.lock();
                            if let Some(project) = proj_guard.as_mut() {
                                for closure in removed_closures {
                                    project.next_closure_id =
                                        project.next_closure_id.max(closure.id + 1);
                                    project.loop_closures.push(closure.clone());
                                }
                                project.touch();
                            }
                        }
                    }
                    merge_result
                } else {
                    // If re-merge fails (e.g., stations now connected), try direct restoration
                    {
                        let mut proj_guard = self.project.lock();
                        match proj_guard.as_mut() {
                            Some(project) => {
                                if let (Some(key_a), Some(key_b)) = (
                                    project.key_for_ref(*station_a),
                                    project.key_for_ref(*station_b),
                                ) {
                                    project.record_station_merge(crate::models::StationMerge {
                                        station_a: key_a,
                                        station_b: key_b,
                                        merged_at: chrono::Utc::now(),
                                    });
                                }
                                // Restore removed closures and bump next_closure_id
                                for closure in removed_closures {
                                    project.next_closure_id =
                                        project.next_closure_id.max(closure.id + 1);
                                    project.loop_closures.push(closure.clone());
                                }
                                project.touch();
                                CommandResult::success()
                            }
                            None => CommandResult::failure("No project open".to_string()),
                        }
                    }
                }
            }

            UndoData::ClosureInfo { closure, position } => {
                match &entry.command {
                    Command::CreateLoopClosure { .. } => {
                        // Undo create by removing the same closure ID.
                        {
                            let mut proj_guard = self.project.lock();
                            match proj_guard.as_mut() {
                                Some(project) => {
                                    if let Some(pos) = project
                                        .loop_closures
                                        .iter()
                                        .position(|lc| lc.id == closure.id)
                                    {
                                        project.loop_closures.remove(pos);
                                    }
                                    project.touch();
                                    CommandResult::success()
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            }
                        }
                    }
                    Command::RemoveLoopClosure { .. } => {
                        // Undo remove by restoring the captured closure AT ITS
                        // ORIGINAL INDEX. The removal was a `Vec::remove`, which
                        // keeps the rest in order, so appending here would
                        // permute a list that is persisted into the `.stalx`,
                        // indexed by the frontend and traversed by
                        // `resolve_offsets` (#413, finding 4). A legacy entry
                        // carries no position and still appends.
                        {
                            let mut proj_guard = self.project.lock();
                            match proj_guard.as_mut() {
                                Some(project) => {
                                    project.next_closure_id =
                                        project.next_closure_id.max(closure.id + 1);
                                    match position {
                                        Some(index) => {
                                            let at = (*index).min(project.loop_closures.len());
                                            project.loop_closures.insert(at, closure.clone());
                                        }
                                        None => project.loop_closures.push(closure.clone()),
                                    }
                                    project.touch();
                                    CommandResult::success()
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            }
                        }
                    }
                    _ => CommandResult::failure("Unsupported closure undo command".to_string()),
                }
            }

            UndoData::ZeroDatumInfo {
                station_ref,
                previous_gps,
                position,
                previous_offsets,
            } => {
                // Restore previous datum state directly without recomputing offsets
                // This is critical because set_zero_datum would compute new offsets
                // instead of restoring the exact previous state
                {
                    let mut proj_guard = self.project.lock();
                    match proj_guard.as_mut() {
                        Some(project) => {
                            // Restore section offsets directly
                            project.restore_section_offsets(previous_offsets);

                            // Stable stored identity for the datum's station.
                            let datum_key = project.key_for_ref(*station_ref);

                            // Restore GPS datum state, AT ITS ORIGINAL INDEX when
                            // the datum has to be put back: the removal was a
                            // `Vec::remove`, so appending permutes a list that
                            // is persisted, indexed by the frontend, and walked
                            // by `resolve_offsets` to decide which datum anchors
                            // a connected group (#413, finding 17). A legacy
                            // entry carries no position and still appends.
                            if let Some(gps) = previous_gps {
                                if let Some(existing) = project
                                    .zero_datums
                                    .iter_mut()
                                    .find(|zd| Some(&zd.station_ref) == datum_key.as_ref())
                                {
                                    existing.gps = *gps;
                                } else if let Some(key) = datum_key {
                                    let datum = crate::models::ZeroDatum {
                                        station_ref: key,
                                        gps: *gps,
                                    };
                                    match position {
                                        Some(index) => {
                                            let at = (*index).min(project.zero_datums.len());
                                            project.zero_datums.insert(at, datum);
                                        }
                                        None => project.zero_datums.push(datum),
                                    }
                                }
                            } else {
                                project
                                    .zero_datums
                                    .retain(|zd| Some(&zd.station_ref) != datum_key.as_ref());
                            }

                            project.touch();
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    }
                }
            }

            UndoData::StationCommentRows {
                station_ref,
                previous,
            } => {
                // [#576 RV19a-2] Put back the exact rows the set replaced: drop
                // whatever names the station now (the row the command stored
                // under the canonical key, if any), then restore each recorded
                // row under its own key - a legacy `Index` key stays one.
                let mut proj_guard = self.project.lock();
                match proj_guard.as_mut() {
                    Some(project) => {
                        let naming: Vec<crate::models::StationKey> = project
                            .station_comments
                            .keys()
                            .filter(|key| {
                                key.section_id == station_ref.section_id
                                    && project.key_is_ref(key, *station_ref)
                            })
                            .cloned()
                            .collect();
                        for key in &naming {
                            project.station_comments.remove(key);
                        }
                        for (key, text) in previous {
                            project.station_comments.insert(key.clone(), text.clone());
                        }
                        project.touch();
                        CommandResult::success()
                    }
                    None => CommandResult::failure("No project open".to_string()),
                }
            }

            UndoData::SegmentMoveInfo {
                previous_offsets, ..
            } => {
                // Restore previous offsets for each affected section
                {
                    let mut proj_guard = self.project.lock();
                    match proj_guard.as_mut() {
                        Some(project) => {
                            project.restore_section_offsets(previous_offsets);
                            project.touch();
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    }
                }
            }

            UndoData::StationMoveInfo {
                station_ref,
                prev_shot_before,
                prev_shot_after,
                prev_offset,
                prev_section_offset,
            } => {
                // Restore adjacent shot data and station offset
                {
                    let mut proj_guard = self.project.lock();
                    match proj_guard.as_mut() {
                        Some(project) => {
                            let section = project
                                .sections
                                .iter_mut()
                                .find(|s| s.id == station_ref.section_id);
                            if let Some(section) = section {
                                // Restore preceding shot
                                if let Some((shot_id, shot_data)) = prev_shot_before {
                                    if let Some(shot) =
                                        section.shots.iter_mut().find(|s| s.id == *shot_id)
                                    {
                                        shot.azimuth_in = shot_data.azimuth_in;
                                        shot.azimuth_out = shot_data.azimuth_out;
                                        shot.length = shot_data.length;
                                        shot.depth_in = shot_data.depth_in;
                                        shot.depth_out = shot_data.depth_out;
                                    }
                                }
                                // Restore following shot
                                if let Some((shot_id, shot_data)) = prev_shot_after {
                                    if let Some(shot) =
                                        section.shots.iter_mut().find(|s| s.id == *shot_id)
                                    {
                                        shot.azimuth_in = shot_data.azimuth_in;
                                        shot.azimuth_out = shot_data.azimuth_out;
                                        shot.length = shot_data.length;
                                        shot.depth_in = shot_data.depth_in;
                                        shot.depth_out = shot_data.depth_out;
                                    }
                                }
                            }
                            // Restore station offset
                            if let Some(key) = project.key_for_ref(*station_ref) {
                                match prev_offset {
                                    Some(offset) => {
                                        project.station_offsets.insert(key, *offset);
                                    }
                                    None => {
                                        project.station_offsets.remove(&key);
                                    }
                                }
                            }
                            // Restore section offset - through the one writer, so a
                            // legacy entry carrying a dust-zero value restores as
                            // ABSENT rather than installing a state the app cannot
                            // otherwise hold (the invariant on
                            // `Project::write_section_offset`). This arm serves
                            // persisted entries only; a live `MoveStation` records
                            // a `ProjectSnapshotPair`.
                            match prev_section_offset {
                                Some(offset) => {
                                    project.set_section_offset(station_ref.section_id, *offset);
                                }
                                None => {
                                    project.remove_section_offset(station_ref.section_id);
                                }
                            }
                            project.touch();
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    }
                }
            }

            UndoData::ProjectSnapshot(json) => {
                // Restore entire project (caches rebuilt centrally; see
                // AppState::restore_snapshot_into).
                {
                    let mut proj = self.project.lock();
                    if Self::restore_snapshot_into(&mut proj, json.as_str()) {
                        CommandResult::success()
                    } else {
                        CommandResult::failure("Failed to parse project snapshot".to_string())
                    }
                }
            }

            UndoData::ProjectSnapshotPair { before, after: _ } => {
                // Restore the "before" snapshot for undo.
                {
                    let mut proj = self.project.lock();
                    if Self::restore_snapshot_into(&mut proj, before.as_str()) {
                        CommandResult::success()
                    } else {
                        CommandResult::failure("Failed to parse project snapshot".to_string())
                    }
                }
            }

            UndoData::None => CommandResult::failure("This command cannot be undone".to_string()),
        };

        // Epilogue: push to redo stack on success, push back to undo stack on failure
        if result.success {
            // [#576 W57-NF1, RV20-6-NF1] No arm may leave a comment key that names
            // no station: the next append or prepend would give it one. Dropped
            // here, after every arm, as the open drops one (Q-W30-3-D1); a snapshot
            // arm already dropped its own in `restore_snapshot_into`.
            self.drop_orphan_comments_after_undo();
            result = self.settle_reversed_sections(&entry.command, result);
            self.undo_stack.lock().push_redo(entry);
            tracing::info!("Undo performed");
        } else {
            let reason = result.error.as_deref().unwrap_or("Unknown error");
            tracing::error!(
                command = %entry.command.description(),
                undo_data_variant = entry.undo_data.variant_name(),
                reason = reason,
                "Undo operation failed after popping entry; pushing back to undo stack"
            );
            self.undo_stack.lock().restore_undo(entry);
        }

        result
    }

    /// Perform redo - re-execute the command or restore snapshot
    pub fn redo(&self) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let _cmd_guard = self.command_lock.lock();
        // [#576 A11] Refused rather than recovered after an internal crash: the
        // inverse would land on what the crash left (`refuse_after_crash`).
        if let Err(refused) = self.refuse_after_crash("Redo") {
            return CommandResult::refused(refused);
        }
        // Read-only edit gate (#191, D-B6): gated unconditionally (see undo()).
        if let Err(f) = self.guard_mutation("Redo") {
            return CommandResult::refused(f);
        }
        self.invalidate_adjusted_cache();
        // [#576 A18(a) / NF-AN3-1] As in `undo()`: past the empty-stack check, a failed
        // step gets the same epilogue as a successful one.
        if let Err(nothing) = self.history_step_available(HistoryMove::Redo) {
            return nothing;
        }
        let result = self.redo_inner();
        // Dirty-bit flip (#191, D-B16): a redo is an edit of a remote project - and so
        // is a failed one that got this far (#576 A18(a)). Set-only; no-op for
        // local-only.
        self.mark_remote_dirty();
        self.reconcile_geometry();
        single_step_answer(result)
    }

    /// [#576 A18(a)] The "is there a step to take" check single-step undo/redo make
    /// before touching anything, mirroring `undo_to`/`redo_to`'s index validation.
    ///
    /// It draws the line the dirty bit needs: a failure HERE changed nothing, while a
    /// failure after it may have (the arms report only after running their inverse),
    /// so only failures past it mark the remote copy un-pushed. Same refusal the
    /// inner steps make (`NOTHING_TO_UNDO` / `NOTHING_TO_REDO`), so callers see no
    /// difference on an empty stack.
    fn history_step_available(&self, direction: HistoryMove) -> Result<(), CommandResult> {
        let stack = self.undo_stack.lock();
        let available = match direction {
            HistoryMove::Undo => stack.can_undo(),
            HistoryMove::Redo => stack.can_redo(),
        };
        if available {
            Ok(())
        } else {
            Err(direction.nothing_to_step())
        }
    }

    /// Redo implementation (caller must hold command_lock)
    fn redo_inner(&self) -> CommandResult {
        // Pop from redo stack
        let mut entry = {
            let mut stack = self.undo_stack.lock();

            let Some(e) = stack.pop_redo() else {
                return HistoryMove::Redo.nothing_to_step();
            };
            e
        };

        // The mirror of the undo side: the live project is this wall's `before`
        // side, so re-record it before stepping through and the undo that follows
        // puts back what this redo destroys (#413, finding 11's re-creation
        // residue). A no-op for every other variant.
        self.refresh_snapshot_pair_counterpart(&mut entry, HistoryMove::Redo);

        // For snapshot-based undo data, restore from snapshot instead of re-executing.
        // This handles cases where re-execution would create different IDs.
        // IMPORTANT: No early returns after the pop - all paths flow through the epilogue.
        let mut result = match &entry.undo_data {
            UndoData::ProjectSnapshotPair { before: _, after } => {
                let mut proj = self.project.lock();
                if Self::restore_snapshot_into(&mut proj, after.as_str()) {
                    CommandResult::success()
                } else {
                    CommandResult::failure("Failed to parse project snapshot".to_string())
                }
            }
            UndoData::CreatedSectionSnapshot {
                section_id,
                section_data,
                identity,
            } => {
                // Redo CreateSection by restoring from the snapshot, which
                // preserves the original id AND the identity `SectionData`
                // cannot carry: every shot uuid (a fresh one would dangle every
                // `StationKey::AfterShot` anchor onto this section, exactly as
                // in `CreatedShotSnapshot`), the reverse parity, and the
                // least-squares gauge pair (#413, finding 1).
                self.restore_section(*section_id, section_data.clone(), identity)
            }
            UndoData::CreatedShotSnapshot {
                section_id,
                shot_id,
                shot_data,
                position,
                after_shot_id,
                before_shot_id,
                after_shot_uuid,
                before_shot_uuid,
                uuid,
                ..
            } => {
                // Redo by re-running the SAME insert the forward command did, with
                // the original uuid forced - so the ref-shift, offset and
                // cascade are identical to the forward path and can't drift (this
                // is the single-source-of-truth that prevents the redo-misalignment
                // bug), and the shot keeps its persistent identity (a fresh uuid
                // here would dangle every AfterShot anchor). `uuid` is None for
                // shots created before this
                // field existed (legacy persisted undo entries) - falls back to a
                // fresh mint, matching the old behavior.
                // Append (no anchor) does no geometry maintenance, so a plain
                // restore is exact there.
                //
                // Every id on this entry is an IN-SESSION id and cannot be
                // trusted after a reload (#413, finding 16): the anchor decides
                // WHERE the shot goes back, and the forced id decides what the
                // recreated shot is called. Both are resolved below - anchors by
                // uuid, the forced id only when it is still free.
                let mut guard = self.project.lock();
                match guard.as_mut() {
                    None => CommandResult::failure("No project open".to_string()),
                    Some(project) => {
                        // A shot with this uuid is ALREADY in the section, so the
                        // insert would give it the same identity twice - the
                        // identity every `StationKey::AfterShot` anchor and the
                        // Ariane export resolve by, and one `ensure_unique_shot_uuids`
                        // re-mints on the next save. That is a corrupt project, so
                        // refuse loudly instead of writing it (#413, finding 16).
                        let duplicate = uuid
                            .as_deref()
                            .and_then(|u| project.shot_id_by_uuid(*section_id, u));
                        if let Some(existing_id) = duplicate {
                            CommandResult::failure(format!(
                                "Refusing to redo the creation of shot {} in section {section_id}: \
                                 the section already holds that uuid (as shot {existing_id}), so \
                                 redoing would duplicate a shot identity",
                                uuid.as_deref().unwrap_or_default()
                            ))
                        } else {
                            let resolve_anchor = |project: &crate::models::Project,
                                                  recorded_id: Option<ShotId>,
                                                  anchor_uuid: Option<&str>|
                             -> Option<ShotId> {
                                let Some(anchor_uuid) = anchor_uuid else {
                                    // Legacy entry: the recorded id is the
                                    // only address it has.
                                    return recorded_id;
                                };
                                let resolved = project.shot_id_by_uuid(*section_id, anchor_uuid);
                                if resolved.is_none() {
                                    tracing::warn!(
                                        section_id = *section_id,
                                        anchor_uuid,
                                        purpose = "redo a shot insert",
                                        "The shot this insert was anchored to is gone; \
                                             falling back to the recorded position rather than \
                                             to its stale id"
                                    );
                                }
                                resolved
                            };
                            let before_anchor = resolve_anchor(
                                project,
                                *before_shot_id,
                                before_shot_uuid.as_deref(),
                            );
                            let after_anchor =
                                resolve_anchor(project, *after_shot_id, after_shot_uuid.as_deref());
                            // Forcing an id the section already uses would give
                            // two shots one id. The id is in-session only (the
                            // loader renumbers it anyway), so minting a fresh one
                            // costs nothing that a collision would not cost more.
                            let id_taken =
                                project.get_section(*section_id).is_some_and(|section| {
                                    section.shots.iter().any(|s| s.id == *shot_id)
                                });
                            let forced_id = (!id_taken).then_some(*shot_id);
                            let ok = if let Some(before_id) = before_anchor {
                                project
                                    .insert_shot_before(
                                        *section_id,
                                        before_id,
                                        shot_data.clone(),
                                        forced_id,
                                        uuid.clone(),
                                    )
                                    .is_some()
                            } else if let Some(after_id) = after_anchor {
                                project
                                    .insert_shot_after(
                                        *section_id,
                                        after_id,
                                        shot_data.clone(),
                                        forced_id,
                                        uuid.clone(),
                                    )
                                    .is_some()
                            } else {
                                // Both anchors absent. For a CURRENT entry this is an append to an
                                // empty section (exact via a raw insert). But a LEGACY persisted
                                // entry (written by a pre-anchor build, reloaded after upgrade) has
                                // both anchors None even for a prepend / mid-section insert, so route
                                // through the proper insert helper using the captured position - which
                                // re-anchors SectionStart on a prepend and cascades offsets on a
                                // mid-insert - instead of a raw insert that would mis-place anchored
                                // tails or start-station refs. An anchor whose uuid no longer
                                // resolves lands here too, for the same reason: a recorded
                                // position is a better guess than an id that now names a
                                // different shot.
                                let section_len = project
                                    .get_section(*section_id)
                                    .map_or(0, |s| s.shots.len());
                                let pos = (*position).min(section_len);
                                if pos == 0 && section_len > 0 {
                                    // Prepend before the current first shot.
                                    let first_id = project
                                        .get_section(*section_id)
                                        .and_then(|s| s.shots.first().map(|sh| sh.id));
                                    match first_id {
                                        Some(fid) => project
                                            .insert_shot_before(
                                                *section_id,
                                                fid,
                                                shot_data.clone(),
                                                forced_id,
                                                uuid.clone(),
                                            )
                                            .is_some(),
                                        None => false,
                                    }
                                } else if pos < section_len {
                                    // Mid-section: insert after the shot currently at pos-1.
                                    let prev_id = project
                                        .get_section(*section_id)
                                        .and_then(|s| s.shots.get(pos - 1).map(|sh| sh.id));
                                    match prev_id {
                                        Some(pid) => project
                                            .insert_shot_after(
                                                *section_id,
                                                pid,
                                                shot_data.clone(),
                                                forced_id,
                                                uuid.clone(),
                                            )
                                            .is_some(),
                                        None => false,
                                    }
                                } else {
                                    // True append (pos == len) or empty section: a raw insert is
                                    // exact - no station shift or SectionStart re-anchor is needed.
                                    if let Some(section) = project.get_section_mut(*section_id) {
                                        // Through the section's mint, like the
                                        // anchored arms above (A18(d), #576):
                                        // `shot_id` arrives verbatim from a
                                        // persisted entry and nothing between the
                                        // `.stalx` and here bounds it, so it is
                                        // honoured only inside the section's id
                                        // window and while free. `ShotId::MAX`
                                        // used to park the counter on `u32::MAX`,
                                        // where the user's next append overflowed
                                        // it (#413 finding 24's class).
                                        let id = section.take_shot_id(forced_id);
                                        section.shots.insert(
                                            pos,
                                            Shot::with_uuid(
                                                id,
                                                shot_data.clone(),
                                                Some(Shot::uuid_or_mint(uuid.clone())),
                                            ),
                                        );
                                        project.reindex_section_shot_uuids(*section_id);
                                        project.touch();
                                        true
                                    } else {
                                        false
                                    }
                                }
                            };
                            if ok {
                                CommandResult::success()
                            } else {
                                CommandResult::failure("Failed to redo shot creation".to_string())
                            }
                        }
                    }
                }
            }
            UndoData::SegmentMoveInfo {
                affected_sections,
                offset,
                ..
            } => {
                // Redo by reapplying the offset to the captured affected sections
                // Filter to sections that still exist (a section may have been deleted since capture)
                {
                    let mut proj_guard = self.project.lock();
                    match proj_guard.as_mut() {
                        Some(project) => {
                            let existing: std::collections::HashSet<SectionId> =
                                project.sections.iter().map(|s| s.id).collect();
                            for sid in affected_sections {
                                if !existing.contains(sid) {
                                    continue;
                                }
                                // Through the same writer the FORWARD move uses
                                // (`adjust_section_offset`): a redo that lands the
                                // offset back within dust of the origin must drop
                                // the key, not leave an inert entry the original
                                // execute did not have (#413 finding 2, and the
                                // invariant on `write_section_offset`).
                                project.adjust_section_offset(*sid, offset.0, offset.1);
                            }
                            project.touch();
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    }
                }
            }
            UndoData::ClosureInfo { closure, .. } => {
                match &entry.command {
                    Command::CreateLoopClosure { .. } => {
                        // Redo create by restoring the captured closure to preserve its ID.
                        // Appended, as the create itself appended - the recorded
                        // `position` belongs to the REMOVE direction (#413,
                        // finding 4) and says nothing about where a create put it.
                        {
                            let mut proj_guard = self.project.lock();
                            match proj_guard.as_mut() {
                                Some(project) => {
                                    project.next_closure_id =
                                        project.next_closure_id.max(closure.id + 1);
                                    project.loop_closures.push(closure.clone());
                                    project.touch();
                                    CommandResult::success()
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            }
                        }
                    }
                    Command::RemoveLoopClosure { .. } => {
                        // Redo remove by removing the same closure ID again.
                        {
                            let mut proj_guard = self.project.lock();
                            match proj_guard.as_mut() {
                                Some(project) => {
                                    if let Some(pos) = project
                                        .loop_closures
                                        .iter()
                                        .position(|lc| lc.id == closure.id)
                                    {
                                        project.loop_closures.remove(pos);
                                    }
                                    project.touch();
                                    CommandResult::success()
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            }
                        }
                    }
                    _ => CommandResult::failure("Unsupported closure redo command".to_string()),
                }
            }
            // [#576 A7] Named rather than `_`, so a new `UndoData` variant has to
            // choose between a restore arm above and re-execution here.
            UndoData::InverseCommand(_)
            | UndoData::PreviousValues { .. }
            | UndoData::SectionSnapshot { .. }
            | UndoData::ShotSnapshot { .. }
            | UndoData::MergeInfo { .. }
            | UndoData::UnmergeInfo { .. }
            | UndoData::ZeroDatumInfo { .. }
            | UndoData::StationCommentRows { .. }
            | UndoData::StationMoveInfo { .. }
            | UndoData::ProjectSnapshot(_)
            | UndoData::None => {
                // Re-execute the original command for other undo data types.
                // The command names its shot by an in-session id, which a reload
                // renumbers, so re-address it through the uuid the entry
                // recorded first (#413, finding 12's redo half).
                match self.command_with_resolved_shot(&entry.command, &entry.undo_data) {
                    Ok(command) => self.execute_without_undo(command),
                    Err(e) => CommandResult::failure(e),
                }
            }
        };

        // Epilogue: push back to undo stack on success, push back to redo stack on failure
        // Important: Use the original entry with its original undo_data, not regenerated data
        // This ensures undo after redo works correctly
        if result.success {
            result = self.settle_reversed_sections(&entry.command, result);
            // A redo entry carrying `UndoData::None` redoes correctly (the catch-all
            // arm above just re-executes `entry.command`), which is why the redo
            // stack is NOT filtered at load. But moving it to the undo stack would
            // re-create the wedge `UndoStack::is_applicable` exists to heal: `undo()`
            // would pop it, fail, and push it straight back, sealing off everything
            // beneath. Drop it here instead - the redo itself already succeeded.
            if matches!(entry.undo_data, UndoData::None) {
                tracing::warn!(
                    command = %entry.command.description(),
                    purpose = "keep the undo stack applicable",
                    "Redone entry carries no undo data; not pushing it onto the undo stack"
                );
            } else {
                let mut stack = self.undo_stack.lock();
                stack.push_undo(entry);
            }
            tracing::info!("Redo performed");
        } else {
            let reason = result.error.as_deref().unwrap_or("Unknown error");
            tracing::error!(
                command = %entry.command.description(),
                undo_data_variant = entry.undo_data.variant_name(),
                reason = reason,
                "Redo operation failed after popping entry; pushing back to redo stack"
            );
            self.undo_stack.lock().push_redo(entry);
        }

        result
    }

    // === Keeping a re-creating entry's identity current ===

    /// Re-read the live section a `CreatedSectionSnapshot` entry re-creates, and
    /// store BOTH halves of it - the shots and their identity - on the entry
    /// (#413, finding 1).
    ///
    /// Called immediately before the undo DELETES that section, which is the
    /// last moment either half exists. It matters because a section can be
    /// healed after the entry was pushed and with no undo entry of its own -
    /// `capture_missing_gauge_anchors` runs on every save, so "create a section,
    /// save, undo, redo" would otherwise put the section back with the gauge
    /// pair discarded, and `ensure_shot_uuids` / `ensure_unique_shot_uuids` are
    /// the same shape one field over.
    ///
    /// The two halves are re-read TOGETHER because `Section::restore_with_identity`
    /// pairs them POSITIONALLY (it zips `section_data.shots` against
    /// `identity.shot_uuids`). Refreshing only the identity mixes an old shot list
    /// with current uuids, so a section whose shots grew, shrank or moved since it
    /// was created comes back as the ORIGINAL shots wearing whichever uuids now sit
    /// at those indices - the identity every `StationKey::AfterShot` anchor and the
    /// Ariane export resolve by, silently re-pointed. Re-snapshotting both makes
    /// the redo put back exactly what the undo destroyed, the same
    /// self-refreshing-wall property `refresh_snapshot_pair_counterpart` gives a
    /// `ProjectSnapshotPair`.
    ///
    /// A no-op for every other variant, and for an entry whose section is
    /// already gone (the undo is about to fail anyway), which keeps the recorded
    /// payload rather than degrading it.
    fn refresh_created_section_identity(&self, entry: &mut UndoEntry) {
        let UndoData::CreatedSectionSnapshot {
            section_id,
            section_data,
            identity,
        } = &mut entry.undo_data
        else {
            return;
        };
        let guard = self.project.lock();
        let Some(section) = guard.as_ref().and_then(|p| p.get_section(*section_id)) else {
            return;
        };
        *section_data = section.to_section_data();
        *identity = SectionIdentity::of(section);
    }

    /// Put a captured `section_offsets` slice back verbatim, `None` meaning "this
    /// section had no key" (#413, finding 27).
    ///
    /// The counterpart of `Project::capture_rigid_connected_section_offsets`, and
    /// the same restore `MergeInfo` / `ZeroDatumInfo` / `SegmentMoveInfo` already
    /// do - factored out here because the two arms that need it run their inverse
    /// first and so cannot restore inside its project lock.
    ///
    /// `Project::restore_section_offsets` routes every value through the one
    /// writer, so this cannot install a dust-zero key even from an entry recorded
    /// by a build that predates that invariant.
    fn restore_captured_section_offsets(
        &self,
        offsets: &std::collections::HashMap<SectionId, Option<(f64, f64)>>,
    ) {
        if offsets.is_empty() {
            return;
        }
        let mut guard = self.project.lock();
        let Some(project) = guard.as_mut() else {
            return;
        };
        project.restore_section_offsets(offsets);
    }

    /// Re-record the half of a `ProjectSnapshotPair` the caller is NOT about to
    /// restore, from the live project (#413, finding 11's re-creation residue).
    ///
    /// A snapshot pair is a wall: nothing above it survives a step through it, so
    /// at the moment a step is taken the live project IS the side being left
    /// behind - `after` when undoing, `before` when redoing. Re-recording that
    /// side makes the pair a SELF-REFRESHING wall: the reverse move puts back
    /// exactly the state this move destroyed, by construction, so undo and redo
    /// of a snapshot pair are exact inverses of each other no matter what
    /// happened to the live project since the entry was pushed.
    ///
    /// It exists because the recorded payload can be STALE in ways the entry
    /// cannot know about. The save-time heals are the whole class: they mutate the
    /// live project with no undo entry of their own (`apply_save_heals` -
    /// uuid mints, the gauge pair `capture_missing_gauge_anchors` captures, the
    /// orphan-offset retain), so a pair recorded before a save holds a pre-heal
    /// project on BOTH sides.
    ///
    /// `reapply_heals_after_restore` (`project_io.rs`)
    /// already covers the side being restored, but only where the restored
    /// project and the live one hold THE SAME SECTION: it is a carry-forward, and
    /// it copies anchors from a live section with the restored one's id. A pair
    /// whose halves disagree about whether a section EXISTS - a `Batch` holding a
    /// `CreateSection`, `execute_delete_shot_range_split`'s re-created tail, any
    /// delete-and-recreate - has no such live counterpart at the moment the
    /// section comes back, so the carry-forward covers nothing there. The refresh
    /// does, and covers every other heal in both directions at the same time. The
    /// two are complementary and both are kept: the refresh fixes the half being
    /// LEFT (so the next move back is exact), the carry-forward fixes the half
    /// being RESTORED (so this move does not revert a heal it never recorded).
    ///
    /// `refresh_created_section_identity` established the same pattern one variant
    /// over, for `CreatedSectionSnapshot`.
    ///
    /// COST: one project serialization per snapshot-pair history move. Not a new
    /// cost class - every command that records a pair already serializes the
    /// project twice on the forward path, and this side of the move already
    /// deserializes one - so it roughly matches what the step costs anyway.
    ///
    /// A serialization failure leaves the recorded payload alone: a stale wall is
    /// recoverable, an empty one is not.
    fn refresh_snapshot_pair_counterpart(&self, entry: &mut UndoEntry, direction: HistoryMove) {
        let UndoData::ProjectSnapshotPair { before, after } = &mut entry.undo_data else {
            return;
        };
        let guard = self.project.lock();
        let Some(project) = guard.as_ref() else {
            return;
        };
        match Self::serialize_project_snapshot(project) {
            Ok(json) => match direction {
                HistoryMove::Undo => *after = json,
                HistoryMove::Redo => *before = json,
            },
            Err(e) => {
                tracing::error!(
                    purpose = "refresh a snapshot pair before stepping through it",
                    reason = %e,
                    "Failed to serialize the live project; keeping the recorded snapshot"
                );
            }
        }
    }

    // === Addressing a shot a persisted entry names ===

    /// The LIVE id of the shot an undo entry addresses.
    ///
    /// `Shot::id` is an in-session id: the loader renumbers every shot to
    /// `section_id * FEATURE_ID_STRIDE + sequence_order`
    /// (`persistence/sqlite/read.rs`) while undo entries reload as opaque JSON,
    /// and `Section::take_shot_id` hands out ids from that same space - so a
    /// persisted id does not miss, it COLLIDES with whatever shot now occupies
    /// the slot (#413, findings 8 and 12), or runs past the range and misses
    /// everything (finding 13). `Shot::uuid` is the address that survives a
    /// save, so it is the one used whenever the entry carries it.
    ///
    /// A recorded uuid that no longer resolves is an ERROR rather than a
    /// fallback to the id: the id is exactly the thing that cannot be trusted,
    /// and hitting the wrong shot silently is the defect this exists to
    /// prevent. `UndoStack::drop_entries_addressing_a_missing_shot` drops such
    /// entries at load so the failure costs one undo step rather than wedging
    /// the history under it.
    fn resolve_undo_shot(
        &self,
        section_id: SectionId,
        uuid: Option<&str>,
        recorded_id: ShotId,
    ) -> Result<ShotId, String> {
        // A legacy entry (written before the uuid was recorded) has nothing but
        // the id, so the id is what it gets - the pre-fix behavior.
        let Some(uuid) = uuid else {
            return Ok(recorded_id);
        };
        let guard = self.project.lock();
        let Some(project) = guard.as_ref() else {
            return Err("No project open".to_string());
        };
        // Spelled distinctly from the id-addressed "Shot {id} not found in
        // section {s}": this one says the shot's persistent IDENTITY is gone,
        // which is a different fact from an id that no longer resolves, and the
        // fuzz gate tells the two apart by exactly this wording.
        project.shot_id_by_uuid(section_id, uuid).ok_or_else(|| {
            format!("Shot {uuid} (addressed by uuid) not found in section {section_id}")
        })
    }

    /// `command` with the shot it addresses re-resolved through the uuid its
    /// undo entry recorded, so a redo edits the shot the forward command edited.
    ///
    /// A no-op for a command that names no shot, and for a legacy entry that
    /// records no uuid.
    fn command_with_resolved_shot(
        &self,
        command: &Command,
        undo_data: &UndoData,
    ) -> Result<Command, String> {
        let Some((target_section, Some(uuid), recorded_id)) = undo_data.shot_target() else {
            return Ok(command.clone());
        };
        let mut resolved = command.clone();
        match &mut resolved {
            Command::UpdateShot {
                section_id,
                shot_id,
                ..
            }
            | Command::SetShotKind {
                section_id,
                shot_id,
                ..
            } if *section_id == target_section && *shot_id == recorded_id => {
                *shot_id = self.resolve_undo_shot(target_section, Some(uuid), recorded_id)?;
            }
            _ => {}
        }
        Ok(resolved)
    }

    // === State queries ===

    /// Check if undo is available
    pub fn can_undo(&self) -> bool {
        self.undo_stack.lock().can_undo()
    }

    /// Check if redo is available
    pub fn can_redo(&self) -> bool {
        self.undo_stack.lock().can_redo()
    }

    /// Get the undo stack as a list of history items for the UI
    pub fn get_undo_stack(&self) -> Vec<crate::core::undo::history_view::UndoHistoryItem> {
        // Build a lightweight section name lookup map, then release the project lock
        // before acquiring the undo_stack lock (avoids holding both simultaneously)
        let section_names = self.history_section_names();
        self.undo_stack
            .lock()
            .undo_entries()
            .enumerate()
            .map(|(idx, entry)| {
                crate::core::undo::history_view::entry_to_history_item(entry, idx, &section_names)
            })
            .collect()
    }

    /// Section id -> name for the history views, read under its own short project
    /// lock so neither view holds `project` and `undo_stack` at once.
    fn history_section_names(&self) -> std::collections::HashMap<u32, Option<String>> {
        self.project
            .lock()
            .as_ref()
            .map(|p| p.sections.iter().map(|s| (s.id, s.name.clone())).collect())
            .unwrap_or_default()
    }

    /// Get the redo stack as a list of history items for the UI
    pub fn get_redo_stack(&self) -> Vec<crate::core::undo::history_view::UndoHistoryItem> {
        let section_names = self.history_section_names();
        self.undo_stack
            .lock()
            .redo_entries()
            .enumerate()
            .map(|(idx, entry)| {
                crate::core::undo::history_view::entry_to_history_item(entry, idx, &section_names)
            })
            .collect()
    }

    /// Perform multiple undos to reach a specific history point
    /// Index 0 = most recent action, so undo_to(N) executes (N+1) undos
    pub fn undo_to(&self, index: usize) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let _cmd_guard = self.command_lock.lock();
        // [#576 A11] Refused rather than recovered after an internal crash: the
        // inverse would land on what the crash left (`refuse_after_crash`).
        if let Err(refused) = self.refuse_after_crash("Undo") {
            return CommandResult::refused(refused);
        }
        // Read-only edit gate (#191, D-B6): gated unconditionally (see undo()).
        if let Err(f) = self.guard_mutation("Undo") {
            return CommandResult::refused(f);
        }

        // Validate index first
        {
            let stack = self.undo_stack.lock();
            if index >= stack.len() {
                return CommandResult::failure(format!(
                    "Invalid undo index: {} (stack has {} entries)",
                    index,
                    stack.len()
                ));
            }
        }

        // Execute (index + 1) undos to include the target index
        let count = index + 1;
        // An ORDERED PER-STEP LOG: concatenated in step order, repeats intact and
        // deliberately NOT parity-reduced. Each entry carries the shot count in
        // force at ITS step, which can differ between steps of one range (a
        // `CreateShot` undone between two reverses of the same section), so the
        // consumer composes them rather than cancelling repeats. Dropping them
        // would leave the gas plan and the distance origin on the mirrored
        // stations after a multi-step undo.
        let mut reversed: Vec<ReversedSection> = Vec::new();
        for i in 0..count {
            let result = self.undo_inner();
            if !result.success {
                // The earlier steps ALREADY flipped their sections and cannot be
                // rolled back, so the failure has to carry what was applied -
                // otherwise the frontend's positional refs are left mirrored with
                // nothing on the wire to say so.
                let failure = history_step_failed(
                    format!(
                        "Undo failed at step {} of {}: {}",
                        i + 1,
                        count,
                        result.error.unwrap_or_else(|| "Unknown error".to_string())
                    ),
                    i + 1,
                    count,
                );
                // The FAILED step can have mutated too: an arm reports its
                // failure only after running its inverse, and the inverse
                // mutates as it goes (`remove_created_shot` ->
                // `Project::remove_shot_with_rewire` drops the removed station's
                // offsets and rewires closures and merges before it can report).
                // The user is left holding that state, so it gets the same
                // reconcile every completed step gets.
                self.reconcile_geometry();
                // [#576 A18(a)] ...and the same dirty flip the success path makes
                // after the loop: the steps before this one changed the project, and
                // this one may have. Returning before the flip left a SpeleoDB copy
                // marked clean, so Done skipped the upload and released, and a re-pull
                // could replace the copy without asking.
                self.mark_remote_dirty();
                return if reversed.is_empty() {
                    failure
                } else {
                    failure.with_reversed_sections(reversed)
                };
            }
            if let Some(entries) = result.reversed_sections {
                reversed.extend(entries);
            }
            // The SAME epilogue single-step `undo()` runs, after EVERY step, so
            // that a jump lands where the equivalent number of single undos
            // lands - which is the contract the history dropdown makes and which
            // `undo_to` was breaking by looping `undo_inner` bare (#413,
            // finding 7). `reconcile_geometry` is what re-derives offsets for a
            // merge a raw restore left drifted past the alignment invariant.
            //
            // PER STEP, not once after the loop: an intermediate state is not
            // one the user ever SEES, but it is one the steps above it READ.
            // The arms that re-derive rather than restore read the live offsets
            // at the moment they run (`PreviousValues` -> `update_shot` ->
            // `realign_after_shot_edit`, `CreatedShotSnapshot` ->
            // `remove_created_shot`), so a drifted intermediate state is an
            // input, and a jump that carries one lands somewhere the single
            // presses do not. The cost of asking is trivial in the common case:
            // `realign_merges` returns immediately unless some merge is beyond
            // the 1 cm invariant, which a state a step just restored normally is
            // not.
            // (`invalidate_adjusted_cache` is deliberately not here: the
            // adjusted cache keys on `project.modified_at`, which every undo
            // bumps through `Project::touch`, so the next read misses and
            // re-solves regardless - measured in the finding's write-up.)
            self.reconcile_geometry();
        }

        // Dirty-bit flip (#191, D-B16): undo_to ran one or more undos. Set-only;
        // no-op for local-only.
        self.mark_remote_dirty();
        tracing::info!(index = index, count = count, "Undo to index completed");
        if reversed.is_empty() {
            CommandResult::success()
        } else {
            CommandResult::success().with_reversed_sections(reversed)
        }
    }

    /// Perform multiple redos to reach a specific history point
    /// Index 0 = most recent undone action, so redo_to(N) executes (N+1) redos
    pub fn redo_to(&self, index: usize) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let _cmd_guard = self.command_lock.lock();
        // [#576 A11] Refused rather than recovered after an internal crash: the
        // inverse would land on what the crash left (`refuse_after_crash`).
        if let Err(refused) = self.refuse_after_crash("Redo") {
            return CommandResult::refused(refused);
        }
        // Read-only edit gate (#191, D-B6): gated unconditionally (see undo()).
        if let Err(f) = self.guard_mutation("Redo") {
            return CommandResult::refused(f);
        }

        // Validate index first
        {
            let stack = self.undo_stack.lock();
            if index >= stack.redo_len() {
                return CommandResult::failure(format!(
                    "Invalid redo index: {} (stack has {} entries)",
                    index,
                    stack.redo_len()
                ));
            }
        }

        // Execute (index + 1) redos to include the target index
        let count = index + 1;
        // An ordered per-step log, repeats intact and not parity-reduced - see
        // `undo_to`.
        let mut reversed: Vec<ReversedSection> = Vec::new();
        for i in 0..count {
            let result = self.redo_inner();
            if !result.success {
                // Same reasoning as `undo_to`: the steps that already ran are not
                // rolled back, so the failure carries their entries - and the
                // failed step's own partial mutation is reconciled with them.
                let failure = history_step_failed(
                    format!(
                        "Redo failed at step {} of {}: {}",
                        i + 1,
                        count,
                        result.error.unwrap_or_else(|| "Unknown error".to_string())
                    ),
                    i + 1,
                    count,
                );
                self.reconcile_geometry();
                // [#576 A18(a)] As in `undo_to`: past the index check, mark the copy.
                self.mark_remote_dirty();
                return if reversed.is_empty() {
                    failure
                } else {
                    failure.with_reversed_sections(reversed)
                };
            }
            if let Some(entries) = result.reversed_sections {
                reversed.extend(entries);
            }
            // The same epilogue single-step `redo()` runs, after every step -
            // see `undo_to` for why it is per step and not once after the loop
            // (#413, finding 7).
            self.reconcile_geometry();
        }

        // Dirty-bit flip (#191, D-B16): redo_to ran one or more redos. Set-only;
        // no-op for local-only.
        self.mark_remote_dirty();
        tracing::info!(index = index, count = count, "Redo to index completed");
        if reversed.is_empty() {
            CommandResult::success()
        } else {
            CommandResult::success().with_reversed_sections(reversed)
        }
    }
}
