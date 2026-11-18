use chrono::NaiveDate;
use tauri::State;

use crate::core::{AppState, Command, CommandResult};
use crate::models::{
    ClosureStrategy, CombineCandidate, CombineLinesOptions, Coordinates, LineId, LineRelationships,
    LineUpdates, SectionId, ShotData, StationRef, SurveyDirection,
};

pub use crate::models::{SegmentGeometry, SegmentLineGeometry};

/// Create a new line. `cave_section_id` (optional, `caveSectionId` from the
/// frontend) puts it straight into that Section (#327), in the same undo step.
#[tauri::command]
pub fn create_line(
    state: State<AppState>,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    direction: Option<SurveyDirection>,
    shots: Vec<ShotData>,
    cave_section_id: Option<SectionId>,
) -> CommandResult {
    create_line_impl(
        state.inner(),
        name,
        surveyor,
        date,
        color,
        direction,
        shots,
        cave_section_id,
    )
}

pub(crate) fn create_line_impl(
    state: &AppState,
    name: Option<String>,
    surveyor: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    direction: Option<SurveyDirection>,
    shots: Vec<ShotData>,
    cave_section_id: Option<SectionId>,
) -> CommandResult {
    // Line names are required and unique: refused here, on the UI path.
    if let Some(refusal) = state.refuse_new_line_name(name.as_deref()) {
        return refusal;
    }
    state.execute(Command::CreateLine {
        name,
        surveyor,
        date,
        color,
        direction,
        shots,
        cave_section_id,
    })
}

/// Update line metadata
#[tauri::command]
pub fn update_line(state: State<AppState>, line_id: LineId, updates: LineUpdates) -> CommandResult {
    update_line_impl(state.inner(), line_id, updates)
}

pub(crate) fn update_line_impl(
    state: &AppState,
    line_id: LineId,
    updates: LineUpdates,
) -> CommandResult {
    state.execute(Command::UpdateLineMetadata { line_id, updates })
}

/// Delete a line
#[tauri::command]
pub fn delete_line(state: State<AppState>, line_id: LineId) -> CommandResult {
    delete_line_impl(state.inner(), line_id)
}

pub(crate) fn delete_line_impl(state: &AppState, line_id: LineId) -> CommandResult {
    state.execute(Command::DeleteLine { line_id })
}

/// Reverse a line end-to-end so the far end becomes station 0.
///
/// Geometry is preserved: every station keeps its world coordinate and only the
/// numbering direction changes. `swap_left_right` additionally mirrors the
/// left/right passage dimensions - opt-in, because whether LRUD was recorded
/// facing along travel is a survey convention the app cannot infer.
#[tauri::command]
pub fn reverse_line(
    state: State<AppState>,
    line_id: LineId,
    swap_left_right: bool,
) -> CommandResult {
    reverse_line_impl(state.inner(), line_id, swap_left_right)
}

pub(crate) fn reverse_line_impl(
    state: &AppState,
    line_id: LineId,
    swap_left_right: bool,
) -> CommandResult {
    state.execute(Command::ReverseLine {
        line_id,
        swap_left_right,
    })
}

/// Move a line by an offset
#[tauri::command]
pub fn move_line(state: State<AppState>, line_id: LineId, offset: Coordinates) -> CommandResult {
    move_line_impl(state.inner(), line_id, offset)
}

pub(crate) fn move_line_impl(
    state: &AppState,
    line_id: LineId,
    offset: Coordinates,
) -> CommandResult {
    state.execute(Command::MoveLine { line_id, offset })
}

/// Move an entire segment (all connected lines) by an offset
#[tauri::command]
pub fn move_segment(state: State<AppState>, line_id: LineId, offset: Coordinates) -> CommandResult {
    state.execute(Command::MoveSegment { line_id, offset })
}

/// Execute a batch of commands as a single undo/redo unit
#[tauri::command]
pub fn execute_batch(state: State<AppState>, commands: Vec<Command>) -> CommandResult {
    state.execute(Command::Batch { commands })
}

/// Get all line IDs connected through merges and aligned closures (rigid groups)
#[tauri::command]
pub fn get_rigid_connected_line_ids(
    state: State<AppState>,
    line_id: LineId,
) -> Result<Vec<LineId>, String> {
    state.get_rigid_connected_line_ids(line_id)
}

