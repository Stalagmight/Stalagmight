use std::path::PathBuf;
use tauri::{Manager, State};

use crate::core::loading::{CreateProjectFullResult, LoadError, LoadProjectFullResult};
use crate::core::{AppState, Command, CommandResult};
use crate::models::{
    AdjustmentReport, AlongLineResult, DistanceField, ProjectMetadata, RulerEndpoint, Station,
    StationRef,
};

/// Update per-project settings (#57): project name + validation thresholds.
/// Stored in the .stalx so they travel with the file between collaborators.
#[tauri::command]
pub fn update_project_settings(
    state: State<AppState>,
    name: String,
    validation: crate::config::ValidationConfig,
) -> CommandResult {
    state.update_project_settings(name, validation)
}

/// Set (or, given `null`/omitted, clear) the open project's entrance
/// elevation in meters (#465). Stored in the .stalx so it travels with the
/// file; the only recovery path for the TML import's no-clobber rule (#431)
/// locking in a wrong first value.
#[tauri::command]
pub fn set_surface_elevation(state: State<AppState>, elevation: Option<f64>) -> CommandResult {
    state.set_surface_elevation(elevation)
}

/// Save the current project.
///
/// `is_autosave` changes NOTHING about the save - it only names who asked for
/// it, so a background write is distinguishable from a Ctrl+S in the log
/// (#429). Absent means manual: every other Rust caller either goes through
/// [`save_project_impl`] or constructs `Command::SaveProject` itself; both
/// funnel into the same `execute` dispatch.
#[tauri::command]
pub fn save_project(
    state: State<AppState>,
    path: Option<PathBuf>,
    is_autosave: Option<bool>,
) -> CommandResult {
    tracing::info!(
        trigger = if is_autosave.unwrap_or(false) {
            "autosave"
        } else {
            "manual"
        },
        path = ?path,
        purpose = "persist edits safely",
        "Save requested"
    );
    save_project_impl(state.inner(), path)
}

pub(crate) fn save_project_impl(state: &AppState, path: Option<PathBuf>) -> CommandResult {
    state.execute(Command::SaveProject { path })
}

/// The "Overwrite" answer to a save the external-modification guard refused
/// (`EXTERNALLY_MODIFIED: ...`, plan 6.5): write the open project over a file
/// another program changed since it was last read or written. `path` is as
/// in [`save_project`]; the frontend's conflict prompt passes `null`. Every
/// other refusal (read-only file, invalid survey) still applies.
#[tauri::command]
pub fn save_project_overwriting(state: State<AppState>, path: Option<PathBuf>) -> CommandResult {
    tracing::info!(
        path = ?path,
        purpose = "replace a project file another program changed, at the user's request",
        "Overwrite save requested"
    );
    state.save_project_overwriting(path)
}

/// The one-line notice a legacy (SQLite) open leaves when the conversion to
/// the station-graph model moved stations (plan 7.2.10), taken once: the
/// frontend asks after a load and shows it as a banner. `None` for a version
/// 2 file and for a conversion that moved nothing.
#[tauri::command]
pub fn take_conversion_notice(state: State<'_, AppState>) -> Option<String> {
    let notice = state.take_conversion_notice();
    if notice.is_some() {
        tracing::info!(
            purpose = "tell the user the old file format conversion moved stations",
            "Handing the conversion notice to the frontend"
        );
    }
    notice
}

/// Whether the open project has changes no save has written yet (the
/// backend's dirty flag). Warning ignores mark it without an undo entry
/// (plan 6.5), so the frontend reads it after one to keep its own dirty flag,
/// which autosave and the exit prompt watch, in step.
#[tauri::command]
pub fn has_unsaved_changes(state: State<'_, AppState>) -> bool {
    state.has_unsaved_changes()
}

/// Why the open project is read-only, when the file was made with a newer
/// Stalagmight (an unknown `requires` feature or unknown survey content, plan
/// 6.4); `None` when it is editable as far as the file is concerned. A peek,
/// like [`history_discarded`]: the condition lasts as long as the project is
/// open, and edits, undo and saves are refused with `READ_ONLY_FILE: ...`.
#[tauri::command]
pub fn project_read_only_reason(state: State<'_, AppState>) -> Option<String> {
    state.read_only_reason()
}

