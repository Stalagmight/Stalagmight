//! The Section commands (#327): create, rename/recolour, delete, move lines
//! into or out of a Section, apply a Section's colour to its lines.
//!
//! Each runs through `execute()` like every edit (the read-only gate, one
//! undo step each): `apply` validates and mutates `project.sections`, the
//! lines' `section_id` and the Section counter; the pipeline's generic
//! changeset records the list and the touched lines on both sides. A
//! Section holds no survey data, so nothing geometric changes. The refusals
//! and their words are 0.6.0's.

use std::collections::HashSet;

use super::command_line::{missing_section, push_ref};
use super::commands::{Applied, Record};
use crate::core::Command;
use crate::models::project::station_naming::name_characters_refusal;
use crate::models::survey::Project;
use crate::models::{
    is_section_color, LineId, Section, SectionId, LINE_GONE, MAX_SECTION_NAME_CHARS,
};

/// Every id in `line_ids` names a line of `project`, deduplicated in order;
/// otherwise the refusal.
fn existing_lines(project: &Project, line_ids: &[LineId]) -> Result<Vec<LineId>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(line_ids.len());
    for id in line_ids {
        if project.line(*id).is_none() {
            return Err(LINE_GONE.to_string());
        }
        if seen.insert(*id) {
            out.push(*id);
        }
    }
    Ok(out)
}

/// A name the USER typed for a Section, trimmed, or why it is refused:
/// empty, a forbidden character, too long, or another Section's (`renaming`
/// names the Section being renamed, whose own name is no collision).
fn validate_section_name(
    project: &Project,
    name: &str,
    renaming: Option<SectionId>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A section needs a name".to_string());
    }
    if let Some(refusal) = name_characters_refusal("section", name) {
        return Err(refusal);
    }
    if name.chars().count() > MAX_SECTION_NAME_CHARS {
        return Err(format!(
            "Section names are at most {MAX_SECTION_NAME_CHARS} characters"
        ));
    }
    if project
        .sections
        .iter()
        .any(|s| s.name == name && Some(s.id) != renaming)
    {
        return Err(format!("There is already a section named \"{name}\""));
    }
    Ok(name.to_string())
}

/// A Section colour the user picked, trimmed, or why it is refused.
fn validate_section_color(color: &str) -> Result<String, String> {
    let color = color.trim();
    if is_section_color(color) {
        Ok(color.to_string())
    } else {
        Err(format!(
            "\"{color}\" is not a section colour; use a hex colour like #4ecdc4"
        ))
    }
}

/// Refused unless the project holds Section `id`.
fn require_section(project: &Project, id: SectionId) -> Result<(), String> {
    if project.sections.iter().any(|s| s.id == id) {
        Ok(())
    } else {
        Err(missing_section(id))
    }
}

/// Put every line of `lines` into `section` (`None`: ungroup them). One
/// pass over the line list, however many lines move.
fn set_lines_section(project: &mut Project, lines: &[LineId], section: Option<SectionId>) {
    if lines.is_empty() {
        return;
    }
    let moving: HashSet<LineId> = lines.iter().copied().collect();
    for line in project.lines_mut() {
        if moving.contains(&line.id) {
            line.section_id = section;
        }
    }
}

/// The execute pipeline's mutation for the cave section family
/// (`commands.rs` dispatches here; plan 9.1). Runs inside `run_recorded`,
/// which snapshots, validates, captures and records around it; `record` is
/// the entry being recorded (worded before the edit), whose refs gain the
/// Section the command names (the created one's id only the result knows).
pub(super) fn apply(
    project: &mut Project,
    record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    match cmd {
        Command::CreateCaveSection {
            name,
            color,
            line_ids,
        } => {
            let name = validate_section_name(project, name, None)?;
            let color = validate_section_color(color)?;
            let lines = existing_lines(project, line_ids)?;
            let id = project.mint_section_id().map_err(|e| e.to_string())?;
            project.sections.push(Section { id, name, color });
            set_lines_section(project, &lines, Some(id));
            push_ref(&mut record.refs.sections, id);
            Ok(Applied {
                cave_section_id: Some(id),
                ..Applied::default()
            })
        }
        Command::UpdateCaveSection {
            section_id,
            name,
            color,
        } => {
            require_section(project, *section_id)?;
            let name = name
                .as_deref()
                .map(|n| validate_section_name(project, n, Some(*section_id)))
                .transpose()?;
            let color = color.as_deref().map(validate_section_color).transpose()?;
            if let Some(section) = project.sections.iter_mut().find(|s| s.id == *section_id) {
                if let Some(name) = name {
                    section.name = name;
                }
                if let Some(color) = color {
                    section.color = color;
                }
            }
            push_ref(&mut record.refs.sections, *section_id);
            Ok(Applied::default())
        }
        Command::DeleteCaveSection { section_id } => {
            let position = project
                .sections
                .iter()
                .position(|s| s.id == *section_id)
                .ok_or_else(|| missing_section(*section_id))?;
            project.sections.remove(position);
            let members: Vec<LineId> = project
                .lines()
                .iter()
                .filter(|l| l.section_id == Some(*section_id))
                .map(|l| l.id)
                .collect();
            set_lines_section(project, &members, None);
            push_ref(&mut record.refs.sections, *section_id);
            Ok(Applied::default())
        }
        Command::MoveLinesToCaveSection {
            line_ids,
            section_id,
        } => {
            if let Some(id) = section_id {
                require_section(project, *id)?;
                push_ref(&mut record.refs.sections, *id);
            }
            let lines = existing_lines(project, line_ids)?;
            set_lines_section(project, &lines, *section_id);
            Ok(Applied::default())
        }
        Command::ApplyCaveSectionColor { section_id } => {
            let color = project
                .sections
                .iter()
                .find(|s| s.id == *section_id)
                .ok_or_else(|| missing_section(*section_id))?
                .color
                .clone();
            if project
                .lines()
                .iter()
                .any(|l| l.section_id == Some(*section_id) && l.color != color)
            {
                for line in project.lines_mut() {
                    if line.section_id == Some(*section_id) {
                        line.color.clone_from(&color);
                    }
                }
            }
            push_ref(&mut record.refs.sections, *section_id);
            Ok(Applied::default())
        }
        _ => Err(format!(
            "{} is not a cave section command",
            crate::core::command_kind(cmd)
        )),
    }
}
