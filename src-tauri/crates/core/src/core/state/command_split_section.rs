//! Split Section Here (#335) as one atomic undo/redo unit.

use super::command_delete_split::section_budget_refusal;
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    Project, SectionId, ShotId, SplitAt, SplitSectionOptions, SplitSide, MIN_SPLIT_CHILD_LENGTH_M,
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
        // Read-only edit gate (#191, D-B6).
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

        // Every refusal is decided first, read-only: the ignore pin below
        // writes to the database, so a refused split must not reach it.
        let cut = {
            let Ok(proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_ref() else {
                return CommandResult::failure("No project open".to_string());
            };
            match Self::validate_cut(project, section_id, at) {
                Ok(cut) => cut,
                Err(reason) => return CommandResult::failure(reason),
            }
        };

        // The shots past the cut move to a new section: position-keyed
        // ignores on them would lapse (see the pin's doc).
        self.pin_position_keyed_ignores_before_move(&[section_id]);

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
            let station_index = match cut {
                Cut::Station(station_index) => station_index,
                Cut::InShot {
                    shot_id,
                    distance,
                    station_index,
                } => {
                    // `validate_cut` ran every check the shot split refuses
                    // on, but the model shortens the tail before it inserts
                    // the head, so a failure rolls back whatever it changed.
                    if let Err(e) = project.split_shot_at_distance(section_id, shot_id, distance) {
                        let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                        return CommandResult::failure(e);
                    }
                    station_index
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
                    // Not expected: `validate_cut` ran the same checks. Undo a
                    // station the shot split may already have inserted.
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
        // Dirty-bit flip (#191, D-B16): set-only; a no-op for a local project.
        self.mark_remote_forward_edit();

        tracing::info!(section_id, second_id, "Split section completed");
        CommandResult::success()
            .with_section_id(second_id)
            .with_section_ids(vec![section_id, second_id])
    }

    /// Resolve `at` to where the split cuts, or why it is refused. Read-only:
    /// every refusal of the split is decided here, before anything changes.
    fn validate_cut(project: &Project, section_id: SectionId, at: SplitAt) -> Result<Cut, String> {
        let Some(section) = project.get_section(section_id) else {
            return Err(format!("Section {section_id} not found"));
        };
        // The split mints the second half's id (#555); refuse rather than
        // truncate the first half with nowhere to put the rest.
        if let Some(refusal) = section_budget_refusal(project, section_id) {
            return Err(refusal.error.unwrap_or_default());
        }
        let cut = match at {
            SplitAt::Station { station_index } => Cut::Station(station_index),
            SplitAt::Shot { shot_id, distance } => {
                let Some(pos) = section.shots.iter().position(|s| s.id == shot_id) else {
                    return Err(format!("Shot {shot_id} not found in section {section_id}"));
                };
                let length = section.shots[pos].length;
                if !distance.is_finite() || distance < 0.0 || distance > length {
                    return Err(format!(
                        "Distance {distance} is outside shot {shot_id} (0 to {length} m)"
                    ));
                }
                if distance < MIN_SPLIT_CHILD_LENGTH_M {
                    Cut::Station(pos as u32)
                } else if length - distance < MIN_SPLIT_CHILD_LENGTH_M {
                    Cut::Station(pos as u32 + 1)
                } else {
                    // Same guard as Insert Station: a new station inside a
                    // shot of an adjusted network changes that network.
                    if project.is_shot_frozen_by_id(section_id, shot_id) {
                        return Err("Cannot split a frozen shot. This shot is part of an \
                                    adjusted survey network."
                            .to_string());
                    }
                    // The station goes in as a new shot of this section,
                    // so it needs room under the shot cap and an id left
                    // in the section's window; checked here, before the
                    // shot split changes anything.
                    if let Some(refusal) = project.shot_insert_refusal(section_id) {
                        return Err(refusal);
                    }
                    // The new station is always a mid-section one, so the
                    // split cannot refuse it for being an end.
                    return Ok(Cut::InShot {
                        shot_id,
                        distance,
                        station_index: pos as u32 + 1,
                    });
                }
            }
        };
        if let Cut::Station(station_index) = cut {
            if let Some(refusal) = project.split_station_refusal(section_id, station_index) {
                return Err(refusal);
            }
        }
        Ok(cut)
    }
}

/// Where a validated split cuts: at an existing station, or inside a shot,
/// which first gets a station inserted `distance` along it.
#[derive(Debug, Clone, Copy)]
enum Cut {
    Station(u32),
    InShot {
        shot_id: ShotId,
        distance: f64,
        station_index: u32,
    },
}
