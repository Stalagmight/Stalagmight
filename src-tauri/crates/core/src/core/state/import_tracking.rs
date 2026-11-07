use anyhow::Result;

use crate::core::CommandResult;

use super::AppState;

impl AppState {
    /// Check if a file hash has already been imported to the project
    pub fn check_duplicate_import(&self, hash: &str) -> Result<bool> {
        let proj_guard = self.project.lock();

        if let Some(project) = proj_guard.as_ref() {
            Ok(project.imported_file_hashes.contains(&hash.to_string()))
        } else {
            Ok(false)
        }
    }

    /// Track an imported file hash in the project
    pub fn track_imported_file(&self, hash: String) -> CommandResult {
        let mut proj_guard = self.project.lock();

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

    /// Batch-check which section hashes have already been imported.
    /// Returns a parallel `Vec<bool>` (true = already imported).
    pub fn check_section_hashes(&self, hashes: &[String]) -> Result<Vec<bool>> {
        let proj_guard = self.project.lock();

        if let Some(project) = proj_guard.as_ref() {
            Ok(hashes
                .iter()
                .map(|h| project.imported_section_hashes.contains(h))
                .collect())
        } else {
            Ok(vec![false; hashes.len()])
        }
    }

    /// Forget import-dedup bookkeeping for an import that was WITHDRAWN (#394).
    ///
    /// The inverse of [`Self::track_imported_file`] + [`Self::track_section_hashes`],
    /// for the one caller that removes the sections an import created and needs
    /// the file to stop counting as imported. Deleting a section does not touch
    /// these lists - they are project-level bookkeeping, not references - so
    /// without this the next attempt at the same file arrives pre-deselected.
    ///
    /// Silent about hashes it does not hold: the caller names what its own run
    /// sent to `import_sections`, and whether each was still tracked is not
    /// something it can know.
    pub fn untrack_import(
        &self,
        file_hashes: &[String],
        section_hashes: &[String],
    ) -> CommandResult {
        let mut proj_guard = self.project.lock();

        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };

        let before = project.imported_file_hashes.len() + project.imported_section_hashes.len();
        project
            .imported_file_hashes
            .retain(|h| !file_hashes.contains(h));
        project
            .imported_section_hashes
            .retain(|h| !section_hashes.contains(h));
        let removed =
            before - project.imported_file_hashes.len() - project.imported_section_hashes.len();
        if removed > 0 {
            project.touch();
            tracing::info!(
                removed,
                purpose = "let a withdrawn import be re-imported",
                "Forgot import tracking for withdrawn sections"
            );
        }
        CommandResult::success()
    }

    /// Batch-track imported section hashes, skipping duplicates.
    pub fn track_section_hashes(&self, hashes: Vec<String>) -> CommandResult {
        let mut proj_guard = self.project.lock();

        if let Some(project) = proj_guard.as_mut() {
            let mut added = 0usize;
            for hash in hashes {
                if !project.imported_section_hashes.contains(&hash) {
                    project.imported_section_hashes.push(hash);
                    added += 1;
                }
            }
            if added > 0 {
                project.touch();
                tracing::info!(added, "Tracked imported section hashes");
            }
            CommandResult::success()
        } else {
            CommandResult::failure("No project open".to_string())
        }
    }
}
