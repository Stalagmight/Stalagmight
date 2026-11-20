use anyhow::Result;

use crate::core::CommandResult;

use super::AppState;

impl AppState {
    /// Check if a file hash has already been imported to the project
    pub fn check_duplicate_import(&self, hash: &str) -> Result<bool> {
        let proj_guard = self
            .project
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock project"))?;

        if let Some(project) = proj_guard.as_ref() {
            Ok(project.imported_file_hashes.contains(&hash.to_string()))
        } else {
            Ok(false)
        }
    }

    /// Track an imported file hash in the project
    pub fn track_imported_file(&self, hash: String) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        if let Some(project) = proj_guard.as_mut() {
            if !project.imported_file_hashes.contains(&hash) {
                project.imported_file_hashes.push(hash);
                project.touch();
                tracing::info!("Tracked imported file hash");
            }
            CommandResult::success()
        } else {
            CommandResult::failure("No project open".to_string())
        }
    }

    /// Batch-check which line hashes have already been imported.
    /// Returns a parallel `Vec<bool>` (true = already imported).
    pub fn check_line_hashes(&self, hashes: &[String]) -> Result<Vec<bool>> {
        let proj_guard = self
            .project
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock project"))?;

        if let Some(project) = proj_guard.as_ref() {
            Ok(hashes
                .iter()
                .map(|h| project.imported_line_hashes.contains(h))
                .collect())
        } else {
            Ok(vec![false; hashes.len()])
        }
    }

    /// Forget import-dedup bookkeeping for an import that was WITHDRAWN (#394).
    ///
    /// The inverse of [`Self::track_imported_file`] + [`Self::track_line_hashes`],
    /// for the one caller that removes the lines an import created and needs
    /// the file to stop counting as imported. Deleting a line does not touch
    /// these lists - they are project-level bookkeeping, not references - so
    /// without this the next attempt at the same file arrives pre-deselected.
    ///
    /// Silent about hashes it does not hold: the caller names what its own run
    /// sent to `import_lines`, and whether each was still tracked is not
    /// something it can know.
    pub fn untrack_import(&self, file_hashes: &[String], line_hashes: &[String]) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        let before = project.imported_file_hashes.len() + project.imported_line_hashes.len();
        project
            .imported_file_hashes
            .retain(|h| !file_hashes.contains(h));
        project
            .imported_line_hashes
            .retain(|h| !line_hashes.contains(h));
        let removed =
            before - project.imported_file_hashes.len() - project.imported_line_hashes.len();
        if removed > 0 {
            project.touch();
            tracing::info!(
                removed,
                purpose = "let a withdrawn import be re-imported",
                "Forgot import tracking for withdrawn lines"
            );
        }
        CommandResult::success()
    }

    /// Batch-track imported line hashes, skipping duplicates.
    pub fn track_line_hashes(&self, hashes: Vec<String>) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };

        if let Some(project) = proj_guard.as_mut() {
            let mut added = 0usize;
            for hash in hashes {
                if !project.imported_line_hashes.contains(&hash) {
                    project.imported_line_hashes.push(hash);
                    added += 1;
                }
            }
            if added > 0 {
                project.touch();
                tracing::info!(added, "Tracked imported line hashes");
            }
            CommandResult::success()
        } else {
            CommandResult::failure("No project open".to_string())
        }
    }
}

/// The execute pipeline's mutation for the import tracking family (`commands.rs`
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
        crate::core::Command::UntrackImport { .. } => {
            Err(super::commands::not_ported("execute::untrack_import"))
        }
        _ => Err(format!(
            "{} is not a import tracking command",
            crate::core::command_kind(cmd)
        )),
    }
}
