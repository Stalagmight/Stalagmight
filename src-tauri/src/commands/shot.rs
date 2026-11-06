use tauri::{AppHandle, Runtime};

use crate::commands::pool;
use crate::core::state::ProjectIdentity;
use crate::core::{Command, CommandError, EditOutcome, SectionCreated, ShotCreated};
use crate::models::{
    DeleteShotImpact, DeleteShotRangeImpact, SectionId, ShotData, ShotId, ShotKind, ShotUpdates,
};

/// Create a new shot
#[tauri::command]
pub async fn create_shot<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    shot: ShotData,
    after_shot_id: Option<ShotId>,
    before_shot_id: Option<ShotId>,
) -> Result<ShotCreated, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        let mut shot = shot;
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
        // `import_sections` as the one door that may carry identities.
        shot.uuid = None;
        state
            .execute(Command::CreateShot {
                section_id,
                shot,
                after_shot_id,
                before_shot_id,
            })
            .into_outcome::<ShotCreated>()
    })
    .await
}

/// Update a shot
#[tauri::command]
pub async fn update_shot<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    shot_id: ShotId,
    updates: ShotUpdates,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::UpdateShot {
                section_id,
                shot_id,
                updates,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Preview impacts of deleting a shot (read-only)
#[tauri::command]
pub async fn preview_delete_shot<R: Runtime>(
    app: AppHandle<R>,
    section_id: SectionId,
    shot_id: ShotId,
) -> Result<DeleteShotImpact, CommandError> {
    pool::run(app, move |state| {
        let project = state
            .settled_snapshot()
            .ok_or_else(|| "No project open".to_string())?;
        project
            .preview_delete_shot(section_id, shot_id)
            .map_err(CommandError::from)
    })
    .await
}

/// Delete a shot
#[tauri::command]
pub async fn delete_shot<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    shot_id: ShotId,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::DeleteShot {
                section_id,
                shot_id,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Delete a mid-section shot and split the section into two named sections
#[tauri::command]
pub async fn delete_shot_and_split<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> Result<SectionCreated, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute_delete_shot_and_split(section_id, shot_id, name_a, name_b)
            .into_outcome::<SectionCreated>()
    })
    .await
}

/// Insert a station mid-shot by splitting a shot at a given distance
#[tauri::command]
pub async fn split_shot_at_distance<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    shot_id: ShotId,
    distance: f64,
) -> Result<ShotCreated, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute_split_shot_at_distance(section_id, shot_id, distance)
            .into_outcome::<ShotCreated>()
    })
    .await
}

/// Delete the last remaining shot in a section and remove the section
#[tauri::command]
pub async fn delete_last_shot_and_section<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    shot_id: ShotId,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute_delete_last_shot_and_section(section_id, shot_id)
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Preview impacts of deleting a contiguous range of shots (read-only)
#[tauri::command]
pub async fn preview_delete_shot_range<R: Runtime>(
    app: AppHandle<R>,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> Result<DeleteShotRangeImpact, CommandError> {
    pool::run(app, move |state| {
        let project = state
            .settled_snapshot()
            .ok_or_else(|| "No project open".to_string())?;
        project
            .preview_delete_shot_range(section_id, start_shot_id, end_shot_id)
            .map_err(CommandError::from)
    })
    .await
}

/// Delete a contiguous interior range of shots and split the section into two
#[tauri::command]
pub async fn delete_shot_range_split<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
    name_a: String,
    name_b: String,
) -> Result<SectionCreated, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute_delete_shot_range_split(section_id, start_shot_id, end_shot_id, name_a, name_b)
            .into_outcome::<SectionCreated>()
    })
    .await
}

/// Delete a contiguous range of shots by gap-closing (shift geometry)
#[tauri::command]
pub async fn delete_shot_range_shift<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    start_shot_id: ShotId,
    end_shot_id: ShotId,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute_delete_shot_range_shift(section_id, start_shot_id, end_shot_id)
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Set a shot's kind: permanent, jump, or reference (#27).
#[tauri::command]
pub async fn set_shot_kind<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    shot_id: ShotId,
    kind: ShotKind,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::SetShotKind {
                section_id,
                shot_id,
                kind,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}
