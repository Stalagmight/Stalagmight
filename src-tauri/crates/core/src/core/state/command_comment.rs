use super::AppState;
use crate::core::{Command, CommandResult, CommentHolder};
use crate::models::{Project, StationRef};

impl AppState {
    pub(super) fn set_station_comment(
        &self,
        station_ref: StationRef,
        comment: Option<String>,
    ) -> CommandResult {
        self.with_project_mut(|project| {
            // Validate station exists
            let line = match project.get_line(station_ref.line_id) {
                Some(s) => s,
                None => {
                    return CommandResult::failure(format!(
                        "Line {} not found",
                        station_ref.line_id
                    ));
                }
            };

            let max_station_index = line.shots.len() as u32;
            if station_ref.station_index > max_station_index {
                return CommandResult::failure(format!(
                    "Station index {} out of range (line has {} stations)",
                    station_ref.station_index,
                    max_station_index + 1
                ));
            }

            match comment {
                Some(text) if !text.is_empty() => {
                    if let Some(key) = project.key_for_ref(station_ref) {
                        project.station_comments.insert(key, text);
                    }
                }
                _ => {
                    if let Some(key) = project.key_for_ref(station_ref) {
                        project.station_comments.remove(&key);
                    }
                }
            }

            project.touch();
            tracing::info!(
                ?station_ref,
                purpose = "set station comment",
                "Station comment updated"
            );
            CommandResult::success()
        })
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
        // [Track B] (#191, D-B6) This is reached ONLY via execute()'s
        // `Command::MoveComment` arm (early `return self.execute_move_comment(..)`),
        // already under execute()'s command_lock AND past its read-only edit gate
        // (MoveComment is not a lifecycle pass-through). It takes no command_lock
        // of its own, so it inherits the guard rather than calling it directly. If
        // a future DIRECT caller is added, it MUST route through guard_mutation -
        // the chokepoint enumeration test fails if MoveComment leaks past execute.
        debug_assert!(
            self.edit_enabled(),
            "execute_move_comment reached with edit disabled - it must run only via execute()'s gate"
        );
        // Step 1: Take BEFORE snapshot
        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Step 2: Read source and target comments, then mutate
        {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };

            let source_comment = match Self::get_holder_comment(project, &source) {
                Ok(c) => c,
                Err(e) => return CommandResult::failure(e),
            };
            let target_comment = match Self::get_holder_comment(project, &target) {
                Ok(c) => c,
                Err(e) => return CommandResult::failure(e),
            };

            // Source must have a comment
            let Some(comment_text) = source_comment else {
                return CommandResult::failure("Source has no comment to move".to_string());
            };

            // Set target = source's comment
            if let Err(e) = Self::set_holder_comment(project, &target, Some(comment_text)) {
                return CommandResult::failure(e);
            }

            // Set source = target's comment (swap) or None (move)
            let new_source_comment = if swap { target_comment } else { None };
            if let Err(e) = Self::set_holder_comment(project, &source, new_source_comment) {
                return CommandResult::failure(e);
            }

            project.touch();
        }

        // Step 3: Take AFTER snapshot
        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };

        // Step 4: Push single undo entry
        self.push_snapshot_undo(cmd, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): moving a comment mutated a remote project.
        // Set-only; no-op for local-only.
        self.mark_remote_forward_edit();

        tracing::info!(
            swap = swap,
            purpose = "move comment atomically",
            "Move comment completed"
        );
        CommandResult::success()
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
                    .ok_or_else(|| format!("Line {} not found", station_ref.line_id))?;
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
                    .ok_or_else(|| format!("Line {line_id} not found"))?;
                let shot = line
                    .get_shot(*shot_id)
                    .ok_or_else(|| format!("Shot {shot_id} not found in line {line_id}"))?;
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
                    .ok_or_else(|| format!("Line {line_id} not found"))?;
                let shot = line
                    .shots
                    .iter_mut()
                    .find(|s| s.id == *shot_id)
                    .ok_or_else(|| format!("Shot {shot_id} not found in line {line_id}"))?;
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
