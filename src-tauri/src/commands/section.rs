use chrono::NaiveDate;
use tauri::State;

use crate::core::{AppState, Command, CommandResult};
use crate::models::{
    ClosureStrategy, CombineCandidate, CombineSectionsOptions, Coordinates, SectionId,
    SectionRelationships, SectionUpdates, ShotData, StationRef, SurveyDirection,
};

pub use crate::models::{SegmentGeometry, SegmentSectionGeometry};

/// Create a new section
#[tauri::command]
pub fn create_section(
    state: State<AppState>,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    direction: Option<SurveyDirection>,
    shots: Vec<ShotData>,
) -> CommandResult {
    create_section_impl(state.inner(), name, surveyor, date, color, direction, shots)
}

pub(crate) fn create_section_impl(
    state: &AppState,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    direction: Option<SurveyDirection>,
    shots: Vec<ShotData>,
) -> CommandResult {
    state.execute(Command::CreateSection {
        name,
        surveyor,
        date,
        color,
        direction,
        shots,
    })
}

/// Update section metadata
#[tauri::command]
pub fn update_section(
    state: State<AppState>,
    section_id: SectionId,
    updates: SectionUpdates,
) -> CommandResult {
    update_section_impl(state.inner(), section_id, updates)
}

pub(crate) fn update_section_impl(
    state: &AppState,
    section_id: SectionId,
    updates: SectionUpdates,
) -> CommandResult {
    state.execute(Command::UpdateSectionMetadata {
        section_id,
        updates,
    })
}

/// Delete a section
#[tauri::command]
pub fn delete_section(state: State<AppState>, section_id: SectionId) -> CommandResult {
    delete_section_impl(state.inner(), section_id)
}

pub(crate) fn delete_section_impl(state: &AppState, section_id: SectionId) -> CommandResult {
    state.execute(Command::DeleteSection { section_id })
}

/// Reverse a section end-to-end so the far end becomes station 0.
///
/// Geometry is preserved: every station keeps its world coordinate and only the
/// numbering direction changes. `swap_left_right` additionally mirrors the
/// left/right passage dimensions - opt-in, because whether LRUD was recorded
/// facing along travel is a survey convention the app cannot infer.
#[tauri::command]
pub fn reverse_section(
    state: State<AppState>,
    section_id: SectionId,
    swap_left_right: bool,
) -> CommandResult {
    reverse_section_impl(state.inner(), section_id, swap_left_right)
}

pub(crate) fn reverse_section_impl(
    state: &AppState,
    section_id: SectionId,
    swap_left_right: bool,
) -> CommandResult {
    state.execute(Command::ReverseSection {
        section_id,
        swap_left_right,
    })
}

/// Move a section by an offset
#[tauri::command]
pub fn move_section(
    state: State<AppState>,
    section_id: SectionId,
    offset: Coordinates,
) -> CommandResult {
    move_section_impl(state.inner(), section_id, offset)
}

pub(crate) fn move_section_impl(
    state: &AppState,
    section_id: SectionId,
    offset: Coordinates,
) -> CommandResult {
    state.execute(Command::MoveSection { section_id, offset })
}

/// Move an entire segment (all connected sections) by an offset
#[tauri::command]
pub fn move_segment(
    state: State<AppState>,
    section_id: SectionId,
    offset: Coordinates,
) -> CommandResult {
    state.execute(Command::MoveSegment { section_id, offset })
}

/// Execute a batch of commands as a single undo/redo unit
#[tauri::command]
pub fn execute_batch(state: State<AppState>, commands: Vec<Command>) -> CommandResult {
    state.execute(Command::Batch { commands })
}

/// Get all section IDs connected through merges and aligned closures (rigid groups)
#[tauri::command]
pub fn get_rigid_connected_section_ids(
    state: State<AppState>,
    section_id: SectionId,
) -> Result<Vec<SectionId>, String> {
    state.get_rigid_connected_section_ids(section_id)
}

/// Get all section IDs belonging to segments that contain a zero datum
#[tauri::command]
pub fn get_anchored_section_ids(state: State<AppState>) -> Result<Vec<SectionId>, String> {
    state.get_anchored_section_ids()
}