/// What a close does with a held SpeleoDB write lock - the exit prompt's three
/// answers, plus the silent default for a project with nothing un-uploaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseLock {
    /// Hand the lock back (holder-checked) - unless the project still has
    /// un-pushed edits, in which case the backend keeps it (see
    /// `speleodb_lock_release`). A failed release keeps the project open.
    Release,
    /// "Keep the lock": close, leave the lock held for the next open to resume.
    Keep,
    /// "Discard changes", confirmed: remove the local copy, then release.
    Discard,
}

impl CloseLock {
    /// `discard_local` wins: it is the one answer that must never be downgraded,
    /// because the user confirmed it and the frontend then continues the exit as
    /// though the copy is gone. `release_lock` defaults to `true`.
    fn from_args(release_lock: Option<bool>, discard_local: Option<bool>) -> Self {
        if discard_local == Some(true) {
            Self::Discard
        } else if release_lock == Some(false) {
            Self::Keep
        } else {
            Self::Release
        }
    }
}

/// Close the current project. `release_lock` (default `true`) hands a held
/// SpeleoDB write lock back first (holder-checked) so a close doesn't strand the
/// server mutex (fix 1) - never over un-pushed edits, which keep it - and closes
/// only once that release is settled: one that fails or is uncertain closes
/// nothing and reports `lock_release: Failed`, for the frontend's "Couldn't hand
/// back the lock" dialog. `false` keeps the lock on purpose (the exit prompt's
/// "Keep the lock", that dialog's Close and keep the lock). `discard_local: true`
/// is the exit prompt's confirmed "Discard changes": the local copy is removed,
/// then the lock is released.
///
/// Async + `spawn_blocking`: the release is a network round trip, and a sync
/// command runs on the main thread - a slow or unreachable server froze the whole
/// UI for the HTTP timeout on every close.
#[tauri::command]
pub async fn close_project(
    app: tauri::AppHandle,
    release_lock: Option<bool>,
    discard_local: Option<bool>,
) -> CommandResult {
    let lock = CloseLock::from_args(release_lock, discard_local);
    // The exit prompt's answers end here (keep / discard / release): the app's
    // exit waits for one in flight instead of cutting it (lock#6).
    let _transfer = super::exit_wait::user_transfer(&app);
    tauri::async_runtime::spawn_blocking(move || {
        close_project_impl(app.state::<AppState>().inner(), lock)
    })
    .await
    .unwrap_or_else(|e| CommandResult::failure(format!("Closing the project failed: {e}")))
}

pub(crate) fn close_project_impl(state: &AppState, lock: CloseLock) -> CommandResult {
    match lock {
        // Release a held SpeleoDB lock BEFORE the close tears down the database
        // (the link row lives there), and close only once it is settled.
        CloseLock::Release => return state.speleodb_close_releasing_lock(),
        // Retain the server mutex past this close. Recorded on the claim BEFORE
        // the teardown, while the link row is still readable - and recorded at
        // all because the stranded-claim janitor cannot otherwise tell a
        // deliberate keep from a session that died holding a lock, and would hand
        // this one back on the next status refresh.
        CloseLock::Keep => state.speleodb_keep_open_lock_deliberately(),
        // Closes (and releases) itself, in the only order that cannot strand the
        // work: the copy goes first, the lock after.
        CloseLock::Discard => return state.speleodb_close_discarding_local(),
    }
    state.execute(Command::CloseProject)
}

/// Sign-out's "Discard" for a SpeleoDB project's cached copy that is NOT open:
/// remove it so the sign-out leaves no un-uploaded work behind. Releases nothing;
/// the sign-out releases the account's remaining locks explicitly afterwards.
///
/// `discard_unpushed` is the user's explicit Discard: without it a copy with
/// un-uploaded (or unreadable) work is refused. A copy that is not the signed-in
/// account's is refused either way.
#[tauri::command]
pub async fn speleodb_discard_cached_copy(
    app: tauri::AppHandle,
    project_id: String,
    discard_unpushed: bool,
) -> CommandResult {
    // A sign-out action: the app's exit waits for it (lock#6).
    let _transfer = super::exit_wait::user_transfer(&app);
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>()
            .inner()
            .speleodb_discard_cached_copy(&project_id, discard_unpushed)
    })
    .await
    .unwrap_or_else(|e| CommandResult::failure(format!("Discarding the copy failed: {e}")))
}

