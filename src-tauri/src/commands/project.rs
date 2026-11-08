use std::path::PathBuf;
use tauri::{Manager, Runtime};
use ts_rs::TS;

use crate::commands::pool;
use crate::commands::write_path::{check_write_path, load_upgrade_refusal, WriteTarget};
use crate::core::loading::{CreateProjectFullResult, LoadError, LoadProjectFullResult};
use crate::core::state::{for_project, ProjectIdentity};
use crate::core::{
    AppState, Command, CommandError, CommandResult, DeleteProjectOutcome, EditOutcome,
    MergeProjectsOutcome, ProjectFileOutcome, SaveOutcome,
};
use crate::models::{
    AlongLineResult, DistanceField, FrozenShotRef, Project, ProjectMetadata, ProjectStats,
    RulerEndpoint, Station, StationRef,
};
use crate::persistence::with_stalx_extension;

/// Update per-project settings (#57): project name + validation thresholds, and
/// (#576 NF-W3-2) the default north reference (`null` = nothing recorded).
/// Stored in the .stalx so they travel with the file between collaborators.
#[tauri::command]
pub async fn update_project_settings<R: Runtime>(
    app: tauri::AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    name: String,
    validation: crate::config::ValidationConfig,
    default_azimuth_reference: Option<crate::models::AzimuthReference>,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .update_project_settings(name, validation, default_azimuth_reference)
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Set (or, given `null`/omitted, clear) the open project's entrance
/// elevation in meters (#465). Stored in the .stalx so it travels with the
/// file; the only recovery path for the TML import's no-clobber rule (#431)
/// locking in a wrong first value.
#[tauri::command]
pub async fn set_surface_elevation<R: Runtime>(
    app: tauri::AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    elevation: Option<f64>,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .set_surface_elevation(elevation)
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Save the current project.
///
/// `is_autosave` changes NOTHING about the save - it only names who asked for
/// it, so a background write is distinguishable from a Ctrl+S in the log
/// (#429). Absent means manual: every other Rust caller either goes through
/// [`save_project_impl`] or constructs `Command::SaveProject` itself; both
/// funnel into the same `execute` dispatch.
#[tauri::command]
pub async fn save_project<R: Runtime>(
    app: tauri::AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    path: Option<PathBuf>,
    is_autosave: Option<bool>,
) -> Result<SaveOutcome, CommandError> {
    pool::run(app, move |state| {
        // [#576 FZ-FZ-INT-4] For the project the save was made for: one that lands
        // after a switch is refused (`PROJECT_CHANGED`) rather than writing the
        // project now open - to the path chosen for the other one, for a Save As.
        for_project(expected_project, || {
            save_project_for(state, path, is_autosave)
        })
    })
    .await
}

fn save_project_for(
    state: &AppState,
    path: Option<PathBuf>,
    is_autosave: Option<bool>,
) -> Result<SaveOutcome, CommandError> {
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
    save_project_impl(state, path).into_outcome::<SaveOutcome>()
}

pub(crate) fn save_project_impl(state: &AppState, path: Option<PathBuf>) -> CommandResult {
    // [#576 C11-D1] A Save As target, as the save will name it.
    if let Some(path) = &path {
        if let Err(why) = check_project_target(&with_stalx_extension(path.clone())) {
            return CommandResult::failure(why);
        }
    }
    state.execute(Command::SaveProject { path })
}

/// [#576 C11-D1] The write-path check for a project file a command writes,
/// replaces or deletes.
fn check_project_target(path: &std::path::Path) -> Result<(), String> {
    check_write_path(path, WriteTarget::Project)
}

/// Close the current project. `release_lock` (default `true`) best-effort releases
/// a held SpeleoDB write lock first so a close doesn't strand the server mutex
/// (fix 1); the close dialog's "Keep lock" path passes `false`.
///
/// `true` is a REQUEST, not a guarantee: like every other silent release it goes
/// through `speleodb_held_lock_release_params`, which declines while the project
/// has un-pushed edits (see `core::state::speleodb_lock_release`). A caller that
/// must release regardless - "discard my edits and release" - calls
/// `speleodb_release_lock` explicitly and then closes.
#[tauri::command]
pub async fn close_project<R: Runtime>(
    app: tauri::AppHandle<R>,
    release_lock: Option<bool>,
) -> Result<ProjectFileOutcome, CommandError> {
    pool::run(app, move |state| {
        // Release a held SpeleoDB lock BEFORE the close tears down the database (the
        // link row lives there). Best-effort + gated so the keep-lock path can opt out.
        if release_lock.unwrap_or(true) {
            state.speleodb_release_held_lock_best_effort();
        } else {
            // "Keep lock": the user chose to hold the server mutex past this close.
            // Recorded on the claim BEFORE the teardown, while the link row is still
            // readable - and recorded at all because the stranded-claim janitor
            // cannot otherwise tell a deliberate keep from a session that died
            // holding a lock, and would hand this one back on the next status poll.
            state.speleodb_keep_open_lock_deliberately();
        }
        state
            .execute(Command::CloseProject)
            .into_outcome::<ProjectFileOutcome>()
    })
    .await
}

/// Get the full project state as JSON.
///
/// [#576 REFRESH-CMDS] Kept deliberately next to [`get_project_state_binary`],
/// which is the app's own read (the refresh after every edit): nothing in the
/// frontend calls this one. It stays for the agent harness (`e2e/agent/`, which
/// reads the project as data) and for debugging, where a readable payload beats
/// the byte envelope. Both read the settled snapshot through `measured_state`, so
/// they cannot disagree. See docs/architecture.md, "Reading the project back".
#[tauri::command]
pub async fn get_project_state<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<ProjectState>, CommandError> {
    pool::run(app, |state| {
        let Some(project) = state.settled_snapshot() else {
            return Ok(None);
        };
        let (stations, stats) = measured_state(state, &project);
        let frozen_shots = project.compute_frozen_shots();

        Ok(Some(ProjectState {
            project,
            stations,
            stats,
            frozen_shots,
        }))
    })
    .await
}

/// [#576 A9-D2] [`get_project_state`] in the load's binary layout
/// (`core::load_binary`, decoded by the frontend's `loadProjectBinary.ts`):
/// the refresh the frontend issues after every edit. At 75,000 shots the JSON
/// form is ~50 MB to build and to `JSON.parse`; this is the typed-array envelope
/// the load already uses. An empty response means no project is open.
#[tauri::command]
pub async fn get_project_state_binary<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<tauri::ipc::Response, CommandError> {
    pool::run(app, |state| {
        // [#576 W75-NF7] Read BEFORE the snapshot: an in-place swap bumps it before
        // the new tree is committed, so this pairs a tree with its own generation or
        // with an older one (a save then refused, the safe side), never an old tree
        // with the new generation.
        let tree_generation = state.tree_generation();
        let Some(project) = state.settled_snapshot() else {
            return Ok(tauri::ipc::Response::new(Vec::new()));
        };
        let (stations, stats) = measured_state(state, &project);
        crate::core::load_binary::encode_refresh(&project, stations, stats, tree_generation)
            .map(tauri::ipc::Response::new)
            .map_err(|e| CommandError::failed(format!("Could not encode the project state: {e}")))
    })
    .await
}

/// The measured stations and the stats a state read sends with the project.
fn measured_state(state: &AppState, project: &Project) -> (Vec<Station>, ProjectStats) {
    let stations = project.compute_station_coordinates();
    let mut stats = project.stats();

    // Calculate penetration distance with computed stations. Reference ties are
    // not swimmable, so they leave the route unless opted back in (#27 q13).
    let include_reference_legs = state.get_config().gas.include_reference_legs;
    stats.penetration_distance =
        project.calculate_penetration_with(&stations, include_reference_legs);

    // Depth is the absolute station range, not a per-section delta walk (#481).
    let (min_depth, max_depth) = Project::depth_range(&stations);
    stats.min_depth = min_depth;
    stats.max_depth = max_depth;
    (stations, stats)
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
///
/// [#576 C7] `null` means no project is open; a field that could not be computed (a
/// panic in the task, an unreadable project) REJECTS, so the overlay drops its ramp
/// instead of holding the previous one as if a recompute were still in flight.
#[tauri::command]
pub async fn get_station_distances_from(
    app_handle: tauri::AppHandle,
    from: Option<StationRef>,
) -> Result<Option<DistanceField>, CommandError> {
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        // AppState::distance_field takes one reconciled snapshot so topology and
        // adjusted coords are consistent (no two-read skew).
        state.inner().distance_field_checked(from.as_ref())
    })
    .await;
    settle_blocking("the distance field", joined)
}

/// Along-cave-line distance between two ruler endpoints (#26): the shortest path
/// through the survey graph (shots weighted by measured length; merges and loop
/// closures zero-cost), plus the ordered shots traversed so the frontend can
/// tint them. Each endpoint is a station or an exact point on a shot. Returns
/// `along_line: null` when the endpoints are in different connected components.
///
/// [#576 C7] `null` means no project is open; a measurement that could not be
/// computed REJECTS rather than reading as "no path".
#[tauri::command]
pub async fn get_along_line_distance(
    app_handle: tauri::AppHandle,
    a: RulerEndpoint,
    b: RulerEndpoint,
) -> Result<Option<AlongLineResult>, CommandError> {
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        state.inner().along_line_between_checked(a, b)
    })
    .await;
    settle_blocking("the along-line distance", joined)
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
///
/// [#576 C7] A solve that could not run - a panic in the task, an unreadable
/// project - REJECTS. It used to resolve to an empty vec, which the frontend took
/// as a successful (blank) adjusted view, so its fallback to the measured view
/// with the "couldn't compute" notice never ran and the user was left in a
/// read-only view with nothing drawn.
#[tauri::command]
pub async fn get_adjusted_stations(
    app_handle: tauri::AppHandle,
) -> Result<Vec<Station>, CommandError> {
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        state.inner().adjusted_stations_checked()
    })
    .await;
    settle_blocking("the closure-adjusted solve", joined).map(Option::unwrap_or_default)
}

