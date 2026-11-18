//! The undo payload of the Section commands (#327): the Section list and the
//! memberships and colours of the lines the command touched, on both sides.
//!
//! A Section command changes nothing but that state - no geometry, no station
//! reference - so restoring it is the whole inverse, in either direction, and
//! no command is re-executed. The payload is small: the Section list (a
//! handful of names) and only the lines whose membership or colour changed,
//! never a project snapshot.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::models::{LineId, Project, Section, SectionId};

/// One line's Section membership and colour on one side of the command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaveSectionMember {
    pub line_id: LineId,
    #[serde(default, rename = "cave_section_id")]
    pub section_id: Option<SectionId>,
    pub color: String,
}

/// The Section state on one side of a Section command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaveSectionsState {
    /// Every Section, in panel order.
    #[serde(default)]
    pub cave_sections: Vec<Section>,
    #[serde(default)]
    pub next_cave_section_id: SectionId,
    /// The lines the command changed, as they stood on this side; on a stub,
    /// every line.
    #[serde(default)]
    pub members: Vec<CaveSectionMember>,
}

impl CaveSectionsState {
    /// The project's Sections and every line's membership and colour.
    fn capture(project: &Project) -> Self {
        Self {
            cave_sections: project.sections.clone(),
            next_cave_section_id: project.next_section_id,
            members: project
                .lines
                .iter()
                .map(|line| CaveSectionMember {
                    line_id: line.id,
                    section_id: line.section_id,
                    color: line.color.clone(),
                })
                .collect(),
        }
    }

    /// Put this side back: the Section list wholesale, then each recorded
    /// line's membership and colour. A recorded line the project no longer
    /// holds is skipped (and counted), never a failed step: a failed undo is
    /// pushed back and would wedge every entry beneath it. A membership that
    /// names a Section this side does not hold is dropped.
    pub fn restore_into(&self, project: &mut Project) -> usize {
        project.sections.clone_from(&self.cave_sections);
        // Never lowered: an undone Section's id stays retired, so a view
        // setting naming Sections by id (`hiddenSections`) never meets a
        // different Section under it.
        project.next_section_id = project.next_section_id.max(self.next_cave_section_id);
        let mut missing = 0usize;
        for member in &self.members {
            match project.get_line_mut(member.line_id) {
                Some(line) => {
                    line.section_id = member.section_id;
                    line.color.clone_from(&member.color);
                }
                None => missing += 1,
            }
        }
        let held: HashSet<SectionId> = project.sections.iter().map(|s| s.id).collect();
        for line in &mut project.lines {
            if line.section_id.is_some_and(|id| !held.contains(&id)) {
                line.section_id = None;
            }
        }
        project.touch();
        missing
    }

    /// The Section `id` on this side.
    pub fn section(&self, id: SectionId) -> Option<&Section> {
        self.cave_sections.iter().find(|s| s.id == id)
    }
}

/// Both sides of one Section command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaveSectionChange {
    /// What the undo restores.
    pub before: CaveSectionsState,
    /// What the redo restores.
    pub after: CaveSectionsState,
}

impl CaveSectionChange {
    /// The stub `generate_undo_data` returns: the whole state before the
    /// command, every line included. Never persisted: `finalize` keeps only
    /// what changed.
    pub fn stub(project: &Project) -> Self {
        Self {
            before: CaveSectionsState::capture(project),
            after: CaveSectionsState::default(),
        }
    }

    /// Complete a stub against the project the command left behind, keeping
    /// only the lines whose membership or colour changed. `None` when the
    /// command changed nothing, so no step is recorded for it.
    pub fn finalize(self, project: &Project) -> Option<Self> {
        let mut after = CaveSectionsState::capture(project);
        let mut before = self.before;
        let then: HashMap<LineId, &CaveSectionMember> =
            before.members.iter().map(|m| (m.line_id, m)).collect();
        let changed: HashSet<LineId> = after
            .members
            .iter()
            .filter(|now| then.get(&now.line_id).is_some_and(|was| *was != *now))
            .map(|now| now.line_id)
            .collect();
        before.members.retain(|m| changed.contains(&m.line_id));
        after.members.retain(|m| changed.contains(&m.line_id));
        (before != after).then_some(Self { before, after })
    }
}
