//! History labels of the Section commands (#327), named from the undo record:
//! a bare command carries a Section's id, while the record holds the Section
//! list on both sides, so the label can say which Section it was even after it
//! was renamed or deleted.

use crate::core::command::lines_label;
use crate::core::{CaveSectionChange, Command, UndoData, UndoEntry};
use crate::models::SectionId;

/// The label of a Section command's entry; `None` for any other entry.
pub(super) fn description(entry: &UndoEntry) -> Option<String> {
    let UndoData::CaveSections(change) = &entry.undo_data else {
        return None;
    };
    Some(match &entry.command {
        Command::CreateCaveSection { name, line_ids, .. } => {
            let name = name.trim();
            if line_ids.is_empty() {
                format!("Create section \"{name}\"")
            } else {
                format!(
                    "Create section \"{name}\" with {}",
                    lines_label(distinct(line_ids))
                )
            }
        }
        Command::UpdateCaveSection { section_id, .. } => {
            let before = name_in(change, *section_id, false);
            let after = name_in(change, *section_id, true);
            if before == after {
                format!("Change colour of section \"{after}\"")
            } else {
                format!("Rename section \"{before}\" to \"{after}\"")
            }
        }
        Command::DeleteCaveSection { section_id } => {
            format!("Delete section \"{}\"", name_in(change, *section_id, false))
        }
        Command::MoveLinesToCaveSection {
            line_ids,
            section_id,
        } => {
            let lines = lines_label(distinct(line_ids));
            match section_id {
                Some(id) => format!("Move {lines} to section \"{}\"", name_in(change, *id, true)),
                None => format!("Move {lines} out of their section"),
            }
        }
        Command::ApplyCaveSectionColor { section_id } => format!(
            "Apply colour of section \"{}\" to its lines",
            name_in(change, *section_id, true)
        ),
        _ => return None,
    })
}

fn distinct(ids: &[crate::models::LineId]) -> usize {
    ids.iter().collect::<std::collections::HashSet<_>>().len()
}

/// The Section's name on one side of the record, falling back to the other
/// side and then to its id.
fn name_in(change: &CaveSectionChange, id: SectionId, after: bool) -> String {
    let (first, second) = if after {
        (&change.after, &change.before)
    } else {
        (&change.before, &change.after)
    };
    first
        .section(id)
        .or_else(|| second.section(id))
        .map_or_else(|| format!("#{id}"), |s| s.name.clone())
}
