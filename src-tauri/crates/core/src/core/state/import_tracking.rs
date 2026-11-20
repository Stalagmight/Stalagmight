//! Import dedup bookkeeping: the file hashes and line content hashes an
//! import records so a re-import is recognised (the DMP wizard deselects what
//! it already has).
//!
//! The two lists are survey content (`Project::imported_file_hashes` /
//! `imported_line_hashes`, captured by the undo engine's `ImportHashes`
//! lists), so every change to them is a recorded edit: an import records
//! them inside its own entry, a withdrawal (#394) forgets them with
//! `Command::UntrackImport` in the same batch as its deletes, and the
//! stand-alone track calls record one entry each. A change made behind the
//! history's back would make the next undo of an entry that touched the
//! lists refuse (its recorded list no longer matches the live one).

use anyhow::Result;

use super::commands::{Applied, Record};
use super::import_funnel::track_hashes;
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::changeset::HistoryRefs;
use crate::models::survey::Project;

impl AppState {
    /// Check if a file hash has already been imported to the project
    pub fn check_duplicate_import(&self, hash: &str) -> Result<bool> {
        let proj_guard = self
            .project
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock project"))?;
        Ok(proj_guard
            .as_ref()
            .is_some_and(|project| project.imported_file_hashes.iter().any(|h| h == hash)))
    }

    /// Batch-check which line hashes have already been imported.
    /// Returns a parallel `Vec<bool>` (true = already imported).
    pub fn check_line_hashes(&self, hashes: &[String]) -> Result<Vec<bool>> {
        let proj_guard = self
            .project
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock project"))?;
        Ok(match proj_guard.as_ref() {
            Some(project) => {
                let held: std::collections::HashSet<&str> = project
                    .imported_line_hashes
                    .iter()
                    .map(String::as_str)
                    .collect();
                hashes.iter().map(|h| held.contains(h.as_str())).collect()
            }
            None => vec![false; hashes.len()],
        })
    }

    /// Track an imported file hash in the project (one undo entry; nothing
    /// is recorded when the hash is already tracked).
    pub fn track_imported_file(&self, hash: String) -> CommandResult {
        self.record_tracking("Track imported file", |project| {
            track_hashes(project, Some(&hash), &[]);
        })
    }

    /// Batch-track imported line hashes, skipping duplicates (one undo
    /// entry; nothing is recorded when every hash is already tracked).
    pub fn track_line_hashes(&self, hashes: Vec<String>) -> CommandResult {
        self.record_tracking("Track imported lines", |project| {
            track_hashes(project, None, &hashes);
        })
    }

    /// Run a tracking change as a recorded edit (gated like every edit).
    fn record_tracking(&self, label: &str, change: impl FnOnce(&mut Project)) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        if let Err(f) = self.guard_mutation(label) {
            return f;
        }
        let recorded = self.run_recorded(
            |_| Record {
                kind: "track_import".to_string(),
                label: label.to_string(),
                refs: HistoryRefs::default(),
                ..Record::default()
            },
            |project, _| {
                change(project);
                Ok(())
            },
        );
        match recorded {
            Ok(()) => {
                tracing::info!(what = label, "Tracked import hashes");
                CommandResult::success()
            }
            Err(failure) => failure,
        }
    }
}

/// Forget import-dedup bookkeeping for an import that was WITHDRAWN (#394):
/// the inverse of the tracking an import does, for the one caller that
/// removes the lines an import created and needs the file to stop counting
/// as imported. Deleting a line does not touch these lists (they are
/// project-level bookkeeping, not references), so without this the next
/// attempt at the same file arrives pre-deselected.
///
/// Silent about hashes it does not hold: the caller names what its own run
/// sent to `import_lines`, and whether each was still tracked is not
/// something it can know. Returns how many entries it removed.
fn untrack(project: &mut Project, file_hashes: &[String], line_hashes: &[String]) -> usize {
    let before = project.imported_file_hashes.len() + project.imported_line_hashes.len();
    project
        .imported_file_hashes
        .retain(|h| !file_hashes.contains(h));
    project
        .imported_line_hashes
        .retain(|h| !line_hashes.contains(h));
    before - project.imported_file_hashes.len() - project.imported_line_hashes.len()
}

/// The execute pipeline's mutation for the import tracking family
/// (`commands.rs` dispatches here; plan 9.1). Runs inside `run_recorded`,
/// which snapshots, validates, captures and records around it: on its own or
/// in a withdrawal batch, an untrack is undoable (0.6.0's stand-alone
/// `UntrackImport` pushed no entry).
pub(super) fn apply(
    project: &mut Project,
    _record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    match cmd {
        Command::UntrackImport {
            file_hashes,
            line_hashes,
        } => {
            let removed = untrack(project, file_hashes, line_hashes);
            if removed > 0 {
                tracing::info!(
                    removed,
                    purpose = "let a withdrawn import be re-imported",
                    "Forgot import tracking for withdrawn lines"
                );
            }
            Ok(Applied::default())
        }
        _ => Err(format!(
            "{} is not an import tracking command",
            crate::core::command_kind(cmd)
        )),
    }
}
