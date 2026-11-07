//! Dive-plan (`.plnx`) IPC commands (#244). Thin wrappers that run the SQLite I/O
//! off the UI thread via `spawn_blocking` and delegate to the `AppState` methods
//! (mirrors `commands::gas`). The snapshot is validated by serde on the way in; the
//! engine re-validates the diver/cylinders when the loaded plan is recomputed.

use tauri::Manager;

use crate::core::state::{for_project, ProjectIdentity};
use crate::core::{AppState, CommandError};
use crate::models::{DivePlan, DivePlanMeta, DivePlanSnapshot, StationRef, StoredAnchor};

/// List the dive plans stored next to the open project (newest first). Empty when
/// there is no saved project or no `.plnx` yet.
#[tauri::command]
pub async fn list_dive_plans(
    app_handle: tauri::AppHandle,
) -> Result<Vec<DivePlanMeta>, CommandError> {
    match tauri::async_runtime::spawn_blocking(move || {
        app_handle.state::<AppState>().inner().list_dive_plans()
    })
    .await
    {
        Ok(res) => res.map_err(CommandError::from),
        Err(e) => Err(CommandError::internal(format!(
            "list dive plans failed: {e}"
        ))),
    }
}

/// Load one plan, degrading stale station refs against the live survey.
#[tauri::command]
pub async fn load_dive_plan(
    app_handle: tauri::AppHandle,
    id: i64,
) -> Result<DivePlan, CommandError> {
    match tauri::async_runtime::spawn_blocking(move || {
        app_handle.state::<AppState>().inner().load_dive_plan(id)
    })
    .await
    {
        Ok(res) => res.map_err(CommandError::from),
        Err(e) => Err(CommandError::internal(format!(
            "load dive plan failed: {e}"
        ))),
    }
}

/// Save a plan next to the open project. `id` is `None` for a new plan (Save as),
/// `Some` to overwrite an existing one (Save).
///
/// [#576 RVF-c-C1] `expected_project` is the project the plan was made for: the
/// `.plnx` is that project's sibling, so a save that reaches the backend after a
/// switch is refused with `PROJECT_CHANGED` rather than written into the project
/// opened since.
#[tauri::command]
pub async fn save_dive_plan(
    app_handle: tauri::AppHandle,
    expected_project: Option<ProjectIdentity>,
    id: Option<i64>,
    name: String,
    snapshot: DivePlanSnapshot,
) -> Result<DivePlanMeta, CommandError> {
    match tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        for_project(expected_project, || {
            state.inner().save_dive_plan_for(id, name, snapshot)
        })
    })
    .await
    {
        Ok(res) => res,
        Err(e) => Err(CommandError::internal(format!(
            "save dive plan failed: {e}"
        ))),
    }
}

/// Describe live plan pins as stable anchors (#310), index-aligned with `refs`; `None` for a ref
/// that names no live station.
///
/// Read-only, and the reason it exists: a `StationRef` is positional, so a plan being edited in
/// Plan mode silently rebinds its pins when a survey edit renumbers stations. The `.plnx` never
/// had that problem because it stores anchors - this hands the same anchors to the frontend for
/// the plan it is holding in memory. An anchor is OPAQUE on the wire (`unknown` in TS): store it
/// and give it back to [`plan_refs_for_anchors`], nothing else.
///
/// The batch is length-capped in `AppState` (`MAX_PLAN_ANCHOR_BATCH`), like every other list a
/// plan can hold: each entry costs a section scan under the project mutex.
#[tauri::command]
pub async fn plan_anchors_for_refs(
    app_handle: tauri::AppHandle,
    refs: Vec<StationRef>,
) -> Result<Vec<Option<StoredAnchor>>, CommandError> {
    match tauri::async_runtime::spawn_blocking(move || {
        app_handle
            .state::<AppState>()
            .inner()
            .plan_anchors_for_refs(refs)
    })
    .await
    {
        Ok(res) => res.map_err(CommandError::from),
        Err(e) => Err(CommandError::internal(format!(
            "plan anchors for refs failed: {e}"
        ))),
    }
}

/// Place anchors against the survey as it stands now (#310), index-aligned with `anchors`;
/// `None` for one that no longer places. The read half of [`plan_anchors_for_refs`], and capped
/// the same way.
#[tauri::command]
pub async fn plan_refs_for_anchors(
    app_handle: tauri::AppHandle,
    anchors: Vec<StoredAnchor>,
) -> Result<Vec<Option<StationRef>>, CommandError> {
    match tauri::async_runtime::spawn_blocking(move || {
        app_handle
            .state::<AppState>()
            .inner()
            .plan_refs_for_anchors(anchors)
    })
    .await
    {
        Ok(res) => res.map_err(CommandError::from),
        Err(e) => Err(CommandError::internal(format!(
            "plan refs for anchors failed: {e}"
        ))),
    }
}

/// Delete a plan next to the open project. [#576 RVF-c-C1] Refused with
/// `PROJECT_CHANGED` when made for another project, as [`save_dive_plan`].
#[tauri::command]
pub async fn delete_dive_plan(
    app_handle: tauri::AppHandle,
    expected_project: Option<ProjectIdentity>,
    id: i64,
) -> Result<(), CommandError> {
    match tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        for_project(expected_project, || state.inner().delete_dive_plan_for(id))
    })
    .await
    {
        Ok(res) => res,
        Err(e) => Err(CommandError::internal(format!(
            "delete dive plan failed: {e}"
        ))),
    }
}
