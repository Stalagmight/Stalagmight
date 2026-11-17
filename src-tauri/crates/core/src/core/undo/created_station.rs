//! The identity of the station a `CreateShot` created, recorded on its
//! `UndoData::CreatedShotSnapshot` so the redo puts the station back with
//! the same Station ID and name (an undo removes the station, and the
//! station-identity epilogue would otherwise give the re-created one a new id).

use serde::{Deserialize, Serialize};

use crate::models::{Line, LineId, Project, StationId, StationRef};

/// A created station's Station ID and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedStation {
    pub id: StationId,
    pub name: String,
}

impl CreatedStation {
    /// The station the shot at `shot_index` of `line` created: the line's
    /// first station when the shot is first in a line of several (only a
    /// prepend puts a shot there, and a prepend's new station is the new
    /// start), otherwise the station after the shot.
    pub fn station_of(line: &Line, shot_index: usize) -> StationRef {
        let index = if shot_index == 0 && line.shots.len() > 1 {
            0
        } else {
            shot_index as u32 + 1
        };
        StationRef::new(line.id, index)
    }

    /// Read the identity of the station the shot `shot_index` of `line_id`
    /// created, off a settled project.
    pub fn read(project: &Project, line_id: LineId, shot_index: usize) -> Option<Self> {
        let station = Self::station_of(project.get_line(line_id)?, shot_index);
        Some(Self {
            id: project.station_id_at(station)?,
            name: project.station_name_at(station)?.to_string(),
        })
    }

    /// Put the recorded identity back on the re-created station of the shot
    /// with `shot_uuid` in `line_id`, replacing what the re-insert named it.
    /// An id another station holds by now is left to the epilogue, which
    /// keeps ids unique.
    pub fn restore(&self, project: &mut Project, line_id: LineId, shot_uuid: &str) {
        let Some(line) = project.get_line(line_id) else {
            return;
        };
        let Some(shot_index) = line
            .shots
            .iter()
            .position(|s| s.uuid.as_deref() == Some(shot_uuid))
        else {
            return;
        };
        let Some(key) = project.key_for_ref(Self::station_of(line, shot_index)) else {
            return;
        };
        if !project.station_ids.values().any(|id| *id == self.id) {
            project.station_ids.insert(key.clone(), self.id);
            project.next_station_id = project.next_station_id.max(self.id.saturating_add(1));
        }
        project.station_names.insert(key, self.name.clone());
    }
}
