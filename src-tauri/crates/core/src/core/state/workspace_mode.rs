//! [#576 C5] The frontend's workspace mode, mirrored so the mutation chokepoint
//! can refuse an edit that reaches the backend while the workspace is read-only.
//!
//! Plan and Export are read-only workspaces (docs/app-modes.md). The frontend
//! hides and disables the editing controls there, but before this mirror the
//! backend had no idea which mode it was in, so any gap in that gating became a
//! real edit to the survey with undo hidden. The frontend now reports every mode
//! change (`set_workspace_mode`), every project-slot commit (both loads, both
//! creates, close) resets it to Survey - the frontend's own reset on load - and
//! `guard_mutation` refuses every guarded entry point while it is Plan or Export.
//!
//! Deny by default, with a short allowlist: the project name and validation
//! thresholds (`update_project_settings`) and the project rename
//! (`set_project_name_for_path`) are deliberately editable in every mode
//! (ProjectSettingsDialog keeps them enabled there), so those two call
//! [`AppState::guard_metadata_mutation`] instead. A new entry point that calls
//! `guard_mutation` is refused in Plan/Export without anyone having to remember.
//!
//! Survey's Closures Computed View is NOT mirrored: it is a Survey sub-view whose
//! undo/redo carve-out (docs/app-modes.md) must keep working, and the backend
//! cannot tell that undo from any other. It stays gated by the UI alone.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::Deserialize;

use super::AppState;
use crate::core::{CommandError, CommandErrorCode};

/// The three workspace modes, spelled as the frontend's `AppMode` spells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMode {
    Survey,
    Plan,
    Export,
}

impl WorkspaceMode {
    /// Whether survey edits are refused in this mode.
    pub fn is_read_only(self) -> bool {
        !matches!(self, Self::Survey)
    }

    /// The mode's name as the mode switcher shows it.
    fn label(self) -> &'static str {
        match self {
            Self::Survey => "Survey",
            Self::Plan => "Plan",
            Self::Export => "Export",
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            Self::Survey => 0,
            Self::Plan => 1,
            Self::Export => 2,
        }
    }

    fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Plan,
            2 => Self::Export,
            _ => Self::Survey,
        }
    }
}

/// The mirrored mode. Transient, never persisted, Survey by default: no project
/// is open at startup and a project always opens in Survey.
#[derive(Debug, Default)]
pub(super) struct WorkspaceModeCell(AtomicU8);

impl WorkspaceModeCell {
    fn get(&self) -> WorkspaceMode {
        WorkspaceMode::from_bits(self.0.load(Ordering::SeqCst))
    }

    fn set(&self, mode: WorkspaceMode) {
        self.0.store(mode.to_bits(), Ordering::SeqCst);
    }
}

impl AppState {
    /// Record the workspace mode the frontend is now in (`set_workspace_mode`).
    pub fn set_workspace_mode(&self, mode: WorkspaceMode) {
        let previous = self.workspace_mode.get();
        self.workspace_mode.set(mode);
        if previous != mode {
            tracing::debug!(
                from = previous.label(),
                to = mode.label(),
                purpose = "mirror the frontend workspace mode for the mutation gate",
                "Workspace mode changed"
            );
        }
    }

    /// The workspace mode the frontend last reported (Survey after any project
    /// open, create or close).
    pub fn workspace_mode(&self) -> WorkspaceMode {
        self.workspace_mode.get()
    }

    /// Back to Survey on every project-slot commit (both loads, both creates,
    /// close), matching the frontend, which opens every project in Survey. Called
    /// beside `set_newer_file_schema` at each of those commits.
    pub(super) fn reset_workspace_mode(&self) {
        self.workspace_mode.set(WorkspaceMode::Survey);
    }

    /// The workspace half of `guard_mutation`: `Err` with the `WORKSPACE_READ_ONLY`
    /// refusal while the workspace is Plan or Export.
    pub(super) fn guard_workspace(&self, what: &str) -> Result<(), CommandError> {
        let mode = self.workspace_mode();
        if !mode.is_read_only() {
            return Ok(());
        }
        tracing::info!(
            what,
            mode = mode.label(),
            purpose = "backstop the read-only Plan/Export workspace",
            "Mutation refused: the workspace is read-only"
        );
        Err(CommandError::new(
            CommandErrorCode::WorkspaceReadOnly,
            format!(
                "{what} is disabled: the {} \
             workspace is read-only. Switch to Survey to edit.",
                mode.label()
            ),
        ))
    }
}
