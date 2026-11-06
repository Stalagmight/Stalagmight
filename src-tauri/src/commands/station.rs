use tauri::{AppHandle, Runtime};

use crate::commands::pool;
use crate::core::state::ProjectIdentity;
use crate::core::{Command, CommandError, CommentHolder, EditOutcome};
use crate::models::{ClosureStrategy, ConnectivityResult, Coordinates, GpsCoordinates, StationRef};

/// Check merge connectivity between two stations (read-only, NOT a Command)
#[tauri::command]
pub async fn check_merge_connectivity<R: Runtime>(
    app: AppHandle<R>,
    station_a: StationRef,
    station_b: StationRef,
) -> Result<ConnectivityResult, CommandError> {
    pool::run(app, move |state| {
        state
            .settled_read(|| state.check_merge_connectivity(station_a, station_b))
            .map_err(CommandError::from)
    })
    .await
}

/// Create a loop closure between two connected stations
#[tauri::command]
pub async fn create_loop_closure<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    station_a: StationRef,
    station_b: StationRef,
    closure_strategy: ClosureStrategy,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::CreateLoopClosure {
                station_a,
                station_b,
                closure_strategy,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Remove a loop closure by ID
#[tauri::command]
pub async fn remove_loop_closure<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    closure_id: u64,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::RemoveLoopClosure { closure_id })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Merge two stations
#[tauri::command]
pub async fn merge_stations<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    station_a: StationRef,
    station_b: StationRef,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::MergeStations {
                station_a,
                station_b,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Unmerge a station
#[tauri::command]
pub async fn unmerge_stations<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    station_ref: StationRef,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::UnmergeStations { station_ref })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Unmerge a specific station pair
#[tauri::command]
pub async fn unmerge_station_pair<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    station_a: StationRef,
    station_b: StationRef,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::UnmergeStationPair {
                station_a,
                station_b,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Set zero datum for a station
#[tauri::command]
pub async fn set_zero_datum<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    station_ref: StationRef,
    coordinates: GpsCoordinates,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::SetZeroDatum {
                station_ref,
                coordinates,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Remove zero datum from a station
#[tauri::command]
pub async fn remove_zero_datum<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    station_ref: StationRef,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::RemoveZeroDatum { station_ref })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Set or clear a station comment
#[tauri::command]
pub async fn set_station_comment<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    station_ref: StationRef,
    comment: Option<String>,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::SetStationComment {
                station_ref,
                comment,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Move or swap a comment between stations/shots atomically (single undo entry)
#[tauri::command]
pub async fn move_comment<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    source: CommentHolder,
    target: CommentHolder,
    swap: bool,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::MoveComment {
                source,
                target,
                swap,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Move a station by a delta offset
#[tauri::command]
pub async fn move_station<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    station_ref: StationRef,
    offset: Coordinates,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::MoveStation {
                station_ref,
                offset,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}
