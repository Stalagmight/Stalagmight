//! Sections on the project: named groupings of lines (#327).
//!
//! The project keeps its [`Section`]s in panel order (`Project::sections`) and
//! every line names the one it belongs to (`Line::section_id`). Names are
//! project-unique: an automatic path (an import, an Add of another project)
//! makes a taken name unique with ` (2)`, ` (3)`, ...; a name the user typed is
//! refused instead ([`Project::validate_section_name`]).

use std::collections::{HashMap, HashSet};

use super::Project;
use crate::models::{
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