/// Get the full project state for the frontend
#[tauri::command]
pub fn get_project_state(state: State<AppState>) -> Option<ProjectState> {
    get_project_state_impl(state.inner())
}

pub(crate) fn get_project_state_impl(state: &AppState) -> Option<ProjectState> {
    state.project_state_wire()
}

/// Per-station along-cave distance from a chosen origin station, or from the
/// zero-datum entrance(s) when `from` is null - backs the distance-gradient
/// overlay (#34). Distances are computed on the **closure-adjusted** geometry so
/// they match the corrected map (falls back to measured when there are no
/// eligible closures). `distances` are index-aligned with `station_refs`; `null`
/// entries are unreachable, and `max_reachable` scales the legend.
///
/// Async + `spawn_blocking` (mirrors `get_adjusted_stations`): the adjusted-coord
/// solve is memoized in `AppState`, and we keep the heavy work off the UI thread.
#[tauri::command]
pub async fn get_station_distances_from(
    app_handle: tauri::AppHandle,
    from: Option<StationRef>,
) -> Option<DistanceField> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        // AppState::distance_field takes one reconciled snapshot so topology and
        // adjusted coords are consistent (no two-read skew).
        state.inner().distance_field(from.as_ref())
    })
    .await
    .ok()
    .flatten()
}

/// Along-cave-line distance between two ruler endpoints (#26): the shortest path
/// through the survey graph (shots weighted by measured length; merges and loop
/// closures zero-cost), plus the ordered shots traversed so the frontend can
/// tint them. Each endpoint is a station or an exact point on a shot. Returns
/// `along_line: null` when the endpoints are in different connected components.
#[tauri::command]
pub async fn get_along_line_distance(
    app_handle: tauri::AppHandle,
    a: RulerEndpoint,
    b: RulerEndpoint,
) -> Option<AlongLineResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        state.inner().along_line_between(a, b)
    })
    .await
    .ok()
    .flatten()
}