/// Get all line IDs belonging to segments that contain a zero datum
#[tauri::command]
pub fn get_anchored_line_ids(state: State<AppState>) -> Result<Vec<LineId>, String> {
    state.get_anchored_line_ids()
}

/// Create a new line and immediately merge one of its stations with an existing station.
/// This is executed as a single undo/redo operation using the internal batch mechanism.
///
/// The merge station's line_id is specified as 0 (placeholder) and replaced with the
/// actual created line ID during execution.
#[tauri::command]
pub fn create_line_and_merge(
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
    cave_section_id: Option<SectionId>,
) -> CommandResult {
    create_line_and_merge_impl(
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
        cave_section_id,
    )
}

/// `cave_section_id` is the new line's Section (#327), the New Line dialog's
/// target: the Section joins the same single undo step.
pub(crate) fn create_line_and_merge_impl(
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
    cave_section_id: Option<SectionId>,
) -> CommandResult {
    if let Some(refusal) = state.refuse_new_line_name(name.as_deref()) {
        return refusal;
    }
    // Use a special internal execution that:
    // 1. Takes a project snapshot for undo
    // 2. Creates the line
    // 3. Merges with the actual line ID (and puts it in its Section)
    // 4. Returns as a single undo unit
    state.execute_create_and_merge_in_cave_section(
        name,
        surveyor,
        date,
        color,
        direction,
        shots,
        import_station_index,
        target_station,
        closure_strategy,
        cave_section_id,
    )
}

/// Get relationships that reference a line (merges, closures, offsets, zero datums).
/// Used by the frontend to show affected relationships before line deletion.
#[tauri::command]
pub fn get_line_relationships(
    state: State<AppState>,
    line_id: LineId,
) -> Result<LineRelationships, String> {
    get_line_relationships_impl(state.inner(), line_id)
}

pub(crate) fn get_line_relationships_impl(
    state: &AppState,
    line_id: LineId,
) -> Result<LineRelationships, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;

    if project.get_line(line_id).is_none() {
        return Err(crate::models::LINE_GONE.to_string());
    }

    Ok(project.get_line_relationships(line_id))
}

/// What the line ending at `station` can be combined with: one candidate
/// per other line ending at the same point (a junction lists them all, each
/// with its own reason when it is blocked). Empty when `station` is not a
/// line end or nothing else touches it.
#[tauri::command]
pub fn combine_candidate(
    state: State<AppState>,
    station: StationRef,
) -> Result<Vec<CombineCandidate>, String> {
    combine_candidate_impl(state.inner(), station)
}

pub(crate) fn combine_candidate_impl(
    state: &AppState,
    station: StationRef,
) -> Result<Vec<CombineCandidate>, String> {
    state.combine_candidates(station)
}

/// Can lines `a` and `b` be combined? `null` when either is missing or they
/// are the same line.
#[tauri::command]
pub fn combine_candidate_for_lines(
    state: State<AppState>,
    a: LineId,
    b: LineId,
) -> Result<Option<CombineCandidate>, String> {
    combine_candidate_for_lines_impl(state.inner(), a, b)
}

pub(crate) fn combine_candidate_for_lines_impl(
    state: &AppState,
    a: LineId,
    b: LineId,
) -> Result<Option<CombineCandidate>, String> {
    state.combine_candidate_for_lines(a, b)
}

/// Combine `other` into `kept`: one line, one undo step.
#[tauri::command]
pub fn combine_lines(
    state: State<AppState>,
    kept: LineId,
    other: LineId,
    options: CombineLinesOptions,
) -> CommandResult {
    combine_lines_impl(state.inner(), kept, other, options)
}

pub(crate) fn combine_lines_impl(
    state: &AppState,
    kept: LineId,
    other: LineId,
    options: CombineLinesOptions,
) -> CommandResult {
    state.execute_combine_lines(kept, other, options)
}

/// Get shot-line geometry for all lines in the segment containing the given line.
/// Coordinates are relative to the given line's station 0.
#[tauri::command]
pub fn get_segment_geometry(
    state: State<AppState>,
    line_id: LineId,
) -> Result<SegmentGeometry, String> {
    get_segment_geometry_impl(state.inner(), line_id)
}

pub(crate) fn get_segment_geometry_impl(
    state: &AppState,
    line_id: LineId,
) -> Result<SegmentGeometry, String> {
    state.get_segment_geometry(line_id)
}
