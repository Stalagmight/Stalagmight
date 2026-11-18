//! The undo payload of "Add .stalx to current project" (#674).
//!
//! An add is ONE undo step that records only what the add brought - its
//! lines and everything hanging off them, the dedup hashes and ignored
//! warnings it carried, and the two project-level fields the shared append can
//! set - rather than a whole-project before/after snapshot, whose size would
//! grow with the cave the files are being added INTO.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{
    Line, LineId, LoopClosure, Project, Section, SectionId, StationId, StationKey, StationMerge,
    ZeroDatum, MAX_LINE_ID,
};

/// One ignored warning the add carried over from the source file, remapped onto
/// the copied line. Keyed by shot uuid, exactly as the open project keys its
/// own ignores (`resolved_warnings`, #57).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarriedIgnore {
    pub line_id: LineId,
    pub shot_uuid: String,
    pub warning_code: String,
    pub resolved_at: DateTime<Utc>,
    #[serde(default)]
    pub resolved_comment: Option<String>,
}

/// What the forward command saw BEFORE it ran, carried from
/// `generate_undo_data` to `finalize_undo_data` and never persisted: the
/// finalized entry holds the difference, not the baseline.
#[derive(Debug, Clone, Default)]
pub struct AddBaseline {
    /// The project slot's generation when the stub was captured, which binds
    /// the whole command to ONE project: the forward step refuses, and the undo
    /// push is skipped, when another project was committed into the slot since.
    /// `None` for the forward step's own rollback point, taken and used under a
    /// single `project` guard.
    pub slot_generation: Option<u64>,
    pub next_line_id: LineId,
    pub line_offsets: HashMap<LineId, (f64, f64)>,
    pub file_hashes: HashSet<String>,
    pub line_hashes: HashSet<String>,
    pub surface_elevation: Option<f64>,
    pub location: Option<String>,
    /// The Sections the project held, so the ones the add appended can be
    /// told apart (#327).
    pub section_ids: HashSet<SectionId>,
}

/// Everything one add put into the open project, and nothing else.
///
/// Undo REMOVES exactly this; redo REINSTALLS it verbatim - the same line
/// ids, shot uuids and closure ids - so redo never re-reads the source file,
/// which may have changed or gone since.
///
/// SELF-REFRESHING, like `CreatedLineSnapshot`: the undo re-captures the
/// lines and their cross-references from the live project immediately
/// before it removes them ([`Self::refresh_from`]), so a save-time heal that
/// touched them after the push (a gauge pair, a minted uuid) comes back on
/// redo rather than being reverted by it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddedContent {
    /// The copied lines in the order they were appended - whole `Line`s,
    /// so identity (uuids, reverse parity, gauge pair) survives a redo.
    pub lines: Vec<Line>,
    #[serde(default)]
    pub line_offsets: Vec<(LineId, (f64, f64))>,
    #[serde(default)]
    pub station_offsets: Vec<(StationKey, (f64, f64, f64))>,
    #[serde(default)]
    pub station_comments: Vec<(StationKey, String)>,
    /// Station names (#676), keyed like the comments.
    #[serde(default)]
    pub station_names: Vec<(StationKey, String)>,
    /// Station IDs, keyed like the comments, so a redo gives the added
    /// stations back their ids. Empty in an entry written before Station IDs
    /// existed (the epilogue then mints fresh ones).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub station_ids: Vec<(StationKey, StationId)>,
    #[serde(default)]
    pub zero_datums: Vec<ZeroDatum>,
    #[serde(default)]
    pub station_merges: Vec<StationMerge>,
    #[serde(default)]
    pub loop_closures: Vec<LoopClosure>,
    /// Import-dedup hashes the add put into the project (ones it already held
    /// are not here, so undo cannot forget a hash an earlier import recorded).
    #[serde(default)]
    pub file_hashes: Vec<String>,
    #[serde(default)]
    pub line_hashes: Vec<String>,
    /// Ignored warnings carried over from the source, remapped.
    #[serde(default)]
    pub ignores: Vec<CarriedIgnore>,
    /// `(before, after)` of the two project-level fields the shared append can
    /// set: the first entrance elevation wins, and locations concatenate.
    #[serde(default)]
    pub surface_elevation: (Option<f64>, Option<f64>),
    #[serde(default)]
    pub location: (Option<String>, Option<String>),
    /// `(line, before, after)` for every PRE-EXISTING line whose offset
    /// moved across the command - its epilogue, not the add itself, is the
    /// only thing that can do that. Normally empty.
    #[serde(default)]
    pub moved_offsets: Vec<(LineId, Option<(f64, f64)>, Option<(f64, f64)>)>,
    /// The Sections the add appended (#327), in panel order. The added lines
    /// name them (`Line::section_id`), so undo removes them with the lines
    /// and redo puts them back with the same ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cave_sections: Vec<Section>,
    /// The stub's baseline; `None` on every finalized entry.
    #[serde(skip)]
    pub baseline: Option<AddBaseline>,
}

