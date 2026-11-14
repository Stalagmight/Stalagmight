use tauri::State;

use crate::core::{AppState, Command, CommandResult, CommentHolder};
use crate::models::{
    ClosureStrategy, ConnectivityResult, Coordinates, GpsCoordinates, StationRef, ZeroDatumPreview,
};

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

/// Set zero datum for a station. `accuracy_m` is the GPS horizontal accuracy
/// in metres (JS sends `accuracyM`); omitted or `null` means the default. On a
/// merge component that is already placed the datum is stored as a GPS fix and
/// nothing moves (#741).
#[tauri::command]
pub fn set_zero_datum(
    state: State<AppState>,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
    accuracy_m: Option<f64>,
) -> CommandResult {
    set_zero_datum_impl(state.inner(), station_ref, coordinates, accuracy_m)
}

pub(crate) fn set_zero_datum_impl(
    state: &AppState,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
    accuracy_m: Option<f64>,
) -> CommandResult {
    state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates,
        accuracy_m,
    })
}

/// What setting a zero datum with `coordinates` on `station_ref` would do, for
/// the Edit Station dialog (read-only, NOT a Command): the role the datum would
/// have, and for a GPS fix the gap to the surveyed position (#741). `None` when
/// the station does not exist.
#[tauri::command]
pub fn preview_zero_datum(
    state: State<AppState>,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
) -> Result<Option<ZeroDatumPreview>, String> {
    preview_zero_datum_impl(state.inner(), station_ref, coordinates)
}

pub(crate) fn preview_zero_datum_impl(
    state: &AppState,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
) -> Result<Option<ZeroDatumPreview>, String> {
    state.preview_zero_datum(station_ref, coordinates)
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
