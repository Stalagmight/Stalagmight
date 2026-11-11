use tauri::State;

use crate::core::{AppState, Command, CommandResult};
use crate::models::{
    DeleteShotImpact, DeleteShotRangeImpact, SectionId, ShotData, ShotId, ShotKind, ShotUpdates,
};

/// Create a new shot
#[tauri::command]
pub fn create_shot(
    state: State<AppState>,
    section_id: SectionId,
    shot: ShotData,
    after_shot_id: Option<ShotId>,
    before_shot_id: Option<ShotId>,
) -> CommandResult {
    create_shot_impl(
        state.inner(),
        section_id,
        shot,
        after_shot_id,
        before_shot_id,
    )
}

pub(crate) fn create_shot_impl(
    state: &AppState,
    section_id: SectionId,
    mut shot: ShotData,
    after_shot_id: Option<ShotId>,
    before_shot_id: Option<ShotId>,
) -> CommandResult {
    // Identity is not the caller's to name (#423). `ShotData::uuid` exists so a
    // uuid can travel with its own shot through the transforms that rebuild
    // shots from their data; it is NOT an input to "create a shot here".
    //
    // Left through, a caller could name an EXISTING shot's uuid and insert the
    // new shot BEFORE it. The first-wins repairs (`Section::new` and the
    // pre-save `ensure_unique_shot_uuids`) keep the first OCCURRENCE, which is
    // now the injected record, and re-mint the original - moving every
    // `AfterShot` anchor on it (merges, closures, zero datums, resolved-warning
    // ignores) onto the injected shot.
    //
    // `execute`'s structural epilogue does catch it on a project that was sound
    // beforehand: `duplicate_shot_uuid` is Critical, so the whole create is
    // rolled back (verified - without this line the create returns "Blocked:
    // this action would corrupt the survey structure"). That is not cover
    // enough. It fires only on NEWLY introduced criticals, so on a project that
    // is already critical (`finalize_structural_edit(pre_had_critical)`) the
    // duplicate is admitted and nothing objects until the pre-save repair
    // quietly re-mints the original. Clearing here is also the better outcome
    // for the sound case: a legitimate "add a shot" keeps working instead of
    // being refused because of a field the client had no business setting.
    //
    // The AUTHORITATIVE clear is in the executor (`AppState::create_shot`,
    // core/state/command_shot.rs) - a wrapper-only clear held only by grace of
    // the batch allowlist refusing `CreateShot` (pre-PR review, #423); the
    // executor holds it for every constructor of the command. This one stays
    // as documentation-plus-belt. `Command::CreateSection` IS batch-allowed
    // and is stripped at its two dispatch arms instead (core/state/commands.rs,
    // undo_restore.rs) - its executor is shared with the importers, whose
    // identities our own parser legitimately adopted. That leaves
    // `import_sections` and `create_section_and_merge` as the doors that may
    // carry identities; both rename any the project already holds
    // (`Project::reassign_repeated_shot_uuids`).
    shot.uuid = None;
    state.execute(Command::CreateShot {
        section_id,
        shot,
        after_shot_id,
        before_shot_id,
    })
}

/// Update a shot
#[tauri::command]
pub fn update_shot(
    state: State<AppState>,
    section_id: SectionId,
    shot_id: ShotId,
    updates: ShotUpdates,
) -> CommandResult {
    update_shot_impl(state.inner(), section_id, shot_id, updates)
}

pub(crate) fn update_shot_impl(
    state: &AppState,
    section_id: SectionId,
    shot_id: ShotId,
    updates: ShotUpdates,
) -> CommandResult {
    state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates,
    })
}

/// Preview impacts of deleting a shot (read-only)
#[tauri::command]
pub fn preview_delete_shot(
    state: State<AppState>,
    section_id: SectionId,
    shot_id: ShotId,
) -> Result<DeleteShotImpact, String> {
    preview_delete_shot_impl(state.inner(), section_id, shot_id)
}

pub(crate) fn preview_delete_shot_impl(
    state: &AppState,
    section_id: SectionId,
    shot_id: ShotId,
) -> Result<DeleteShotImpact, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;
    project.preview_delete_shot(section_id, shot_id)
}