/// Create a new section and immediately merge one of its stations with an existing station.
/// This is executed as a single undo/redo operation using the internal batch mechanism.
///
/// The merge station's section_id is specified as 0 (placeholder) and replaced with the
/// actual created section ID during execution.
#[tauri::command]
pub fn create_section_and_merge(
    state: State<AppState>,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    direction: Option<SurveyDirection>,
    shots: Vec<ShotData>,
    import_station_index: u32,
    target_station: StationRef,
    closure_strategy: ClosureStrategy,
) -> CommandResult {
    create_section_and_merge_impl(
        state.inner(),
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
}

pub(crate) fn create_section_and_merge_impl(
    state: &AppState,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    direction: Option<SurveyDirection>,
    shots: Vec<ShotData>,
    import_station_index: u32,
    target_station: StationRef,
    closure_strategy: ClosureStrategy,
) -> CommandResult {
    // Use a special internal execution that:
    // 1. Takes a project snapshot for undo
    // 2. Creates the section
    // 3. Merges with the actual section ID
    // 4. Returns as a single undo unit
    state.execute_create_and_merge(
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
}

/// Get relationships that reference a section (merges, closures, offsets, zero datums).
/// Used by the frontend to show affected relationships before section deletion.
#[tauri::command]
pub fn get_section_relationships(
    state: State<AppState>,
    section_id: SectionId,
) -> Result<SectionRelationships, String> {
    get_section_relationships_impl(state.inner(), section_id)
}

pub(crate) fn get_section_relationships_impl(
    state: &AppState,
    section_id: SectionId,
) -> Result<SectionRelationships, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;

    if project.get_section(section_id).is_none() {
        return Err(format!("Section {section_id} not found"));
    }

    Ok(project.get_section_relationships(section_id))
}

/// Can the section ending at `station` combine with another section ending at
/// the same point? `null` when `station` is not a section end or nothing else
/// touches it; otherwise the candidate, with the reason when it is blocked.
#[tauri::command]
pub fn combine_candidate(
    state: State<AppState>,
    station: StationRef,
) -> Result<Option<CombineCandidate>, String> {
    combine_candidate_impl(state.inner(), station)
}

pub(crate) fn combine_candidate_impl(
    state: &AppState,
    station: StationRef,
) -> Result<Option<CombineCandidate>, String> {
    state.combine_candidate(station)
}

/// Can sections `a` and `b` be combined? `null` when either is missing or they
/// are the same section.
#[tauri::command]
pub fn combine_candidate_for_sections(
    state: State<AppState>,
    a: SectionId,
    b: SectionId,
) -> Result<Option<CombineCandidate>, String> {
    combine_candidate_for_sections_impl(state.inner(), a, b)
}

pub(crate) fn combine_candidate_for_sections_impl(
    state: &AppState,
    a: SectionId,
    b: SectionId,
) -> Result<Option<CombineCandidate>, String> {
    state.combine_candidate_for_sections(a, b)
}

/// Combine `other` into `kept`: one section, one undo step.
#[tauri::command]
pub fn combine_sections(
    state: State<AppState>,
    kept: SectionId,
    other: SectionId,
    options: CombineSectionsOptions,
) -> CommandResult {
    combine_sections_impl(state.inner(), kept, other, options)
}

pub(crate) fn combine_sections_impl(
    state: &AppState,
    kept: SectionId,
    other: SectionId,
    options: CombineSectionsOptions,
) -> CommandResult {
    state.execute_combine_sections(kept, other, options)
}

/// Get shot-line geometry for all sections in the segment containing the given section.
/// Coordinates are relative to the given section's station 0.
#[tauri::command]
pub fn get_segment_geometry(
    state: State<AppState>,
    section_id: SectionId,
) -> Result<SegmentGeometry, String> {
    get_segment_geometry_impl(state.inner(), section_id)
}

pub(crate) fn get_segment_geometry_impl(
    state: &AppState,
    section_id: SectionId,
) -> Result<SegmentGeometry, String> {
    state.get_segment_geometry(section_id)
}
