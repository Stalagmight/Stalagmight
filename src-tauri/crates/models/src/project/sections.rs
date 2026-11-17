//! Sections on the project: named groupings of lines (#327).
//!
//! The project keeps its [`Section`]s in panel order (`Project::sections`) and
//! every line names the one it belongs to (`Line::section_id`). Names are
//! project-unique: an automatic path (an import, an Add of another project)
//! makes a taken name unique with ` (2)`, ` (3)`, ...; a name the user typed is
//! refused instead ([`Project::validate_section_name`]).

use std::collections::HashSet;

use super::Project;
use crate::{
    clamp_section_name, is_section_color, Line, LineId, Section, SectionId, MAX_SECTION_NAME_CHARS,
    SECTION_PALETTE,
};

impl Project {
    /// The Section with `id`.
    pub fn section(&self, id: SectionId) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }

    /// The Section named exactly `name` (after trimming it).
    pub fn section_named(&self, name: &str) -> Option<&Section> {
        let name = name.trim();
        self.sections.iter().find(|s| s.name == name)
    }

    /// The lines in Section `id`, in project order.
    pub fn lines_in_section(&self, id: SectionId) -> impl Iterator<Item = &Line> + '_ {
        self.lines.iter().filter(move |l| l.section_id == Some(id))
    }

    /// A name the USER typed for a Section, trimmed, or why it is refused:
    /// empty, too long, or another Section's (`renaming` names the Section
    /// being renamed, whose own name is no collision).
    ///
    /// # Errors
    /// The message the user sees.
    pub fn validate_section_name(
        &self,
        name: &str,
        renaming: Option<SectionId>,
    ) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A section needs a name".to_string());
        }
        if name.chars().count() > MAX_SECTION_NAME_CHARS {
            return Err(format!(
                "Section names are at most {MAX_SECTION_NAME_CHARS} characters"
            ));
        }
        if self
            .sections
            .iter()
            .any(|s| s.name == name && Some(s.id) != renaming)
        {
            return Err(format!("There is already a section named \"{name}\""));
        }
        Ok(name.to_string())
    }

    /// A Section colour the user picked, trimmed, or why it is refused.
    ///
    /// # Errors
    /// The message the user sees.
    pub fn validate_section_color(color: &str) -> Result<String, String> {
        let color = color.trim();
        if is_section_color(color) {
            Ok(color.to_string())
        } else {
            Err(format!(
                "\"{color}\" is not a section colour; use a hex colour like #4ecdc4"
            ))
        }
    }

    /// `base` made unique among the Section names, for automatic paths:
    /// trimmed and clamped, then `base (2)`, `base (3)`, ... while taken. An
    /// empty base becomes `Section`.
    pub fn unique_section_name(&self, base: &str) -> String {
        let base = clamp_section_name(base);
        let base = if base.is_empty() {
            "Section".to_string()
        } else {
            base
        };
        let taken: HashSet<&str> = self.sections.iter().map(|s| s.name.as_str()).collect();
        if !taken.contains(base.as_str()) {
            return base;
        }
        (2..=u32::MAX)
            .map(|n| format!("{base} ({n})"))
            .find(|candidate| !taken.contains(candidate.as_str()))
            .unwrap_or(base)
    }

    /// The colour an automatic Section takes: the next one of
    /// [`SECTION_PALETTE`], by how many Sections the project already has.
    pub fn next_section_color(&self) -> String {
        SECTION_PALETTE[self.sections.len() % SECTION_PALETTE.len()].to_string()
    }

    /// Append a Section with the next id. The name and colour are taken as
    /// they are: callers validate ([`Self::validate_section_name`]) or dedupe
    /// ([`Self::unique_section_name`]) first.
    pub fn add_section(&mut self, name: String, color: String) -> SectionId {
        let above_all = self
            .sections
            .iter()
            .map(|s| s.id.saturating_add(1))
            .max()
            .unwrap_or(0);
        let id = self.next_section_id.max(above_all);
        self.next_section_id = id.saturating_add(1);
        self.sections.push(Section { id, name, color });
        id
    }

    /// The Section named `name`, created (deduped, palette colour) when the
    /// project has none: an import groups into a Section of the same name
    /// rather than beside it.
    pub fn section_named_or_new(&mut self, name: &str) -> SectionId {
        let name = clamp_section_name(name);
        if let Some(existing) = self.section_named(&name) {
            return existing.id;
        }
        let name = self.unique_section_name(&name);
        let color = self.next_section_color();
        self.add_section(name, color)
    }

    /// Remove Section `id` and ungroup its lines. `None` when there is none.
    pub fn remove_section(&mut self, id: SectionId) -> Option<Section> {
        let position = self.sections.iter().position(|s| s.id == id)?;
        let removed = self.sections.remove(position);
        for line in &mut self.lines {
            if line.section_id == Some(id) {
                line.section_id = None;
            }
        }
        Some(removed)
    }

    /// Put line `line_id` into Section `section` (`None`: ungroup it).
    /// Whether the line exists; the Section is the caller's to check.
    pub fn set_line_section(&mut self, line_id: LineId, section: Option<SectionId>) -> bool {
        match self.get_line_mut(line_id) {
            Some(line) => {
                line.section_id = section;
                true
            }
            None => false,
        }
    }

    /// Group imported lines by name: each `(line, name)` joins the Section
    /// called `name`, which is created when the project has none
    /// ([`Self::section_named_or_new`]). A line that does not exist is skipped.
    pub fn assign_sections_by_name(&mut self, assignments: &[(LineId, String)]) {
        for (line_id, name) in assignments {
            if name.trim().is_empty() || self.get_line(*line_id).is_none() {
                continue;
            }
            let section = self.section_named_or_new(name);
            self.set_line_section(*line_id, Some(section));
        }
    }

    /// Bring the Sections a file or another tool handed us to what this build
    /// maintains, returning how many things it changed:
    /// - a repeated Section id keeps its first Section (later ones are dropped);
    /// - an empty name is replaced and a repeated name made unique (the later
    ///   Section is renamed), as on every automatic path;
    /// - a line naming a Section that does not exist is ungrouped;
    /// - `next_section_id` is moved above every id.
    pub fn normalize_sections(&mut self) -> usize {
        let mut changed = 0usize;
        let mut ids: HashSet<SectionId> = HashSet::new();
        let before = self.sections.len();
        self.sections.retain(|s| ids.insert(s.id));
        changed += before - self.sections.len();

        let mut names: HashSet<String> = HashSet::new();
        for index in 0..self.sections.len() {
            let current = clamp_section_name(&self.sections[index].name);
            let base = if current.is_empty() {
                "Section".to_string()
            } else {
                current
            };
            let unique = if names.contains(&base) {
                (2..=u32::MAX)
                    .map(|n| format!("{base} ({n})"))
                    .find(|candidate| {
                        !names.contains(candidate)
                            && !self.sections.iter().any(|s| &s.name == candidate)
                    })
                    .unwrap_or_else(|| base.clone())
            } else {
                base
            };
            if unique != self.sections[index].name {
                self.sections[index].name.clone_from(&unique);
                changed += 1;
            }
            names.insert(unique);
        }

        for line in &mut self.lines {
            if let Some(section) = line.section_id {
                if !ids.contains(&section) {
                    line.section_id = None;
                    changed += 1;
                }
            }
        }

        let above_all = ids.iter().map(|id| id.saturating_add(1)).max().unwrap_or(0);
        if self.next_section_id < above_all {
            self.next_section_id = above_all;
            changed += 1;
        }
        changed
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::*;
    use crate::{LineData, ShotData};

    fn project_with_lines(n: usize) -> (Project, Vec<LineId>) {
        let mut project = Project::new("Sections".to_string(), None);
        let ids = (0..n)
            .map(|i| {
                project
                    .add_line(LineData {
                        name: Some(format!("Line {i}")),
                        shots: vec![ShotData {
                            length: 5.0,
                            ..Default::default()
                        }],
                        ..Default::default()
                    })
                    .id
            })
            .collect();
        (project, ids)
    }

    #[test]
    fn ids_are_allocated_above_every_existing_one() {
        let (mut project, _) = project_with_lines(0);
        let a = project.add_section("A".into(), "#123456".into());
        let b = project.add_section("B".into(), "#123456".into());
        assert_eq!((a, b), (0, 1));
        project.remove_section(b);
        let c = project.add_section("C".into(), "#123456".into());
        assert_eq!(c, 2, "a removed id is not handed out again in-session");
        project.next_section_id = 0;
        let d = project.add_section("D".into(), "#123456".into());
        assert_eq!(d, 3, "a stale counter cannot reuse a live id");
    }

    #[test]
    fn user_names_are_validated_and_automatic_ones_deduped() {
        let (mut project, _) = project_with_lines(0);
        let upper = project.add_section("Upper level".into(), "#123456".into());
        assert_eq!(
            project.validate_section_name("  Upper level ", None),
            Err("There is already a section named \"Upper level\"".to_string())
        );
        assert_eq!(
            project.validate_section_name("Upper level", Some(upper)),
            Ok("Upper level".to_string()),
            "renaming a section to its own name is no collision"
        );
        assert!(project.validate_section_name("   ", None).is_err());
        assert_eq!(
            project.unique_section_name("Upper level"),
            "Upper level (2)"
        );
        project.add_section("Upper level (2)".into(), "#123456".into());
        assert_eq!(
            project.unique_section_name(" Upper level"),
            "Upper level (3)"
        );
        assert_eq!(project.unique_section_name(""), "Section");
    }

    #[test]
    fn colours_must_be_hex() {
        assert_eq!(
            Project::validate_section_color(" #AbC "),
            Ok("#AbC".to_string())
        );
        assert!(Project::validate_section_color("blue").is_err());
    }

    #[test]
    fn removing_a_section_ungroups_its_lines() {
        let (mut project, ids) = project_with_lines(3);
        let s = project.add_section("S".into(), "#123456".into());
        let t = project.add_section("T".into(), "#123456".into());
        project.set_line_section(ids[0], Some(s));
        project.set_line_section(ids[1], Some(s));
        project.set_line_section(ids[2], Some(t));
        let removed = project.remove_section(s).unwrap();
        assert_eq!(removed.name, "S");
        assert_eq!(project.get_line(ids[0]).unwrap().section_id, None);
        assert_eq!(project.get_line(ids[1]).unwrap().section_id, None);
        assert_eq!(project.get_line(ids[2]).unwrap().section_id, Some(t));
        assert!(project.remove_section(s).is_none());
    }

    #[test]
    fn import_grouping_reuses_a_same_name_section() {
        let (mut project, ids) = project_with_lines(3);
        let existing = project.add_section("Upstream".into(), "#123456".into());
        project.assign_sections_by_name(&[
            (ids[0], "Upstream".to_string()),
            (ids[1], "Downstream".to_string()),
            (ids[2], "Downstream".to_string()),
            (999, "Ghost".to_string()),
        ]);
        assert_eq!(project.get_line(ids[0]).unwrap().section_id, Some(existing));
        let downstream = project.section_named("Downstream").unwrap().id;
        assert_eq!(
            project.get_line(ids[1]).unwrap().section_id,
            Some(downstream)
        );
        assert_eq!(
            project.get_line(ids[2]).unwrap().section_id,
            Some(downstream)
        );
        assert!(
            project.section_named("Ghost").is_none(),
            "no section for a line that does not exist"
        );
        assert_eq!(project.sections.len(), 2);
    }

    #[test]
    fn normalization_drops_dangling_memberships_and_repairs_the_list() {
        let (mut project, ids) = project_with_lines(3);
        project.sections = vec![
            Section {
                id: 4,
                name: "A".into(),
                color: "#123456".into(),
            },
            Section {
                id: 4,
                name: "Dup id".into(),
                color: "#123456".into(),
            },
            Section {
                id: 7,
                name: "A".into(),
                color: "#123456".into(),
            },
            Section {
                id: 8,
                name: "  ".into(),
                color: "#123456".into(),
            },
        ];
        project.next_section_id = 1;
        project.get_line_mut(ids[0]).unwrap().section_id = Some(4);
        project.get_line_mut(ids[1]).unwrap().section_id = Some(5);
        project.get_line_mut(ids[2]).unwrap().section_id = Some(7);
        assert!(project.normalize_sections() > 0);
        let names: Vec<&str> = project.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["A", "A (2)", "Section"]);
        assert_eq!(project.get_line(ids[0]).unwrap().section_id, Some(4));
        assert_eq!(project.get_line(ids[1]).unwrap().section_id, None);
        assert_eq!(project.get_line(ids[2]).unwrap().section_id, Some(7));
        assert_eq!(project.next_section_id, 9);
        assert_eq!(project.normalize_sections(), 0, "a fixpoint");
    }

    #[test]
    fn serde_keys_never_use_the_disk_names_of_lines() {
        let (mut project, ids) = project_with_lines(1);
        let s = project.add_section("S".into(), "#123456".into());
        project.set_line_section(ids[0], Some(s));
        let json = serde_json::to_value(&project).unwrap();
        assert!(json.get("cave_sections").is_some());
        assert_eq!(json["next_cave_section_id"], 1);
        assert_eq!(json["lines"][0]["cave_section_id"], s);
        assert!(json["lines"][0].get("section_id").is_none());
        let back: Project = serde_json::from_value(json).unwrap();
        assert_eq!(back.sections, project.sections);
        assert_eq!(back.lines[0].section_id, Some(s));

        let (plain, _) = project_with_lines(1);
        let json = serde_json::to_value(&plain).unwrap();
        assert!(
            json.get("cave_sections").is_none()
                && json.get("next_cave_section_id").is_none()
                && json["lines"][0].get("cave_section_id").is_none(),
            "a project without sections serializes as before: {json}"
        );
    }
}