impl AddedContent {
    /// The stub `generate_undo_data` returns: the baseline only, captured under
    /// the `project` guard that read `slot_generation`.
    pub fn stub(project: &Project, slot_generation: Option<u64>) -> Self {
        Self {
            baseline: Some(AddBaseline {
                slot_generation,
                next_line_id: project.next_line_id,
                line_offsets: project.line_offsets.clone(),
                file_hashes: project.imported_file_hashes.iter().cloned().collect(),
                line_hashes: project.imported_line_hashes.iter().cloned().collect(),
                surface_elevation: project.surface_elevation,
                location: project.location.clone(),
                section_ids: project.sections.iter().map(|s| s.id).collect(),
            }),
            ..Self::default()
        }
    }

    /// Complete a stub against the project the command left behind. `None` when
    /// this is not a stub.
    pub fn finalize(self, project: &Project, ignores: Vec<CarriedIgnore>) -> Option<Self> {
        let baseline = self.baseline?;
        let new_ids: HashSet<LineId> = project
            .lines
            .iter()
            .map(|s| s.id)
            .filter(|id| *id >= baseline.next_line_id)
            .collect();
        let mut content = Self::capture(project, &new_ids);
        content.file_hashes = project
            .imported_file_hashes
            .iter()
            .filter(|h| !baseline.file_hashes.contains(*h))
            .cloned()
            .collect();
        content.line_hashes = project
            .imported_line_hashes
            .iter()
            .filter(|h| !baseline.line_hashes.contains(*h))
            .cloned()
            .collect();
        content.ignores = ignores;
        content.surface_elevation = (baseline.surface_elevation, project.surface_elevation);
        content.location = (baseline.location, project.location.clone());
        let mut moved: Vec<_> = project
            .lines
            .iter()
            .map(|s| s.id)
            .filter(|id| !new_ids.contains(id))
            .filter_map(|id| {
                let before = baseline.line_offsets.get(&id).copied();
                let after = project.line_offsets.get(&id).copied();
                (before != after).then_some((id, before, after))
            })
            .collect();
        moved.sort_by_key(|(id, _, _)| *id);
        content.moved_offsets = moved;
        content.cave_sections = project
            .sections
            .iter()
            .filter(|s| !baseline.section_ids.contains(&s.id))
            .cloned()
            .collect();
        content.shrink_to_fit();
        Some(content)
    }

    /// Give back the room the filtered collections grew past their length:
    /// the record lives as long as its entry.
    fn shrink_to_fit(&mut self) {
        self.lines.shrink_to_fit();
        self.line_offsets.shrink_to_fit();
        self.station_offsets.shrink_to_fit();
        self.station_comments.shrink_to_fit();
        self.station_names.shrink_to_fit();
        self.station_ids.shrink_to_fit();
        self.zero_datums.shrink_to_fit();
        self.station_merges.shrink_to_fit();
        self.loop_closures.shrink_to_fit();
        self.file_hashes.shrink_to_fit();
        self.line_hashes.shrink_to_fit();
        self.moved_offsets.shrink_to_fit();
        self.cave_sections.shrink_to_fit();
    }

    /// The slot generation a stub was captured against; `None` on a finalized
    /// entry and on a rollback point.
    pub fn slot_generation(&self) -> Option<u64> {
        self.baseline.as_ref().and_then(|b| b.slot_generation)
    }

    /// The ids of the lines this add created.
    pub fn line_ids(&self) -> HashSet<LineId> {
        self.lines.iter().map(|s| s.id).collect()
    }

    /// Read the lines in `ids` and every cross-reference into them off the
    /// live project, in a deterministic order (the payload is persisted, and a
    /// `HashMap` walk would re-serialize it differently every save).
    fn capture(project: &Project, ids: &HashSet<LineId>) -> Self {
        let mut line_offsets: Vec<_> = project
            .line_offsets
            .iter()
            .filter(|(id, _)| ids.contains(*id))
            .map(|(id, offset)| (*id, *offset))
            .collect();
        line_offsets.sort_by_key(|(id, _)| *id);
        let mut station_offsets: Vec<_> = project
            .station_offsets
            .iter()
            .filter(|(key, _)| ids.contains(&key.line_id))
            .map(|(key, offset)| (key.clone(), *offset))
            .collect();
        station_offsets.sort_by_key(|(key, _)| (key.line_id, key.to_string()));
        let station_comments = station_texts_in(&project.station_comments, ids);
        let station_names = station_texts_in(&project.station_names, ids);
        let mut station_ids: Vec<(StationKey, StationId)> = project
            .station_ids
            .iter()
            .filter(|(key, _)| ids.contains(&key.line_id))
            .map(|(key, id)| (key.clone(), *id))
            .collect();
        station_ids.sort_by_key(|(key, _)| (key.line_id, key.to_string()));
        let touches =
            |a: &StationKey, b: &StationKey| ids.contains(&a.line_id) || ids.contains(&b.line_id);
        Self {
            lines: project
                .lines
                .iter()
                .filter(|s| ids.contains(&s.id))
                .cloned()
                .collect(),
            line_offsets,
            station_offsets,
            station_comments,
            station_names,
            station_ids,
            zero_datums: project
                .zero_datums
                .iter()
                .filter(|zd| ids.contains(&zd.station_ref.line_id))
                .cloned()
                .collect(),
            station_merges: project
                .station_merges
                .iter()
                .filter(|m| touches(&m.station_a, &m.station_b))
                .cloned()
                .collect(),
            loop_closures: project
                .loop_closures
                .iter()
                .filter(|c| touches(&c.station_a, &c.station_b))
                .cloned()
                .collect(),
            ..Self::default()
        }
    }

