use crate::core::{Command, CommandResult, ReversedSection, UndoData, UndoEntry};
use crate::models::{SectionId, SectionIdentity, SectionUpdates, Shot, ShotId, ShotUpdates};

use super::AppState;

/// Which way through the history a step is going, for the two paths that have to
/// treat the two halves of a `ProjectSnapshotPair` asymmetrically.
#[derive(Clone, Copy)]
enum HistoryMove {
    Undo,
    Redo,
}

impl AppState {
    // === Project operations ===

    /// Perform undo - restore previous state based on undo data
    pub fn undo(&self) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): you cannot mutate a read-only remote
        // project even to revert. Gated unconditionally.
        if let Err(f) = self.guard_mutation("Undo") {
            return f;
        }
        self.invalidate_adjusted_cache();
        let result = self.undo_inner();
        if result.success {
            // Dirty-bit flip (#191, D-B16): a successful undo is an edit of a
            // remote project. Set-only; no-op for local-only.
            self.mark_remote_dirty();
            // Reconcile derived offsets after the restore. For snapshot-restore
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
        }
        result
    }

    /// Undo implementation (caller must hold command_lock)
    fn undo_inner(&self) -> CommandResult {
        // Pop from undo stack
        let mut entry = {
            let Ok(mut stack) = self.undo_stack.lock() else {
                return CommandResult::failure("Failed to lock undo stack".to_string());
            };

            let Some(e) = stack.pop_undo() else {
                return CommandResult::failure("Nothing to undo".to_string());
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
        // for every other variant. What that side recorded is read first: a
        // gauge pair it already held is the edit's own, not a later save's.
        let edited = Self::recorded_counterpart_gauges(&entry, HistoryMove::Undo);
        self.refresh_snapshot_pair_counterpart(&mut entry, HistoryMove::Undo);
        // The same self-refresh for an add (#674): re-read what it brought off
        // the live project, so the redo reinstalls what this undo removes.
        self.refresh_added_content(&mut entry);

        // Apply undo based on undo_data type
        // IMPORTANT: No early returns after this point - all paths must flow
        // through the epilogue so the entry is either pushed to redo (success)
        // or pushed back to undo (failure).
        let mut result = match &entry.undo_data {
            // A station name's inverse restores the recorded value verbatim
            // instead of re-running the sanitizing forward setter, which would
            // trim a name a file stored untrimmed.
            UndoData::InverseCommand(Command::SetStationName { station_ref, name }) => {
                self.restore_station_name(*station_ref, name.clone())
            }
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

            UndoData::ShotCredits {
                changes,
                section_values,
            } => self.undo_shot_credits(changes, section_values.as_ref()),

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
                consumed_legacy,
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
                            // Shot-less again: the section keeps the legacy
                            // pair its first shot took.
                            if let Some(pair) = consumed_legacy {
                                self.restore_consumed_legacy(*section_id, pair);
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
                match self.project.lock() {
                    Ok(mut proj_guard) => match proj_guard.as_mut() {
                        Some(project) => {
                            // Restore section offsets to pre-merge positions
                            project.restore_section_offsets(previous_offsets);

                            // Then unmerge the stations
                            project.unmerge_stations(*station_a, *station_b);
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    },
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
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
                        if let Ok(mut proj_guard) = self.project.lock() {
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
                    match self.project.lock() {
                        Ok(mut proj_guard) => match proj_guard.as_mut() {
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
                        },
                        Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                    }
                }
            }

            UndoData::ClosureInfo { closure, position } => {
                match &entry.command {
                    Command::CreateLoopClosure { .. } => {
                        // Undo create by removing the same closure ID.
                        match self.project.lock() {
                            Ok(mut proj_guard) => match proj_guard.as_mut() {
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
                            },
                            Err(_) => CommandResult::failure("Failed to lock project".to_string()),
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
                        match self.project.lock() {
                            Ok(mut proj_guard) => match proj_guard.as_mut() {
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
                            },
                            Err(_) => CommandResult::failure("Failed to lock project".to_string()),
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
                match self.project.lock() {
                    Ok(mut proj_guard) => match proj_guard.as_mut() {
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
                    },
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                }
            }

            UndoData::SegmentMoveInfo {
                previous_offsets, ..
            } => {
                // Restore previous offsets for each affected section
                match self.project.lock() {
                    Ok(mut proj_guard) => match proj_guard.as_mut() {
                        Some(project) => {
                            project.restore_section_offsets(previous_offsets);
                            project.touch();
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    },
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
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
                match self.project.lock() {
                    Ok(mut proj_guard) => match proj_guard.as_mut() {
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
                                    crate::models::Project::write_section_offset(
                                        &mut project.section_offsets,
                                        station_ref.section_id,
                                        *offset,
                                    );
                                }
                                None => {
                                    project.section_offsets.remove(&station_ref.section_id);
                                }
                            }
                            project.touch();
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    },
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                }
            }

            UndoData::ProjectSnapshot(json) => {
                // Restore entire project (caches rebuilt centrally; see
                // AppState::restore_snapshot_into).
                match self.project.lock() {
                    Ok(mut proj) => {
                        if Self::restore_snapshot_into(&mut proj, json.as_str()) {
                            CommandResult::success()
                        } else {
                            CommandResult::failure("Failed to parse project snapshot".to_string())
                        }
                    }
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                }
            }

            UndoData::ProjectSnapshotPair { before, after: _ } => {
                // Restore the "before" snapshot for undo.
                match self.project.lock() {
                    Ok(mut proj) => {
                        if Self::restore_snapshot_into_with(
                            &mut proj,
                            before.as_str(),
                            edited.as_ref(),
                        ) {
                            CommandResult::success()
                        } else {
                            CommandResult::failure("Failed to parse project snapshot".to_string())
                        }
                    }
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                }
            }

            UndoData::AddedProjectContent(content) => {
                // Remove exactly what the add brought, guard-free: an inverse
                // consults no user-intent guard. Its saved ignores stay on disk
                // (see `remove_pending_ignores`).
                let mut guard = self
                    .project
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match guard.as_mut() {
                    Some(project) => {
                        content.remove_from(project);
                        self.remove_pending_ignores(&content.section_ids());
                        CommandResult::success()
                    }
                    None => CommandResult::failure("No project open".to_string()),
                }
            }

            UndoData::None => CommandResult::failure("This command cannot be undone".to_string()),
        };

        // Epilogue: push to redo stack on success, push back to undo stack on failure
        if result.success {
            result = self.settle_reversed_sections(&entry.command, result);
            if let Ok(mut stack) = self.undo_stack.lock() {
                stack.push_redo(entry);
            }
            tracing::info!("Undo performed");
        } else {
            let reason = result.error.as_deref().unwrap_or("Unknown error");
            tracing::error!(
                command = %entry.command.description(),
                undo_data_variant = entry.undo_data.variant_name(),
                reason = reason,
                "Undo operation failed after popping entry; pushing back to undo stack"
            );
            match self.undo_stack.lock() {
                Ok(mut stack) => stack.push_undo(entry),
                Err(_) => {
                    tracing::error!("Failed to re-acquire undo stack lock to push back entry");
                }
            }
        }

        result
    }

    /// Perform redo - re-execute the command or restore snapshot
    pub fn redo(&self) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated unconditionally (see undo()).
        if let Err(f) = self.guard_mutation("Redo") {
            return f;
        }
        self.invalidate_adjusted_cache();
        let result = self.redo_inner();
        if result.success {
            // Dirty-bit flip (#191, D-B16): a successful redo is an edit of a
            // remote project. Set-only; no-op for local-only.
            self.mark_remote_dirty();
            self.reconcile_geometry();
        }
        result
    }

    /// Redo implementation (caller must hold command_lock)
    fn redo_inner(&self) -> CommandResult {
        // Pop from redo stack
        let mut entry = {
            let Ok(mut stack) = self.undo_stack.lock() else {
                return CommandResult::failure("Failed to lock undo stack".to_string());
            };

            let Some(e) = stack.pop_redo() else {
                return CommandResult::failure("Nothing to redo".to_string());
            };
            e
        };

        // The mirror of the undo side: the live project is this wall's `before`
        // side, so re-record it before stepping through and the undo that follows
        // puts back what this redo destroys (#413, finding 11's re-creation
        // residue). A no-op for every other variant.
        let edited = Self::recorded_counterpart_gauges(&entry, HistoryMove::Redo);
        self.refresh_snapshot_pair_counterpart(&mut entry, HistoryMove::Redo);

        // For snapshot-based undo data, restore from snapshot instead of re-executing.
        // This handles cases where re-execution would create different IDs.
        // IMPORTANT: No early returns after the pop - all paths flow through the epilogue.
        let mut result = match &entry.undo_data {
            UndoData::ProjectSnapshotPair { before: _, after } => match self.project.lock() {
                Ok(mut proj) => {
                    if Self::restore_snapshot_into_with(&mut proj, after.as_str(), edited.as_ref())
                    {
                        CommandResult::success()
                    } else {
                        CommandResult::failure("Failed to parse project snapshot".to_string())
                    }
                }
                Err(_) => CommandResult::failure("Failed to lock project".to_string()),
            },
            // Reinstall the recorded content verbatim - same section, shot and
            // closure ids - never re-reading the file, which a reloaded entry
            // does not even name (#674).
            UndoData::AddedProjectContent(content) => {
                let mut guard = self
                    .project
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match guard.as_mut() {
                    Some(project) => match content.install_into(project) {
                        Ok(()) => {
                            self.add_pending_ignores(project, &content.ignores);
                            CommandResult::success()
                        }
                        Err(e) => CommandResult::failure(e),
                    },
                    None => CommandResult::failure("No project open".to_string()),
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
                let mut guard = match self.project.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
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
                                        // SATURATING, because `shot_id` arrives
                                        // verbatim from a persisted entry and
                                        // nothing between the `.stalx` and here
                                        // bounds it: `ShotId::MAX` panics a debug
                                        // build on the `+ 1` and wraps a release
                                        // one, after which `Section::take_shot_id`
                                        // hands the wrapped value straight back
                                        // out as a colliding id. Total for every
                                        // value the type admits, exactly as
                                        // `shot_display_position` is for the same
                                        // reason (#413 finding 24).
                                        //
                                        // Monotonicity is preserved without a
                                        // `max`: the assignment is guarded by
                                        // `id >= next_shot_id`, so the saturated
                                        // `ShotId::MAX` is still >= the counter it
                                        // replaces.
                                        let id = forced_id.unwrap_or_else(|| {
                                            let next = section.next_shot_id;
                                            section.next_shot_id = next.saturating_add(1);
                                            next
                                        });
                                        if id >= section.next_shot_id {
                                            section.next_shot_id = id.saturating_add(1);
                                        }
                                        section.shots.insert(
                                            pos,
                                            Shot::with_uuid(
                                                id,
                                                shot_data.clone(),
                                                Some(Shot::uuid_or_mint(uuid.clone())),
                                            ),
                                        );
                                        project.touch();
                                        true
                                    } else {
                                        false
                                    }
                                }
                            };
                            if ok {
                                // The re-created first shot carries the pair as
                                // its credit again; the section lets it go.
                                if let Some(section) = project.get_section_mut(*section_id) {
                                    section.legacy_surveyor = None;
                                    section.legacy_date = None;
                                }
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
                match self.project.lock() {
                    Ok(mut proj_guard) => match proj_guard.as_mut() {
                        Some(project) => {
                            let existing: std::collections::HashSet<SectionId> =
                                project.sections.iter().map(|s| s.id).collect();
                            for sid in affected_sections {
                                if !existing.contains(sid) {
                                    continue;
                                }
                                let current = project
                                    .section_offsets
                                    .get(sid)
                                    .copied()
                                    .unwrap_or((0.0, 0.0));
                                let new_offset = (current.0 + offset.0, current.1 + offset.1);
                                // Through the same writer the FORWARD move uses
                                // (`adjust_section_offset`): a redo that lands the
                                // offset back within dust of the origin must drop
                                // the key, not leave an inert entry the original
                                // execute did not have (#413 finding 2, and the
                                // invariant on `write_section_offset`).
                                crate::models::Project::write_section_offset(
                                    &mut project.section_offsets,
                                    *sid,
                                    new_offset,
                                );
                            }
                            project.touch();
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    },
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                }
            }
            UndoData::ClosureInfo { closure, .. } => {
                match &entry.command {
                    Command::CreateLoopClosure { .. } => {
                        // Redo create by restoring the captured closure to preserve its ID.
                        // Appended, as the create itself appended - the recorded
                        // `position` belongs to the REMOVE direction (#413,
                        // finding 4) and says nothing about where a create put it.
                        match self.project.lock() {
                            Ok(mut proj_guard) => match proj_guard.as_mut() {
                                Some(project) => {
                                    project.next_closure_id =
                                        project.next_closure_id.max(closure.id + 1);
                                    project.loop_closures.push(closure.clone());
                                    project.touch();
                                    CommandResult::success()
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            },
                            Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                        }
                    }
                    Command::RemoveLoopClosure { .. } => {
                        // Redo remove by removing the same closure ID again.
                        match self.project.lock() {
                            Ok(mut proj_guard) => match proj_guard.as_mut() {
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
                            },
                            Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                        }
                    }
                    _ => CommandResult::failure("Unsupported closure redo command".to_string()),
                }
            }
            // An `EditShotCredit` names explicit shots by in-session id, which a
            // reload renumbers - so its redo writes the recorded `after` credits
            // by uuid instead of re-executing. (A section-metadata edit re-executes
            // below: it names its section by the id that survives a save.)
            UndoData::ShotCredits { changes, .. }
                if matches!(entry.command, Command::EditShotCredit { .. }) =>
            {
                self.write_recorded_credits(changes, true)
            }
            _ => {
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
            } else if let Ok(mut stack) = self.undo_stack.lock() {
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
            match self.undo_stack.lock() {
                Ok(mut stack) => stack.push_redo(entry),
                Err(_) => {
                    tracing::error!("Failed to re-acquire undo stack lock to push back entry");
                }
            }
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
        let guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(section) = guard.as_ref().and_then(|p| p.get_section(*section_id)) else {
            return;
        };
        *section_data = section.to_section_data();
        *identity = SectionIdentity::of(section);
    }

    /// Re-record an add's content from the live project immediately before its
    /// undo removes it (#674) - `refresh_created_section_identity` for a whole
    /// add. Keeps the recorded payload when any of its sections is already gone,
    /// rather than degrading it to what little is left.
    fn refresh_added_content(&self, entry: &mut UndoEntry) {
        let UndoData::AddedProjectContent(content) = &mut entry.undo_data else {
            return;
        };
        let guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(project) = guard.as_ref() else {
            return;
        };
        let ids = content.section_ids();
        if !ids.iter().all(|id| project.get_section(*id).is_some()) {
            return;
        }
        content.refresh_from(project);
        content.ignores = self.ignores_on_sections(&ids);
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
        let mut guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(project) = guard.as_mut() else {
            return;
        };
        project.restore_section_offsets(offsets);
    }

    /// The gauge pairs the half of a snapshot pair this step LEAVES BEHIND
    /// recorded (`after` when undoing, `before` when redoing), read before
    /// [`Self::refresh_snapshot_pair_counterpart`] overwrites it with the live
    /// project. A live pair that is one of these is the edit's own, which the
    /// restore's save-heal carry-forward must not copy onto the other half
    /// (`reapply_heals_after_restore`). `None` for every other variant.
    fn recorded_counterpart_gauges(
        entry: &UndoEntry,
        direction: HistoryMove,
    ) -> Option<super::project_io::RecordedGauges> {
        let UndoData::ProjectSnapshotPair { before, after } = &entry.undo_data else {
            return None;
        };
        super::project_io::recorded_gauge_pairs(match direction {
            HistoryMove::Undo => after,
            HistoryMove::Redo => before,
        })
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
        let guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
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
        let guard = match self.project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
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
        self.undo_stack
            .lock()
            .map(|s| s.can_undo())
            .unwrap_or(false)
    }

    /// Check if redo is available
    pub fn can_redo(&self) -> bool {
        self.undo_stack
            .lock()
            .map(|s| s.can_redo())
            .unwrap_or(false)
    }

    /// Get the undo stack as a list of history items for the UI
    pub fn get_undo_stack(&self) -> Vec<crate::core::undo::history_view::UndoHistoryItem> {
        // Build a lightweight section name lookup map, then release the project lock
        // before acquiring the undo_stack lock (avoids holding both simultaneously)
        let section_names: std::collections::HashMap<u32, Option<String>> = self
            .project
            .lock()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|p| p.sections.iter().map(|s| (s.id, s.name.clone())).collect())
            })
            .unwrap_or_default();

        self.undo_stack
            .lock()
            .map(|stack| {
                stack
                    .undo_entries()
                    .enumerate()
                    .map(|(idx, entry)| {
                        crate::core::undo::history_view::entry_to_history_item(
                            entry,
                            idx,
                            &section_names,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the redo stack as a list of history items for the UI
    pub fn get_redo_stack(&self) -> Vec<crate::core::undo::history_view::UndoHistoryItem> {
        let section_names: std::collections::HashMap<u32, Option<String>> = self
            .project
            .lock()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|p| p.sections.iter().map(|s| (s.id, s.name.clone())).collect())
            })
            .unwrap_or_default();

        self.undo_stack
            .lock()
            .map(|stack| {
                stack
                    .redo_entries()
                    .enumerate()
                    .map(|(idx, entry)| {
                        crate::core::undo::history_view::entry_to_history_item(
                            entry,
                            idx,
                            &section_names,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Perform multiple undos to reach a specific history point
    /// Index 0 = most recent action, so undo_to(N) executes (N+1) undos
    pub fn undo_to(&self, index: usize) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated unconditionally (see undo()).
        if let Err(f) = self.guard_mutation("Undo") {
            return f;
        }

        // Validate index first
        {
            let Ok(stack) = self.undo_stack.lock() else {
                return CommandResult::failure("Failed to lock undo stack".to_string());
            };
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
                let failure = CommandResult::failure(format!(
                    "Undo failed at step {} of {}: {}",
                    i + 1,
                    count,
                    result.error.unwrap_or_else(|| "Unknown error".to_string())
                ));
                // The FAILED step can have mutated too: an arm reports its
                // failure only after running its inverse, and the inverse
                // mutates as it goes (`remove_created_shot` ->
                // `Project::remove_shot_with_rewire` drops the removed station's
                // offsets and rewires closures and merges before it can report).
                // The user is left holding that state, so it gets the same
                // reconcile every completed step gets.
                self.reconcile_geometry();
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
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated unconditionally (see undo()).
        if let Err(f) = self.guard_mutation("Redo") {
            return f;
        }

        // Validate index first
        {
            let Ok(stack) = self.undo_stack.lock() else {
                return CommandResult::failure("Failed to lock undo stack".to_string());
            };
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
                let failure = CommandResult::failure(format!(
                    "Redo failed at step {} of {}: {}",
                    i + 1,
                    count,
                    result.error.unwrap_or_else(|| "Unknown error".to_string())
                ));
                self.reconcile_geometry();
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
