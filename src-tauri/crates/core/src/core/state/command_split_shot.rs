//! Insert Station (#59): split one shot at a distance into two collinear
//! shots with a new station between them, as one recorded edit
//! (`survey::Project::split_shot_at_distance` on `run_recorded`).

use super::command_delete_split::recipe_record;
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::changeset::PreviewRow;
use crate::models::survey::Project;
use crate::models::{LineId, ShotData, ShotId, ShotUpdates, StationId};

/// `Insert station Main Line / 4a`: the history label, read after the split
/// (the station's name is the one the split gave it).
fn insert_station_label(project: &Project, line: LineId, station: StationId) -> String {
    if project.line(line).is_some() && project.stations.contains_key(&station) {
        format!(
            "Insert station {}",
            project.station_text(station, Some(line))
        )
    } else {
        format!("Insert station in line \"{}\"", project.line_label_of(line))
    }
}

/// 0.6.0's batch preview detail of the new head shot.
fn created_details(data: &ShotData) -> String {
    format!(
        "Length: {:.2}, Azimuth: {:.1}",
        data.length, data.azimuth_in
    )
}

/// 0.6.0's batch preview detail of the tail's updates.
fn updated_details(updates: &ShotUpdates) -> String {
    let mut parts = Vec::new();
    if let Some(length) = updates.length {
        parts.push(format!("Length: {length:.2}"));
    }
    if let Some(azimuth) = updates.azimuth_in {
        parts.push(format!("Azimuth: {azimuth:.1}"));
    }
    if let Some(depth) = updates.depth_in {
        parts.push(format!("Depth: {depth:.2}"));
    }
    parts.join(", ")
}

impl AppState {
    /// Insert a station mid-shot (issue #59) as one undo/redo unit: splits
    /// `shot_id` at `distance` metres from its start into two collinear
    /// shots (`survey::Project::split_shot_at_distance`: a new head shot
    /// with a fresh uuid, the old shot kept as the tail with its id, uuid,
    /// comment and marker). Returns the head's session id as `shot_id`.
    ///
    /// The history entry reads as 0.6.0's: kind `batch`, labelled `Insert
    /// station Main Line / 4a` after the station the split made, with the
    /// two preview rows of the shot it created and the shot it shortened.
    pub fn execute_split_shot_at_distance(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        distance: f64,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Insert station (split shot)") {
            return f;
        }
        if !distance.is_finite() {
            return CommandResult::failure("Distance must be finite".to_string());
        }
        tracing::info!(
            line_id,
            shot_id,
            distance,
            purpose = "insert station mid-shot",
            "Executing split_shot_at_distance as single undo unit"
        );
        let result = self.run_recorded(
            |project| {
                recipe_record(
                    project,
                    format!(
                        "Insert station in line \"{}\"",
                        project.line_label_of(line_id)
                    ),
                    line_id,
                )
            },
            |project, record| {
                // Frozen-geometry guard (S12): a split changes both
                // children's geometry, which would break the adjusted
                // network's frozen geometry. A missing shot falls through to
                // the split's own refusal.
                if super::command_shot::shot_index(project, line_id, shot_id)
                    .is_some_and(|i| super::command_shot::any_shot_frozen(project, line_id, [i]))
                {
                    return Err(super::commands::FROZEN_GEOMETRY.to_string());
                }
                let made = project
                    .split_shot_at_distance(line_id, shot_id, distance, None)
                    .map_err(|e| e.to_string())?
                    .value;
                record.label = insert_station_label(project, line_id, made.station);
                record.refs.stations.push(made.station);
                record.refs.shots.push(made.head_uuid.clone());
                if let Some(tail) = project
                    .line(line_id)
                    .and_then(|l| l.shots.iter().find(|s| s.id == shot_id))
                {
                    record.refs.shots.push(tail.uuid.clone());
                }
                let create = Command::CreateShot {
                    line_id,
                    shot: made.head_data.clone(),
                    after_shot_id: None,
                    before_shot_id: Some(shot_id),
                };
                let update = Command::UpdateShot {
                    line_id,
                    shot_id,
                    updates: made.tail_updates.clone(),
                };
                record.preview.rows = vec![
                    PreviewRow {
                        field: super::labels::command_label(project, &create),
                        old: String::new(),
                        new: created_details(&made.head_data),
                    },
                    PreviewRow {
                        field: super::labels::command_label(project, &update),
                        old: String::new(),
                        new: updated_details(&made.tail_updates),
                    },
                ];
                Ok(made.head)
            },
        );
        match result {
            Ok(head) => {
                tracing::info!(
                    line_id,
                    shot_id,
                    new_shot_id = head,
                    purpose = "insert station mid-shot",
                    "Split shot at distance completed"
                );
                CommandResult::success().with_shot_id(head)
            }
            Err(failure) => failure,
        }
    }
}