    /// Re-read everything the live project can tell us - the lines, their
    /// cross-references, the "after" side of the project-level fields and of any
    /// moved pre-existing offset - so the redo puts back what the undo is about
    /// to destroy. The hashes and ignores are kept as recorded: nothing on
    /// `Project` says which hashes or ignores the add brought.
    pub fn refresh_from(&mut self, project: &Project) {
        let ids = self.line_ids();
        let live = Self::capture(project, &ids);
        self.lines = live.lines;
        self.line_offsets = live.line_offsets;
        self.station_offsets = live.station_offsets;
        self.station_comments = live.station_comments;
        self.station_names = live.station_names;
        self.station_ids = live.station_ids;
        self.zero_datums = live.zero_datums;
        self.station_merges = live.station_merges;
        self.loop_closures = live.loop_closures;
        self.surface_elevation.1 = project.surface_elevation;
        self.location.1.clone_from(&project.location);
        for (id, _, after) in &mut self.moved_offsets {
            *after = project.line_offsets.get(id).copied();
        }
        let added: HashSet<SectionId> = self.cave_sections.iter().map(|s| s.id).collect();
        self.cave_sections = project
            .sections
            .iter()
            .filter(|s| added.contains(&s.id))
            .cloned()
            .collect();
        self.shrink_to_fit();
    }

    /// Undo: take out exactly what the add put in and put back the
    /// project-level fields it changed. Any merge or closure still touching a
    /// removed line goes too - it could only dangle.
    ///
    /// The id allocators are deliberately left alone, so within a session a
    /// later command never reuses an id this redo may need again. Across a
    /// reload that does not hold - the load derives `next_line_id` as the
    /// highest saved id + 1 - and [`Self::install_into`] then refuses an id
    /// that is already taken.
    pub fn remove_from(&self, project: &mut Project) {
        let ids = self.line_ids();
        project.lines.retain(|s| !ids.contains(&s.id));
        project.line_offsets.retain(|id, _| !ids.contains(id));
        project
            .station_offsets
            .retain(|key, _| !ids.contains(&key.line_id));
        project
            .station_comments
            .retain(|key, _| !ids.contains(&key.line_id));
        project
            .station_names
            .retain(|key, _| !ids.contains(&key.line_id));
        // `next_station_id` stays, like the line id allocator: a later
        // command never reuses an id this redo may need again.
        project
            .station_ids
            .retain(|key, _| !ids.contains(&key.line_id));
        project
            .zero_datums
            .retain(|zd| !ids.contains(&zd.station_ref.line_id));
        project
            .station_merges
            .retain(|m| !ids.contains(&m.station_a.line_id) && !ids.contains(&m.station_b.line_id));
        project
            .loop_closures
            .retain(|c| !ids.contains(&c.station_a.line_id) && !ids.contains(&c.station_b.line_id));
        let file_hashes: HashSet<&String> = self.file_hashes.iter().collect();
        project
            .imported_file_hashes
            .retain(|h| !file_hashes.contains(h));
        let line_hashes: HashSet<&String> = self.line_hashes.iter().collect();
        project
            .imported_line_hashes
            .retain(|h| !line_hashes.contains(h));
        project.surface_elevation = self.surface_elevation.0;
        project.location.clone_from(&self.location.0);
        for (id, before, _) in &self.moved_offsets {
            write_offset(project, *id, *before);
        }
        for section in &self.cave_sections {
            project.remove_section(section.id);
        }
        project.rebuild_line_index();
        project.rebuild_shot_uuid_index();
        project.touch();
    }

