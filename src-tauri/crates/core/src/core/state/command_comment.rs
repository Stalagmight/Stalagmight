use super::AppState;
use crate::core::{Command, CommandResult, CommentHolder};
use crate::models::{Project, StationRef};

impl AppState {
    pub(super) fn set_station_comment(
        &self,
        station_ref: StationRef,
        comment: Option<String>,
    ) -> CommandResult {
        let _ = (&station_ref, &comment);
        crate::core::stub::not_ported("command_comment::set_station_comment")
    }

    /// Execute a move-comment operation as a single atomic undo/redo unit.
    /// Handles its own undo push (like `execute_batch` / `execute_delete_shot_and_split`).
    pub(super) fn execute_move_comment(
        &self,
        source: CommentHolder,
        target: CommentHolder,
        swap: bool,
        cmd: Command,
    ) -> CommandResult {
        let _ = (&source, &target, &swap, &cmd);
        crate::core::stub::not_ported("command_comment::execute_move_comment")
    }

    /// Read the comment from a CommentHolder (station or shot)
    fn get_holder_comment(
        project: &Project,
        holder: &CommentHolder,
    ) -> Result<Option<String>, String> {
        match holder {
            CommentHolder::Station { station_ref } => {
                // Validate station exists
                let line = project
                    .get_line(station_ref.line_id)
                    .ok_or_else(|| crate::models::LINE_GONE.to_string())?;
                let max_station_index = line.shots.len() as u32;
                if station_ref.station_index > max_station_index {
                    return Err(format!(
                        "Station index {} out of range",
                        station_ref.station_index
                    ));
                }
                Ok(project
                    .key_for_ref(*station_ref)
                    .and_then(|key| project.station_comments.get(&key).cloned()))
            }
            CommentHolder::Shot { line_id, shot_id } => {
                let line = project
                    .get_line(*line_id)
                    .ok_or_else(|| crate::models::LINE_GONE.to_string())?;
                let shot = line
                    .get_shot(*shot_id)
                    .ok_or_else(|| crate::models::SHOT_GONE.to_string())?;
                Ok(shot.comment.clone())
            }
        }
    }

    /// Set the comment on a CommentHolder (station or shot)
    fn set_holder_comment(
        project: &mut Project,
        holder: &CommentHolder,
        comment: Option<String>,
    ) -> Result<(), String> {
        match holder {
            CommentHolder::Station { station_ref } => {
                match comment {
                    Some(text) if !text.is_empty() => {
                        if let Some(key) = project.key_for_ref(*station_ref) {
                            project.station_comments.insert(key, text);
                        }
                    }
                    _ => {
                        if let Some(key) = project.key_for_ref(*station_ref) {
                            project.station_comments.remove(&key);
                        }
                    }
                }
                Ok(())
            }
            CommentHolder::Shot { line_id, shot_id } => {
                let line = project
                    .get_line_mut(*line_id)
                    .ok_or_else(|| crate::models::LINE_GONE.to_string())?;
                let shot = line
                    .shots
                    .iter_mut()
                    .find(|s| s.id == *shot_id)
                    .ok_or_else(|| crate::models::SHOT_GONE.to_string())?;
                match comment {
                    Some(text) if !text.is_empty() => {
                        shot.comment = Some(text);
                    }
                    _ => {
                        shot.comment = None;
                    }
                }
                Ok(())
            }
        }
    }
}

/// The execute pipeline's mutation for the comment family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it; `record` is the entry being
/// recorded (worded before the edit), for rows only the result can say.
pub(super) fn apply(
    project: &mut crate::models::survey::Project,
    record: &mut super::commands::Record,
    cmd: &Command,
) -> Result<super::commands::Applied, String> {
    let _ = (&mut *project, &mut *record);
    match cmd {
        Command::SetStationComment { .. } => {
            Err(super::commands::not_ported("execute::set_station_comment"))
        }
        Command::MoveComment { .. } => Err(super::commands::not_ported("execute::move_comment")),
        _ => Err(format!(
            "{} is not a comment command",
            crate::core::command_kind(cmd)
        )),
    }
}
