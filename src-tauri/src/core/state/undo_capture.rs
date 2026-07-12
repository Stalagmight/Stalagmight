use crate::core::{Command, UndoData};
use crate::models::{ClosureStrategy, SectionId, SectionUpdates, ShotUpdates};

use super::AppState;

impl AppState {
    /// Generate undo data before executing a command
    pub(super) fn generate_undo_data(&self, cmd: &Command) -> UndoData {
        let Ok(proj_guard) = self.project.lock() else {
            return UndoData::None;
        };

        let Some(project) = proj_guard.as_ref() else {
            return UndoData::None;
        };

        match cmd {
            // For create commands, we'll fill in the ID after execution
            Command::CreateSection { .. } => UndoData::None, // Will be finalized after execution
            Command::CreateShot { .. } => UndoData::None,    // Will be finalized after execution

            // For DeleteSection, capture full project snapshot (before/after pair)
            // because cleanup_section_references removes merges, closures, offsets, and
            // zero datums — a SectionSnapshot alone cannot restore those.
            // Guard: only serialize if section exists (avoids overhead for doomed commands).
            Command::DeleteSection { section_id } => {
                if project.get_section(*section_id).is_some() {
                    match serde_json::to_string(project) {
                        Ok(before) => UndoData::ProjectSnapshotPair {
                            before,
                            after: String::new(), // Filled by finalize_undo_data
                        },
                        Err(_) => UndoData::None,
                    }
                } else {
                    UndoData::None
                }
            }

            // For DeleteShot, capture full project snapshot (before/after pair)
            // because remove_shot shifts station refs — a ShotSnapshot alone cannot
            // restore the original station references.
            Command::DeleteShot {
                section_id,
                shot_id,
            } => {
                if let Some(section) = project.get_section(*section_id) {
                    if section.shots.iter().any(|s| s.id == *shot_id) {
                        match serde_json::to_string(project) {
                            Ok(before) => UndoData::ProjectSnapshotPair {
                                before,
                                after: String::new(), // Filled by finalize_undo_data
                            },
                            Err(_) => UndoData::None,
                        }
                    } else {
                        UndoData::None
                    }
                } else {
                    UndoData::None
                }
            }

            // For update commands, capture previous values
            Command::UpdateShot {
                section_id,
                shot_id,
                updates,
            } => {
                if let Some(section) = project.get_section(*section_id) {
                    if let Some(shot) = section.get_shot(*shot_id) {
                        // Capture only the fields that are being updated
                        let prev_updates = ShotUpdates {
                            azimuth_in: updates.azimuth_in.map(|_| shot.azimuth_in),
                            azimuth_out: updates.azimuth_out.map(|_| shot.azimuth_out),
                            length: updates.length.map(|_| shot.length),
                            depth_in: updates.depth_in.map(|_| shot.depth_in),
                            depth_out: updates.depth_out.map(|_| shot.depth_out),
                            pitch_in: updates.pitch_in.as_ref().map(|_| shot.pitch_in),
                            pitch_out: updates.pitch_out.as_ref().map(|_| shot.pitch_out),
                            left: updates.left.as_ref().map(|_| shot.left),
                            right: updates.right.as_ref().map(|_| shot.right),
                            up: updates.up.as_ref().map(|_| shot.up),
                            down: updates.down.as_ref().map(|_| shot.down),
                            comment: updates.comment.as_ref().map(|_| shot.comment.clone()),
                            is_reference: updates.is_reference.map(|_| shot.is_reference),
                            marker_index: updates.marker_index.as_ref().map(|_| shot.marker_index),
                            temperature: updates.temperature.as_ref().map(|_| shot.temperature),
                        };

                        if let Ok(json) = serde_json::to_string(&prev_updates) {
                            UndoData::PreviousValues {
                                section_id: *section_id,
                                shot_id: Some(*shot_id),
                                values_json: json,
                            }
                        } else {
                            UndoData::None
                        }
                    } else {
                        UndoData::None
                    }
                } else {
                    UndoData::None
                }
            }

            Command::UpdateSectionMetadata {
                section_id,
                updates,
            } => {
                if let Some(section) = project.get_section(*section_id) {
                    // Capture previous section metadata
                    let prev_updates = SectionUpdates {
                        name: updates.name.as_ref().map(|_| section.name.clone()),
                        surveyor: updates.surveyor.as_ref().map(|_| section.surveyor.clone()),
                        date: updates.date.as_ref().map(|_| section.date),
                        color: updates.color.as_ref().map(|_| section.color.clone()),
                        direction: updates.direction.map(|_| section.direction),
                        quality: updates.quality.map(|_| section.quality),
                    };

                    if let Ok(json) = serde_json::to_string(&prev_updates) {
                        UndoData::PreviousValues {
                            section_id: *section_id,
                            shot_id: None,
                            values_json: json,
                        }
                    } else {
                        UndoData::None
                    }
                } else {
                    UndoData::None
                }
            }

            // For reference status toggle, inverse is toggling back
            Command::SetShotReference {
                section_id,
                shot_id,
                is_reference,
            } => UndoData::InverseCommand(Command::SetShotReference {
                section_id: *section_id,
                shot_id: *shot_id,
                is_reference: !*is_reference,
            }),

            // Merge can shift sections BEYOND the two merge groups: after aligning
            // the groups, cascade_connection_offsets re-anchors any section reachable
            // through an ALIGNED loop closure (cascade phase 2), and such a section is
            // in neither merge group. A lightweight MergeInfo only restores the two
            // groups' offsets, so that external section stays drifted on undo — and
            // realign_merges() cannot self-heal it (it checks merge alignment only,
            // never closures). So capture a full before/after pair (like
            // Delete/Unmerge); undo then restores EVERY section_offset the cascade
            // could have touched.
            Command::MergeStations {
                station_a,
                station_b,
            } => {
                if project.get_section(station_a.section_id).is_some()
                    && project.get_section(station_b.section_id).is_some()
                {
                    match serde_json::to_string(project) {
                        Ok(before) => UndoData::ProjectSnapshotPair {
                            before,
                            after: String::new(), // Filled by finalize_undo_data
                        },
                        Err(_) => UndoData::None,
                    }
                } else {
                    UndoData::None
                }
            }

            // For unmerge, we need to know which stations to re-merge
            // Also capture closures that will be removed during unmerge
            // Unmerge removes a merge edge, which can strand a section so its sole
            // remaining bridge is a loop closure — execute() then converts that
            // closure into a merge and re-derives offsets (#8). A lightweight
            // UnmergeInfo can only re-create the removed merge; it cannot reverse
            // that conversion (the closure would stay a merge, forming a cycle) or
            // restore the pre-unmerge offsets. So capture a full before/after pair
            // (matching DeleteShot/DeleteSection, which trigger the same side
            // effect) — undo then restores the merge AND the converted closure AND
            // the original positions atomically.
            Command::UnmergeStations { station_ref } => {
                let involved = project.station_merges.iter().any(|m| {
                    project.key_is_ref(&m.station_a, *station_ref)
                        || project.key_is_ref(&m.station_b, *station_ref)
                });
                if involved {
                    match serde_json::to_string(project) {
                        Ok(before) => UndoData::ProjectSnapshotPair {
                            before,
                            after: String::new(), // Filled by finalize_undo_data
                        },
                        Err(_) => UndoData::None,
                    }
                } else {
                    UndoData::None
                }
            }

            Command::UnmergeStationPair {
                station_a,
                station_b,
            } => {
                let merged = project.station_merges.iter().any(|m| {
                    (project.key_is_ref(&m.station_a, *station_a)
                        && project.key_is_ref(&m.station_b, *station_b))
                        || (project.key_is_ref(&m.station_a, *station_b)
                            && project.key_is_ref(&m.station_b, *station_a))
                });
                if merged {
                    match serde_json::to_string(project) {
                        Ok(before) => UndoData::ProjectSnapshotPair {
                            before,
                            after: String::new(), // Filled by finalize_undo_data
                        },
                        Err(_) => UndoData::None,
                    }
                } else {
                    UndoData::None
                }
            }

            // For create loop closure, capture snapshot when it mutates shot data
            Command::CreateLoopClosure {
                closure_strategy, ..
            } => {
                if matches!(
                    closure_strategy,
                    ClosureStrategy::ProportionalByLength | ClosureStrategy::EqualDistribution
                ) {
                    match serde_json::to_string(project) {
                        Ok(before) => UndoData::ProjectSnapshotPair {
                            before,
                            after: String::new(), // Filled by finalize_undo_data
                        },
                        Err(_) => UndoData::None,
                    }
                } else {
                    UndoData::None
                }
            }

            Command::RemoveLoopClosure { closure_id } => {
                if let Some(closure) = project.loop_closures.iter().find(|lc| lc.id == *closure_id)
                {
                    UndoData::ClosureInfo {
                        closure: closure.clone(),
                    }
                } else {
                    UndoData::None
                }
            }

            // For zero datum operations - capture GPS and section offsets
            Command::SetZeroDatum { station_ref, .. } => {
                // Find if there was a previous datum for this station
                let previous_gps = project
                    .zero_datums
                    .iter()
                    .find(|zd| project.key_is_ref(&zd.station_ref, *station_ref))
                    .map(|zd| zd.gps);

                // Capture section offsets for all connected sections
                // This allows undo to restore exact previous state
                let previous_offsets =
                    project.capture_connected_section_offsets(station_ref.section_id);

                UndoData::ZeroDatumInfo {
                    station_ref: *station_ref,
                    previous_gps,
                    previous_offsets,
                }
            }

            Command::RemoveZeroDatum { station_ref } => {
                // Capture the current datum
                let previous_gps = project
                    .zero_datums
                    .iter()
                    .find(|zd| project.key_is_ref(&zd.station_ref, *station_ref))
                    .map(|zd| zd.gps);

                // Capture section offsets for all connected sections
                // RemoveZeroDatum doesn't change offsets, but we capture for consistency
                let previous_offsets =
                    project.capture_connected_section_offsets(station_ref.section_id);

                UndoData::ZeroDatumInfo {
                    station_ref: *station_ref,
                    previous_gps,
                    previous_offsets,
                }
            }

            // For station comment, inverse is restoring the previous comment
            Command::SetStationComment {
                station_ref,
                comment: _,
            } => {
                let previous_comment = project
                    .key_for_ref(*station_ref)
                    .and_then(|key| project.station_comments.get(&key).cloned());
                UndoData::InverseCommand(Command::SetStationComment {
                    station_ref: *station_ref,
                    comment: previous_comment,
                })
            }

            // Move commands use inverse commands
            // MoveSection and MoveSegment both move the whole rigid-connected group
            // (MoveSection delegates to move_segment), so both capture every affected
            // section and its previous offset at execution time — undo/redo stays
            // correct even if the connectivity graph changes later. Filter to existing
            // sections to avoid stale merges/closures referencing deleted sections
            // (matches move_segment()'s own filtering).
            Command::MoveSection { section_id, offset }
            | Command::MoveSegment { section_id, offset } => {
                let existing: std::collections::HashSet<SectionId> =
                    project.sections.iter().map(|s| s.id).collect();
                let connected: Vec<SectionId> = project
                    .get_rigid_connected_section_ids(*section_id)
                    .into_iter()
                    .filter(|id| existing.contains(id))
                    .collect();
                let mut previous_offsets = std::collections::HashMap::new();
                for sid in &connected {
                    previous_offsets.insert(*sid, project.section_offsets.get(sid).copied());
                }
                UndoData::SegmentMoveInfo {
                    affected_sections: connected,
                    previous_offsets,
                    offset: (offset.x, offset.y),
                }
            }

            // MoveStation rewrites the raw geometry of the two adjacent shots and can,
            // through reconcile, re-anchor sections connected by merges/aligned closures.
            // A lightweight StationMoveInfo captured only azimuth/length/depth of two
            // shots and one section offset — it dropped pitch/lrud/temperature and any
            // re-cascaded section. Capture a full before/after pair (like Merge/Unmerge)
            // so undo restores every field and every offset the move could have touched.
            Command::MoveStation {
                station_ref,
                offset: _,
            } => {
                if project.get_section(station_ref.section_id).is_some() {
                    match serde_json::to_string(project) {
                        Ok(before) => UndoData::ProjectSnapshotPair {
                            before,
                            after: String::new(), // Filled by finalize_undo_data
                        },
                        Err(_) => UndoData::None,
                    }
                } else {
                    UndoData::None
                }
            }

            // MoveComment: undo data is handled entirely inside execute_move_comment(),
            // which captures before+after snapshots and pushes directly.
            Command::MoveComment { .. } => UndoData::None,

            // Batch: undo data is handled entirely inside execute_batch(),
            // which captures before+after snapshots and pushes directly.
            Command::Batch { .. } => UndoData::None,

            // AddMarker: finalized after execution (we don't know the new ID yet)
            Command::AddMarker { .. } => UndoData::None,

            // UpdateMarker: capture old marker state so undo can restore it
            Command::UpdateMarker { marker_id, .. } => {
                if let Some(marker) = project.markers.iter().find(|m| m.id == *marker_id) {
                    UndoData::MarkerInfo {
                        marker: marker.clone(),
                    }
                } else {
                    UndoData::None
                }
            }

            // DeleteMarker: capture the marker before deletion so undo can restore it
            Command::DeleteMarker { marker_id } => {
                if let Some(marker) = project.markers.iter().find(|m| m.id == *marker_id) {
                    UndoData::MarkerInfo {
                        marker: marker.clone(),
                    }
                } else {
                    UndoData::None
                }
            }

            // Non-undoable commands
            _ => UndoData::None,
        }
    }

