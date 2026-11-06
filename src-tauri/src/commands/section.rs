use chrono::NaiveDate;
use tauri::{AppHandle, Runtime};

use crate::commands::pool;
use crate::core::state::ProjectIdentity;
use crate::core::{
    BatchOutcome, Command, CommandError, EditOutcome, ReverseOutcome, SectionCreated,
};
use crate::models::{
    ClosureStrategy, Coordinates, SectionId, SectionRelationships, SectionUpdates, ShotData,
    StationRef, SurveyDirection,
};

pub use crate::models::{SegmentGeometry, SegmentSectionGeometry};

/// Create a new section
#[tauri::command]
pub async fn create_section<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    direction: Option<SurveyDirection>,
    shots: Vec<ShotData>,
) -> Result<SectionCreated, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::CreateSection {
                name,
                surveyor,
                date,
                color,
                direction,
                shots,
            })
            .into_outcome::<SectionCreated>()
    })
    .await
}

/// Update section metadata
#[tauri::command]
pub async fn update_section<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    updates: SectionUpdates,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::UpdateSectionMetadata {
                section_id,
                updates,
            })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Delete a section
#[tauri::command]
pub async fn delete_section<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::DeleteSection { section_id })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Reverse a section end-to-end so the far end becomes station 0.
///
/// Geometry is preserved: every station keeps its world coordinate and only the
/// numbering direction changes. `swap_left_right` additionally mirrors the
/// left/right passage dimensions - opt-in, because whether LRUD was recorded
/// facing along travel is a survey convention the app cannot infer.
#[tauri::command]
pub async fn reverse_section<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    swap_left_right: bool,
) -> Result<ReverseOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::ReverseSection {
                section_id,
                swap_left_right,
            })
            .into_outcome::<ReverseOutcome>()
    })
    .await
}

/// Move a section by an offset
#[tauri::command]
pub async fn move_section<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    offset: Coordinates,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::MoveSection { section_id, offset })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Move an entire segment (all connected sections) by an offset
#[tauri::command]
pub async fn move_segment<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    section_id: SectionId,
    offset: Coordinates,
) -> Result<EditOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::MoveSegment { section_id, offset })
            .into_outcome::<EditOutcome>()
    })
    .await
}

/// Execute a batch of commands as a single undo/redo unit
#[tauri::command]
pub async fn execute_batch<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    commands: Vec<Command>,
) -> Result<BatchOutcome, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        state
            .execute(Command::Batch { commands })
            .into_outcome::<BatchOutcome>()
    })
    .await
}

/// Get all section IDs connected through merges and aligned closures (rigid groups)
#[tauri::command]
pub async fn get_rigid_connected_section_ids<R: Runtime>(
    app: AppHandle<R>,
    section_id: SectionId,
) -> Result<Vec<SectionId>, CommandError> {
    // Not under `settled_read`: the query takes `command_lock` itself (#576 C2).
    pool::run(app, move |state| {
        state
            .get_rigid_connected_section_ids(section_id)
            .map_err(CommandError::from)
    })
    .await
}

/// Get all section IDs belonging to segments that contain a zero datum
#[tauri::command]
pub async fn get_anchored_section_ids<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<SectionId>, CommandError> {
    // Not under `settled_read`: the query takes `command_lock` itself (#576 C2).
    pool::run(app, |state| {
        state.get_anchored_section_ids().map_err(CommandError::from)
    })
    .await
}

/// Create a new section and immediately merge one of its stations with an existing station.
/// This is executed as a single undo/redo operation using the internal batch mechanism.
///
/// The merge station's section_id is specified as 0 (placeholder) and replaced with the
/// actual created section ID during execution.
#[tauri::command]
pub async fn create_section_and_merge<R: Runtime>(
    app: AppHandle<R>,
    expected_project: Option<ProjectIdentity>,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    direction: Option<SurveyDirection>,
    shots: Vec<ShotData>,
    import_station_index: u32,
    target_station: StationRef,
    closure_strategy: ClosureStrategy,
) -> Result<SectionCreated, CommandError> {
    pool::run_mutation(app, expected_project, move |state| {
        // Use a special internal execution that:
        // 1. Takes a project snapshot for undo
        // 2. Creates the section
        // 3. Merges with the actual section ID
        // 4. Returns as a single undo unit
        state
            .execute_create_and_merge(
                name,
                surveyor,
                date,
                color,
                direction,
                shots,
                import_station_index,
                target_station,
                closure_strategy,
            )
            .into_outcome::<SectionCreated>()
    })
    .await
}

/// Get relationships that reference a section (merges, closures, offsets, zero datums).
/// Used by the frontend to show affected relationships before section deletion.
#[tauri::command]
pub async fn get_section_relationships<R: Runtime>(
    app: AppHandle<R>,
    section_id: SectionId,
) -> Result<SectionRelationships, CommandError> {
    pool::run(app, move |state| {
        let project = state
            .settled_snapshot()
            .ok_or_else(|| "No project open".to_string())?;

        if project.get_section(section_id).is_none() {
            return Err(CommandError::failed(format!(
                "Section {section_id} not found"
            )));
        }

        Ok(project.get_section_relationships(section_id))
    })
    .await
}

/// Get shot-line geometry for all sections in the segment containing the given section.
/// Coordinates are relative to the given section's station 0.
#[tauri::command]
pub async fn get_segment_geometry<R: Runtime>(
    app: AppHandle<R>,
    section_id: SectionId,
) -> Result<SegmentGeometry, CommandError> {
    // Not under `settled_read`: the query takes `command_lock` itself (#576 C2).
    pool::run(app, move |state| {
        state
            .get_segment_geometry(section_id)
            .map_err(CommandError::from)
    })
    .await
}
