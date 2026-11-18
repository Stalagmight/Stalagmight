//! Sections on the project: named groupings of lines (#327).
//!
//! The project keeps its [`Section`]s in panel order (`Project::sections`) and
//! every line names the one it belongs to (`Line::section_id`). Names are
//! project-unique: an automatic path (an import, an Add of another project)
//! makes a taken name unique with ` (2)`, ` (3)`, ...; a name the user typed is
//! refused instead ([`Project::validate_section_name`]).

use std::collections::{HashMap, HashSet};

use super::Project;
use crate::{
    clamp_section_name, is_section_color, Line, LineId, Section, SectionId, MAX_SECTION_ID,
    MAX_SECTION_NAME_CHARS, SECTION_PALETTE,
};

/// What an automatic path names a Section whose own name is empty.
const FALLBACK_SECTION_NAME: &str = "Section";

/// `name` as an automatic path takes it: cleaned and clamped
/// ([`clamp_section_name`]), [`FALLBACK_SECTION_NAME`] when nothing is left.
fn automatic_section_name(name: &str) -> String {
    let name = clamp_section_name(name);
    if name.is_empty() {
        FALLBACK_SECTION_NAME.to_string()
    } else {
        name
    }
}

/// Unique Section names for an automatic path that names many at once. The
/// names to avoid live in one set that only grows, so a ` (n)` once seen
/// taken stays taken, and the next search for the same base resumes at the
/// first `n` not yet tried: naming N Sections after one base costs O(N)
/// probes, not a rescan from ` (2)` per Section (a crafted file of thousands
/// of same-named Sections hung its open on that rescan).
struct SectionNamer {
    taken: HashSet<String>,
    next_suffix: HashMap<String, u64>,
}