/// [#576 C7] A blocking query's outcome as the frontend receives it: the query's own
/// answer, or - when the task panicked or was cancelled (`joined` is `Err`) - a
/// rejection naming `what` failed. Never an empty answer standing in for a failure.
/// [#576 C4] The query's own failure is `FAILED`; a crashed task is
/// `INTERNAL_ERROR`, as on the shared pool.
pub(crate) fn settle_blocking<T, E: std::fmt::Display>(
    what: &str,
    joined: Result<Result<T, String>, E>,
) -> Result<T, CommandError> {
    match joined {
        Ok(answer) => answer
            .map_err(|reason| CommandError::failed(format!("Could not compute {what}: {reason}"))),
        Err(crash) => Err(CommandError::internal(format!(
            "Could not compute {what}: the computation failed ({crash})"
        ))),
    }
}

/// Get list of recent projects
#[tauri::command]
pub async fn list_recent_projects<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<ProjectMetadata>, CommandError> {
    pool::run(app, |state| Ok(state.get_recent_projects())).await
}

/// Remove a project from the recent projects list
#[tauri::command]
pub async fn remove_from_recent_projects<R: Runtime>(
    app: tauri::AppHandle<R>,
    path: PathBuf,
) -> Result<ProjectFileOutcome, CommandError> {
    pool::run(app, move |state| {
        state
            .remove_from_recent_projects(path)
            .into_outcome::<ProjectFileOutcome>()
    })
    .await
}

