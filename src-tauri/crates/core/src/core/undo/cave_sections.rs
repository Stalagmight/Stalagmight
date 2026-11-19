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

use crate::models::{LineId, Project, Section, SectionId, MAX_SECTION_ID};

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
    /// pushed back and would wedge every entry beneath it. The record is file
    /// content (the history is persisted), so what it installs is then
    /// normalized as a load is: repeated ids and names, unclean names, bad
    /// colours, ids past the bound, and memberships naming no Section.
    pub fn restore_into(&self, project: &mut Project) -> usize {
        project.sections.clone_from(&self.cave_sections);
        // Never lowered: an undone Section's id stays retired, so a view
        // setting naming Sections by id (hiddenSections`) never meets a
        // different Section under it. A recorded counter past
        // `MAX_SECTION_ID` is no counter this build keeps (a corrupt or
        // crafted record): it is ignored rather than raising the counter to
        // where ids saturate.
        if self.next_cave_section_id <= MAX_SECTION_ID {
            project.next_section_id = project.next_section_id.max(self.next_cave_section_id);
        }
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
        project.normalize_sections();
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
        // Captured for every line, kept for the entry's life: give the
        // lines it did not change their memory back.
        for state in [&mut before, &mut after] {
            state.members.retain(|m| changed.contains(&m.line_id));
            state.members.shrink_to_fit();
        }
        (before != after).then_some(Self { before, after })
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::*;
    use crate::models::{LineData, ShotData, MAX_SECTION_ID, SECTION_PALETTE};

    fn section(id: SectionId, name: &str, color: &str) -> Section {
        Section {
            id,
            name: name.into(),
            color: color.into(),
        }
    }

    /// The history is persisted, so a record is file content: what it
    /// installs is normalized as a load is, and its counter cannot push ids
    /// to where they saturate.
    #[test]
    fn a_crafted_record_installs_normalized_sections() {
        let mut project = Project::new("Undo".to_string(), None);
        let lines: Vec<LineId> = (0..3)
            .map(|_| {
                project
                    .add_line(LineData {
                        shots: vec![ShotData {
                            length: 5.0,
                            ..Default::default()
                        }],
                        ..Default::default()
                    })
                    .id
            })
            .collect();
        project.next_section_id = 4;
        let member = |line_id, section_id| CaveSectionMember {
            line_id,
            section_id,
            color: "#123456".into(),
        };
        let state = CaveSectionsState {
            cave_sections: vec![
                section(1, "Upper", "#abcdef"),
                section(1, "Duplicate id", "#abcdef"),
                section(2, "Upper", "javascript:alert(1)"),
                section(3, "\u{0}", "#123"),
                section(u32::MAX, "Line\nforged", "#123456"),
            ],
            next_cave_section_id: u32::MAX,
            members: vec![
                member(lines[0], Some(1)),
                member(lines[1], Some(u32::MAX)),
                member(lines[2], Some(9)),
            ],
        };
        assert_eq!(state.restore_into(&mut project), 0);
        let got: Vec<(SectionId, &str, &str)> = project
            .sections
            .iter()
            .map(|s| (s.id, s.name.as_str(), s.color.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (1, "Upper", "#abcdef"),
                (2, "Upper (2)", SECTION_PALETTE[1]),
                (3, "Section", "#123"),
                (4, "Line forged", "#123456"),
            ]
        );
        let section_of = |id| project.get_line(id).unwrap().section_id;
        assert_eq!(section_of(lines[0]), Some(1));
        assert_eq!(
            section_of(lines[1]),
            Some(4),
            "follows its re-minted Section"
        );
        assert_eq!(section_of(lines[2]), None, "names no Section");
        assert_eq!(project.next_section_id, 5);
        assert!(project.next_section_id <= MAX_SECTION_ID);
        assert_eq!(project.normalize_sections(), 0, "already settled");
        let fresh = project.add_section("New".into(), "#000000".into());
        assert_eq!(fresh, 5);
    }
}
