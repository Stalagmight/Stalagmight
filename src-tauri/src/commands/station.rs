use tauri::State;

use crate::core::{AppState, Command, CommandResult, CommentHolder};
use crate::models::{
    ClosureStrategy, ConnectivityResult, Coordinates, DetachLinePreview, GpsCoordinates, LineId,
    LoopClosureNaming, RemoveClosureNaming, StationId, StationOwner, StationRef, StationRename,
    ZeroDatumPreview,
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

/// Create a loop closure between two connected stations (RS 5.2). `keep`
/// (JS `keep`) is the Station ID of the end kept, the other end then shows
/// it; omitted or `null`: "Keep both names". `renames` (JS `renames`) are
/// the dialog's answers to the name collisions; omitted or `null`: no dialog
/// asked, so rule L letters them and the result's `identities` says so.
#[tauri::command]
pub fn create_loop_closure(
    state: State<AppState>,
    station_a: StationRef,
    station_b: StationRef,
    closure_strategy: ClosureStrategy,
    keep: Option<StationId>,
    renames: Option<Vec<StationRename>>,
) -> CommandResult {
    create_loop_closure_impl(
        state.inner(),
        (station_a, station_b),
        closure_strategy,
        keep,
        renames,
    )
}

pub(crate) fn create_loop_closure_impl(
    state: &AppState,
    (station_a, station_b): (StationRef, StationRef),
    closure_strategy: ClosureStrategy,
    keep: Option<StationId>,
    renames: Option<Vec<StationRename>>,
) -> CommandResult {
    state.execute(Command::CreateLoopClosure {
        station_a,
        station_b,
        closure_strategy,
        keep,
        renames,
    })
}

/// The Create Loop Closure dialog's naming part (read-only, NOT a Command,
/// RS 5.2 and 7): both ends, the ends that may be kept, the default, and
/// what each choice ("Keep both names" included) does to names.
#[tauri::command]
pub fn preview_loop_closure(
    state: State<AppState>,
    station_a: StationRef,
    station_b: StationRef,
    closure_strategy: ClosureStrategy,
) -> Result<LoopClosureNaming, String> {
    preview_loop_closure_impl(state.inner(), station_a, station_b, closure_strategy)
}

pub(crate) fn preview_loop_closure_impl(
    state: &AppState,
    station_a: StationRef,
    station_b: StationRef,
    closure_strategy: ClosureStrategy,
) -> Result<LoopClosureNaming, String> {
    state.preview_loop_closure(station_a, station_b, closure_strategy)
}

/// Remove a loop closure by ID (RS 5.3): the end it archived shows its own
/// station again. `renames` as for [`create_loop_closure`].
#[tauri::command]
pub fn remove_loop_closure(
    state: State<AppState>,
    closure_id: u64,
    renames: Option<Vec<StationRename>>,
) -> CommandResult {
    remove_loop_closure_impl(state.inner(), closure_id, renames)
}

pub(crate) fn remove_loop_closure_impl(
    state: &AppState,
    closure_id: u64,
    renames: Option<Vec<StationRename>>,
) -> CommandResult {
    state.execute(Command::RemoveLoopClosure {
        closure_id,
        renames,
    })
}

/// What Remove Closure does to names (read-only, NOT a Command, RS 5.3):
/// the station that comes back and the names that would then repeat.
#[tauri::command]
pub fn preview_remove_loop_closure(
    state: State<AppState>,
    closure_id: u64,
) -> Result<RemoveClosureNaming, String> {
    preview_remove_loop_closure_impl(state.inner(), closure_id)
}

pub(crate) fn preview_remove_loop_closure_impl(
    state: &AppState,
    closure_id: u64,
) -> Result<RemoveClosureNaming, String> {
    state.preview_remove_loop_closure(closure_id)
}

/// Merge two stations into one shared station (a junction, RS 4.1). The
/// piece of `station_a` moves onto `station_b` unless that would move a
/// placed piece. `keep` (JS `keep`) is the Station ID of the station kept,
/// shown on every line; omitted or `null`: the station that stays put.
/// `renames` as for [`create_loop_closure`].
#[tauri::command]
pub fn merge_stations(
    state: State<AppState>,
    station_a: StationRef,
    station_b: StationRef,
    keep: Option<StationId>,
    renames: Option<Vec<StationRename>>,
) -> CommandResult {
    merge_stations_impl(state.inner(), (station_a, station_b), keep, renames)
}

pub(crate) fn merge_stations_impl(
    state: &AppState,
    (station_a, station_b): (StationRef, StationRef),
    keep: Option<StationId>,
    renames: Option<Vec<StationRename>>,
) -> CommandResult {
    state.execute(Command::MergeStations {
        station_a,
        station_b,
        keep,
        renames,
    })
}

/// Unmerge a station: detach the slot's line from the junction (RS 4.2: it
/// gets back its own station when a merge archived it, else a station of
/// its own where the junction was); the other lines stay joined. A no-op on
/// a station that is on one line only. `renames` as for
/// [`create_loop_closure`]; `rest_keep` (JS `restKeep`) picks the station
/// the other lines take back when the line keeps the junction.
#[tauri::command]
pub fn unmerge_stations(
    state: State<AppState>,
    station_ref: StationRef,
    renames: Option<Vec<StationRename>>,
    rest_keep: Option<StationId>,
) -> CommandResult {
    unmerge_stations_impl(state.inner(), station_ref, renames, rest_keep)
}

pub(crate) fn unmerge_stations_impl(
    state: &AppState,
    station_ref: StationRef,
    renames: Option<Vec<StationRename>>,
    rest_keep: Option<StationId>,
) -> CommandResult {
    state.execute(Command::UnmergeStations {
        station_ref,
        renames,
        rest_keep,
    })
}

/// What detaching line `line_id` from the station at `station_ref` would do
/// (read-only, NOT a Command; review finding UX-4, RS 4.2 and 7): what each
/// line ends up with, what comes back (name, comment, Station ID), the lines
/// that come off together, the remaining-junction choices (`rest_keep`, JS
/// `restKeep`, picks one), the station a loop closure showed before, and
/// each name collision with its two answers. Refused as the detach is.
#[tauri::command]
pub fn preview_detach_line(
    state: State<AppState>,
    station_ref: StationRef,
    line_id: LineId,
    rest_keep: Option<StationId>,
) -> Result<DetachLinePreview, String> {
    preview_detach_line_impl(state.inner(), station_ref, line_id, rest_keep)
}

pub(crate) fn preview_detach_line_impl(
    state: &AppState,
    station_ref: StationRef,
    line_id: LineId,
    rest_keep: Option<StationId>,
) -> Result<DetachLinePreview, String> {
    state.preview_detach_line(station_ref, line_id, rest_keep)
}

/// Unmerge a specific station pair: the two slots must be one station on two
/// lines; `station_b`'s line is detached from it. `renames` and `rest_keep`
/// as for [`unmerge_stations`].
#[tauri::command]
pub fn unmerge_station_pair(
    state: State<AppState>,
    station_a: StationRef,
    station_b: StationRef,
    renames: Option<Vec<StationRename>>,
    rest_keep: Option<StationId>,
) -> CommandResult {
    unmerge_station_pair_impl(state.inner(), (station_a, station_b), renames, rest_keep)
}

pub(crate) fn unmerge_station_pair_impl(
    state: &AppState,
    (station_a, station_b): (StationRef, StationRef),
    renames: Option<Vec<StationRename>>,
    rest_keep: Option<StationId>,
) -> CommandResult {
    state.execute(Command::UnmergeStationPair {
        station_a,
        station_b,
        renames,
        rest_keep,
    })
}

/// Whose station the slot shows (read-only, NOT a Command, RS 5.4): what
/// Set Station Name and Set Station Comment through it change (the shown
/// identity, on every line it is shown on), and on a foreign slot or a loop
/// end the two ways back the prompt offers.
#[tauri::command]
pub fn preview_station_identity(
    state: State<AppState>,
    station_ref: StationRef,
) -> Result<StationOwner, String> {
    preview_station_identity_impl(state.inner(), station_ref)
}

pub(crate) fn preview_station_identity_impl(
    state: &AppState,
    station_ref: StationRef,
) -> Result<StationOwner, String> {
    state.preview_station_identity(station_ref)
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

/// Set or clear a station comment: the comment of the identity the slot
/// shows (RS 5.4; `preview_station_identity` says whose it is).
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

/// Rename a station: the identity the slot shows, on every line it is shown
/// on (RS 5.4; `preview_station_identity` says whose it is and which lines).
/// Trimmed; refused when empty or when another station of a line it is
/// shown on holds the name ("Line X already has a station named Y").
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