impl SectionNamer {
    fn avoiding<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            taken: names.into_iter().map(str::to_string).collect(),
            next_suffix: HashMap::new(),
        }
    }

    /// The first `base (n)`, `n >= 2`, not taken; taken from then on.
    fn suffixed(&mut self, base: &str) -> String {
        let n = self.next_suffix.entry(base.to_string()).or_insert(2);
        let name = loop {
            let candidate = format!("{base} ({n})");
            *n += 1;
            if !self.taken.contains(&candidate) {
                break candidate;
            }
        };
        self.taken.insert(name.clone());
        name
    }

    /// `name` as an automatic path takes it, then `name (2)`, `name (3)`, ...
    /// while taken; taken from then on.
    fn claim(&mut self, name: &str) -> String {
        let base = automatic_section_name(name);
        if self.taken.insert(base.clone()) {
            base
        } else {
            self.suffixed(&base)
        }
    }
}

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
        if let Some(refusal) = super::station_naming::name_characters_refusal("section", name) {
            return Err(refusal);
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
    /// cleaned and clamped ([`clamp_section_name`]), then `base (2)`,
    /// `base (3)`, ... while taken. An empty base becomes `Section`.
    pub fn unique_section_name(&self, base: &str) -> String {
        SectionNamer::avoiding(self.sections.iter().map(|s| s.name.as_str())).claim(base)
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
        self.reserve_section_ids(1);
        self.push_section(name, color)
    }

    /// Append copies of `sections` (another project's, on an Add) with fresh
    /// ids, each name made unique among the project's Sections and the copies
    /// before it as [`Self::unique_section_name`] would, the colours as given.
    /// Returns the new ids in order. Linear in the Sections held and copied,
    /// where `add_section` in a loop rescans the list per Section.
    pub fn append_sections(&mut self, sections: &[Section]) -> Vec<SectionId> {
        self.reserve_section_ids(sections.len());
        let mut namer = SectionNamer::avoiding(self.sections.iter().map(|s| s.name.as_str()));
        sections
            .iter()
            .map(|section| {
                let name = namer.claim(&section.name);
                self.push_section(name, section.color.clone())
            })
            .collect()
    }

    /// Make room for `count` fresh ids: `next_section_id` raised above every
    /// held id (a stale counter never hands out a live one), and, when `count`
    /// more would reach [`MAX_SECTION_ID`], every Section renumbered first.
    /// The next `count` [`Self::push_section`]s then take valid, unused ids
    /// rather than saturating on one.
    fn reserve_section_ids(&mut self, count: usize) {
        let above_all = self
            .sections
            .iter()
            .map(|s| s.id.saturating_add(1))
            .max()
            .unwrap_or(0);
        self.next_section_id = self.next_section_id.max(above_all);
        let room = u64::from(MAX_SECTION_ID.saturating_sub(self.next_section_id));
        if count as u64 > room {
            self.renumber_sections();
        }
    }

    /// Append one Section under the counter; [`Self::reserve_section_ids`]
    /// made room for it.
    fn push_section(&mut self, name: String, color: String) -> SectionId {
        let id = self.next_section_id;
        self.next_section_id = id + 1;
        self.sections.push(Section { id, name, color });
        id
    }

    /// Renumber the Sections `0, 1, ...` in panel order, their lines following
    /// them (a line naming no held Section is ungrouped), the counter after
    /// them. Only a crafted counter or id, or 2^31 Sections created, leaves no
    /// room below [`MAX_SECTION_ID`]; the hidden-Sections view setting (keyed
    /// by id) may then name another Section.
    fn renumber_sections(&mut self) {
        tracing::warn!(
            sections = self.sections.len(),
            next_section_id = self.next_section_id,
            purpose = "keep Section ids unique and bounded",
            "Section ids would overflow; renumbering every Section"
        );
        let mut remap: HashMap<SectionId, SectionId> = HashMap::with_capacity(self.sections.len());
        for (index, section) in self.sections.iter_mut().enumerate() {
            let id = index as SectionId;
            remap.entry(section.id).or_insert(id);
            section.id = id;
        }
        for line in &mut self.lines {
            line.section_id = line.section_id.and_then(|old| remap.get(&old).copied());
        }
        self.next_section_id = self.sections.len() as SectionId;
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
    /// called `name` (as [`clamp_section_name`] cleans it), which is created
    /// when the project has none, as [`Self::section_named_or_new`] would. A
    /// line that does not exist, or a name that cleans to nothing, is
    /// skipped. Linear in the assignments and the Sections.
    pub fn assign_sections_by_name(&mut self, assignments: &[(LineId, String)]) {
        let wanted = |project: &Self, line_id: LineId, name: &str| {
            let name = clamp_section_name(name);
            (!name.is_empty() && project.get_line(line_id).is_some()).then_some(name)
        };
        // The names no Section has yet, in first-use order, so the ids are
        // reserved (and any renumbering done) before one is handed out.
        let mut fresh: Vec<String> = Vec::new();
        {
            let held: HashSet<&str> = self.sections.iter().map(|s| s.name.as_str()).collect();
            let mut seen: HashSet<String> = HashSet::new();
            for (line_id, name) in assignments {
                if let Some(name) = wanted(self, *line_id, name) {
                    if !held.contains(name.as_str()) && seen.insert(name.clone()) {
                        fresh.push(name);
                    }
                }
            }
        }
        let mut by_name: HashMap<String, SectionId> = HashMap::with_capacity(self.sections.len());
        if !fresh.is_empty() {
            self.reserve_section_ids(fresh.len());
            let mut namer = SectionNamer::avoiding(self.sections.iter().map(|s| s.name.as_str()));
            for name in fresh {
                let unique = namer.claim(&name);
                let color = self.next_section_color();
                let id = self.push_section(unique, color);
                by_name.insert(name, id);
            }
        }
        for section in &self.sections {
            by_name.entry(section.name.clone()).or_insert(section.id);
        }
        for (line_id, name) in assignments {
            let Some(name) = wanted(self, *line_id, name) else {
                continue;
            };
            if let Some(&section) = by_name.get(&name) {
                self.set_line_section(*line_id, Some(section));
            }
        }
    }

    /// Bring the Sections a file, an undo record or another tool handed us to
    /// what this build maintains, returning how many things it changed:
    /// - a repeated Section id keeps its first Section (later ones are dropped);
    /// - a line naming a Section that does not exist is ungrouped;
    /// - an id at or past [`MAX_SECTION_ID`] is re-minted, its lines following
    ///   it, and a counter past the bound starts again above the valid ids;
    /// - a name is cleaned as on every automatic path ([`clamp_section_name`]),
    ///   an empty one replaced and a repeated one made unique (the later
    ///   Section is renamed);
    /// - a colour that is not `#rgb` / `#rrggbb` (trimmed) takes the palette
    ///   colour of the Section's place;
    /// - `next_section_id` is moved above every id.
    ///
    /// Linear in the Sections and lines: a file's rows are not bounded by
    /// anything the user did.
    pub fn normalize_sections(&mut self) -> usize {
        let mut changed = 0usize;
        let mut ids: HashSet<SectionId> = HashSet::with_capacity(self.sections.len());
        let before = self.sections.len();
        self.sections.retain(|s| ids.insert(s.id));
        changed += before - self.sections.len();

        for line in &mut self.lines {
            if line.section_id.is_some_and(|id| !ids.contains(&id)) {
                line.section_id = None;
                changed += 1;
            }
        }

        changed += self.bound_section_ids();

        // A repeat is decided against the names already settled; its new name
        // avoids those and every name a Section still holds, so a later
        // `A (2)` is not renamed for an earlier repeat of `A`.
        let mut namer = SectionNamer::avoiding(self.sections.iter().map(|s| s.name.as_str()));
        let mut settled: HashSet<String> = HashSet::with_capacity(self.sections.len());
        for (index, section) in self.sections.iter_mut().enumerate() {
            let base = automatic_section_name(&section.name);
            let name = if settled.contains(&base) {
                namer.suffixed(&base)
            } else {
                namer.taken.insert(base.clone());
                base
            };
            if name != section.name {
                section.name.clone_from(&name);
                changed += 1;
            }
            settled.insert(name);

            if !is_section_color(&section.color) {
                let trimmed = section.color.trim();
                section.color = if is_section_color(trimmed) {
                    trimmed.to_string()
                } else {
                    SECTION_PALETTE[index % SECTION_PALETTE.len()].to_string()
                };
                changed += 1;
            }
        }

        let above_all = self.sections.iter().map(|s| s.id + 1).max().unwrap_or(0);
        if self.next_section_id < above_all {
            self.next_section_id = above_all;
            changed += 1;
        }
        changed
    }

    /// Every Section id below [`MAX_SECTION_ID`] and the counter at most it:
    /// a Section holding an id past the bound gets a fresh one, its lines
    /// following it (every Section is renumbered when there is no room); a
    /// counter past the bound starts again above the valid ids. Runs with the
    /// ids unique and every membership naming a held Section. Returns how
    /// many things it changed.
    fn bound_section_ids(&mut self) -> usize {
        let mut changed = 0;
        if self.next_section_id > MAX_SECTION_ID {
            self.next_section_id = 0;
            changed += 1;
        }
        let invalid = self
            .sections
            .iter()
            .filter(|s| s.id >= MAX_SECTION_ID)
            .count();
        if invalid == 0 {
            return changed;
        }
        let above_valid = self
            .sections
            .iter()
            .filter(|s| s.id < MAX_SECTION_ID)
            .map(|s| s.id + 1)
            .max()
            .unwrap_or(0);
        self.next_section_id = self.next_section_id.max(above_valid);
        if invalid as u64 > u64::from(MAX_SECTION_ID - self.next_section_id) {
            self.renumber_sections();
            return changed + invalid;
        }
        let mut remap: HashMap<SectionId, SectionId> = HashMap::with_capacity(invalid);
        for section in &mut self.sections {
            if section.id >= MAX_SECTION_ID {
                remap.insert(section.id, self.next_section_id);
                section.id = self.next_section_id;
                self.next_section_id += 1;
            }
        }
        for line in &mut self.lines {
            if let Some(&id) = line.section_id.and_then(|old| remap.get(&old)) {
                line.section_id = Some(id);
            }
        }
        changed + invalid
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

    fn named(id: SectionId, name: &str, color: &str) -> Section {
        Section {
            id,
            name: name.into(),
            color: color.into(),
        }
    }

    /// A crafted file of 20k Sections sharing one name (the read is not
    /// bounded by anything the user did): every repeat gets the next free
    /// suffix in one pass. The old search restarted at ` (2)` for each and
    /// scanned every Section per candidate, cubic in the rows.
    #[test]
    fn normalizing_many_repeats_of_one_name_is_linear() {
        const N: u32 = 20_000;
        let (mut project, _) = project_with_lines(0);
        project.sections = (0..N).map(|id| named(id, "X", "#123456")).collect();
        // A later Section already holding a suffix keeps it; the repeats skip it.
        project.sections.push(named(N, "X (3)", "#123456"));
        let started = std::time::Instant::now();
        project.normalize_sections();
        let elapsed = started.elapsed();
        let names: HashSet<&str> = project.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), project.sections.len(), "every name unique");
        assert_eq!(project.sections[0].name, "X");
        assert_eq!(project.sections[1].name, "X (2)");
        assert_eq!(
            project.sections[2].name, "X (4)",
            "X (3) is held further down"
        );
        assert_eq!(project.sections[N as usize].name, "X (3)");
        assert_eq!(project.normalize_sections(), 0, "a fixpoint");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "normalizing took {elapsed:?}"
        );
    }

    /// The batch paths (an Add's copies, an import's groups) dedupe in one pass
    /// too, with the names `unique_section_name` gives one at a time.
    #[test]
    fn batch_appends_dedupe_like_one_at_a_time() {
        let (mut project, ids) = project_with_lines(2);
        project.add_section("Upper".into(), "#123456".into());
        let copies: Vec<Section> = (0..5_000)
            .map(|id| named(id, " Upper ", "#abcdef"))
            .collect();
        let started = std::time::Instant::now();
        let new_ids = project.append_sections(&copies);
        assert_eq!(new_ids.len(), 5_000);
        assert_eq!(new_ids[0], 1);
        assert_eq!(project.sections[1].name, "Upper (2)");
        assert_eq!(project.sections[5_000].name, "Upper (5001)");
        assert_eq!(project.next_section_id, 5_001);

        let assignments: Vec<(LineId, String)> = vec![
            (ids[0], "Lower\tlevel".to_string()),
            (ids[1], "Lower level".to_string()),
            (999, "Ghost".to_string()),
            (ids[1], "\u{200B}".to_string()),
        ];
        project.assign_sections_by_name(&assignments);
        let lower = project.section_named("Lower level").unwrap().id;
        assert_eq!(project.get_line(ids[0]).unwrap().section_id, Some(lower));
        assert_eq!(project.get_line(ids[1]).unwrap().section_id, Some(lower));
        assert!(project.section_named("Ghost").is_none());
        assert!(
            project.section_named("Section").is_none(),
            "a name that cleans to nothing is skipped"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// File or undo-record content: control characters and invisible marks
    /// leave a name, an empty one is replaced, and a colour that is not a hex
    /// colour takes the palette colour of the Section's place.
    #[test]
    fn normalization_cleans_names_and_colours() {
        let (mut project, _) = project_with_lines(0);
        project.sections = vec![
            named(0, "Up\nper", " #AbC "),
            named(1, "\u{202E}\u{0}", "red"),
            named(2, "Upper", "#12345"),
            named(3, "Fine", "#4ecdc4"),
        ];
        assert!(project.normalize_sections() > 0);
        let got: Vec<(&str, &str)> = project
            .sections
            .iter()
            .map(|s| (s.name.as_str(), s.color.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("Up per", "#AbC"),
                ("Section", SECTION_PALETTE[1]),
                ("Upper", SECTION_PALETTE[2]),
                ("Fine", "#4ecdc4"),
            ]
        );
        assert_eq!(project.normalize_sections(), 0, "a fixpoint");
    }

    /// Ids are bounded like Station IDs: a counter at the bound renumbers the
    /// Sections (lines following) instead of minting one id twice, an id past
    /// it is re-minted on normalization, and a counter past it starts again
    /// above the valid ids.
    #[test]
    fn section_ids_stay_below_the_bound() {
        let (mut project, ids) = project_with_lines(2);
        let a = project.add_section("A".into(), "#123456".into());
        project.set_line_section(ids[0], Some(a));
        project.next_section_id = MAX_SECTION_ID;
        let b = project.add_section("B".into(), "#123456".into());
        let c = project.add_section("C".into(), "#123456".into());
        assert_ne!(b, c, "no id is handed out twice");
        assert!(b < MAX_SECTION_ID && c < MAX_SECTION_ID);
        let a = project.section_named("A").unwrap().id;
        assert_eq!(
            project.get_line(ids[0]).unwrap().section_id,
            Some(a),
            "the line follows its Section"
        );

        project.next_section_id = u32::MAX;
        assert_eq!(project.add_section("D".into(), "#123456".into()), 3);
        assert_eq!(project.add_section("E".into(), "#123456".into()), 4);

        let (mut project, ids) = project_with_lines(2);
        project.sections = vec![
            named(5, "Low", "#123456"),
            named(u32::MAX, "High", "#123456"),
        ];
        project.next_section_id = u32::MAX;
        project.get_line_mut(ids[0]).unwrap().section_id = Some(u32::MAX);
        project.get_line_mut(ids[1]).unwrap().section_id = Some(5);
        assert!(project.normalize_sections() > 0);
        let high = project.section_named("High").unwrap().id;
        assert_eq!(high, 6);
        assert_eq!(project.get_line(ids[0]).unwrap().section_id, Some(high));
        assert_eq!(project.get_line(ids[1]).unwrap().section_id, Some(5));
        assert_eq!(project.next_section_id, 7);
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
