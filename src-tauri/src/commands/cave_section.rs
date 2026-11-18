//! Sections (#327): named groupings of lines. Every command is one undo step
//! through `AppState::execute` (and so behind the read-only gate). The
//! Section id travels as `caveSectionId` from the frontend (`cave_section_id`
//! here), never `sectionId`: `section_id` is the persisted name of a LINE id.

use tauri::State;

use crate::core::{AppState, Command, CommandResult};
use crate::models::{LineId, SectionId};

/// Create a Section named `name` (refused when empty or taken) with colour
/// `color` (`#rgb` / `#rrggbb`), holding `line_ids` (optional, may be empty).
/// The result's `cave_section_id` is the new Section's id.
#[tauri::command]
pub fn create_cave_section(
    state: State<AppState>,
    name: String,
    color: String,
    line_ids: Option<Vec<LineId>>,
) -> CommandResult {
    state.execute(Command::CreateCaveSection {
        name,
        color,
        line_ids: line_ids.unwrap_or_default(),
    })
}

/// Rename and/or recolour a Section; an absent field is kept. A name another
/// Section has is refused with a message naming it.
#[tauri::command]
pub fn update_cave_section(
    state: State<AppState>,
    cave_section_id: SectionId,
    name: Option<String>,
    color: Option<String>,
) -> CommandResult {
    state.execute(Command::UpdateCaveSection {
        section_id: cave_section_id,
        name,
        color,
    })
}

/// Delete a Section; its lines become ungrouped. Undo restores both.
#[tauri::command]
pub fn delete_cave_section(state: State<AppState>, cave_section_id: SectionId) -> CommandResult {
    state.execute(Command::DeleteCaveSection {
        section_id: cave_section_id,
    })
}

/// Move lines into a Section, or out of any (`cave_section_id` null/absent).
#[tauri::command]
pub fn move_lines_to_cave_section(
    state: State<AppState>,
    line_ids: Vec<LineId>,
    cave_section_id: Option<SectionId>,
) -> CommandResult {
    state.execute(Command::MoveLinesToCaveSection {
        line_ids,
        section_id: cave_section_id,
    })
}

/// Give every line of a Section the Section's colour, as one undo step.
#[tauri::command]
pub fn apply_cave_section_color(
    state: State<AppState>,
    cave_section_id: SectionId,
) -> CommandResult {
    state.execute(Command::ApplyCaveSectionColor {
        section_id: cave_section_id,
    })
}