    /// Redo: reinstall the recorded content verbatim, appended where the add
    /// appended it. Refuses (with nothing changed) when a recorded line id is
    /// already taken - two lines sharing an id is a corrupt project - and
    /// when the record itself is not one an add could have produced (a line
    /// id past [`MAX_LINE_ID`], or two recorded lines sharing an id): the
    /// entry is persisted with the file, so it is read back like any other input.
    pub fn install_into(&self, project: &mut Project) -> Result<(), String> {
        let ids = self.line_ids();
        if ids.len() != self.lines.len() {
            return Err("Cannot redo the add: its record names a line twice".to_string());
        }
        if let Some(id) = ids.iter().find(|id| **id > MAX_LINE_ID) {
            return Err(format!(
                "Cannot redo the add: its record names line {id}, past the largest \
                 line id ({MAX_LINE_ID})"
            ));
        }
        if let Some(taken) = project.lines.iter().find(|s| ids.contains(&s.id)) {
            return Err(format!(
                "Cannot redo the add: line {} already exists",
                taken.id
            ));
        }
        if let Some(taken) = self
            .cave_sections
            .iter()
            .find(|s| project.section(s.id).is_some())
        {
            return Err(format!(
                "Cannot redo the add: section {} already exists",
                taken.id
            ));
        }
        // An entry recorded before the survey credit moved onto the shots
        // carries it at line level; hand it to the shots on the way in.
        project
            .lines
            .extend(self.lines.iter().cloned().map(|mut line| {
                line.absorb_legacy_credit();
                line
            }));
        if let Some(max) = ids.iter().max() {
            project.next_line_id = project.next_line_id.max(max.saturating_add(1));
        }
        for (id, offset) in &self.line_offsets {
            Project::write_line_offset(&mut project.line_offsets, *id, *offset);
        }
        for (key, offset) in &self.station_offsets {
            project.station_offsets.insert(key.clone(), *offset);
        }
        for (key, comment) in &self.station_comments {
            project
                .station_comments
                .insert(key.clone(), comment.clone());
        }
        for (key, name) in &self.station_names {
            project.station_names.insert(key.clone(), name.clone());
        }
        // An id another station holds by now (across a reload of a crafted
        // entry) is left to the epilogue, which keeps ids unique.
        let held: HashSet<StationId> = project.station_ids.values().copied().collect();
        for (key, id) in &self.station_ids {
            if !held.contains(id) {
                project.station_ids.insert(key.clone(), *id);
                project.next_station_id = project.next_station_id.max(id.saturating_add(1));
            }
        }
        project.zero_datums.extend(self.zero_datums.iter().cloned());
        for merge in &self.station_merges {
            project.record_station_merge(merge.clone());
        }
        project
            .loop_closures
            .extend(self.loop_closures.iter().cloned());
        if let Some(max) = self.loop_closures.iter().map(|c| c.id).max() {
            project.next_closure_id = project.next_closure_id.max(max.saturating_add(1));
        }
        extend_unique_hashes(&mut project.imported_file_hashes, &self.file_hashes);
        extend_unique_hashes(&mut project.imported_line_hashes, &self.line_hashes);
        project.surface_elevation = self.surface_elevation.1;
        project.location.clone_from(&self.location.1);
        for (id, _, after) in &self.moved_offsets {
            write_offset(project, *id, *after);
        }
        project.sections.extend(self.cave_sections.iter().cloned());
        if let Some(max) = self.cave_sections.iter().map(|s| s.id).max() {
            project.next_section_id = project.next_section_id.max(max.saturating_add(1));
        }
        // A line naming a Section the project does not hold is ungrouped.
        project.normalize_sections();
        project.rebuild_line_index();
        project.rebuild_shot_uuid_index();
        project.touch();
        Ok(())
    }
}

/// The `(key, text)` entries of a station-keyed text map (comments, names) on
/// `ids`, sorted so the persisted payload is stable.
fn station_texts_in(
    map: &HashMap<StationKey, String>,
    ids: &HashSet<LineId>,
) -> Vec<(StationKey, String)> {
    let mut entries: Vec<_> = map
        .iter()
        .filter(|(key, _)| ids.contains(&key.line_id))
        .map(|(key, text)| (key.clone(), text.clone()))
        .collect();
    entries.sort_by_key(|(key, _)| (key.line_id, key.to_string()));
    entries
}

/// Append each of `incoming` that `list` does not hold yet, in order - one set
/// built up front instead of a scan of `list` per item.
pub fn extend_unique_hashes(list: &mut Vec<String>, incoming: &[String]) {
    let mut held: HashSet<String> = list.iter().cloned().collect();
    for hash in incoming {
        if held.insert(hash.clone()) {
            list.push(hash.clone());
        }
    }
}

/// `Some` writes through the dust-zero gate, `None` means "no key".
fn write_offset(project: &mut Project, id: LineId, value: Option<(f64, f64)>) {
    match value {
        Some(offset) => Project::write_line_offset(&mut project.line_offsets, id, offset),
        None => {
            project.line_offsets.remove(&id);
        }
    }
}
