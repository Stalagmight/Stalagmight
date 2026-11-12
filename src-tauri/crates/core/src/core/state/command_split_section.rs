//! Split Section Here (#335) as one atomic undo/redo unit.

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    SectionId, SplitAt, SplitSectionOptions, SplitSide, FEATURE_ID_STRIDE,
    MAX_SECTIONS_IN_ID_SPACE, MAX_SECTION_ID, MIN_SPLIT_CHILD_LENGTH_M,
};

impl AppState {
    /// Split a section in two at a mid-section station, or at a point along a
    /// shot (inserting a station there first), deleting nothing. One undo
    /// entry (`ProjectSnapshotPair`) covers the optional station insert and
    /// the split. Returns the new second half's id as `section_id`, and both
    /// halves as `section_ids` (first half first).
    pub fn execute_split_section(
        &self,
        section_id: SectionId,
        at: SplitAt,
        options: SplitSectionOptions,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        if let Err(f) = self.guard_mutation("Split section") {
            return f;
        }
        if options.datum_side == SplitSide::Both {
            return CommandResult::failure(
                "A zero datum can stay on only one half of a split".to_string(),
            );
        }
        tracing::info!(
            section_id,
            ?at,
            "Executing split_section as single undo unit"
        );

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // This path bypasses `execute()`, so it carries its own frame check
        // (#469): moving a datum to the second half can re-define the frame.
        let frame_before = self.frame_datum_key();
        let undo_name_b = options.name_b.clone();

        let second_id = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            let Some(section) = project.get_section(section_id) else {
                return CommandResult::failure(format!("Section {section_id} not found"));
            };
            // The split mints the second half's id (#555); refuse before any
            // mutation rather than truncate the first half with nowhere to put
            // the rest.
            if project.next_section_id > MAX_SECTION_ID {
                return CommandResult::failure(format!(
                    "Cannot split section {section_id}: the project has reached the \
                     maximum number of sections (limit = {MAX_SECTIONS_IN_ID_SPACE}); \
                     shot ids encode as section_id * {FEATURE_ID_STRIDE} + position \
                     in 32 bits"
                ));
            }

            // Resolve the cut to a station index, splitting the shot first when
            // the cut lands inside one. Every refusal happens before that
            // insert, so nothing below needs a rollback except a model error.
            let station_index = match at {
                SplitAt::Station { station_index } => station_index,
                SplitAt::Shot { shot_id, distance } => {
                    let Some(pos) = section.shots.iter().position(|s| s.id == shot_id) else {
                        return CommandResult::failure(format!(
                            "Shot {shot_id} not found in section {section_id}"
                        ));
                    };
                    let length = section.shots[pos].length;
                    if !distance.is_finite() || distance < 0.0 || distance > length {
                        return CommandResult::failure(format!(
                            "Distance {distance} is outside shot {shot_id} (0 to {length} m)"
                        ));
                    }
                    if distance < MIN_SPLIT_CHILD_LENGTH_M {
                        pos as u32
                    } else if length - distance < MIN_SPLIT_CHILD_LENGTH_M {
                        pos as u32 + 1
                    } else {
                        // Same guard as Insert Station: a new station inside a
                        // shot of an adjusted network changes that network.
                        if project.is_shot_frozen_by_id(section_id, shot_id) {
                            return CommandResult::failure(
                                "Cannot split a frozen shot. This shot is part of an \
                                 adjusted survey network."
                                    .to_string(),
                            );
                        }
                        if let Err(e) =
                            project.split_shot_at_distance(section_id, shot_id, distance)
                        {
                            return CommandResult::failure(e);
                        }
                        pos as u32 + 1
                    }
                }
            };

            match project.split_section_at_station(section_id, station_index, &options) {
                Ok(id) => {
                    // A half whose only tie is a loop closure is merged instead
                    // (issue #8), as the delete-and-split path does.
                    project.convert_orphan_closures_to_merges();
                    project.touch();
                    id
                }
                Err(e) => {
                    // Only reachable after a shot split inserted a station.
                    let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                    return CommandResult::failure(e);
                }
            }
        };

        self.renormalize_frame_if_changed(frame_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };
        // The label only names the operation in history. It must not contain a
        // ReverseSection: undo/redo settle reversed sections off this command.
        let label = Command::Batch {
            commands: vec![Command::CreateSection {
                name: Some(undo_name_b),
                surveyor: None,
                date: None,
                color: String::new(),
                direction: None,
                shots: Vec::new(),
            }],
        };
        self.push_snapshot_undo(label, before_snapshot, after_snapshot);
        self.mark_remote_forward_edit();

        tracing::info!(section_id, second_id, "Split section completed");
        CommandResult::success()
            .with_section_id(second_id)
            .with_section_ids(vec![section_id, second_id])
    }
}
