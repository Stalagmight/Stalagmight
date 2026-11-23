//! Station names at the state layer: the Set Station Name command (plan 5.3)
//! and the UI's check of a new line's name.
//!
//! A station's identity is structural on the station graph (its
//! `StationId`, shared by every slot of a junction), so there is no
//! identity epilogue any more: a command that mints a station names it.

use super::commands::{Applied, Record};
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::station_naming::name_characters_refusal;
use crate::models::survey::Project;
use crate::models::{StationId, StationRef, LINE_GONE};

impl AppState {
    /// Line names are required and unique in the project, so a user's new
    /// line must be named, and not after another line ("A line named X
    /// already exists"). The UI path (the `create_line` IPC commands) asks
    /// here; the automatic paths (imports, split and combine default names)
    /// name and de-duplicate their lines themselves. `None` when the name is
    /// fine or no project is open (the command then fails on its own).
    pub fn refuse_new_line_name(&self, name: Option<&str>) -> Option<CommandResult> {
        self.new_line_name_refusal(name)
    }
}

/// The execute pipeline's mutation for the station name family (`commands.rs`
/// dispatches here; plan 9.1). Runs inside `run_recorded`, which snapshots,
/// validates, captures and records around it. A data-table paste of names is
/// a `Batch` of these, one entry for the whole paste.
pub(super) fn apply(
    project: &mut Project,
    _record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    match cmd {
        Command::SetStationName { station_ref, name } => {
            set_station_name(project, *station_ref, name)?;
            Ok(Applied::default())
        }
        _ => Err(format!(
            "{} is not a station name command",
            crate::core::command_kind(cmd)
        )),
    }
}

/// Rename the station at `slot` to `name`, trimmed (0.6.0's
/// `set_station_name` on the station graph): the identity the slot shows is
/// renamed (RS: a junction shows its kept station on every line, a loop end
/// its closure's kept end), so every line it is shown on shows the new name.
/// `preview_station_identity` says whose station it is and every line the
/// rename changes; RS-UI asks first on a foreign slot or a loop end (RS 5.4). The name the slot already shows is a no-op before any other
/// check, as 0.6.0's execute answered it (the pipeline then records nothing
/// and keeps the redo branch). Otherwise the name must be non-empty, free of
/// control and invisible formatting characters, and shown for no other
/// station on a line that shows the renamed identity.
fn set_station_name(project: &mut Project, slot: StationRef, name: &str) -> Result<(), String> {
    let station = named_station(project, slot)?;
    let name = name.trim();
    let current = project.shown_name(station);
    if !name.is_empty() && current == Some(name) {
        return Ok(());
    }
    if let Some(refusal) = station_name_refusal(project, station, slot, name) {
        return Err(refusal);
    }
    project.set_shown_name(station, name.to_string());
    tracing::info!(
        ?slot,
        station,
        purpose = "rename station",
        "Station name updated"
    );
    Ok(())
}

/// The station a rename addresses, or 0.6.0's refusal: the line is gone, or
/// the index is past its last station.
fn named_station(project: &Project, slot: StationRef) -> Result<StationId, String> {
    if project.line(slot.line_id).is_none() {
        return Err(LINE_GONE.to_string());
    }
    project
        .station_at(slot)
        .ok_or_else(|| "Station not found".to_string())
}

/// Why `station` (addressed at `slot`) cannot be named `name` (trimmed), in
/// 0.6.0's words. A clash is reported on the slot's own line when the name is
/// taken there, else on the first other line showing the renamed name that
/// holds it.
fn station_name_refusal(
    project: &Project,
    station: StationId,
    slot: StationRef,
    name: &str,
) -> Option<String> {
    if name.is_empty() {
        return Some("A station name cannot be empty".to_string());
    }
    if let Some(refusal) = name_characters_refusal("station", name) {
        return Some(refusal);
    }
    let conflicts = project.conflicts_with_name(station, name);
    let clash = conflicts
        .iter()
        .find(|c| c.line == slot.line_id)
        .or_else(|| conflicts.first())?;
    Some(format!(
        "Line \"{}\" already has a station named \"{name}\"",
        project.line_label_of(clash.line)
    ))
}
