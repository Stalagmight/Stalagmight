//! Read-only browsing of the open project's saved file (data table fullscreen
//! mode, "Raw data"): the members of the version 2 `.stalx`, one table per
//! member and one row per record, as of the last save. Values are stringified
//! server-side; see `AppState::read_raw_table`.

use tauri::State;

use crate::core::AppState;
use crate::persistence::RawTableData;

/// List the raw tables (the saved file's members) of the open project
#[tauri::command]
pub fn get_raw_table_names(state: State<AppState>) -> Result<Vec<String>, String> {
    get_raw_table_names_impl(state.inner())
}

pub(crate) fn get_raw_table_names_impl(state: &AppState) -> Result<Vec<String>, String> {
    state.list_raw_table_names().map_err(|e| e.to_string())
}

/// Read a page of stringified rows from a raw table
#[tauri::command]
pub fn get_raw_table(
    state: State<AppState>,
    table: String,
    limit: u32,
    offset: u32,
) -> Result<RawTableData, String> {
    get_raw_table_impl(state.inner(), table, limit, offset)
}

pub(crate) fn get_raw_table_impl(
    state: &AppState,
    table: String,
    limit: u32,
    offset: u32,
) -> Result<RawTableData, String> {
    state
        .read_raw_table(&table, limit, offset)
        .map_err(|e| e.to_string())
}
