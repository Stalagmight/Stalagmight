//! `Command::SetStationName`: set or clear the free-text name a station carries
//! beside its positional `S:I` id (`Project::station_names`). The twin of
//! `SetStationComment` (`command_comment.rs`), keyed the same way through
//! `Project::key_for_ref`.

use super::AppState;
use crate::core::CommandResult;
use crate::models::{sanitize_station_name, Project, StationRef};

impl AppState {
    /// Forward edit: store `name` as [`sanitize_station_name`] leaves it, or
    /// remove the station's name when nothing is left (`None`, empty or
    /// whitespace-only). Fails, changing nothing, when the section is missing or
    /// the index is past the section's last station.
    pub(super) fn set_station_name(
        &self,
        station_ref: StationRef,
        name: Option<&str>,
    ) -> CommandResult {
        let name = name.and_then(sanitize_station_name);
        self.with_project_mut(|project| write_station_name(project, station_ref, name))
    }

    /// Inverse of [`Self::set_station_name`]: put the recorded previous name back
    /// byte for byte. An inverse RESTORES rather than re-derives: re-running the
    /// forward setter would trim and clean the recorded value, so a name a file
    /// stored untrimmed (or with a control character the edit path never admits)
    /// would not come back as it was.
    pub(super) fn restore_station_name(
        &self,
        station_ref: StationRef,
        name: Option<String>,
    ) -> CommandResult {
        self.with_project_mut(|project| write_station_name(project, station_ref, name))
    }
}

/// Validate `station_ref` and write `name` (or remove the entry for `None`).
fn write_station_name(
    project: &mut Project,
    station_ref: StationRef,
    name: Option<String>,
) -> CommandResult {
    let Some(section) = project.get_section(station_ref.section_id) else {
        return CommandResult::failure(format!("Section {} not found", station_ref.section_id));
    };
    let last_station = section.shots.len() as u32;
    if station_ref.station_index > last_station {
        return CommandResult::failure(format!(
            "Station index {} out of range (section has {} stations)",
            station_ref.station_index,
            last_station + 1
        ));
    }
    // A station past the first is keyed by the uuid of the shot that ends at it.
    // A shot with no uuid has no stable identity to key a name to; refuse rather
    // than report success for a name that was never stored.
    let Some(key) = project.key_for_ref(station_ref) else {
        return CommandResult::failure(format!(
            "Station {station_ref} has no stable identity to name"
        ));
    };
    match name {
        Some(text) => {
            project.station_names.insert(key, text);
        }
        None => {
            project.station_names.remove(&key);
        }
    }
    project.touch();
    tracing::info!(
        ?station_ref,
        purpose = "set station name",
        "Station name updated"
    );
    CommandResult::success()
}
