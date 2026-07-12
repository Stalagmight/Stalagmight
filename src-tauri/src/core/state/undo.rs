use crate::core::{Command, CommandResult, UndoData};
use crate::models::{SectionId, SectionUpdates, Shot, ShotUpdates};

use super::AppState;

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
            // valid aligned state, so realign_merges early-returns (no-op) — EXCEPT
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
        let entry = {
            let Ok(mut stack) = self.undo_stack.lock() else {
                return CommandResult::failure("Failed to lock undo stack".to_string());
            };

            let Some(e) = stack.pop_undo() else {
                return CommandResult::failure("Nothing to undo".to_string());
            };
            e
        };

        // Apply undo based on undo_data type
        // IMPORTANT: No early returns after this point — all paths must flow
        // through the epilogue so the entry is either pushed to redo (success)
        // or pushed back to undo (failure).
        let result = match &entry.undo_data {
            UndoData::InverseCommand(inverse_cmd) => {
                // Execute the inverse command (without pushing to undo stack)
                self.execute_without_undo(inverse_cmd.clone())
            }

            UndoData::PreviousValues {
                section_id,
                shot_id,
                values_json,
            } => {
                // Restore previous values
                if let Some(shot_id) = shot_id {
                    // Restore shot values
                    if let Ok(prev_updates) =
                        serde_json::from_str::<ShotUpdates>(values_json.as_str())
                    {
                        self.update_shot(*section_id, *shot_id, prev_updates)
                    } else {
                        CommandResult::failure("Failed to parse undo data".to_string())
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
                // Restore deleted section
                self.restore_section(*section_id, section_data.clone())
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
                ..
            } => {
                // Undo a CreateShot by deleting the shot
                self.delete_shot(*section_id, *shot_id)
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
                                    project.station_merges.push(crate::models::StationMerge {
                                        station_a: key_a,
                                        station_b: key_b,
                                        merged_at: chrono::Utc::now(),
                                    });
                                }
                                // Direct push bypassed Project::record_station_merge,
                                // so the adjacency cache is stale. Drop it; the next
                                // BFS will fall back to the linear scan until rebuilt.
                                project.merge_adjacency_cache.clear();
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

            UndoData::ClosureInfo { closure } => {
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
                        // Undo remove by restoring the captured closure.
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
                    _ => CommandResult::failure("Unsupported closure undo command".to_string()),
                }
            }

            UndoData::ZeroDatumInfo {
                station_ref,
                previous_gps,
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

                            // Restore GPS datum state
                            if let Some(gps) = previous_gps {
                                if let Some(existing) = project
                                    .zero_datums
                                    .iter_mut()
                                    .find(|zd| Some(&zd.station_ref) == datum_key.as_ref())
                                {
                                    existing.gps = *gps;
                                } else if let Some(key) = datum_key {
                                    project.zero_datums.push(crate::models::ZeroDatum {
                                        station_ref: key,
                                        gps: *gps,
                                    });
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
                            // Restore section offset
                            match prev_section_offset {
                                Some(offset) => {
                                    project
                                        .section_offsets
                                        .insert(station_ref.section_id, *offset);
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
                        if Self::restore_snapshot_into(&mut proj, before.as_str()) {
                            CommandResult::success()
                        } else {
                            CommandResult::failure("Failed to parse project snapshot".to_string())
                        }
                    }
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                }
            }

            UndoData::MarkerInfo { marker } => {
                match &entry.command {
                    Command::AddMarker { .. } => {
                        // Undo add by removing the marker.
                        match self.project.lock() {
                            Ok(mut proj_guard) => match proj_guard.as_mut() {
                                Some(project) => {
                                    project.markers.retain(|m| m.id != marker.id);
                                    project.touch();
                                    CommandResult::success()
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            },
                            Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                        }
                    }
                    Command::UpdateMarker { .. } => {
                        // Undo update by restoring the captured old state.
                        match self.project.lock() {
                            Ok(mut proj_guard) => match proj_guard.as_mut() {
                                Some(project) => {
                                    if let Some(m) =
                                        project.markers.iter_mut().find(|m| m.id == marker.id)
                                    {
                                        *m = marker.clone();
                                    }
                                    project.touch();
                                    CommandResult::success()
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            },
                            Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                        }
                    }
                    Command::DeleteMarker { .. } => {
                        // Undo delete by restoring the captured marker.
                        match self.project.lock() {
                            Ok(mut proj_guard) => match proj_guard.as_mut() {
                                Some(project) => {
                                    project.next_marker_id =
                                        project.next_marker_id.max(marker.id + 1);
                                    project.markers.push(marker.clone());
                                    project.touch();
                                    CommandResult::success()
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            },
                            Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                        }
                    }
                    _ => CommandResult::failure("Unsupported marker undo command".to_string()),
                }
            }

            UndoData::None => CommandResult::failure("This command cannot be undone".to_string()),
        };

        // Epilogue: push to redo stack on success, push back to undo stack on failure
        if result.success {
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
        let entry = {
            let Ok(mut stack) = self.undo_stack.lock() else {
                return CommandResult::failure("Failed to lock undo stack".to_string());
            };

            let Some(e) = stack.pop_redo() else {
                return CommandResult::failure("Nothing to redo".to_string());
            };
            e
        };

        // For snapshot-based undo data, restore from snapshot instead of re-executing.
        // This handles cases where re-execution would create different IDs.
        // IMPORTANT: No early returns after the pop — all paths flow through the epilogue.
        let result = match &entry.undo_data {
            UndoData::ProjectSnapshotPair { before: _, after } => match self.project.lock() {
                Ok(mut proj) => {
                    if Self::restore_snapshot_into(&mut proj, after.as_str()) {
                        CommandResult::success()
                    } else {
                        CommandResult::failure("Failed to parse project snapshot".to_string())
                    }
                }
                Err(_) => CommandResult::failure("Failed to lock project".to_string()),
            },
            UndoData::CreatedSectionSnapshot {
                section_id,
                section_data,
            } => {
                // Redo CreateSection by restoring from snapshot to preserve original ID
                self.restore_section(*section_id, section_data.clone())
            }
            UndoData::CreatedShotSnapshot {
                section_id,
                shot_id,
                shot_data,
                position,
                after_shot_id,
                before_shot_id,
                uuid,
                ..
            } => {
                // Redo by re-running the SAME insert the forward command did, with
                // the original id AND uuid forced — so the ref-shift, offset and
                // cascade are identical to the forward path and can't drift (this
                // is the single-source-of-truth that prevents the redo-misalignment
                // bug), and the shot keeps its stable collaboration identity (a
                // fresh uuid here would dangle every AfterShot anchor and break the
                // Ariane tier-1 match). `uuid` is None for shots created before this
                // field existed (legacy persisted undo entries) — falls back to a
                // fresh mint, matching the old behavior.
                // Append (no anchor) does no geometry maintenance, so a plain
                // restore is exact there.
                let mut guard = match self.project.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                let Some(project) = guard.as_mut() else {
                    return CommandResult::failure("No project open".to_string());
                };
                let ok = if let Some(before_id) = before_shot_id {
                    project
                        .insert_shot_before(
                            *section_id,
                            *before_id,
                            shot_data.clone(),
                            Some(*shot_id),
                            uuid.clone(),
                        )
                        .is_some()
                } else if let Some(after_id) = after_shot_id {
                    project
                        .insert_shot_after(
                            *section_id,
                            *after_id,
                            shot_data.clone(),
                            Some(*shot_id),
                            uuid.clone(),
                        )
                        .is_some()
                } else {
                    // Both anchors absent. For a CURRENT entry this is an append to an
                    // empty section (exact via a raw insert). But a LEGACY persisted
                    // entry (written by a pre-anchor build, reloaded after upgrade) has
                    // both anchors None even for a prepend / mid-section insert, so route
                    // through the proper insert helper using the captured position — which
                    // re-anchors SectionStart on a prepend and cascades offsets on a
                    // mid-insert — instead of a raw insert that would mis-place anchored
                    // tails or start-station refs.
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
                                    Some(*shot_id),
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
                                    Some(*shot_id),
                                    uuid.clone(),
                                )
                                .is_some(),
                            None => false,
                        }
                    } else {
                        // True append (pos == len) or empty section: a raw insert is
                        // exact — no station shift or SectionStart re-anchor is needed.
                        let id = *shot_id;
                        if let Some(section) = project.get_section_mut(*section_id) {
                            if id >= section.next_shot_id {
                                section.next_shot_id = id + 1;
                            }
                            section
                                .shots
                                .insert(pos, Shot::with_uuid(id, shot_data.clone(), uuid.clone()));
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
                                project.section_offsets.insert(*sid, new_offset);
                            }
                            project.touch();
                            CommandResult::success()
                        }
                        None => CommandResult::failure("No project open".to_string()),
                    },
                    Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                }
            }
            UndoData::ClosureInfo { closure } => {
                match &entry.command {
                    Command::CreateLoopClosure { .. } => {
                        // Redo create by restoring the captured closure to preserve its ID.
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
            UndoData::MarkerInfo { marker } => {
                match &entry.command {
                    Command::AddMarker { .. } => {
                        // Redo add by restoring the captured marker with original ID.
                        match self.project.lock() {
                            Ok(mut proj_guard) => match proj_guard.as_mut() {
                                Some(project) => {
                                    project.next_marker_id =
                                        project.next_marker_id.max(marker.id + 1);
                                    project.markers.push(marker.clone());
                                    project.touch();
                                    CommandResult::success().with_marker_id(marker.id)
                                }
                                None => CommandResult::failure("No project open".to_string()),
                            },
                            Err(_) => CommandResult::failure("Failed to lock project".to_string()),
                        }
                    }
                    Command::UpdateMarker { .. } | Command::DeleteMarker { .. } => {
                        // Redo update/delete by re-executing (marker_id is stable).
                        self.execute_without_undo(entry.command.clone())
                    }
                    _ => CommandResult::failure("Unsupported marker redo command".to_string()),
                }
            }

            _ => {
                // Re-execute the original command for other undo data types
                self.execute_without_undo(entry.command.clone())
            }
        };

        // Epilogue: push back to undo stack on success, push back to redo stack on failure
        // Important: Use the original entry with its original undo_data, not regenerated data
        // This ensures undo after redo works correctly
        if result.success {
            if let Ok(mut stack) = self.undo_stack.lock() {
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
    pub fn get_undo_stack(&self) -> Vec<crate::commands::history::UndoHistoryItem> {
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
                        crate::commands::history::entry_to_history_item(entry, idx, &section_names)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the redo stack as a list of history items for the UI
    pub fn get_redo_stack(&self) -> Vec<crate::commands::history::UndoHistoryItem> {
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
                        crate::commands::history::entry_to_history_item(entry, idx, &section_names)
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
        for i in 0..count {
            let result = self.undo_inner();
            if !result.success {
                return CommandResult::failure(format!(
                    "Undo failed at step {} of {}: {}",
                    i + 1,
                    count,
                    result.error.unwrap_or_else(|| "Unknown error".to_string())
                ));
            }
        }

        // Dirty-bit flip (#191, D-B16): undo_to ran one or more undos. Set-only;
        // no-op for local-only.
        self.mark_remote_dirty();
        tracing::info!(index = index, count = count, "Undo to index completed");
        CommandResult::success()
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
        for i in 0..count {
            let result = self.redo_inner();
            if !result.success {
                return CommandResult::failure(format!(
                    "Redo failed at step {} of {}: {}",
                    i + 1,
                    count,
                    result.error.unwrap_or_else(|| "Unknown error".to_string())
                ));
            }
        }

        // Dirty-bit flip (#191, D-B16): redo_to ran one or more redos. Set-only;
        // no-op for local-only.
        self.mark_remote_dirty();
        tracing::info!(index = index, count = count, "Redo to index completed");
        CommandResult::success()
    }
}