/// Get closure-adjusted station coordinates for the current project.
///
/// This is the lazy counterpart to [`get_project_state`], whose `stations` are
/// always the raw *measured* geometry. The frontend calls this only when the
/// user switches the display to the closure-adjusted view. The adjustment is
/// derived non-destructively (the measured project is never mutated) and the
/// result is memoized in [`AppState`], so the heavy solve runs at most once per
/// data/config version. Runs on a blocking thread to keep the UI responsive on
/// large surveys.
///
/// Returns the measured coordinates verbatim when there are no eligible
/// closures, and an empty vec when no project is open.
#[tauri::command]
pub async fn get_adjusted_stations(app_handle: tauri::AppHandle) -> Vec<Station> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        state.inner().get_adjusted_stations().unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// What the Closures Computed View left out (#737): the loop closures it skips
/// because they tie the two ends of one shot together (computed now), and the
/// parts of the survey whose solve failed and are shown measured (from the
/// memoized solve of the current project only - this never runs the solve).
/// The frontend asks right after [`get_adjusted_stations`], so the solve it
/// reports on is the one it just drew. `get_adjusted_stations` keeps returning
/// a bare `Vec<Station>`, so the export path and other callers are unchanged.
///
/// An empty report when no project is open.
#[tauri::command]
pub async fn get_adjustment_report<R: tauri::Runtime>(
    app_handle: tauri::AppHandle<R>,
) -> AdjustmentReport {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        state.inner().get_adjustment_report().unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// Get list of recent projects
#[tauri::command]
pub fn list_recent_projects(state: State<AppState>) -> Vec<ProjectMetadata> {
    list_recent_projects_impl(state.inner())
}

pub(crate) fn list_recent_projects_impl(state: &AppState) -> Vec<ProjectMetadata> {
    state.get_recent_projects()
}

/// Remove a project from the recent projects list
#[tauri::command]
pub fn remove_from_recent_projects(state: State<AppState>, path: PathBuf) -> CommandResult {
    remove_from_recent_projects_impl(state.inner(), path)
}

pub(crate) fn remove_from_recent_projects_impl(state: &AppState, path: PathBuf) -> CommandResult {
    state.remove_from_recent_projects(path)
}

/// Permanently delete a project's files from disk and drop it from the recent
/// list. The destructive counterpart to `remove_from_recent_projects`; see
/// [`AppState::delete_project`] for the OS-trash / permanent-delete semantics.
///
/// Async, off the main thread: deleting the open SpeleoDB project hands its write
/// lock back over the network once the file is gone, after waiting for any
/// SpeleoDB lock operation in flight. As a synchronous command that froze the
/// whole window for the length of both.
#[tauri::command]
pub async fn delete_project(app: tauri::AppHandle, path: PathBuf) -> CommandResult {
    tauri::async_runtime::spawn_blocking(move || {
        delete_project_impl(app.state::<AppState>().inner(), path)
    })
    .await
    .unwrap_or_else(|e| CommandResult::failure(format!("Delete task failed: {e}")))
}

pub(crate) fn delete_project_impl(state: &AppState, path: PathBuf) -> CommandResult {
    state.delete_project(path)
}

/// Rename a recent project's display name by path (project need not be open).
#[tauri::command]
pub fn set_project_name_for_path(
    state: State<AppState>,
    path: PathBuf,
    name: String,
) -> CommandResult {
    state.set_project_name_for_path(path, name)
}

/// Merge multiple projects into a new project file
/// This creates a NEW output file, appends all lines with renumbered IDs,
/// keeps all zero datums, and optionally archives the original files.
#[tauri::command]
pub fn merge_projects(
    state: State<AppState>,
    source_paths: Vec<PathBuf>,
    output_path: PathBuf,
    output_name: String,
    archive_originals: bool,
) -> CommandResult {
    merge_projects_impl(
        state.inner(),
        source_paths,
        output_path,
        output_name,
        archive_originals,
    )
}

pub(crate) fn merge_projects_impl(
    state: &AppState,
    source_paths: Vec<PathBuf>,
    output_path: PathBuf,
    output_name: String,
    archive_originals: bool,
) -> CommandResult {
    state.execute(Command::MergeProjects {
        source_paths,
        output_path,
        output_name,
        archive_originals,
    })
}

/// Combined project state for frontend: the station-graph wire projection
/// (`survey::wire::ProjectStateWire`, the TS `ProjectState`).
pub type ProjectState = crate::models::survey::wire::ProjectStateWire;

/// Load project with full progress reporting.
/// Uses spawn_blocking for UI responsiveness during computation phases.
///
/// Returns LoadProjectFullResult containing project, stations, stats, warnings, and display settings.
/// Emits "loading-progress" events during load for progress UI.
///
/// Note: Uses AppHandle instead of State<'_, AppState> because Tauri's State<'_, T>
/// wraps a reference (&'r Arc<T>) that cannot outlive the function body. For spawn_blocking,
/// we need 'static ownership, which AppHandle provides via app_handle.state::<T>().
#[tauri::command]
pub async fn load_project_full(
    app_handle: tauri::AppHandle,
    path: PathBuf,
) -> Result<LoadProjectFullResult, LoadError> {
    // [perf-debug] Log when the Rust handler is about to return so we can
    // subtract that from the JS-side IPC wait time and isolate IPC
    // transport + JS JSON.parse. The "load_project_full handler returning"
    // line lets us compute: js_total - rust_total ≈ serialize + parse.
    let t_start = std::time::Instant::now();
    let result: Result<Result<LoadProjectFullResult, LoadError>, _> =
        tauri::async_runtime::spawn_blocking(move || {
            let state = app_handle.state::<AppState>();
            state.do_load_project_full(path)
        })
        .await;
    let handler_elapsed = t_start.elapsed();

    let mapped = match result {
        Ok(Ok(load_result)) => Ok(load_result),
        Ok(Err(load_error)) => Err(load_error),
        Err(join_error) => Err(LoadError::internal(join_error.to_string())),
    };
    tracing::debug!(
        handler_ms = handler_elapsed.as_millis() as u64,
        ok = mapped.is_ok(),
        "load_project_full handler returning (serialize+transport+JSON.parse begins now)"
    );
    mapped
}

/// Binary variant of `load_project_full` that returns a msgpack-encoded
/// payload with shot/station bulk data packed into typed-array byte buffers.
///
/// Replaces ~25 MB of JSON with ~7 MB of binary on a 75k-shot project, and
/// removes the synchronous `serde_json::to_string` + `JSON.parse` cost from
/// both threads. See [`crate::core::load_binary`] for the wire format. The
/// JSON variant is kept for back-compat and tests; new callers should use
/// this one.
#[tauri::command]
pub async fn load_project_full_binary(
    app_handle: tauri::AppHandle,
    path: PathBuf,
) -> Result<tauri::ipc::Response, LoadError> {
    let t_start = std::time::Instant::now();
    let result: Result<Result<LoadProjectFullResult, LoadError>, _> =
        tauri::async_runtime::spawn_blocking(move || {
            let state = app_handle.state::<AppState>();
            state.do_load_project_full(path)
        })
        .await;
    let handler_elapsed = t_start.elapsed();

    let load_result = match result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(e),
        Err(join_err) => return Err(LoadError::internal(join_err.to_string())),
    };

    let encode_start = std::time::Instant::now();
    let bytes = crate::core::load_binary::encode(load_result)
        .map_err(|e| LoadError::internal(format!("encode binary: {e}")))?;
    let encode_elapsed = encode_start.elapsed();

    tracing::debug!(
        handler_ms = handler_elapsed.as_millis() as u64,
        encode_ms = encode_elapsed.as_millis() as u64,
        bytes = bytes.len(),
        "load_project_full_binary returning"
    );

    Ok(tauri::ipc::Response::new(bytes))
}

