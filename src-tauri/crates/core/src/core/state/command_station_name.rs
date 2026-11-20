//! Station identity at the state layer: the Set Station Name command and the
//! normalization epilogue every mutation path runs
//! ([`AppState::settle_station_identities`]).

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{LineId, LineUpdates, Project, StationRef};

impl AppState {
    /// Rename a station: trimmed, non-empty and unique within its line
    /// (`Project::validate_station_name`), so a duplicate is refused with
    /// "Line X already has a station named Y".
    pub(super) fn set_station_name(&self, station_ref: StationRef, name: &str) -> CommandResult {
        let _ = (&station_ref, &name);
        crate::core::stub::not_ported("command_station_name::set_station_name")
    }

    /// Line names are required and unique in the project, so a user's new
    /// line must be named, and not after another line ("A line named X
    /// already exists"). The UI path (the `create_line` IPC commands) asks
    /// here; the automatic paths (imports, split and combine default names)
    /// are de-duplicated by the epilogue instead.
    pub fn refuse_new_line_name(&self, name: Option<&str>) -> Option<CommandResult> {
        self.with_open_project(|project| {
            project
                .validate_line_name(name.unwrap_or_default(), None)
                .err()
                .map(CommandResult::failure)
        })
    }

    /// A user rename of a line ([`Command::UpdateLineMetadata`] carrying a
    /// name) must keep it named and unique, as [`Self::refuse_new_line_name`].
    /// Asked by the forward command only - an undo or redo puts back a name the
    /// project already held - and `None` for any other command.
    pub(super) fn refuse_line_rename(&self, cmd: &Command) -> Option<CommandResult> {
        let Command::UpdateLineMetadata {
            line_id,
            updates: LineUpdates {
                name: Some(name), ..
            },
        } = cmd
        else {
            return None;
        };
        let line_id: LineId = *line_id;
        self.with_open_project(|project| {
            project.get_line(line_id)?;
            project
                .validate_line_name(name.as_deref().unwrap_or_default(), Some(line_id))
                .err()
                .map(CommandResult::failure)
        })
    }

    /// `f` on the open project; `None` when there is none (the command then
    /// fails on its own, with the usual message).
    fn with_open_project<R>(&self, f: impl FnOnce(&Project) -> Option<R>) -> Option<R> {
        let _ = (&f,);
        crate::core::stub::not_ported("command_station_name::with_open_project")
    }

    /// The station-identity epilogue: give every station that lacks one a
    /// name and a Station ID, and every line a unique name
    /// (`Project::ensure_station_identities`). Run after every successful
    /// mutating command, before its undo entry is finalized, after every
    /// undo/redo step, and on the after-snapshot of every command that records
    /// a snapshot pair, so the id an edit allocates is part of the state its
    /// undo entry recorded. Leaves a valid id or name untouched. Costs
    /// O(stations) even when nothing is missing, so a forward command that
    /// cannot change station identity skips it
    /// (`Command::can_change_station_identity`).
    pub(super) fn settle_station_identities(&self) {
        crate::core::stub::not_ported::<()>("command_station_name::settle_station_identities");
    }
}

/// [`AppState::settle_station_identities`] on a project already held.
pub(super) fn settle(project: &mut Project) {
    project.ensure_station_identities();
}

/// The execute pipeline's mutation for the station name family (`commands.rs`
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
        Command::SetStationName { .. } => {
            Err(super::commands::not_ported("execute::set_station_name"))
        }
        _ => Err(format!(
            "{} is not a station name command",
            crate::core::command_kind(cmd)
        )),
    }
}
