use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{LineId, ShotId};

impl AppState {
    /// Execute an insert-station-mid-shot operation (issue #59) as a single
    /// atomic undo/redo unit: splits `shot_id` at `distance` meters from its
    /// start into two collinear child shots (see
    /// [`crate::models::Project::split_shot_at_distance`]). Uses a
    /// `ProjectSnapshotPair` for single-undo, mirroring
    /// `execute_delete_shot_and_split`.
    pub fn execute_split_shot_at_distance(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        distance: f64,
    ) -> CommandResult {
        // Serialize command execution to prevent concurrent interleaving
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Insert station (split shot)") {
            return f;
        }

        // Reject non-finite distance before it reaches the coordinate path
        // (mirrors create_shot's ShotData::all_finite rejection).
        if !distance.is_finite() {
            return CommandResult::failure("Distance must be finite".to_string());
        }

        tracing::info!(
            line_id,
            shot_id,
            distance,
            purpose = "insert station mid-shot",
            "Executing split_shot_at_distance as single undo unit"
        );

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        let (new_shot_id, head_data, tail_updates) = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };

            // Frozen-geometry guard (mirrors update_shot/delete_shot): refuse to
            // split a shot that is part of an adjusted survey network -- a split
            // changes both children's geometry, which would silently break the
            // LSQ solution's frozen geometry. Checked before the split mutates.
            // (`is_shot_frozen_by_id` returns false for a missing line/shot, so
            // the not-found error still surfaces from the split below.)
            if project.is_shot_frozen_by_id(line_id, shot_id) {
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
            match project.split_shot_at_distance(line_id, shot_id, distance) {
                Ok(v) => v,
                Err(e) => return CommandResult::failure(e),
            }
        };

        // Custom path bypasses execute()'s reconcile; re-derive offsets here
        // and before the after-snapshot so undo/redo and the saved file reflect
        // any drift (the split itself needs none, but this mirrors the other
        // custom execute_* paths for safety).
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        // The label names the station the split made, as the after-state
        // (station names settled) holds it.
        let label = self.batch_label(|project| {
            crate::core::undo::history_batch::insert_station(project, line_id, new_shot_id)
        });
        let batch_cmd = Command::Batch {
            commands: vec![
                Command::CreateShot {
                    line_id,
                    shot: head_data,
                    after_shot_id: None,
                    before_shot_id: Some(shot_id),
                },
                Command::UpdateShot {
                    line_id,
                    shot_id,
                    updates: tail_updates,
                },
            ],
            label,
        };
        self.push_snapshot_undo(batch_cmd, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): a split mutated a remote project.
        // Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        tracing::info!(
            line_id,
            shot_id,
            new_shot_id,
            purpose = "insert station mid-shot",
            "Split shot at distance completed"
        );

        CommandResult::success().with_shot_id(new_shot_id)
    }
}