/// Cancel any active loading operation.
/// Idempotent - safe to call anytime, even when no load is active.
#[tauri::command]
pub fn cancel_loading(state: State<AppState>) {
    state.cancel_loading();
}

/// Create a persisted project with full progress reporting.
///
/// This creates a new project file immediately, emitting progress events
/// during creation phases (CreatingFile, InitializingProject, SavingSettings).
///
/// Unlike `create_project`, this command supports assigning a canonical save path
/// during project creation and can optionally defer the initial disk write.
/// It:
/// - Emits "loading-progress" events for UI feedback
/// - Returns the project with display settings
/// - Persists immediately unless `defer_initial_save` is true
///
/// # Arguments
/// * `name` - Project name (used for both filename and project metadata)
/// * `project_location` - Optional cave location metadata (free text).
/// * `save_directory` - Optional directory path to save the project in.
///   If omitted (and `save_path` is also omitted), uses user's documents
///   `stalagmight` subdirectory.
/// * `save_path` - Optional full file path to save the project to.
///   If provided, this takes precedence over `save_directory`.
/// * `defer_initial_save` - Optional flag to reserve `project.file_path` without
///   writing the file yet. First save/autosave creates the file.
///
/// # Technical Note
/// Uses AppHandle instead of State<'_, AppState> because Tauri's State<'_, T>
/// wraps a reference (&'r Arc<T>) that cannot outlive the function body. For spawn_blocking,
/// we need 'static ownership, which AppHandle provides via app_handle.state::<T>().
/// Shots the last load could not read a `kind` for (#27), taken once.
///
/// The loader already substituted [`ShotKind::Reference`] for each - fail-closed,
/// so an unreadable byte cannot put a swimmable leg into gas, penetration or the
/// ruler - but that is a guess. The frontend calls this after a load and, when
/// the list is non-empty, asks the user what each shot actually is; their answer
/// lands as an ordinary `SetShotKind`, undoable like any other edit.
///
/// Taking rather than peeking makes it one-shot: a project refresh cannot
/// resurrect a prompt the user already dismissed.
///
/// `loading_id` is which load is asking. Two loads can overlap - switching
/// projects cancels the first, but a cancelled load keeps running to its next
/// checkpoint - so an unversioned slot could hand this project the shot ids of
/// one the user navigated away from. A mismatch returns nothing.
#[tauri::command]
pub fn take_unrecognised_shot_kinds(
    state: State<'_, AppState>,
    loading_id: u64,
) -> Vec<crate::models::UnrecognisedShotKind> {
    let found = state.take_unrecognised_shot_kinds(loading_id);
    if !found.is_empty() {
        tracing::info!(
            count = found.len(),
            purpose = "prompt the user to resolve unreadable shot kinds (#27)",
            "Handing unrecognised shot kinds to the frontend"
        );
    }
    found
}

