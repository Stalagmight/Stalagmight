//! The undo payload of "Add .stalx to current project" (#674).
//!
//! An add is ONE undo step that records only what the add brought - its
//! sections and everything hanging off them, the dedup hashes and ignored
//! warnings it carried, and the two project-level fields the shared append can
//! set - rather than a whole-project before/after snapshot, whose size would
//! grow with the cave the files are being added INTO.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{
    LoopClosure, Project, Section, SectionId, StationKey, StationMerge, ZeroDatum, MAX_SECTION_ID,
};

/// One ignored warning the add carried over from the source file, remapped onto
/// the copied section. Keyed by shot uuid, exactly as the open project keys its
/// own ignores (`resolved_warnings`, #57).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarriedIgnore {
    pub section_id: SectionId,
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
    pub next_section_id: SectionId,
    pub section_offsets: HashMap<SectionId, (f64, f64)>,
    pub file_hashes: HashSet<String>,
    pub section_hashes: HashSet<String>,
    pub surface_elevation: Option<f64>,
    pub location: Option<String>,
}

/// Everything one add put into the open project, and nothing else.
///
/// Undo REMOVES exactly this; redo REINSTALLS it verbatim - the same section
/// ids, shot uuids and closure ids - so redo never re-reads the source file,
/// which may have changed or gone since.
///
/// SELF-REFRESHING, like `CreatedSectionSnapshot`: the undo re-captures the
/// sections and their cross-references from the live project immediately
/// before it removes them ([`Self::refresh_from`]), so a save-time heal that
/// touched them after the push (a gauge pair, a minted uuid) comes back on
/// redo rather than being reverted by it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddedContent {
    /// The copied sections in the order they were appended - whole `Section`s,
    /// so identity (uuids, reverse parity, gauge pair) survives a redo.
    pub sections: Vec<Section>,
    #[serde(default)]
    pub section_offsets: Vec<(SectionId, (f64, f64))>,
    #[serde(default)]
    pub station_offsets: Vec<(StationKey, (f64, f64, f64))>,
    #[serde(default)]
    pub station_comments: Vec<(StationKey, String)>,
    /// Station names (#676), keyed like the comments.
    #[serde(default)]
    pub station_names: Vec<(StationKey, String)>,
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
    pub section_hashes: Vec<String>,
    /// Ignored warnings carried over from the source, remapped.
    #[serde(default)]
    pub ignores: Vec<CarriedIgnore>,
    /// `(before, after)` of the two project-level fields the shared append can
    /// set: the first entrance elevation wins, and locations concatenate.
    #[serde(default)]
    pub surface_elevation: (Option<f64>, Option<f64>),
    #[serde(default)]
    pub location: (Option<String>, Option<String>),
    /// `(section, before, after)` for every PRE-EXISTING section whose offset
    /// moved across the command - its epilogue, not the add itself, is the
    /// only thing that can do that. Normally empty.
    #[serde(default)]
    pub moved_offsets: Vec<(SectionId, Option<(f64, f64)>, Option<(f64, f64)>)>,
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
                next_section_id: project.next_section_id,
                section_offsets: project.section_offsets.clone(),
                file_hashes: project.imported_file_hashes.iter().cloned().collect(),
                section_hashes: project.imported_section_hashes.iter().cloned().collect(),
                surface_elevation: project.surface_elevation,
                location: project.location.clone(),
            }),
            ..Self::default()
        }
    }

    /// Complete a stub against the project the command left behind. `None` when
    /// this is not a stub.
    pub fn finalize(self, project: &Project, ignores: Vec<CarriedIgnore>) -> Option<Self> {
        let baseline = self.baseline?;
        let new_ids: HashSet<SectionId> = project
            .sections
            .iter()
            .map(|s| s.id)
            .filter(|id| *id >= baseline.next_section_id)
            .collect();
        let mut content = Self::capture(project, &new_ids);
        content.file_hashes = project
            .imported_file_hashes
            .iter()
            .filter(|h| !baseline.file_hashes.contains(*h))
            .cloned()
            .collect();
        content.section_hashes = project
            .imported_section_hashes
            .iter()
            .filter(|h| !baseline.section_hashes.contains(*h))
            .cloned()
            .collect();
        content.ignores = ignores;
        content.surface_elevation = (baseline.surface_elevation, project.surface_elevation);
        content.location = (baseline.location, project.location.clone());
        let mut moved: Vec<_> = project
            .sections
            .iter()
            .map(|s| s.id)
            .filter(|id| !new_ids.contains(id))
            .filter_map(|id| {
                let before = baseline.section_offsets.get(&id).copied();
                let after = project.section_offsets.get(&id).copied();
                (before != after).then_some((id, before, after))
            })
            .collect();
        moved.sort_by_key(|(id, _, _)| *id);
        content.moved_offsets = moved;
        Some(content)
    }

    /// The slot generation a stub was captured against; `None` on a finalized
    /// entry and on a rollback point.
    pub fn slot_generation(&self) -> Option<u64> {
        self.baseline.as_ref().and_then(|b| b.slot_generation)
    }

    /// The ids of the sections this add created.
    pub fn section_ids(&self) -> HashSet<SectionId> {
        self.sections.iter().map(|s| s.id).collect()
    }

    /// Read the sections in `ids` and every cross-reference into them off the
    /// live project, in a deterministic order (the payload is persisted, and a
    /// `HashMap` walk would re-serialize it differently every save).
    fn capture(project: &Project, ids: &HashSet<SectionId>) -> Self {
        let mut section_offsets: Vec<_> = project
            .section_offsets
            .iter()
            .filter(|(id, _)| ids.contains(*id))
            .map(|(id, offset)| (*id, *offset))
            .collect();
        section_offsets.sort_by_key(|(id, _)| *id);
        let mut station_offsets: Vec<_> = project
            .station_offsets
            .iter()
            .filter(|(key, _)| ids.contains(&key.section_id))
            .map(|(key, offset)| (key.clone(), *offset))
            .collect();
        station_offsets.sort_by_key(|(key, _)| (key.section_id, key.to_string()));
        let station_comments = station_texts_in(&project.station_comments, ids);
        let station_names = station_texts_in(&project.station_names, ids);
        let touches = |a: &StationKey, b: &StationKey| {
            ids.contains(&a.section_id) || ids.contains(&b.section_id)
        };
        Self {
            sections: project
                .sections
                .iter()
                .filter(|s| ids.contains(&s.id))
                .cloned()
                .collect(),
            section_offsets,
            station_offsets,
            station_comments,
            station_names,
            zero_datums: project
                .zero_datums
                .iter()
                .filter(|zd| ids.contains(&zd.station_ref.section_id))
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

    /// Re-read everything the live project can tell us - the sections, their
    /// cross-references, the "after" side of the project-level fields and of any
    /// moved pre-existing offset - so the redo puts back what the undo is about
    /// to destroy. The hashes and ignores are kept as recorded: nothing on
    /// `Project` says which hashes or ignores the add brought.
    pub fn refresh_from(&mut self, project: &Project) {
        let ids = self.section_ids();
        let live = Self::capture(project, &ids);
        self.sections = live.sections;
        self.section_offsets = live.section_offsets;
        self.station_offsets = live.station_offsets;
        self.station_comments = live.station_comments;
        self.station_names = live.station_names;
        self.zero_datums = live.zero_datums;
        self.station_merges = live.station_merges;
        self.loop_closures = live.loop_closures;
        self.surface_elevation.1 = project.surface_elevation;
        self.location.1.clone_from(&project.location);
        for (id, _, after) in &mut self.moved_offsets {
            *after = project.section_offsets.get(id).copied();
        }
    }

    /// Undo: take out exactly what the add put in and put back the
    /// project-level fields it changed. Any merge or closure still touching a
    /// removed section goes too - it could only dangle.
    ///
    /// The id allocators are deliberately left alone, so within a session a
    /// later command never reuses an id this redo may need again. Across a
    /// reload that does not hold - the load derives `next_section_id` as the
    /// highest saved id + 1 - and [`Self::install_into`] then refuses an id
    /// that is already taken.
    pub fn remove_from(&self, project: &mut Project) {
        let ids = self.section_ids();
        project.sections.retain(|s| !ids.contains(&s.id));
        project.section_offsets.retain(|id, _| !ids.contains(id));
        project
            .station_offsets
            .retain(|key, _| !ids.contains(&key.section_id));
        project
            .station_comments
            .retain(|key, _| !ids.contains(&key.section_id));
        project
            .station_names
            .retain(|key, _| !ids.contains(&key.section_id));
        project
            .zero_datums
            .retain(|zd| !ids.contains(&zd.station_ref.section_id));
        project.station_merges.retain(|m| {
            !ids.contains(&m.station_a.section_id) && !ids.contains(&m.station_b.section_id)
        });
        project.loop_closures.retain(|c| {
            !ids.contains(&c.station_a.section_id) && !ids.contains(&c.station_b.section_id)
        });
        let file_hashes: HashSet<&String> = self.file_hashes.iter().collect();
        project
            .imported_file_hashes
            .retain(|h| !file_hashes.contains(h));
        let section_hashes: HashSet<&String> = self.section_hashes.iter().collect();
        project
            .imported_section_hashes
            .retain(|h| !section_hashes.contains(h));
        project.surface_elevation = self.surface_elevation.0;
        project.location.clone_from(&self.location.0);
        for (id, before, _) in &self.moved_offsets {
            write_offset(project, *id, *before);
        }
        project.rebuild_section_index();
        project.rebuild_shot_uuid_index();
        project.touch();
    }

    /// Redo: reinstall the recorded content verbatim, appended where the add
    /// appended it. Refuses (with nothing changed) when a recorded section id is
    /// already taken - two sections sharing an id is a corrupt project - and
    /// when the record itself is not one an add could have produced (a section
    /// id past [`MAX_SECTION_ID`], or two recorded sections sharing an id): the
    /// entry is persisted with the file, so it is read back like any other input.
    pub fn install_into(&self, project: &mut Project) -> Result<(), String> {
        let ids = self.section_ids();
        if ids.len() != self.sections.len() {
            return Err("Cannot redo the add: its record names a section twice".to_string());
        }
        if let Some(id) = ids.iter().find(|id| **id > MAX_SECTION_ID) {
            return Err(format!(
                "Cannot redo the add: its record names section {id}, past the largest \
                 section id ({MAX_SECTION_ID})"
            ));
        }
        if let Some(taken) = project.sections.iter().find(|s| ids.contains(&s.id)) {
            return Err(format!(
                "Cannot redo the add: section {} already exists",
                taken.id
            ));
        }
        // An entry recorded before the survey credit moved onto the shots
        // carries it at section level; hand it to the shots on the way in.
        project
            .sections
            .extend(self.sections.iter().cloned().map(|mut section| {
                section.absorb_legacy_credit();
                section
            }));
        if let Some(max) = ids.iter().max() {
            project.next_section_id = project.next_section_id.max(max.saturating_add(1));
        }
        for (id, offset) in &self.section_offsets {
            Project::write_section_offset(&mut project.section_offsets, *id, *offset);
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
        extend_unique_hashes(&mut project.imported_section_hashes, &self.section_hashes);
        project.surface_elevation = self.surface_elevation.1;
        project.location.clone_from(&self.location.1);
        for (id, _, after) in &self.moved_offsets {
            write_offset(project, *id, *after);
        }
        project.rebuild_section_index();
        project.rebuild_shot_uuid_index();
        project.touch();
        Ok(())
    }
}

/// The `(key, text)` entries of a station-keyed text map (comments, names) on
/// `ids`, sorted so the persisted payload is stable.
fn station_texts_in(
    map: &HashMap<StationKey, String>,
    ids: &HashSet<SectionId>,
) -> Vec<(StationKey, String)> {
    let mut entries: Vec<_> = map
        .iter()
        .filter(|(key, _)| ids.contains(&key.section_id))
        .map(|(key, text)| (key.clone(), text.clone()))
        .collect();
    entries.sort_by_key(|(key, _)| (key.section_id, key.to_string()));
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
fn write_offset(project: &mut Project, id: SectionId, value: Option<(f64, f64)>) {
    match value {
        Some(offset) => Project::write_section_offset(&mut project.section_offsets, id, offset),
        None => {
            project.section_offsets.remove(&id);
        }
    }
}
