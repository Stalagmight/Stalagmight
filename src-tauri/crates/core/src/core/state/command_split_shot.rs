use super::command_epilogue::EpilogueSteps;
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{SectionId, ShotId};

impl AppState {
    /// Execute an insert-station-mid-shot operation (issue #59) as a single
    /// atomic undo/redo unit: splits `shot_id` at `distance` meters from its
    /// start into two collinear child shots (see
    /// [`crate::models::Project::split_shot_at_distance`]). Uses a
    /// `ProjectSnapshotPair` for single-undo, mirroring
    /// `execute_delete_shot_and_split`.
    pub fn execute_split_shot_at_distance(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
        distance: f64,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let _cmd_guard = self.command_lock.lock();
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Insert station (split shot)") {
            return CommandResult::refused(f);
        }

        // Reject non-finite distance before it reaches the coordinate path
        // (mirrors create_shot's ShotData::all_finite rejection).
        if !distance.is_finite() {
            return CommandResult::failure("Distance must be finite".to_string());
        }

        tracing::info!(
            section_id,
            shot_id,
            distance,
            purpose = "insert station mid-shot",
            "Executing split_shot_at_distance as single undo unit"
        );

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // [#576 A4] The frame-defining datum before the split, for the shared
        // epilogue's frame check - which this path never ran before.
        let frame_before = self.frame_datum_key();

        let (new_shot_id, head_data, tail_updates) = {
            let mut proj_guard = self.project.lock();
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };

            // Frozen-geometry guard (mirrors update_shot/delete_shot): refuse to
            // split a shot that is part of an adjusted survey network -- a split
            // changes both children's geometry, which would silently break the
            // LSQ solution's frozen geometry. Checked before the split mutates.
            // (`is_shot_frozen_by_id` returns false for a missing section/shot, so
            // the not-found error still surfaces from the split below.)
            if project.is_shot_frozen_by_id(section_id, shot_id) {
                return CommandResult::failure(
                    "Cannot edit geometry of a frozen shot. This shot is part of an adjusted survey network."
                        .to_string(),
                );
            }

            // Perform the split ONCE. `split_shot_at_distance` validates the
            // distance (identical error messages), mutates in place, and returns
            // the new head shot's data plus the tail's updates -- the exact pieces
            // the undo-label Batch below needs, so we never recompute the split
            // just to describe it. The Command variants are a descriptive
            // placeholder for the undo entry (see push_snapshot_undo below); the
            // authoritative mutation is the ProjectSnapshotPair.
            match project.split_shot_at_distance(section_id, shot_id, distance) {
                Ok(v) => v,
                Err(e) => return CommandResult::failure(e),
            }
        };

        // [#576 A4] The shared forward epilogue, every step, before the after-
        // snapshot. This path used to run only the reconcile, while its comment
        // claimed to mirror the other custom paths: the orphan conversion and the
        // frame check were missing. The split keeps the original shot's uuid on the
        // tail and inserts a new head before it, so both are no-ops for it today;
        // running them is what keeps that true when the split changes.
        let batch_cmd = Command::Batch {
            commands: vec![
                Command::CreateShot {
                    section_id,
                    shot: head_data,
                    after_shot_id: None,
                    before_shot_id: Some(shot_id),
                },
                Command::UpdateShot {
                    section_id,
                    shot_id,
                    updates: tail_updates,
                },
            ],
        };
        if let Err(failure) =
            self.record_snapshot_edit(frame_before, EpilogueSteps::ALL, batch_cmd, before_snapshot)
        {
            return failure;
        }

        tracing::info!(
            section_id,
            shot_id,
            new_shot_id,
            purpose = "insert station mid-shot",
            "Split shot at distance completed"
        );

        CommandResult::success().with_shot_id(new_shot_id)
    }
}