    /// Finalize undo data after command execution (for commands that create new entities)
    pub(super) fn finalize_undo_data(&self, cmd: &Command, undo_data: UndoData) -> UndoData {
        let Ok(proj_guard) = self.project.lock() else {
            return undo_data;
        };

        let Some(project) = proj_guard.as_ref() else {
            return undo_data;
        };

        match cmd {
            Command::AddMarker { .. } => {
                // Capture the just-created marker (the last one in the vec, since
                // add_marker appends) so undo can delete by ID and redo can restore
                // the exact same marker without re-assigning a different ID.
                if let Some(marker) = project.markers.last() {
                    UndoData::MarkerInfo {
                        marker: marker.clone(),
                    }
                } else {
                    undo_data
                }
            }

            Command::CreateSection { .. } => {
                // Capture full snapshot of the just-created section (the last one)
                // so that redo can restore it with its original ID instead of
                // re-executing CreateSection (which would assign a new ID).
                if let Some(section) = project.sections.last() {
                    UndoData::CreatedSectionSnapshot {
                        section_id: section.id,
                        section_data: section.to_section_data(),
                    }
                } else {
                    undo_data
                }
            }

            Command::CreateShot {
                section_id,
                after_shot_id,
                before_shot_id,
                ..
            } => {
                // Capture full snapshot of the just-created shot so that redo can
                // restore it with its original ID instead of re-executing CreateShot
                // (which would assign a new ID).
                if let Some(section) = project.get_section(*section_id) {
                    // The just-created shot has ID = next_shot_id - 1
                    let shot_id = section.next_shot_id - 1;
                    // Find the shot by ID to get its actual position (not last() which
                    // would be wrong for mid-section inserts via after_shot_id)
                    if let Some((position, shot)) = section
                        .shots
                        .iter()
                        .enumerate()
                        .find(|(_, s)| s.id == shot_id)
                    {
                        // For insert_before: station_shift_from = position (the array position)
                        // For insert_after/append: station_shift_from = position + 1
                        // (insert_after creates new station at pos+2 of old array, but after
                        // insertion it's at position+1 in the new array; for redo we need the
                        // from_index matching the shift semantics of insert_before)
                        let station_shift_from = if before_shot_id.is_some() {
                            position as u32
                        } else {
                            // For insert_after and append: the new station doesn't displace
                            // existing ones in the same way. We use position + 1 so that on redo,
                            // refs at station >= position+1 get shifted up (matching insert_after
                            // semantics of shifting >= pos+2, but pos here is already post-insert).
                            // Actually, for append (end of section), no refs need shifting since
                            // there are no stations beyond it. Using section.shots.len() as u32
                            // ensures no refs match.
                            (position + 1) as u32
                        };
                        UndoData::CreatedShotSnapshot {
                            section_id: *section_id,
                            shot_id,
                            shot_data: shot.to_shot_data(),
                            position,
                            station_shift_from,
                            after_shot_id: *after_shot_id,
                            before_shot_id: *before_shot_id,
                            uuid: shot.uuid.clone(),
                        }
                    } else {
                        undo_data
                    }
                } else {
                    undo_data
                }
            }

            Command::CreateLoopClosure {
                station_a,
                station_b,
                closure_strategy,
            } => {
                if matches!(
                    closure_strategy,
                    ClosureStrategy::ProportionalByLength | ClosureStrategy::EqualDistribution
                ) {
                    if let UndoData::ProjectSnapshotPair { before, .. } = undo_data {
                        match serde_json::to_string(project) {
                            Ok(after) => UndoData::ProjectSnapshotPair { before, after },
                            Err(_) => UndoData::None,
                        }
                    } else {
                        undo_data
                    }
                } else {
                    // Capture the newly created closure from the project's loop_closures
                    if let Some(closure) = project.loop_closures.iter().find(|lc| {
                        (project.key_is_ref(&lc.station_a, *station_a)
                            && project.key_is_ref(&lc.station_b, *station_b))
                            || (project.key_is_ref(&lc.station_a, *station_b)
                                && project.key_is_ref(&lc.station_b, *station_a))
                    }) {
                        UndoData::ClosureInfo {
                            closure: closure.clone(),
                        }
                    } else {
                        undo_data
                    }
                }
            }

            // DeleteSection/DeleteShot/Merge/Unmerge/MoveStation:
            // fill in the "after" snapshot now that the mutation (and any cascade
            // re-anchoring, orphan-closure conversion, and offset reconcile triggered
            // in execute()) is done.
            Command::DeleteSection { .. }
            | Command::DeleteShot { .. }
            | Command::MergeStations { .. }
            | Command::UnmergeStations { .. }
            | Command::UnmergeStationPair { .. }
            | Command::MoveStation { .. } => {
                if let UndoData::ProjectSnapshotPair { before, .. } = undo_data {
                    match serde_json::to_string(project) {
                        Ok(after) => UndoData::ProjectSnapshotPair { before, after },
                        Err(_) => UndoData::None,
                    }
                } else {
                    undo_data
                }
            }

            // MoveComment: finalize is a no-op; execute_move_comment handles its own snapshots.
            Command::MoveComment { .. } => undo_data,

            // Batch: finalize_undo_data is a no-op; execute_batch handles
            // ProjectSnapshotPair construction directly so after-snapshot
            // failure can trigger rollback.
            Command::Batch { .. } => undo_data,

            _ => undo_data,
        }
    }

    /// Check if a command should be pushed to undo stack
    pub(super) fn is_undoable(cmd: &Command) -> bool {
        !matches!(
            cmd,
            Command::OpenProject { .. }
                | Command::SaveProject { .. }
                | Command::CloseProject
                | Command::CreateProject { .. }
                | Command::MergeProjects { .. }
                // Pushes its own snapshot-pair undo inside apply_collab_changes.
                | Command::ApplyCollabChanges
                // Records its own snapshot-pair undo inside repair_open_project_integrity;
                // never routed through execute() anyway.
                | Command::RepairIntegrity
        )
    }
}
