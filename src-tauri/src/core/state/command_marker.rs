use crate::core::CommandResult;
use crate::models::{MarkerDirection, MarkerType, MarkerUpdates, SectionId, ShotId};

use super::AppState;

impl AppState {
    pub(super) fn add_marker(
        &self,
        section_id: SectionId,
        shot_id: ShotId,
        position: f32,
        marker_type: MarkerType,
        color: String,
        text: String,
        direction: Option<MarkerDirection>,
    ) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };
        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };
        match project.add_marker(
            section_id,
            shot_id,
            position,
            marker_type,
            color,
            text,
            direction,
        ) {
            Ok(marker) => {
                tracing::debug!(marker_id = marker.id, "Marker added");
                CommandResult::success().with_marker_id(marker.id)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to add marker");
                CommandResult::failure(e)
            }
        }
    }

    pub(super) fn update_marker(&self, marker_id: u64, updates: MarkerUpdates) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };
        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };
        match project.update_marker(marker_id, updates) {
            Ok(_) => {
                tracing::debug!(marker_id, "Marker updated");
                CommandResult::success()
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to update marker");
                CommandResult::failure(e)
            }
        }
    }

    pub(super) fn delete_marker(&self, marker_id: u64) -> CommandResult {
        let Ok(mut proj_guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };
        let Some(project) = proj_guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };
        match project.delete_marker(marker_id) {
            Ok(_) => {
                tracing::debug!(marker_id, "Marker deleted");
                CommandResult::success()
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to delete marker");
                CommandResult::failure(e)
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::core::Command;
    use crate::models::{MarkerDirection, MarkerType, ShotData};

    fn make_state_with_section_and_shot() -> (AppState, SectionId, ShotId) {
        let state = AppState::new().unwrap();
        state.execute(Command::CreateProject {
            name: "Marker Test".to_string(),
            location: None,
        });
        let res = state.execute(Command::CreateSection {
            name: Some("S1".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![ShotData {
                azimuth_in: 10.0,
                azimuth_out: 10.0,
                length: 5.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            }],
        });
        let section_id = res.section_id.unwrap();
        let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;
        (state, section_id, shot_id)
    }

    #[test]
    fn add_marker_succeeds_and_returns_id() {
        let (state, section_id, shot_id) = make_state_with_section_and_shot();
        let res = state.execute(Command::AddMarker {
            section_id,
            shot_id,
            position: 50.0,
            marker_type: MarkerType::Ndm,
            color: "#ffffff".to_string(),
            text: "Test".to_string(),
            direction: None,
        });
        assert!(res.success, "{:?}", res.error);
        assert!(res.marker_id.is_some());
        let proj = state.get_project_state().unwrap();
        assert_eq!(proj.markers.len(), 1);
        assert_eq!(proj.markers[0].id, res.marker_id.unwrap());
    }

    #[test]
    fn add_marker_rejects_invalid_color() {
        let (state, section_id, shot_id) = make_state_with_section_and_shot();
        let res = state.execute(Command::AddMarker {
            section_id,
            shot_id,
            position: 50.0,
            marker_type: MarkerType::Ndm,
            color: "url(https://evil.example/x)".to_string(),
            text: String::new(),
            direction: None,
        });
        assert!(!res.success, "expected failure for CSS-injection color");
    }

    #[test]
    fn add_marker_rejects_out_of_range_position() {
        let (state, section_id, shot_id) = make_state_with_section_and_shot();
        let res = state.execute(Command::AddMarker {
            section_id,
            shot_id,
            position: 101.0,
            marker_type: MarkerType::Ndm,
            color: "#aabbcc".to_string(),
            text: String::new(),
            direction: None,
        });
        assert!(!res.success, "expected failure for position > 100");
    }

    #[test]
    fn add_marker_rejects_bad_section_id() {
        let (state, _section_id, shot_id) = make_state_with_section_and_shot();
        let res = state.execute(Command::AddMarker {
            section_id: 9999,
            shot_id,
            position: 50.0,
            marker_type: MarkerType::Ndm,
            color: "#aabbcc".to_string(),
            text: String::new(),
            direction: None,
        });
        assert!(!res.success, "expected failure for nonexistent section");
    }

    #[test]
    fn update_marker_rejects_bad_marker_id() {
        let (state, _, _) = make_state_with_section_and_shot();
        let res = state.execute(Command::UpdateMarker {
            marker_id: 9999,
            updates: MarkerUpdates {
                text: Some("x".to_string()),
                ..Default::default()
            },
        });
        assert!(!res.success, "expected failure for nonexistent marker_id");
    }

    #[test]
    fn delete_marker_rejects_bad_marker_id() {
        let (state, _, _) = make_state_with_section_and_shot();
        let res = state.execute(Command::DeleteMarker { marker_id: 9999 });
        assert!(!res.success, "expected failure for nonexistent marker_id");
    }

    #[test]
    fn add_marker_undo_removes_it() {
        let (state, section_id, shot_id) = make_state_with_section_and_shot();
        let res = state.execute(Command::AddMarker {
            section_id,
            shot_id,
            position: 50.0,
            marker_type: MarkerType::Ndm,
            color: "#ffffff".to_string(),
            text: String::new(),
            direction: None,
        });
        assert!(res.success);
        assert_eq!(state.get_project_state().unwrap().markers.len(), 1);

        let undo_res = state.undo();
        assert!(undo_res.success, "{:?}", undo_res.error);
        assert!(state.get_project_state().unwrap().markers.is_empty());

        // redo restores the marker
        let redo_res = state.redo();
        assert!(redo_res.success, "{:?}", redo_res.error);
        assert_eq!(state.get_project_state().unwrap().markers.len(), 1);
    }

    #[test]
    fn update_marker_succeeds_and_is_undoable() {
        let (state, section_id, shot_id) = make_state_with_section_and_shot();
        let add_res = state.execute(Command::AddMarker {
            section_id,
            shot_id,
            position: 25.0,
            marker_type: MarkerType::Ndm,
            color: "#ffffff".to_string(),
            text: "Original".to_string(),
            direction: None,
        });
        let marker_id = add_res.marker_id.unwrap();

        let upd_res = state.execute(Command::UpdateMarker {
            marker_id,
            updates: MarkerUpdates {
                text: Some("Updated".to_string()),
                color: Some("#ff9f43".to_string()),
                ..Default::default()
            },
        });
        assert!(upd_res.success, "{:?}", upd_res.error);
        let proj = state.get_project_state().unwrap();
        assert_eq!(proj.markers[0].text, "Updated");
        assert_eq!(proj.markers[0].color, "#ff9f43");

        let undo_res = state.undo();
        assert!(undo_res.success, "{:?}", undo_res.error);
        let proj = state.get_project_state().unwrap();
        assert_eq!(proj.markers[0].text, "Original");
        assert_eq!(proj.markers[0].color, "#ffffff");

        // redo re-applies the update
        let redo_res = state.redo();
        assert!(redo_res.success, "{:?}", redo_res.error);
        let proj = state.get_project_state().unwrap();
        assert_eq!(proj.markers[0].text, "Updated");
    }

    #[test]
    fn delete_marker_succeeds_and_is_undoable() {
        let (state, section_id, shot_id) = make_state_with_section_and_shot();
        let add_res = state.execute(Command::AddMarker {
            section_id,
            shot_id,
            position: 75.0,
            marker_type: MarkerType::Arrow,
            color: "#54a0ff".to_string(),
            text: String::new(),
            direction: Some(MarkerDirection::TowardEnd),
        });
        let marker_id = add_res.marker_id.unwrap();

        let del_res = state.execute(Command::DeleteMarker { marker_id });
        assert!(del_res.success, "{:?}", del_res.error);
        assert!(state.get_project_state().unwrap().markers.is_empty());

        let undo_res = state.undo();
        assert!(undo_res.success, "{:?}", undo_res.error);
        let proj = state.get_project_state().unwrap();
        assert_eq!(proj.markers.len(), 1);
        assert_eq!(proj.markers[0].id, marker_id);

        // redo removes it again
        let redo_res = state.redo();
        assert!(redo_res.success, "{:?}", redo_res.error);
        assert!(state.get_project_state().unwrap().markers.is_empty());
    }
}
