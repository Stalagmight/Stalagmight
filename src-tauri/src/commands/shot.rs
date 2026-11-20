use tauri::State;

use crate::core::{AppState, Command, CommandResult};
use crate::models::{
    CreditEdit, CreditTargets, DeleteShotImpact, DeleteShotRangeImpact, LineId, ShotData, ShotId,
    ShotKind, ShotUpdates, SplitAt, SplitLineOptions, SplitPreview,
};

/// Create a new shot
#[tauri::command]
pub fn create_shot(
    state: State<AppState>,
    line_id: LineId,
    shot: ShotData,
    after_shot_id: Option<ShotId>,
    before_shot_id: Option<ShotId>,
) -> CommandResult {
    create_shot_impl(state.inner(), line_id, shot, after_shot_id, before_shot_id)
}

pub(crate) fn create_shot_impl(
    state: &AppState,
    line_id: LineId,
    mut shot: ShotData,
    after_shot_id: Option<ShotId>,
    before_shot_id: Option<ShotId>,
) -> CommandResult {
    // Identity is not the caller's to name (#423). `ShotData::uuid` exists so
    // a uuid can travel with its own shot through the transforms that rebuild
    // shots from their data; it is NOT an input to "create a shot here". A
    // uuid another shot holds would be refused by the survey model, but even a
    // fresh one is the project's to mint, not the client's.
    //
    // The AUTHORITATIVE clear is in the executor (`command_shot::apply`,
    // core/state/command_shot.rs), for every constructor of the command; this
    // one stays as documentation-plus-belt. Undo and redo never replay the
    // command (they step its recorded changeset), so no identity is lost to
    // either clear. The import doors (`import_lines`, `create_line_and_merge`)
    // are the ones that may carry identities, and re-mint any the project
    // already holds.
    shot.uuid = None;
    state.execute(Command::CreateShot {
        line_id,
        shot,
        after_shot_id,
        before_shot_id,
    })
}

/// Update a shot
#[tauri::command]
pub fn update_shot(
    state: State<AppState>,
    line_id: LineId,
    shot_id: ShotId,
    updates: ShotUpdates,
) -> CommandResult {
    update_shot_impl(state.inner(), line_id, shot_id, updates)
}

pub(crate) fn update_shot_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
    updates: ShotUpdates,
) -> CommandResult {
    state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates,
    })
}

/// Preview impacts of deleting a shot (read-only)
#[tauri::command]
pub fn preview_delete_shot(
    state: State<AppState>,
    line_id: LineId,
    shot_id: ShotId,
) -> Result<DeleteShotImpact, String> {
    preview_delete_shot_impl(state.inner(), line_id, shot_id)
}

pub(crate) fn preview_delete_shot_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
) -> Result<DeleteShotImpact, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;
    let preview = project
        .preview_delete_shot(line_id, shot_id)
        .map_err(|e| e.to_string())?;
    Ok(project.delete_shot_impact(&preview))
}

/// Delete a shot
#[tauri::command]
pub fn delete_shot(state: State<AppState>, line_id: LineId, shot_id: ShotId) -> CommandResult {
    delete_shot_impl(state.inner(), line_id, shot_id)
}

pub(crate) fn delete_shot_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
) -> CommandResult {
    state.execute(Command::DeleteShot { line_id, shot_id })
}

/// Delete a mid-line shot and split the line into two named lines
#[tauri::command]
pub fn delete_shot_and_split(
    state: State<AppState>,
    line_id: LineId,
    shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> CommandResult {
    delete_shot_and_split_impl(state.inner(), line_id, shot_id, name_a, name_b)
}

pub(crate) fn delete_shot_and_split_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> CommandResult {
    state.execute_delete_shot_and_split(line_id, shot_id, name_a, name_b)
}

/// Split a line in two at a mid-line station or a point along a shot,
/// deleting nothing (#335).
#[tauri::command]
pub fn split_line(
    state: State<AppState>,
    line_id: LineId,
    at: SplitAt,
    options: SplitLineOptions,
) -> CommandResult {
    split_line_impl(state.inner(), line_id, at, options)
}

pub(crate) fn split_line_impl(
    state: &AppState,
    line_id: LineId,
    at: SplitAt,
    options: SplitLineOptions,
) -> CommandResult {
    state.execute_split_line(line_id, at, options)
}

/// What Split Line Here would do beyond the cut, without doing it: how many
/// loop closures it would turn into merges, and how far that moves a half. The
/// dialog shows it before the user confirms; a refused split returns its
/// reason.
#[tauri::command]
pub fn split_line_preview(
    state: State<AppState>,
    line_id: LineId,
    at: SplitAt,
    options: SplitLineOptions,
) -> Result<SplitPreview, String> {
    split_line_preview_impl(state.inner(), line_id, at, &options)
}

pub(crate) fn split_line_preview_impl(
    state: &AppState,
    line_id: LineId,
    at: SplitAt,
    options: &SplitLineOptions,
) -> Result<SplitPreview, String> {
    state.split_line_preview(line_id, at, options)
}

/// Insert a station mid-shot by splitting a shot at a given distance
#[tauri::command]
pub fn split_shot_at_distance(
    state: State<AppState>,
    line_id: LineId,
    shot_id: ShotId,
    distance: f64,
) -> CommandResult {
    split_shot_at_distance_impl(state.inner(), line_id, shot_id, distance)
}

pub(crate) fn split_shot_at_distance_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
    distance: f64,
) -> CommandResult {
    state.execute_split_shot_at_distance(line_id, shot_id, distance)
}