/// [#473] Whether the open project's edit history was unreadable and discarded.
///
/// The load paths substitute an empty stack for a history they cannot read - corrupt,
/// truncated, or crafted past the decompression bounds - because opening the project
/// without its history beats not opening it. Until this command existed, the only
/// record of that was a `tracing::warn!`, so the user's entire undo AND redo history
/// disappeared with nothing on screen to say so.
///
/// Peeking, not taking, unlike [`take_unrecognised_shot_kinds`]: that one drives a
/// one-shot prompt the user answers, this describes an ongoing condition that lasts as
/// long as the project is open (docs/notifications.md: banner-class). Dismissal is
/// frontend state, as it is for the integrity banner.
///
/// No `loading_id` for the same reason: the flag is written under the same `project`
/// mutex acquisition that commits the project, so it cannot describe a different one.
#[tauri::command]
pub fn history_discarded(state: State<'_, AppState>) -> bool {
    state.history_discarded()
}

/// Whether the history [`history_discarded`] reports was written by an earlier
/// version of the app (the format before compact undo entries) rather than found
/// unreadable, so the banner can say so. A peek, like its sibling.
#[tauri::command]
pub fn history_discarded_from_earlier_version(state: State<'_, AppState>) -> bool {
    state.history_discarded_from_earlier_version()
}

#[tauri::command]
pub async fn create_project_full(
    app_handle: tauri::AppHandle,
    name: String,
    project_location: Option<String>,
    save_directory: Option<PathBuf>,
    save_path: Option<PathBuf>,
    defer_initial_save: Option<bool>,
) -> Result<CreateProjectFullResult, LoadError> {
    // Use AppHandle to access state inside spawn_blocking
    // (State<'_, T> cannot be moved due to lifetime constraints)
    let result: Result<Result<CreateProjectFullResult, LoadError>, _> =
        tauri::async_runtime::spawn_blocking(move || {
            let state = app_handle.state::<AppState>();
            state.do_create_project_full(
                name,
                project_location,
                save_directory,
                save_path,
                defer_initial_save.unwrap_or(false),
            )
        })
        .await;

    match result {
        Ok(Ok(create_result)) => Ok(create_result),
        Ok(Err(load_error)) => Err(load_error),
        Err(join_error) => {
            // Task panicked or was cancelled
            Err(LoadError::internal(join_error.to_string()))
        }
    }
}

#[cfg(test)]
mod close_lock_tests {
    use super::CloseLock;

    #[test]
    fn close_lock_reads_the_exit_prompt_answer_from_the_ipc_arguments() {
        // Omitted entirely: the ordinary close, which releases (never over
        // un-pushed edits - that veto lives in the backend, not here).
        assert_eq!(CloseLock::from_args(None, None), CloseLock::Release);
        assert_eq!(CloseLock::from_args(Some(true), None), CloseLock::Release);
        assert_eq!(CloseLock::from_args(Some(false), None), CloseLock::Keep);
        assert_eq!(CloseLock::from_args(None, Some(false)), CloseLock::Release);
        // The user's confirmed discard is never downgraded by a stray release_lock.
        assert_eq!(CloseLock::from_args(None, Some(true)), CloseLock::Discard);
        assert_eq!(
            CloseLock::from_args(Some(false), Some(true)),
            CloseLock::Discard
        );
    }
}

/// The `.stalx` v2 file-status commands, end to end on an [`AppState`]: what
/// the frontend's banners, save prompt and dirty tracking read.
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: a failed setup step is the point"
)]
mod file_status_tests {
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use crate::core::state::{EXTERNALLY_MODIFIED_CODE, READ_ONLY_FILE_CODE};
    use crate::core::{AppState, Command, CommandResult};
    use crate::models::survey::{LineMeta, Project, StartAt};
    use crate::models::{ShotData, ShotUpdates};
    use crate::persistence::stalx_v2::{self, Extras, Mode};

