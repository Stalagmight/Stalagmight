use tauri::State;

use crate::core::{AppState, Command, CommandResult};
use crate::models::{MarkerDirection, MarkerType, MarkerUpdates};

#[tauri::command]
pub fn add_marker(
    state: State<AppState>,
    section_id: u32,
    shot_id: u32,
    position: f32,
    marker_type: MarkerType,
    color: String,
    text: String,
    direction: Option<MarkerDirection>,
) -> CommandResult {
    add_marker_impl(
        state.inner(),
        section_id,
        shot_id,
        position,
        marker_type,
        color,
        text,
        direction,
    )
}

pub(crate) fn add_marker_impl(
    state: &AppState,
    section_id: u32,
    shot_id: u32,
    position: f32,
    marker_type: MarkerType,
    color: String,
    text: String,
    direction: Option<MarkerDirection>,
) -> CommandResult {
    state.execute(Command::AddMarker {
        section_id,
        shot_id,
        position,
        marker_type,
        color,
        text,
        direction,
    })
}

#[tauri::command]
pub fn update_marker(
    state: State<AppState>,
    marker_id: u64,
    updates: MarkerUpdates,
) -> CommandResult {
    update_marker_impl(state.inner(), marker_id, updates)
}

pub(crate) fn update_marker_impl(
    state: &AppState,
    marker_id: u64,
    updates: MarkerUpdates,
) -> CommandResult {
    state.execute(Command::UpdateMarker { marker_id, updates })
}

#[tauri::command]
pub fn delete_marker(state: State<AppState>, marker_id: u64) -> CommandResult {
    delete_marker_impl(state.inner(), marker_id)
}

pub(crate) fn delete_marker_impl(state: &AppState, marker_id: u64) -> CommandResult {
    state.execute(Command::DeleteMarker { marker_id })
}
