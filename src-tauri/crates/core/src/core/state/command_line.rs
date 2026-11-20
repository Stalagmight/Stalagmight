use super::AppState;
use crate::core::{CommandResult, Validator};
use crate::models::{
    LineData, LineId, LineUpdates, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
};

impl AppState {
    pub(super) fn create_line(&self, data: LineData) -> CommandResult {
        let _ = (&data,);
        crate::core::stub::not_ported("command_line::create_line")
    }

    pub(super) fn delete_line(&self, line_id: LineId) -> CommandResult {
        let _ = (&line_id,);
        crate::core::stub::not_ported("command_line::delete_line")
    }

    /// Reverse a line end-to-end (issue #24).  The far end becomes station 0
    /// and every station keeps its world coordinate; see
    /// [`crate::models::Project::reverse_line`] for the transform.
    ///
    /// Frozen shots are deliberately NOT consulted here.  `update_shot` rejects
    /// azimuth/depth/pitch edits on a frozen shot because those move the station
    /// it is pinned to, but reverse re-associates each reading with the other end
    /// of the same physical line and leaves the line itself untouched, so the
    /// frozen invariant still holds.  Applying the frozen check would block
    /// reverse on any datum-anchored line for no benefit.
    pub(super) fn reverse_line(&self, line_id: LineId, swap_lr: bool) -> CommandResult {
        let _ = (&line_id, &swap_lr);
        crate::core::stub::not_ported("command_line::reverse_line")
    }

    pub(super) fn update_line(&self, line_id: LineId, updates: LineUpdates) -> CommandResult {
        let _ = (&line_id, &updates);
        crate::core::stub::not_ported("command_line::update_line")
    }
}

/// The execute pipeline's mutation for the line family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit), for rows only the result can say.
pub(super) fn apply(
    project: &mut crate::models::survey::Project,
    record: &mut super::commands::Record,
    cmd: &crate::core::Command,
) -> Result<super::commands::Applied, String> {
    let _ = (&mut *project, &mut *record);
    match cmd {
        crate::core::Command::CreateLine { .. } => {
            Err(super::commands::not_ported("execute::create_line"))
        }
        crate::core::Command::DeleteLine { line_id } => {
            project.delete_line(*line_id).map_err(|e| e.to_string())?;
            Ok(super::commands::Applied::default())
        }
        crate::core::Command::UpdateLineMetadata { .. } => {
            Err(super::commands::not_ported("execute::update_line_metadata"))
        }
        crate::core::Command::ReverseLine {
            line_id,
            swap_left_right,
        } => {
            let shot_count = project
                .line(*line_id)
                .map(|l| u32::try_from(l.shots.len()).unwrap_or(u32::MAX))
                .ok_or_else(|| format!("Line {line_id} not found"))?;
            project
                .reverse_line(*line_id, *swap_left_right)
                .map_err(|e| e.to_string())?;
            Ok(super::commands::Applied {
                reversed: vec![crate::core::ReversedLine {
                    line_id: *line_id,
                    shot_count,
                }],
                ..super::commands::Applied::default()
            })
        }
        _ => Err(format!(
            "{} is not a line command",
            crate::core::command_kind(cmd)
        )),
    }
}