    /// One free line of two shots, laid out from its start at the origin.
    fn survey() -> Project {
        let mut p = Project::new("File status", None);
        let shot = |length: f64| ShotData {
            length,
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            depth_in: 1.0,
            depth_out: 1.0,
            ..ShotData::default()
        };
        let line = p
            .add_line(
                LineMeta::default(),
                StartAt::New,
                vec![shot(10.0), shot(7.5)],
            )
            .unwrap();
        let start = p.line(line).unwrap().start;
        p.set_placement(start, 0.0, 0.0).unwrap();
        assert!(p.validate().is_empty(), "{:?}", p.validate());
        p
    }

    fn write_v2(dir: &Path) -> PathBuf {
        let path = dir.join("status.stalx");
        let bytes = stalx_v2::encode(&survey(), &Extras::default(), Mode::Normal).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn open(path: &Path) -> AppState {
        let state = AppState::new().unwrap();
        let r = state.execute(Command::OpenProject {
            path: path.to_path_buf(),
        });
        assert!(r.success, "{:?}", r.error);
        state
    }

    fn edit(state: &AppState) -> CommandResult {
        let project = state.get_project_state().unwrap();
        let line = &project.lines()[0];
        state.execute(Command::UpdateShot {
            line_id: line.id,
            shot_id: line.shots[0].id,
            updates: ShotUpdates {
                length: Some(11.25),
                ..ShotUpdates::default()
            },
        })
    }

    #[test]
    fn an_ordinary_v2_file_reports_nothing_and_tracks_unsaved_changes() {
        let dir = TempDir::new().unwrap();
        let state = open(&write_v2(dir.path()));
        assert_eq!(state.read_only_reason(), None);
        assert_eq!(state.take_conversion_notice(), None);
        assert!(!state.has_unsaved_changes());

        assert!(edit(&state).success);
        assert!(state.has_unsaved_changes());
        let r = state.execute(Command::SaveProject { path: None });
        assert!(r.success, "{:?}", r.error);
        assert!(!state.has_unsaved_changes());
    }

    #[test]
    fn a_file_changed_on_disk_is_refused_and_the_overwrite_answer_writes_it() {
        let dir = TempDir::new().unwrap();
        let path = write_v2(dir.path());
        let state = open(&path);
        assert!(edit(&state).success);

        // Another program rewrites the file (same survey, other bytes).
        let mut theirs = std::fs::read_to_string(&path).unwrap();
        theirs.push('\n');
        std::fs::write(&path, &theirs).unwrap();

        let r = state.execute(Command::SaveProject { path: None });
        assert!(!r.success);
        assert!(r
            .error
            .as_deref()
            .unwrap_or_default()
            .starts_with(EXTERNALLY_MODIFIED_CODE));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);
        assert!(state.has_unsaved_changes());

        let r = state.save_project_overwriting(None);
        assert!(r.success, "{:?}", r.error);
        assert_ne!(std::fs::read_to_string(&path).unwrap(), theirs);
        assert!(!state.has_unsaved_changes());
        // And the guard follows the bytes it wrote: a plain save passes again.
        assert!(edit(&state).success);
        let r = state.execute(Command::SaveProject { path: None });
        assert!(r.success, "{:?}", r.error);
    }

    #[test]
    fn a_file_made_with_a_newer_stalagmight_says_why_it_is_read_only() {
        let dir = TempDir::new().unwrap();
        let path = write_v2(dir.path());
        let text = std::fs::read_to_string(&path).unwrap();
        let patched = text.replacen("\"requires\":[]", "\"requires\":[\"x-lofts\"]", 1);
        assert_ne!(patched, text);
        std::fs::write(&path, patched.as_bytes()).unwrap();
        let state = open(&path);

        let reason = state.read_only_reason().expect("read-only");
        assert!(reason.contains("newer Stalagmight"), "{reason}");
        assert!(reason.contains("x-lofts"), "{reason}");
        // The Overwrite answer is no way around it.
        let r = state.save_project_overwriting(None);
        assert!(!r.success);
        assert!(r
            .error
            .as_deref()
            .unwrap_or_default()
            .starts_with(READ_ONLY_FILE_CODE));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), patched);
    }
}
