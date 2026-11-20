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
        let _ = (&line_id, &shot_id, &distance);
        crate::core::stub::not_ported("command_split_shot::execute_split_shot_at_distance")
    }
}
