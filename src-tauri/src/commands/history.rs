use tauri::{AppHandle, Runtime};

use crate::commands::pool;
use crate::core::{CommandError, HistoryOutcome};

pub use crate::core::undo::history_view::{
    entry_to_history_item, FieldChange, PreviewData, UndoHistoryItem,
};

/// Undo the last command
#[tauri::command]
pub async fn undo<R: Runtime>(app: AppHandle<R>) -> Result<HistoryOutcome, CommandError> {
    pool::run_mutation(app, |state| state.undo().into_outcome()).await
}

/// Redo the last undone command
#[tauri::command]
pub async fn redo<R: Runtime>(app: AppHandle<R>) -> Result<HistoryOutcome, CommandError> {
    pool::run_mutation(app, |state| state.redo().into_outcome()).await
}

/// Check if undo is available
#[tauri::command]
pub async fn can_undo<R: Runtime>(app: AppHandle<R>) -> Result<bool, CommandError> {
    pool::run(app, |state| Ok(state.settled_read(|| state.can_undo()))).await
}

/// Check if redo is available
#[tauri::command]
pub async fn can_redo<R: Runtime>(app: AppHandle<R>) -> Result<bool, CommandError> {
    pool::run(app, |state| Ok(state.settled_read(|| state.can_redo()))).await
}

/// Get the undo stack as a list of history items
/// Index 0 = most recent action (first to undo)
///
/// Read between commands (#576 C2): an undo in flight has popped its entry and
/// not yet pushed it onto the redo stack.
#[tauri::command]
pub async fn get_undo_stack<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<UndoHistoryItem>, CommandError> {
    pool::run(app, |state| {
        Ok(state.settled_read(|| state.get_undo_stack()))
    })
    .await
}

/// Get the redo stack as a list of history items
/// Index 0 = most recent undone action (first to redo)
#[tauri::command]
pub async fn get_redo_stack<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<UndoHistoryItem>, CommandError> {
    pool::run(app, |state| {
        Ok(state.settled_read(|| state.get_redo_stack()))
    })
    .await
}

/// Undo to a specific index in the undo stack
/// Index 0 = most recent action, so undo_to(N) executes (N+1) undos
#[tauri::command]
pub async fn undo_to<R: Runtime>(
    app: AppHandle<R>,
    index: usize,
) -> Result<HistoryOutcome, CommandError> {
    pool::run_mutation(app, move |state| state.undo_to(index).into_outcome()).await
}

/// Redo to a specific index in the redo stack
/// Index 0 = most recent undone action, so redo_to(N) executes (N+1) redos
#[tauri::command]
pub async fn redo_to<R: Runtime>(
    app: AppHandle<R>,
    index: usize,
) -> Result<HistoryOutcome, CommandError> {
    pool::run_mutation(app, move |state| state.redo_to(index).into_outcome()).await
}
