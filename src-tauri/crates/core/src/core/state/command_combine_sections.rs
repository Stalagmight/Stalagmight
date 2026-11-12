//! Combine Sections as one atomic undo/redo unit: the inverse of Split Section
//! Here (`command_split_section.rs`), and built the same way.

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    CombineCandidate, CombineSectionsOptions, Project, SectionId, SectionUpdates, StationRef,
};

impl AppState {
    /// Read-only: can the section ending at `station` combine with another
    /// one ending there? `None` when `station` is not a section end or nothing
    /// else touches it. Answered under the project lock without cloning the
    /// project: the context menu asks on every right-click of an end station.
    pub fn combine_candidate(
        &self,
        station: StationRef,
    ) -> Result<Option<CombineCandidate>, String> {
        self.with_open_project(|project| project.combine_candidate(station))
    }

    /// Read-only: can sections `a` and `b` be combined (the Sections panel's
    /// two-section selection)? `None` when either is missing or they are the
    /// same section.
    pub fn combine_candidate_for_sections(
        &self,
        a: SectionId,
        b: SectionId,
    ) -> Result<Option<CombineCandidate>, String> {
        self.with_open_project(|project| project.combine_candidate_for_sections(a, b))
    }

    fn with_open_project<T>(&self, read: impl FnOnce(&Project) -> T) -> Result<T, String> {
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let project = guard
            .as_ref()
            .ok_or_else(|| "No project open".to_string())?;
        Ok(read(project))
    }

    /// Combine `other` into `kept` (see `Project::combine_sections`). One undo
    /// entry (`ProjectSnapshotPair`) covers the optional reverse of `other` and
    /// the combine. A refusal (the candidate's blocked reason) changes nothing
    /// and pushes no undo entry. Returns `kept` as `section_id`.
    pub fn execute_combine_sections(
        &self,
        kept: SectionId,
        other: SectionId,
        options: CombineSectionsOptions,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6).
        if let Err(f) = self.guard_mutation("Combine sections") {
            return f;
        }
        tracing::info!(
            kept,
            other,
            "Executing combine_sections as single undo unit"
        );

        // Refuse before the snapshot: the check is read-only, so a blocked
        // combine costs no serialization and cannot touch the project.
        {
            let Ok(proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_ref() else {
                return CommandResult::failure("No project open".to_string());
            };
            match project.combine_candidate_for_sections(kept, other) {
                None => {
                    return CommandResult::failure(format!(
                        "Sections {kept} and {other} cannot be combined"
                    ))
                }
                Some(candidate) => {
                    if let Some(reason) = candidate.blocked_reason {
                        return CommandResult::failure(reason);
                    }
                }
            }
        }

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // This path bypasses `execute()`, so it carries its own frame check
        // (#469): a zero datum on the other section moves to the kept one,
        // which re-keys the frame-defining station.
        let frame_before = self.frame_datum_key();
        let label_updates = SectionUpdates {
            name: Some(Some(options.name.clone())),
            color: Some(options.color.clone()),
            ..Default::default()
        };

        {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            if let Err(e) = project.combine_sections(kept, other, &options) {
                // The model re-checks before it mutates, so this is reachable
                // only if the project changed under the check above; put it
                // back regardless.
                let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                return CommandResult::failure(e);
            }
            // No orphan-closure sweep (the split runs one): a combine joins two
            // sections and never strands a section on a lone closure.
            project.touch();
        }

        self.renormalize_frame_if_changed(frame_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };
        // The label only names the operation in history. It must not contain a
        // ReverseSection even when `other` was reversed: undo/redo settle
        // reversed sections off this command, and `other` no longer exists.
        let label = Command::Batch {
            commands: vec![
                Command::UpdateSectionMetadata {
                    section_id: kept,
                    updates: label_updates,
                },
                Command::DeleteSection { section_id: other },
            ],
        };
        self.push_snapshot_undo(label, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): set-only; a no-op for a local project.
        self.mark_remote_forward_edit();

        tracing::info!(kept, other, "Combine sections completed");
        CommandResult::success()
            .with_section_id(kept)
            .with_section_ids(vec![kept])
    }
}