/// Permanently delete a project's files from disk and drop it from the recent
/// list. The destructive counterpart to `remove_from_recent_projects`; see
/// [`AppState::delete_project`] for the OS-trash / permanent-delete semantics.
#[tauri::command]
pub async fn delete_project<R: Runtime>(
    app: tauri::AppHandle<R>,
    path: PathBuf,
) -> Result<DeleteProjectOutcome, CommandError> {
    pool::run(app, move |state| {
        if let Err(why) = check_project_target(&path) {
            return Err(CommandError::failed(why));
        }
        state
            .delete_project(path)
            .into_outcome::<DeleteProjectOutcome>()
    })
    .await
}

/// Rename a recent project's display name by path (project need not be open).
#[tauri::command]
pub async fn set_project_name_for_path<R: Runtime>(
    app: tauri::AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    path: PathBuf,
    name: String,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        if let Err(why) = check_project_target(&path) {
            return Err(CommandError::failed(why));
        }
        state
            .set_project_name_for_path(path, name)
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Merge multiple projects into a new project file
/// This creates a NEW output file, appends all sections with renumbered IDs,
/// keeps all zero datums, and optionally archives the original files.
#[tauri::command]
pub async fn merge_projects<R: Runtime>(
    app: tauri::AppHandle<R>,
    source_paths: Vec<PathBuf>,
    output_path: PathBuf,
    output_name: String,
    archive_originals: bool,
) -> Result<MergeProjectsOutcome, CommandError> {
    pool::run(app, move |state| {
        // The new file, and - when they are to be moved into the archive - the sources.
        let moved: &[PathBuf] = if archive_originals {
            &source_paths
        } else {
            &[]
        };
        let targets =
            std::iter::once(with_stalx_extension(output_path.clone())).chain(moved.iter().cloned());
        for target in targets {
            if let Err(why) = check_project_target(&target) {
                return Err(CommandError::failed(why));
            }
        }
        state
            .execute(Command::MergeProjects {
                source_paths,
                output_path,
                output_name,
                archive_originals,
            })
            .into_outcome::<MergeProjectsOutcome>()
    })
    .await
}

/// Combined project state for frontend
#[derive(serde::Serialize, TS)]
pub struct ProjectState {
    /// Serialized through [`ProjectIpc`] so cross-references reach the frontend
    /// in positional wire form (the ts-rs `Project` shape), not the internal
    /// `StationKey` shape that derived serde would emit. ts-rs still reads the
    /// field type (`Project`) for the generated binding, so the TS contract is
    /// unchanged; only the runtime serde payload is corrected.
    ///
    /// A snapshot of the open project (#576 A9), not a copy of it; ts-rs reads
    /// `Arc<Project>` as `Project`.
    #[serde(serialize_with = "serialize_project_snapshot")]
    pub project: std::sync::Arc<Project>,
    pub stations: Vec<Station>,
    pub stats: ProjectStats,
    pub frozen_shots: Vec<FrozenShotRef>,
}

fn serialize_project_snapshot<S: serde::Serializer>(
    project: &std::sync::Arc<Project>,
    s: S,
) -> Result<S::Ok, S::Error> {
    crate::models::project::serialize_project_ipc(project, s)
}

/// Load project with full progress reporting.
/// Uses spawn_blocking for UI responsiveness during computation phases.
///
/// Returns LoadProjectFullResult containing project, stations, stats, warnings, and display settings.
/// Emits "loading-progress" events during load for progress UI.
///
/// Note: Uses AppHandle instead of State<'_, AppState> because Tauri's State<'_, T>
/// wraps a reference (&'r Arc<T>) that cannot outlive the function body. For spawn_blocking,
/// we need 'static ownership, which AppHandle provides via app_handle.state::<T>().
///
/// [#576 W44-NF3] `load_token` is the name the webview gave this load; its Cancel
/// (`cancel_loading`) names it too, and cancels nothing else.
#[tauri::command]
pub async fn load_project_full(
    app_handle: tauri::AppHandle,
    path: PathBuf,
    load_token: Option<String>,
) -> Result<LoadProjectFullResult, CommandError> {
    // [perf-debug] Log when the Rust handler is about to return so we can
    // subtract that from the JS-side IPC wait time and isolate IPC
    // transport + JS JSON.parse. The "load_project_full handler returning"
    // line lets us compute: js_total - rust_total ≈ serialize + parse.
    let t_start = std::time::Instant::now();
    // [#576 RV4-R-M3-OPEN] Opens anywhere; only the in-place upgrade of an
    // older-format file (and its backup copy) is refused where nothing may be written.
    let upgrade_refusal = load_upgrade_refusal(&path);
    let result: Result<Result<LoadProjectFullResult, LoadError>, _> =
        tauri::async_runtime::spawn_blocking(move || {
            let state = app_handle.state::<AppState>();
            state.do_load_project_full_with(path, upgrade_refusal.as_ref(), load_token.as_deref())
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
    mapped.map_err(CommandError::from)
}

/// Binary variant of `load_project_full` that returns a msgpack-encoded
/// payload with shot/station bulk data packed into typed-array byte buffers.
///
/// Replaces ~25 MB of JSON with ~7 MB of binary on a 75k-shot project, and
/// removes the synchronous `serde_json::to_string` + `JSON.parse` cost from
/// both threads. See [`crate::core::load_binary`] for the wire format. The
/// JSON variant is kept for back-compat and tests; new callers should use
/// this one. `load_token`: as for `load_project_full`.
#[tauri::command]
pub async fn load_project_full_binary(
    app_handle: tauri::AppHandle,
    path: PathBuf,
    load_token: Option<String>,
) -> Result<tauri::ipc::Response, CommandError> {
    let t_start = std::time::Instant::now();
    // [#576 RV4-R-M3-OPEN] Opens anywhere; only the in-place upgrade of an
    // older-format file (and its backup copy) is refused where nothing may be written.
    let upgrade_refusal = load_upgrade_refusal(&path);
    let app = app_handle.clone();
    let result: Result<Result<LoadProjectFullResult, LoadError>, _> =
        tauri::async_runtime::spawn_blocking(move || {
            let state = app_handle.state::<AppState>();
            state.do_load_project_full_with(path, upgrade_refusal.as_ref(), load_token.as_deref())
        })
        .await;
    let handler_elapsed = t_start.elapsed();

    let load_result = match result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(e.into()),
        Err(join_err) => return Err(LoadError::internal(join_err.to_string()).into()),
    };
    // [#576 W75-NF7] The generation the tree was committed under: a swap that bumps
    // it must first close this very project, which the frontend's switch and swap
    // ordering keep from overlapping its own load.
    let tree_generation = app.state::<AppState>().tree_generation();

    let encode_start = std::time::Instant::now();
    let bytes = crate::core::load_binary::encode(load_result, tree_generation)
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

/// Cancel the load named `load_token` (the name the webview gave the open, create
/// or file import it started), if it is the one in progress.
/// Idempotent - safe to call anytime, even when no load is active.
///
/// [#576 W44-NF3] A Cancel names its load: one that runs after its load ended and
/// a newer one started must not cancel the newer one. A name that is not the
/// active load's, and no name, cancel nothing. A name that is not the active
/// load's is kept, a bounded few: a file import of that name that has not
/// registered yet starts cancelled (Codex review on #630).
#[tauri::command]
pub async fn cancel_loading<R: Runtime>(
    app: tauri::AppHandle<R>,
    load_token: Option<String>,
) -> Result<(), CommandError> {
    pool::run(app, move |state| {
        state.cancel_loading(load_token.as_deref());
        Ok(())
    })
    .await
}

/// Create a persisted project with full progress reporting.
///
/// This creates a new project database file immediately, emitting progress events
/// during creation phases (CreatingDatabase, InitializingProject, SavingSettings).
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
pub async fn take_unrecognised_shot_kinds<R: Runtime>(
    app: tauri::AppHandle<R>,
    loading_id: u64,
) -> Result<Vec<crate::models::UnrecognisedShotKind>, CommandError> {
    pool::run(app, move |state| {
        let found = state.take_unrecognised_shot_kinds(loading_id);
        if !found.is_empty() {
            tracing::info!(
                count = found.len(),
                purpose = "prompt the user to resolve unreadable shot kinds (#27)",
                "Handing unrecognised shot kinds to the frontend"
            );
        }
        Ok(found)
    })
    .await
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
pub async fn history_discarded<R: Runtime>(app: tauri::AppHandle<R>) -> Result<bool, CommandError> {
    pool::run(app, |state| Ok(state.history_discarded())).await
}

/// [#576 C5] The frontend reports its workspace mode (`survey` / `plan` /
/// `export`) on every mode change. While it is Plan or Export the mutation
/// chokepoint refuses every survey edit with the coded `WORKSPACE_READ_ONLY`
/// refusal, so a gap in the UI's read-only gating cannot reach the project.
/// The backend resets it to Survey on every project open, create and close, as
/// the frontend does.
#[tauri::command]
pub async fn set_workspace_mode<R: Runtime>(
    app: tauri::AppHandle<R>,
    mode: crate::core::WorkspaceMode,
) -> Result<(), CommandError> {
    pool::run(app, move |state| {
        state.set_workspace_mode(mode);
        Ok(())
    })
    .await
}

/// The body of [`create_project_full`], with the write-path check passed in (the
/// testable core).
///
/// [#576 C11-D1, RV13-1] A create the webview names a file or a folder for runs
/// `check` on the file the create opens: the folder's `<Name>.stalx`, or the
/// `_N` name it moves on to when that one is taken. It used to check a stand-in
/// `<folder>/project.stalx` for a folder, and the named file before any rename,
/// so a link at the name really opened was never judged. The default location
/// (neither named) is not the webview's to choose and is not checked, as before.
///
/// `load_token` [#576 W44-NF3]: the name the webview gave this create, which its
/// Cancel (`cancel_loading`) names.
#[expect(
    clippy::result_large_err,
    reason = "the LoadError every create path returns; boxing it is a repo-wide change"
)]
pub(crate) fn create_project_full_checked(
    state: &AppState,
    check: &dyn Fn(&std::path::Path) -> Result<(), String>,
    load_token: Option<&str>,
    name: String,
    project_location: Option<String>,
    save_directory: Option<PathBuf>,
    save_path: Option<PathBuf>,
    defer_initial_save: bool,
) -> Result<CreateProjectFullResult, LoadError> {
    if save_path.is_none() && save_directory.is_none() {
        return state.do_create_project_full_named(
            load_token,
            name,
            project_location,
            None,
            None,
            defer_initial_save,
        );
    }
    state.do_create_project_full_checked(
        check,
        load_token,
        name,
        project_location,
        save_directory,
        save_path,
        defer_initial_save,
    )
}

#[tauri::command]
pub async fn create_project_full(
    app_handle: tauri::AppHandle,
    name: String,
    project_location: Option<String>,
    save_directory: Option<PathBuf>,
    save_path: Option<PathBuf>,
    defer_initial_save: Option<bool>,
    load_token: Option<String>,
) -> Result<CreateProjectFullResult, CommandError> {
    // Use AppHandle to access state inside spawn_blocking
    // (State<'_, T> cannot be moved due to lifetime constraints)
    let result: Result<Result<CreateProjectFullResult, LoadError>, _> =
        tauri::async_runtime::spawn_blocking(move || {
            create_project_full_checked(
                &app_handle.state::<AppState>(),
                &check_project_target,
                load_token.as_deref(),
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
        Ok(Err(load_error)) => Err(load_error.into()),
        Err(join_error) => {
            // Task panicked or was cancelled
            Err(LoadError::internal(join_error.to_string()).into())
        }
    }
}

/// [#576 RV15-2] Whether [`create_project_full`] would refuse this create's
/// target, asked before the webview cancels a load in progress or shows its
/// loading state: the write-path check that create runs, on the same file (the
/// name resolved, any `_N` rename made), with the same error, and nothing
/// created, cancelled or emitted. The create checks again, so this is a
/// courtesy to the user, not the guard.
#[tauri::command]
pub async fn check_create_target<R: Runtime>(
    app: tauri::AppHandle<R>,
    name: String,
    save_directory: Option<PathBuf>,
    save_path: Option<PathBuf>,
) -> Result<(), CommandError> {
    pool::run(app, move |state| {
        check_create_target_checked(
            state,
            &check_project_target,
            &name,
            save_directory,
            save_path,
        )
        .map_err(CommandError::from)
    })
    .await
}

/// The body of [`check_create_target`], with the write-path check passed in (the
/// testable core). A create that names neither a file nor a folder is not
/// checked, as [`create_project_full_checked`] does not check it.
#[expect(
    clippy::result_large_err,
    reason = "the LoadError every create path returns; boxing it is a repo-wide change"
)]
pub(crate) fn check_create_target_checked(
    state: &AppState,
    check: &dyn Fn(&std::path::Path) -> Result<(), String>,
    name: &str,
    save_directory: Option<PathBuf>,
    save_path: Option<PathBuf>,
) -> Result<(), LoadError> {
    if save_path.is_none() && save_directory.is_none() {
        return Ok(());
    }
    state.check_create_target(check, name, save_directory.as_ref(), save_path)
}