/// Delete the last remaining shot in a line and remove the line
#[tauri::command]
pub fn delete_last_shot_and_line(
    state: State<AppState>,
    line_id: LineId,
    shot_id: ShotId,
) -> CommandResult {
    delete_last_shot_and_line_impl(state.inner(), line_id, shot_id)
}

pub(crate) fn delete_last_shot_and_line_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
) -> CommandResult {
    state.execute_delete_last_shot_and_line(line_id, shot_id)
}

/// Preview impacts of deleting a contiguous range of shots (read-only)
#[tauri::command]
pub fn preview_delete_shot_range(
    state: State<AppState>,
    line_id: LineId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> Result<DeleteShotRangeImpact, String> {
    preview_delete_shot_range_impl(state.inner(), line_id, start_shot_id, end_shot_id)
}

pub(crate) fn preview_delete_shot_range_impl(
    state: &AppState,
    line_id: LineId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> Result<DeleteShotRangeImpact, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;
    let preview = project
        .preview_delete_shot_range(line_id, start_shot_id, end_shot_id)
        .map_err(|e| e.to_string())?;
    Ok(project.delete_range_impact(&preview))
}

/// Delete a contiguous interior range of shots and split the line into two
#[tauri::command]
pub fn delete_shot_range_split(
    state: State<AppState>,
    line_id: LineId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> CommandResult {
    delete_shot_range_split_impl(
        state.inner(),
        line_id,
        start_shot_id,
        end_shot_id,
        name_a,
        name_b,
    )
}

pub(crate) fn delete_shot_range_split_impl(
    state: &AppState,
    line_id: LineId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> CommandResult {
    state.execute_delete_shot_range_split(line_id, start_shot_id, end_shot_id, name_a, name_b)
}

/// Delete a contiguous range of shots by gap-closing (shift geometry)
#[tauri::command]
pub fn delete_shot_range_shift(
    state: State<AppState>,
    line_id: LineId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> CommandResult {
    delete_shot_range_shift_impl(state.inner(), line_id, start_shot_id, end_shot_id)
}

pub(crate) fn delete_shot_range_shift_impl(
    state: &AppState,
    line_id: LineId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> CommandResult {
    state.execute_delete_shot_range_shift(line_id, start_shot_id, end_shot_id)
}

/// Set a shot's kind: permanent, jump, or reference (#27).
#[tauri::command]
pub fn set_shot_kind(
    state: State<AppState>,
    line_id: LineId,
    shot_id: ShotId,
    kind: ShotKind,
) -> CommandResult {
    set_shot_kind_impl(state.inner(), line_id, shot_id, kind)
}

pub(crate) fn set_shot_kind_impl(
    state: &AppState,
    line_id: LineId,
    shot_id: ShotId,
    kind: ShotKind,
) -> CommandResult {
    state.execute(Command::SetShotKind {
        line_id,
        shot_id,
        kind,
    })
}

/// Edit the survey credit (surveyors, explorers, date) of a whole line or of
/// explicit shots, as ONE undo step however many shots it touches.
#[tauri::command]
pub fn edit_shot_credit(
    state: State<AppState>,
    targets: CreditTargets,
    edit: CreditEdit,
) -> CommandResult {
    edit_shot_credit_impl(state.inner(), targets, edit)
}

pub(crate) fn edit_shot_credit_impl(
    state: &AppState,
    targets: CreditTargets,
    edit: CreditEdit,
) -> CommandResult {
    state.execute(Command::EditShotCredit { targets, edit })
}
