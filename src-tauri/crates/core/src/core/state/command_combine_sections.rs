//! Combine Sections (#707) as one atomic undo/redo unit, plus its read-only
//! eligibility queries.

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{CombineCandidate, CombineOptions, SectionId, StationRef};

impl AppState {
    /// Join `other` onto `kept`, which keeps its id and direction. One undo
    /// entry (`ProjectSnapshotPair`) covers the optional reverse of `other` and
    /// the join. Returns `kept` as `section_id`.
    pub fn execute_combine_sections(
        &self,
        kept: SectionId,
        other: SectionId,
        options: CombineOptions,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        if let Err(f) = self.guard_mutation("Combine sections") {
            return f;
        }
        tracing::info!(
            kept,
            other,
            "Executing combine_sections as single undo unit"
        );

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // This path bypasses `execute()`, so it carries its own frame check
        // (#469): dropping the other end's zero datum can re-define the frame.
        let frame_before = self.frame_datum_key();

        {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            if let Err(e) = project.combine_sections(kept, other, &options) {
                let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                return CommandResult::failure(e);
            }
            project.touch();
        }

        self.renormalize_frame_if_changed(frame_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };
        // The label only names the operation in history. It must not contain a
        // ReverseSection: undo/redo settle reversed sections off this command.
        let label = Command::Batch {
            commands: vec![Command::DeleteSection { section_id: other }],
        };
        self.push_snapshot_undo(label, before_snapshot, after_snapshot);
        self.mark_remote_forward_edit();

        tracing::info!(kept, other, "Combine sections completed");
        CommandResult::success().with_section_id(kept)
    }

    /// Whether two sections can be combined (read-only); `None` when they do
    /// not meet end to end.
    pub fn combine_candidate(
        &self,
        a: SectionId,
        b: SectionId,
    ) -> Result<Option<CombineCandidate>, String> {
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let project = guard
            .as_ref()
            .ok_or_else(|| "No project open".to_string())?;
        Ok(project.combine_candidate(a, b))
    }

    /// The combine offered at a section-end station (read-only); `None` unless
    /// exactly one other section's end meets it there.
    pub fn combine_candidate_at_station(
        &self,
        station: StationRef,
    ) -> Result<Option<CombineCandidate>, String> {
        let guard = self
            .project
            .lock()
            .map_err(|_| "Failed to lock project".to_string())?;
        let project = guard
            .as_ref()
            .ok_or_else(|| "No project open".to_string())?;
        Ok(project.combine_candidate_at_station(station))
    }
}