/// Delete a shot
#[tauri::command]
pub fn delete_shot(
    state: State<AppState>,
    section_id: SectionId,
    shot_id: ShotId,
) -> CommandResult {
    delete_shot_impl(state.inner(), section_id, shot_id)
}

pub(crate) fn delete_shot_impl(
    state: &AppState,
    section_id: SectionId,
    shot_id: ShotId,
) -> CommandResult {
    state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    })
}

/// Delete a mid-section shot and split the section into two named sections
#[tauri::command]
pub fn delete_shot_and_split(
    state: State<AppState>,
    section_id: SectionId,
    shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> CommandResult {
    delete_shot_and_split_impl(state.inner(), section_id, shot_id, name_a, name_b)
}

pub(crate) fn delete_shot_and_split_impl(
    state: &AppState,
    section_id: SectionId,
    shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> CommandResult {
    state.execute_delete_shot_and_split(section_id, shot_id, name_a, name_b)
}

/// Insert a station mid-shot by splitting a shot at a given distance
#[tauri::command]
pub fn split_shot_at_distance(
    state: State<AppState>,
    section_id: SectionId,
    shot_id: ShotId,
    distance: f64,
) -> CommandResult {
    split_shot_at_distance_impl(state.inner(), section_id, shot_id, distance)
}

pub(crate) fn split_shot_at_distance_impl(
    state: &AppState,
    section_id: SectionId,
    shot_id: ShotId,
    distance: f64,
) -> CommandResult {
    state.execute_split_shot_at_distance(section_id, shot_id, distance)
}

/// Delete the last remaining shot in a section and remove the section
#[tauri::command]
pub fn delete_last_shot_and_section(
    state: State<AppState>,
    section_id: SectionId,
    shot_id: ShotId,
) -> CommandResult {
    delete_last_shot_and_section_impl(state.inner(), section_id, shot_id)
}

pub(crate) fn delete_last_shot_and_section_impl(
    state: &AppState,
    section_id: SectionId,
    shot_id: ShotId,
) -> CommandResult {
    state.execute_delete_last_shot_and_section(section_id, shot_id)
}

/// Preview impacts of deleting a contiguous range of shots (read-only)
#[tauri::command]
pub fn preview_delete_shot_range(
    state: State<AppState>,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> Result<DeleteShotRangeImpact, String> {
    preview_delete_shot_range_impl(state.inner(), section_id, start_shot_id, end_shot_id)
}

pub(crate) fn preview_delete_shot_range_impl(
    state: &AppState,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> Result<DeleteShotRangeImpact, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;
    project.preview_delete_shot_range(section_id, start_shot_id, end_shot_id)
}

/// Delete a contiguous interior range of shots and split the section into two
#[tauri::command]
pub fn delete_shot_range_split(
    state: State<AppState>,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> CommandResult {
    delete_shot_range_split_impl(
        state.inner(),
        section_id,
        start_shot_id,
        end_shot_id,
        name_a,
        name_b,
    )
}

pub(crate) fn delete_shot_range_split_impl(
    state: &AppState,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> CommandResult {
    state.execute_delete_shot_range_split(section_id, start_shot_id, end_shot_id, name_a, name_b)
}

/// Delete a contiguous range of shots by gap-closing (shift geometry)
#[tauri::command]
pub fn delete_shot_range_shift(
    state: State<AppState>,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> CommandResult {
    delete_shot_range_shift_impl(state.inner(), section_id, start_shot_id, end_shot_id)
}

pub(crate) fn delete_shot_range_shift_impl(
    state: &AppState,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> CommandResult {
    state.execute_delete_shot_range_shift(section_id, start_shot_id, end_shot_id)
}

/// Set a shot's kind: permanent, jump, or reference (#27).
#[tauri::command]
pub fn set_shot_kind(
    state: State<AppState>,
    section_id: SectionId,
    shot_id: ShotId,
    kind: ShotKind,
) -> CommandResult {
    set_shot_kind_impl(state.inner(), section_id, shot_id, kind)
}

pub(crate) fn set_shot_kind_impl(
    state: &AppState,
    section_id: SectionId,
    shot_id: ShotId,
    kind: ShotKind,
) -> CommandResult {
    state.execute(Command::SetShotKind {
        section_id,
        shot_id,
        kind,
    })
}
