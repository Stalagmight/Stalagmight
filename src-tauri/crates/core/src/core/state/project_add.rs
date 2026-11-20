//! "Add .stalx to current project" (#674): append another project file's
//! survey into the OPEN project, as one undoable command.
//!
//! PARKED by the station-graph switch (C0): `Command::AddProjectFile` reaches
//! `execute_add_project_file` (after the edit gates) and refuses with "not
//! yet ported". The port
//! (onto `survey::append_project` and the pipeline's `run_recorded`, with the
//! source's ignores carried into the in-memory `ignored_warnings`) is a work
//! package of the C0 breakdown; the 0.6.0 module is
//! `git show ee49b1299:src-tauri/crates/core/src/core/state/project_add.rs`.

use super::AppState;
use crate::core::{Command, CommandResult};

impl AppState {
    /// `Command::AddProjectFile` (reached through `execute`, after its gates):
    /// read and migrate the source outside the project lock, then append it
    /// through `run_recorded` as one undo step. Not ported in C0.
    pub(super) fn execute_add_project_file(&self, cmd: &Command) -> CommandResult {
        let _ = cmd;
        crate::core::stub::not_ported("project_add::execute_add_project_file")
    }
}
