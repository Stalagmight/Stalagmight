use tauri::State;

use crate::core::{AppState, Command, CommandResult, CommentHolder};
use crate::models::{ClosureStrategy, ConnectivityResult, Coordinates, GpsCoordinates, StationRef};

/// Check merge connectivity between two stations (read-only, NOT a Command)
#[tauri::command]
pub fn check_merge_connectivity(
    state: State<AppState>,
    station_a: StationRef,
    station_b: StationRef,
) -> Result<ConnectivityResult, String> {
    check_merge_connectivity_impl(state.inner(), station_a, station_b)
}

pub(crate) fn check_merge_connectivity_impl(
    state: &AppState,
    station_a: StationRef,
    station_b: StationRef,
) -> Result<ConnectivityResult, String> {
    state.check_merge_connectivity(station_a, station_b)
}

/// Create a loop closure between two connected stations
#[tauri::command]
pub fn create_loop_closure(
    state: State<AppState>,
    station_a: StationRef,
    station_b: StationRef,
    closure_strategy: ClosureStrategy,
) -> CommandResult {
    create_loop_closure_impl(state.inner(), station_a, station_b, closure_strategy)
}

pub(crate) fn create_loop_closure_impl(
    state: &AppState,
    station_a: StationRef,
    station_b: StationRef,
    closure_strategy: ClosureStrategy,
) -> CommandResult {
    state.execute(Command::CreateLoopClosure {
        station_a,
        station_b,
        closure_strategy,
    })
}

/// Remove a loop closure by ID
#[tauri::command]
pub fn remove_loop_closure(state: State<AppState>, closure_id: u64) -> CommandResult {
    remove_loop_closure_impl(state.inner(), closure_id)
}

pub(crate) fn remove_loop_closure_impl(state: &AppState, closure_id: u64) -> CommandResult {
    state.execute(Command::RemoveLoopClosure { closure_id })
}

/// Merge two stations
#[tauri::command]
pub fn merge_stations(
    state: State<AppState>,
    station_a: StationRef,
    station_b: StationRef,
) -> CommandResult {
    merge_stations_impl(state.inner(), station_a, station_b)
}

pub(crate) fn merge_stations_impl(
    state: &AppState,
    station_a: StationRef,
    station_b: StationRef,
) -> CommandResult {
    state.execute(Command::MergeStations {
        station_a,
        station_b,
    })
}

/// Unmerge a station
#[tauri::command]
pub fn unmerge_stations(state: State<AppState>, station_ref: StationRef) -> CommandResult {
    unmerge_stations_impl(state.inner(), station_ref)
}

pub(crate) fn unmerge_stations_impl(state: &AppState, station_ref: StationRef) -> CommandResult {
    state.execute(Command::UnmergeStations { station_ref })
}

/// Unmerge a specific station pair
#[tauri::command]
pub fn unmerge_station_pair(
    state: State<AppState>,
    station_a: StationRef,
    station_b: StationRef,
) -> CommandResult {
    unmerge_station_pair_impl(state.inner(), station_a, station_b)
}

pub(crate) fn unmerge_station_pair_impl(
    state: &AppState,
    station_a: StationRef,
    station_b: StationRef,
) -> CommandResult {
    state.execute(Command::UnmergeStationPair {
        station_a,
        station_b,
    })
}

/// Set zero datum for a station
#[tauri::command]
pub fn set_zero_datum(
    state: State<AppState>,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
) -> CommandResult {
    set_zero_datum_impl(state.inner(), station_ref, coordinates)
}

pub(crate) fn set_zero_datum_impl(
    state: &AppState,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
) -> CommandResult {
    state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates,
    })
}

/// Remove zero datum from a station
#[tauri::command]
pub fn remove_zero_datum(state: State<AppState>, station_ref: StationRef) -> CommandResult {
    remove_zero_datum_impl(state.inner(), station_ref)
}

pub(crate) fn remove_zero_datum_impl(state: &AppState, station_ref: StationRef) -> CommandResult {
    state.execute(Command::RemoveZeroDatum { station_ref })
}

/// Set or clear a station comment
#[tauri::command]
pub fn set_station_comment(
    state: State<AppState>,
    station_ref: StationRef,
    comment: Option<String>,
) -> CommandResult {
    set_station_comment_impl(state.inner(), station_ref, comment)
}

pub(crate) fn set_station_comment_impl(
    state: &AppState,
    station_ref: StationRef,
    comment: Option<String>,
) -> CommandResult {
    state.execute(Command::SetStationComment {
        station_ref,
        comment,
    })
}

/// Set or clear a station name (the free-text identifier shown beside its
/// positional id). Whitespace-only or `None` removes it; otherwise the backend
/// stores it trimmed, cleaned of control/bidi characters and clamped (see
/// `sanitize_station_name`).
#[tauri::command]
pub fn set_station_name(
    state: State<AppState>,
    station_ref: StationRef,
    name: Option<String>,
) -> CommandResult {
    set_station_name_impl(state.inner(), station_ref, name)
}

pub(crate) fn set_station_name_impl(
    state: &AppState,
    station_ref: StationRef,
    name: Option<String>,
) -> CommandResult {
    state.execute(Command::SetStationName { station_ref, name })
}

/// Move or swap a comment between stations/shots atomically (single undo entry)
#[tauri::command]
pub fn move_comment(
    state: State<AppState>,
    source: CommentHolder,
    target: CommentHolder,
    swap: bool,
) -> CommandResult {
    move_comment_impl(state.inner(), source, target, swap)
}

pub(crate) fn move_comment_impl(
    state: &AppState,
    source: CommentHolder,
    target: CommentHolder,
    swap: bool,
) -> CommandResult {
    state.execute(Command::MoveComment {
        source,
        target,
        swap,
    })
}

/// Move a station by a delta offset
#[tauri::command]
pub fn move_station(
    state: State<AppState>,
    station_ref: StationRef,
    offset: Coordinates,
) -> CommandResult {
    move_station_impl(state.inner(), station_ref, offset)
}

pub(crate) fn move_station_impl(
    state: &AppState,
    station_ref: StationRef,
    offset: Coordinates,
) -> CommandResult {
    state.execute(Command::MoveStation {
        station_ref,
        offset,
    })
}
