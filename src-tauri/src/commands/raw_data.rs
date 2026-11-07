//! Read-only browsing of the raw sqlite tables in the current project
//! database (data table fullscreen mode). Values are stringified server-side;
//! see `Database::read_raw_table`.

use tauri::{AppHandle, Runtime};

use crate::commands::pool;
use crate::core::CommandError;
use crate::persistence::RawTableData;

/// List the raw sqlite table names in the current project database
#[tauri::command]
pub async fn get_raw_table_names<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<String>, CommandError> {
    pool::run(app, |state| {
        state
            .list_raw_table_names()
            .map_err(|e| CommandError::failed(e.to_string()))
    })
    .await
}

/// Read a page of stringified rows from a raw sqlite table
#[tauri::command]
pub async fn get_raw_table<R: Runtime>(
    app: AppHandle<R>,
    table: String,
    limit: u32,
    offset: u32,
) -> Result<RawTableData, CommandError> {
    pool::run(app, move |state| {
        state
            .read_raw_table(&table, limit, offset)
            .map_err(|e| CommandError::failed(e.to_string()))
    })
    .await
}
