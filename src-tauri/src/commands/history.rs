use tauri::State;

use crate::core::{AppState, CommandResult};

pub use crate::core::undo::{FieldChange, PreviewData, UndoHistoryItem};

/// Undo the last command
#[tauri::command]
pub fn undo(state: State<AppState>) -> CommandResult {
    undo_impl(state.inner())
}

pub(crate) fn undo_impl(state: &AppState) -> CommandResult {
    state.undo()
}

/// Redo the last undone command
#[tauri::command]
pub fn redo(state: State<AppState>) -> CommandResult {
    redo_impl(state.inner())
}

pub(crate) fn redo_impl(state: &AppState) -> CommandResult {
    state.redo()
}

/// Check if undo is available
#[tauri::command]
pub fn can_undo(state: State<AppState>) -> bool {
    can_undo_impl(state.inner())
}

pub(crate) fn can_undo_impl(state: &AppState) -> bool {
    state.can_undo()
}

/// Check if redo is available
#[tauri::command]
pub fn can_redo(state: State<AppState>) -> bool {
    can_redo_impl(state.inner())
}

pub(crate) fn can_redo_impl(state: &AppState) -> bool {
    state.can_redo()
}

/// Get the undo stack as a list of history items
/// Index 0 = most recent action (first to undo)
#[tauri::command]
pub fn get_undo_stack(state: State<AppState>) -> Vec<UndoHistoryItem> {
    get_undo_stack_impl(state.inner())
}

pub(crate) fn get_undo_stack_impl(state: &AppState) -> Vec<UndoHistoryItem> {
    state.get_undo_stack()
}

/// Get the redo stack as a list of history items
/// Index 0 = most recent undone action (first to redo)
#[tauri::command]
pub fn get_redo_stack(state: State<AppState>) -> Vec<UndoHistoryItem> {
    get_redo_stack_impl(state.inner())
}

pub(crate) fn get_redo_stack_impl(state: &AppState) -> Vec<UndoHistoryItem> {
    state.get_redo_stack()
}

/// Undo to a specific index in the undo stack
/// Index 0 = most recent action, so undo_to(N) executes (N+1) undos
#[tauri::command]
pub fn undo_to(index: usize, state: State<AppState>) -> CommandResult {
    undo_to_impl(index, state.inner())
}

pub(crate) fn undo_to_impl(index: usize, state: &AppState) -> CommandResult {
    state.undo_to(index)
}

/// Redo to a specific index in the redo stack
/// Index 0 = most recent undone action, so redo_to(N) executes (N+1) redos
#[tauri::command]
pub fn redo_to(index: usize, state: State<AppState>) -> CommandResult {
    redo_to_impl(index, state.inner())
}

pub(crate) fn redo_to_impl(index: usize, state: &AppState) -> CommandResult {
    state.redo_to(index)
}
