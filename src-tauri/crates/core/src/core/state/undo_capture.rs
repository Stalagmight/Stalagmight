use crate::core::{Command, UndoData};
use crate::models::{ClosureStrategy, SectionId, SectionUpdates, ShotId, ShotUpdates};

use super::AppState;

impl AppState {
    /// Generate undo data before executing a command
    pub(super) fn generate_undo_data(&self, cmd: &Command) -> UndoData {
        let proj_guard = self.project.lock();

        let Some(project) = proj_guard.as_ref() else {
            return UndoData::None;
        };

        match cmd {
            // For create commands, we'll fill in the ID after execution
            Command::CreateSection { .. } => UndoData::None, // Will be finalized after execution
            // A STUB, filled in by `finalize_undo_data` once the shot exists -
            // the same shape `ProjectSnapshotPair { after: String::new() }` uses
            // below. It is a stub rather than `UndoData::None` because ONE of its
            // fields can only be read BEFORE the insert: the `section_offsets` the
            // insert's cascade can reach, as they stood, which the undo has to
            // hand back rather than let the removal re-derive (#413, finding 27's
            // second arm). Every other field is placeholder and is overwritten; if the
            // finalize cannot find the created shot it yields `UndoData::None`,
            // exactly as this arm used to.
            Command::CreateShot { section_id, .. } => UndoData::CreatedShotSnapshot {
                section_id: *section_id,
                shot_id: 0,
                shot_data: crate::models::ShotData::default(),
                position: 0,
                station_shift_from: 0,
                after_shot_id: None,
                before_shot_id: None,
                after_shot_uuid: None,
                before_shot_uuid: None,
                uuid: None,
                previous_offsets: project.capture_rigid_connected_section_offsets(*section_id),
            },

            // For DeleteSection, capture full project snapshot (before/after pair)
            // because cleanup_section_references removes merges, closures, offsets, and
            // zero datums - a SectionSnapshot alone cannot restore those.
            // Guard: only serialize if section exists (avoids overhead for doomed commands).
            Command::DeleteSection { section_id } => {
                if project.get_section(*section_id).is_some() {
                    match AppState::serialize_project_snapshot(project) {
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

            // For ReverseSection, capture full project snapshot (before/after pair).
            // Re-running the reverse would be a valid inverse in exact arithmetic,
            // but the snapshot keeps undo bit-exact (the f64 `(az + 180) % 360`
            // round trip is not - pinned by
            // `reverse_of_an_unrepresentable_azimuth_round_trips_to_within_an_ulp`)
            // and spares redo from having to reproduce the anchor mirroring, the
            // section-offset compensation.
            // A section with no shots is a single station and reverses to itself,
            // so capturing it would push two identical full-project snapshots and
            // mark the project dirty for an operation that changed nothing. The
            // resulting `None` never reaches the stack - `execute()` drops an
            // unundoable entry rather than wedging every older one under it.
            Command::ReverseSection { section_id, .. } => {
                if project
                    .get_section(*section_id)
                    .is_some_and(|s| !s.shots.is_empty())
                {
                    match AppState::serialize_project_snapshot(project) {
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
            // because remove_shot shifts station refs - a ShotSnapshot alone cannot
            // restore the original station references.
            Command::DeleteShot {
                section_id,
                shot_id,
            } => {
                if let Some(section) = project.get_section(*section_id) {
                    if section.shots.iter().any(|s| s.id == *shot_id) {
                        match AppState::serialize_project_snapshot(project) {
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
                            kind: updates.kind.map(|_| shot.kind),
                            marker_index: updates.marker_index.as_ref().map(|_| shot.marker_index),
                            temperature: updates.temperature.as_ref().map(|_| shot.temperature),
                        };

                        if let Ok(json) = serde_json::to_string(&prev_updates) {
                            UndoData::PreviousValues {
                                section_id: *section_id,
                                shot_id: Some(*shot_id),
                                // The address that survives a save: the id
                                // beside it is renumbered on load (#413).
                                shot_uuid: shot.uuid.clone(),
                                values_json: json,
                                // What the undo's re-execution will RE-DERIVE and
                                // therefore has to be handed back verbatim
                                // instead (#413, finding 27). Captured only for a
                                // geometry edit, because that is the only one
                                // `update_shot` realigns for - and `prev_updates`
                                // mirrors which fields `updates` sets, so the undo
                                // realigns exactly when the forward command did.
                                previous_offsets: if updates.touches_geometry() {
                                    project.capture_rigid_connected_section_offsets(*section_id)
                                } else {
                                    std::collections::HashMap::new()
                                },
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
                            shot_uuid: None,
                            values_json: json,
                            // `update_section` writes metadata and no geometry, so
                            // its undo re-derives no offsets to restore.
                            previous_offsets: std::collections::HashMap::new(),
                        }
                    } else {
                        UndoData::None
                    }
                } else {
                    UndoData::None
                }
            }

            // The old boolean toggle inverted from the command alone (`!flag`).
            // A 3-way kind cannot (#27 q12): "undo setting this to Reference"
            // has to know whether it was Normal or Jump before. So read the
            // current kind out of the project and carry the previous value.
            // Falls back to UndoData::None if the shot is gone, matching how
            // the other shot commands degrade rather than inventing a value.
            //
            // Recorded as `PreviousValues` rather than the
            // `InverseCommand(SetShotKind)` it used to be, because that variant
            // is a NEWTYPE around `Command` and so has nowhere to put the
            // target's uuid: the inverse command names the shot by its
            // in-session id, which the loader renumbers, so after a save/reopen
            // the undo reset a DIFFERENT shot's kind (#413, finding 12). This
            // is the same restore - one field put back to its previous value -
            // through the variant that can carry the address that survives a
            // save. Persisted `InverseCommand(SetShotKind)` entries still
            // restore through their own arm in `undo_inner`.
            Command::SetShotKind {
                section_id,
                shot_id,
                ..
            } => project
                .get_section(*section_id)
                .and_then(|section| section.get_shot(*shot_id))
                .and_then(|shot| {
                    let prev_updates = ShotUpdates {
                        kind: Some(shot.kind),
                        ..Default::default()
                    };
                    serde_json::to_string(&prev_updates)
                        .ok()
                        .map(|json| UndoData::PreviousValues {
                            section_id: *section_id,
                            shot_id: Some(*shot_id),
                            shot_uuid: shot.uuid.clone(),
                            values_json: json,
                            // A kind is not geometry (`Command::affects_geometry`
                            // says so and `set_shot_kind` realigns nothing), so
                            // this undo re-derives no offsets to restore.
                            previous_offsets: std::collections::HashMap::new(),
                        })
                })
                .unwrap_or(UndoData::None),

            // Merge can shift sections BEYOND the two merge groups: after aligning
            // the groups, cascade_connection_offsets re-anchors any section reachable
            // through an ALIGNED loop closure (cascade phase 2), and such a section is
            // in neither merge group. A lightweight MergeInfo only restores the two
            // groups' offsets, so that external section stays drifted on undo - and
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
                    match AppState::serialize_project_snapshot(project) {
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
            // remaining bridge is a loop closure - execute() then converts that
            // closure into a merge and re-derives offsets (#8). A lightweight
            // UnmergeInfo can only re-create the removed merge; it cannot reverse
            // that conversion (the closure would stay a merge, forming a cycle) or
            // restore the pre-unmerge offsets. So capture a full before/after pair
            // (matching DeleteShot/DeleteSection, which trigger the same side
            // effect) - undo then restores the merge AND the converted closure AND
            // the original positions atomically.
            Command::UnmergeStations { station_ref } => {
                let involved = project.station_merges.iter().any(|m| {
                    project.key_is_ref(&m.station_a, *station_ref)
                        || project.key_is_ref(&m.station_b, *station_ref)
                });
                if involved {
                    match AppState::serialize_project_snapshot(project) {
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
                    match AppState::serialize_project_snapshot(project) {
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
                    match AppState::serialize_project_snapshot(project) {
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
                if let Some((position, closure)) = project
                    .loop_closures
                    .iter()
                    .enumerate()
                    .find(|(_, lc)| lc.id == *closure_id)
                {
                    UndoData::ClosureInfo {
                        closure: closure.clone(),
                        // The removal is a `Vec::remove`, so undo has to be a
                        // `Vec::insert` at the same index (#413, finding 4).
                        position: Some(position),
                    }
                } else {
                    UndoData::None
                }
            }

            // For zero datum operations - capture GPS and section offsets
            Command::SetZeroDatum { station_ref, .. } => {
                // Find if there was a previous datum for this station, and WHERE
                // in the list it sat (#413, finding 17).
                let previous = project
                    .zero_datums
                    .iter()
                    .enumerate()
                    .find(|(_, zd)| project.key_is_ref(&zd.station_ref, *station_ref));

                // Capture EVERY section's offset, not the datum's merge group.
                // Adding the first datum makes this station the georeference
                // origin, and undoing that add REMOVES it again - an inverse that
                // performs a frame change, and the frame is the one thing whose
                // blast radius is the whole project rather than a connected set
                // (`Project::renormalize_frame`, #469 item 1). The narrow capture
                // was exact only as long as no offset outside the merge group
                // could move; it no longer is, and an inverse that re-derives what
                // it cannot restore is the shape this programme exists to
                // eliminate. Whole-project here is affordable for the same reason
                // it is not on `UpdateShot`: setting a GPS anchor is a rare,
                // deliberate act.
                let previous_offsets = project.capture_all_section_offsets();

                UndoData::ZeroDatumInfo {
                    station_ref: *station_ref,
                    previous_gps: previous.map(|(_, zd)| zd.gps),
                    position: previous.map(|(position, _)| position),
                    previous_offsets,
                }
            }

            // A whole-project pair rather than the `ZeroDatumInfo` this used to
            // record, because removing the anchor is no longer the only thing
            // this command does: it is flagged `can_orphan_closures`, so
            // `execute()`'s epilogue converts the sole-bridge closure the removal
            // just made convertible (see `command_integrity`), and the reconcile
            // then re-solves around the new rigid merge. `ZeroDatumInfo` can put
            // back a datum and a map of offsets; it cannot put back a closure that
            // has become a merge. The pair restores both directions verbatim, and
            // it is SELF-REFRESHING (`refresh_snapshot_pair_counterpart`), so the
            // conversion lands in the `after` half by construction rather than by
            // anyone remembering to record it.
            //
            // Two whole snapshots is the expensive capture in this file, and it is
            // affordable HERE specifically: removing a GPS anchor is a rare,
            // deliberate act on a handful of datums, not a per-keystroke edit like
            // `UpdateShot`. `SetZeroDatum` keeps `ZeroDatumInfo` - it adds an
            // anchor, which converts nothing.
            //
            // Guarded on the datum existing so a doomed command (the station is
            // not a datum - `remove_zero_datum` fails) serializes nothing; the
            // resulting `None` never reaches the stack, as `execute()` only pushes
            // on success.
            Command::RemoveZeroDatum { station_ref } => {
                if project
                    .zero_datums
                    .iter()
                    .any(|zd| project.key_is_ref(&zd.station_ref, *station_ref))
                {
                    match AppState::serialize_project_snapshot(project) {
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
            // section and its previous offset at execution time - undo/redo stays
            // correct even if the connectivity graph changes later. Filter to existing
            // sections to avoid stale merges/closures referencing deleted sections
            // (matches move_segment()'s own filtering).
            Command::MoveSection { section_id, offset }
            | Command::MoveSegment { section_id, offset } => {
                let existing: std::collections::HashSet<SectionId> =
                    project.sections.iter().map(|s| s.id).collect();
                let mut connected: Vec<SectionId> = project
                    .get_rigid_connected_section_ids(*section_id)
                    .into_iter()
                    .filter(|id| existing.contains(id))
                    .collect();
                // [#576 FZ-DET-3] In id order: the group is a set, and its
                // iteration order made one move record (and save) a different
                // entry each time.
                connected.sort_unstable();
                let mut previous_offsets = std::collections::HashMap::new();
                for sid in &connected {
                    previous_offsets.insert(*sid, project.section_offsets().get(sid).copied());
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
            // shots and one section offset - it dropped pitch/lrud/temperature and any
            // re-cascaded section. Capture a full before/after pair (like Merge/Unmerge)
            // so undo restores every field and every offset the move could have touched.
            Command::MoveStation {
                station_ref,
                offset: _,
            } => {
                if project.get_section(station_ref.section_id).is_some() {
                    match AppState::serialize_project_snapshot(project) {
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

            // [#576 A7] Every variant is named, so a new command cannot fall
            // through to "no undo data" without someone deciding it should.
            //
            // Not undoable at all (`is_undoable` is false), so never reached.
            Command::CreateProject { .. }
            | Command::OpenProject { .. }
            | Command::SaveProject { .. }
            | Command::CloseProject
            | Command::MergeProjects { .. }
            | Command::RepairIntegrity => UndoData::None,
            // Label-only: `execute()` refuses them, and the paths that apply
            // them record their own snapshot pair (`push_snapshot_undo`).
            Command::UpdateProjectSettings | Command::SetSurfaceElevation => UndoData::None,
            // Undoable only as part of a `Batch` (the DMP import sends it inside
            // one), whose snapshot pair covers it. On its own it records nothing,
            // so `execute()` pushes no entry - but it still clears redo and marks
            // a remote copy edited, which is why `is_undoable` keeps it.
            Command::UntrackImport { .. } => UndoData::None,
        }
    }

    /// Finalize undo data after command execution (for commands that create new entities)
    pub(super) fn finalize_undo_data(&self, cmd: &Command, undo_data: UndoData) -> UndoData {
        let proj_guard = self.project.lock();

        let Some(project) = proj_guard.as_ref() else {
            return undo_data;
        };

        match cmd {
            Command::CreateSection { .. } => {
                // Capture full snapshot of the just-created section (the last one)
                // so that redo can restore it with its original ID instead of
                // re-executing CreateSection (which would assign a new ID).
                if let Some(section) = project.sections.last() {
                    UndoData::CreatedSectionSnapshot {
                        section_id: section.id,
                        section_data: section.to_section_data(),
                        // `SectionData` carries no uuid, no reverse parity and
                        // no gauge pair, so redo would rebuild the section with
                        // a fresh identity (#413, finding 1). The undo refreshes
                        // this immediately before it deletes the section, which
                        // is what carries a gauge pair captured by a save that
                        // happened after this push.
                        identity: crate::models::SectionIdentity::of(section),
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
                // The one field that had to be read BEFORE the insert, carried
                // across from the stub `generate_undo_data` built (#413, finding
                // 27's second arm).
                let previous_offsets = match &undo_data {
                    UndoData::CreatedShotSnapshot {
                        previous_offsets, ..
                    } => previous_offsets.clone(),
                    _ => std::collections::HashMap::new(),
                };
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
                        // The anchors' uuids alongside their ids, for the same
                        // reason the shot's own uuid is recorded: the ids are
                        // renumbered on load, and the anchor decides WHERE the
                        // redo puts the shot back (#413, finding 16).
                        let anchor_uuid = |anchor: Option<ShotId>| {
                            anchor
                                .and_then(|id| section.get_shot(id))
                                .and_then(|anchor_shot| anchor_shot.uuid.clone())
                        };
                        UndoData::CreatedShotSnapshot {
                            section_id: *section_id,
                            shot_id,
                            shot_data: shot.to_shot_data(),
                            position,
                            station_shift_from,
                            after_shot_id: *after_shot_id,
                            before_shot_id: *before_shot_id,
                            after_shot_uuid: anchor_uuid(*after_shot_id),
                            before_shot_uuid: anchor_uuid(*before_shot_id),
                            uuid: shot.uuid.clone(),
                            previous_offsets,
                        }
                    } else {
                        // The stub could not be completed, so it must not be
                        // pushed: `UndoData::None`, which is what this arm
                        // yielded before the stub existed.
                        UndoData::None
                    }
                } else {
                    UndoData::None
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
                        match AppState::serialize_project_snapshot(project) {
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
                            // A create APPENDS, so its redo appends too; only
                            // `RemoveLoopClosure` has an index to restore.
                            position: None,
                        }
                    } else {
                        undo_data
                    }
                }
            }

            // DeleteSection/DeleteShot/ReverseSection/Merge/Unmerge/MoveStation/
            // RemoveZeroDatum: fill in the "after" snapshot now that the mutation
            // (and any cascade re-anchoring, orphan-closure conversion, and offset
            // reconcile triggered in execute()) is done. `RemoveZeroDatum` is here
            // FOR the conversion - it is the whole reason it records a pair.
            Command::DeleteSection { .. }
            | Command::DeleteShot { .. }
            | Command::ReverseSection { .. }
            | Command::MergeStations { .. }
            | Command::UnmergeStations { .. }
            | Command::UnmergeStationPair { .. }
            | Command::RemoveZeroDatum { .. }
            | Command::MoveStation { .. } => {
                if let UndoData::ProjectSnapshotPair { before, .. } = undo_data {
                    match AppState::serialize_project_snapshot(project) {
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

            // [#576 A7] Complete before execution: `generate_undo_data` already
            // recorded everything the undo needs.
            Command::UpdateSectionMetadata { .. }
            | Command::UpdateShot { .. }
            | Command::SetShotKind { .. }
            | Command::RemoveLoopClosure { .. }
            | Command::SetZeroDatum { .. }
            | Command::SetStationComment { .. }
            | Command::MoveSection { .. }
            | Command::MoveSegment { .. } => undo_data,

            // Record nothing here (see the matching arms of
            // `generate_undo_data`): not undoable, label-only, or batch-only.
            Command::CreateProject { .. }
            | Command::OpenProject { .. }
            | Command::SaveProject { .. }
            | Command::CloseProject
            | Command::MergeProjects { .. }
            | Command::RepairIntegrity
            | Command::UpdateProjectSettings
            | Command::SetSurfaceElevation
            | Command::UntrackImport { .. } => undo_data,
        }
    }

    /// Check if a command should be pushed to undo stack
    ///
    /// [#576 A7] An exhaustive classification, not a list of exceptions: a new
    /// `Command` variant does not compile until it is placed in one of the two
    /// arms, so it cannot become silently un-undoable (and silently clear
    /// redo) by being forgotten.
    pub(super) fn is_undoable(cmd: &Command) -> bool {
        match cmd {
            // This is also a privacy boundary for Full-mode SpeleoDB history: the
            // lifecycle variants carry local filesystem paths, so making one
            // undoable can publish those paths in `command_json`. Keep this arm and
            // `path_bearing_commands_never_enter_shared_undo_history` in sync.
            Command::OpenProject { .. }
            | Command::SaveProject { .. }
            | Command::CloseProject
            | Command::CreateProject { .. }
            | Command::MergeProjects { .. }
            // Records its own snapshot-pair undo inside repair_open_project_integrity;
            // never routed through execute() anyway.
            | Command::RepairIntegrity => false,

            Command::CreateSection { .. }
            | Command::DeleteSection { .. }
            | Command::UpdateSectionMetadata { .. }
            | Command::ReverseSection { .. }
            | Command::CreateShot { .. }
            | Command::UpdateShot { .. }
            | Command::DeleteShot { .. }
            | Command::SetShotKind { .. }
            | Command::MergeStations { .. }
            | Command::UnmergeStations { .. }
            | Command::UnmergeStationPair { .. }
            | Command::CreateLoopClosure { .. }
            | Command::RemoveLoopClosure { .. }
            | Command::SetZeroDatum { .. }
            | Command::RemoveZeroDatum { .. }
            | Command::SetStationComment { .. }
            | Command::MoveSection { .. }
            | Command::MoveSegment { .. }
            | Command::MoveStation { .. }
            | Command::MoveComment { .. }
            | Command::Batch { .. }
            // Batch-only in practice; see its `generate_undo_data` arm for why
            // it stays in this arm although it records nothing standalone.
            | Command::UntrackImport { .. }
            // Label-only entries pushed by their own paths; `execute()` refuses
            // them before this classification matters.
            | Command::UpdateProjectSettings
            | Command::SetSurfaceElevation => true,
        }
    }
}
