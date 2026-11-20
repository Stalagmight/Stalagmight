use tauri::State;

use crate::core::{AppState, Command, CommandResult, CommentHolder};
use crate::models::{
    ClosureStrategy, ConnectivityResult, Coordinates, DetachLinePreview, GpsCoordinates, LineId,
    StationRef, ZeroDatumPreview,
};

/// Check merge connectivity between two stations (read-only, NOT a Command):
/// a connected pair goes to the closure dialog (with its figures and the
/// across-one-shot refusal), an unconnected one may merge (with the datum
/// shift when both pieces are placed).
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

/// Merge two stations into one shared station (a junction). The piece of
/// `station_a` moves onto `station_b` unless that would move a placed piece.
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

/// Unmerge a station: detach the slot's line from the junction (it gets a
/// station of its own where the junction was); the other lines stay joined.
/// A no-op on a station that is on one line only.
#[tauri::command]
pub fn unmerge_stations(state: State<AppState>, station_ref: StationRef) -> CommandResult {
    unmerge_stations_impl(state.inner(), station_ref)
}

pub(crate) fn unmerge_stations_impl(state: &AppState, station_ref: StationRef) -> CommandResult {
    state.execute(Command::UnmergeStations { station_ref })
}

/// What detaching line `line_id` from the station at `station_ref` would give
/// the line (read-only, NOT a Command): the new station's name and Station
/// ID, and whether they are the line's own from before the merge (review
/// finding UX-4). Refused as the detach is.
#[tauri::command]
pub fn preview_detach_line(
    state: State<AppState>,
    station_ref: StationRef,
    line_id: LineId,
) -> Result<DetachLinePreview, String> {
    preview_detach_line_impl(state.inner(), station_ref, line_id)
}

pub(crate) fn preview_detach_line_impl(
    state: &AppState,
    station_ref: StationRef,
    line_id: LineId,
) -> Result<DetachLinePreview, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;
    let station = project
        .station_at(station_ref)
        .ok_or_else(|| "Station not found".to_string())?;
    let naming = project
        .detach_naming(station, line_id)
        .map_err(|e| e.to_string())?;
    Ok(DetachLinePreview {
        name: naming.name,
        station_id: naming.id,
        restored: naming.restored_alias,
    })
}

/// Unmerge a specific station pair: the two slots must be one station on two
/// lines; `station_b`'s line is detached from it.
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
/// piece that is already placed the datum is stored as a GPS fix and nothing
/// moves (#741). `entrance` (JS `entrance`) is the user's answer to
/// "is this station an entrance?"; omitted or `null` keeps the stored answer,
/// which for a new datum is the default rule.
#[tauri::command]
pub fn set_zero_datum(
    state: State<AppState>,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
    accuracy_m: Option<f64>,
    entrance: Option<bool>,
) -> CommandResult {
    set_zero_datum_impl(
        state.inner(),
        station_ref,
        coordinates,
        accuracy_m,
        entrance,
    )
}

pub(crate) fn set_zero_datum_impl(
    state: &AppState,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
    accuracy_m: Option<f64>,
    entrance: Option<bool>,
) -> CommandResult {
    state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates,
        accuracy_m,
        entrance,
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

/// Rename a station. Trimmed; refused when empty or when another station of
/// the same line holds the name ("Line X already has a station named Y").
/// Undoable.
#[tauri::command]
pub fn set_station_name(
    state: State<AppState>,
    station_ref: StationRef,
    name: String,
) -> CommandResult {
    set_station_name_impl(state.inner(), station_ref, name)
}

pub(crate) fn set_station_name_impl(
    state: &AppState,
    station_ref: StationRef,
    name: String,
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
