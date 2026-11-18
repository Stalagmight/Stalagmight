//! The Section commands (#327): create, rename/recolour, delete, move lines
//! into or out of a Section, apply a Section's colour to its lines - and
//! `CreateLine`'s executor, which can put the new line straight into a
//! Section.
//!
//! The five commands run through `execute()` like every edit (the read-only
//! gate, one undo step each). Their undo record is
//! [`UndoData::CaveSections`](crate::core::UndoData::CaveSections): the Section
//! list and the touched lines' memberships and colours on both sides, captured
//! by `generate_undo_data` / `finalize_undo_data`, so these executors only
//! validate and mutate.

use std::collections::HashSet;

use crate::core::CommandResult;
use crate::models::{LineData, LineId, Project, SectionId};

use super::AppState;

/// Every id in `line_ids` names a line of `project`, deduplicated in order;
/// otherwise the refusal.
fn existing_lines(project: &Project, line_ids: &[LineId]) -> Result<Vec<LineId>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(line_ids.len());
    for id in line_ids {
        if project.get_line(*id).is_none() {
            return Err(crate::models::LINE_GONE.to_string());
        }
        if seen.insert(*id) {
            out.push(*id);
        }
    }
    Ok(out)
}

/// The refusal for a Section id the project does not hold.
fn missing_section(id: SectionId) -> String {
    format!("Section {id} not found; it may have been deleted")
}

impl AppState {
    /// Run `edit` on the open project, or fail when none is open.
    fn with_project_for_sections(
        &self,
        edit: impl FnOnce(&mut Project) -> Result<CommandResult, String>,
    ) -> CommandResult {
        let Ok(mut guard) = self.project.lock() else {
            return CommandResult::failure("Failed to lock project".to_string());
        };
        let Some(project) = guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };
        edit(project).unwrap_or_else(CommandResult::failure)
    }

    /// `Command::CreateCaveSection`: a Section named `name` (user-typed, so
    /// refused when empty or taken) with `color`, holding `line_ids`.
    pub(super) fn create_cave_section(
        &self,
        name: &str,
        color: &str,
        line_ids: &[LineId],
    ) -> CommandResult {
        self.with_project_for_sections(|project| {
            let name = project.validate_section_name(name, None)?;
            let color = Project::validate_section_color(color)?;
            let lines = existing_lines(project, line_ids)?;
            let id = project.add_section(name, color);
            for line in lines {
                project.set_line_section(line, Some(id));
            }
            project.touch();
            Ok(CommandResult::success().with_cave_section_id(id))
        })
    }

    /// `Command::UpdateCaveSection`: rename (refused when empty or another
    /// Section's) and/or recolour.
    pub(super) fn update_cave_section(
        &self,
        section_id: SectionId,
        name: Option<&str>,
        color: Option<&str>,
    ) -> CommandResult {
        self.with_project_for_sections(|project| {
            if project.section(section_id).is_none() {
                return Err(missing_section(section_id));
            }
            let name = name
                .map(|n| project.validate_section_name(n, Some(section_id)))
                .transpose()?;
            let color = color.map(Project::validate_section_color).transpose()?;
            if let Some(section) = project.sections.iter_mut().find(|s| s.id == section_id) {
                if let Some(name) = name {
                    section.name = name;
                }
                if let Some(color) = color {
                    section.color = color;
                }
            }
            project.touch();
            Ok(CommandResult::success())
        })
    }

    /// `Command::DeleteCaveSection`: remove the Section; its lines become
    /// ungrouped.
    pub(super) fn delete_cave_section(&self, section_id: SectionId) -> CommandResult {
        self.with_project_for_sections(|project| {
            project
                .remove_section(section_id)
                .ok_or_else(|| missing_section(section_id))?;
            project.touch();
            Ok(CommandResult::success())
        })
    }

    /// `Command::MoveLinesToCaveSection`: put `line_ids` into `section`, or
    /// ungroup them (`None`). Refused whole when a line or the Section is
    /// missing.
    pub(super) fn move_lines_to_cave_section(
        &self,
        line_ids: &[LineId],
        section: Option<SectionId>,
    ) -> CommandResult {
        self.with_project_for_sections(|project| {
            if let Some(id) = section {
                if project.section(id).is_none() {
                    return Err(missing_section(id));
                }
            }
            for line in existing_lines(project, line_ids)? {
                project.set_line_section(line, section);
            }
            project.touch();
            Ok(CommandResult::success())
        })
    }

    /// `Command::ApplyCaveSectionColor`: every line of the Section takes its
    /// colour.
    pub(super) fn apply_cave_section_color(&self, section_id: SectionId) -> CommandResult {
        self.with_project_for_sections(|project| {
            let color = project
                .section(section_id)
                .ok_or_else(|| missing_section(section_id))?
                .color
                .clone();
            for line in &mut project.lines {
                if line.section_id == Some(section_id) {
                    line.color.clone_from(&color);
                }
            }
            project.touch();
            Ok(CommandResult::success())
        })
    }

    /// `Command::CreateLine`'s executor, shared by `execute()` and the batch
    /// and undo path (`execute_without_undo`): the line and, when the command
    /// names one, its Section (#327) - so a New Line created into a Section is
    /// ONE `CreateLine` undo step whose recorded identity
    /// (`finalize_undo_data`, `Project::line_identity`) carries the
    /// membership and the stations' ids and names that `execute()`'s epilogue
    /// settled. A Section that no longer exists refuses the command before
    /// anything changes.
    ///
    /// Identity is not the caller's to name (#423): the command arrives
    /// verbatim off IPC (`create_line`, `execute_batch`), so the shots' uuids
    /// are stripped HERE, not in `AppState::create_line` - the importers
    /// funnel through that executor with identities our own parser
    /// legitimately adopted.
    pub(super) fn create_line_command(
        &self,
        mut data: LineData,
        section: Option<SectionId>,
    ) -> CommandResult {
        for shot in &mut data.shots {
            shot.uuid = None;
        }
        if let Some(section_id) = section {
            let exists = self
                .project
                .lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(|p| p.section(section_id).is_some()));
            match exists {
                Some(true) => {}
                Some(false) => return CommandResult::failure(missing_section(section_id)),
                None => return CommandResult::failure("No project open".to_string()),
            }
        }
        let result = self.create_line(data);
        if let (true, Some(section_id), Some(line_id)) = (result.success, section, result.line_id) {
            let mut guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(project) = guard.as_mut() {
                project.set_line_section(line_id, Some(section_id));
            }
        }
        result
    }
}
