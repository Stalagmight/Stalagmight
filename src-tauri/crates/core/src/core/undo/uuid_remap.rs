//! Translating a loaded undo/redo history through the load's shot UUID heal.
//!
//! A file saved before shot UUIDs were unique across the project can hold one
//! id in several sections. The load renames the repeats
//! (`Project::make_shot_uuids_project_unique`), but the persisted history was
//! recorded against the OLD ids: an entry that addresses `(section, old id)`
//! would no longer find its shot, and a snapshot restored from it would put the
//! repeats back. So right after the history is loaded, before
//! [`UndoStack::drop_entries_addressing_a_missing_shot`] judges what it can
//! address, every entry is rewritten with one map: the history then reads as if
//! those shots had always had their new ids.
//!
//! The map ([`history_remap`]) reuses the live renames verbatim and extends
//! them to shots only the history still holds (a deleted copy that an undo would
//! bring back). It is a function of the file alone, so independent loads of one
//! file - a reopen without saving, a SpeleoDB Unwind - agree.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::time::Instant;

use serde::Deserialize;

use crate::models::{
    remap_project_json, reminted_shot_uuid, reminted_shot_uuid_avoiding, LoopClosure, Project,
    SectionData, SectionId, ShotUuidRemap, StationAnchor, StationKey,
};

use super::{UndoData, UndoEntry, UndoStack};

impl UndoStack {
    /// Translate the loaded history through `renamed` (what this load's heal did
    /// to the live project), then drop the entries that still cannot address
    /// their shot. The one call both load paths make after
    /// [`Self::load_from_persisted_with_timestamps`]. Returns how many entries
    /// were rewritten.
    pub fn reconcile_with_loaded_project(
        &mut self,
        project: &Project,
        renamed: &ShotUuidRemap,
    ) -> usize {
        let translated = self.translate_shot_uuids(project, renamed);
        self.drop_entries_addressing_a_missing_shot(project);
        translated
    }

    /// Rewrite every entry's shot ids with [`history_remap`]. Returns how many
    /// entries changed; the byte totals are recounted when any did.
    ///
    /// One pass reads what the history holds; the common case (nothing to rename)
    /// returns there, so a history with no repeats costs one light probe of each
    /// snapshot and is left byte for byte.
    pub fn translate_shot_uuids(&mut self, project: &Project, renamed: &ShotUuidRemap) -> usize {
        let started = Instant::now();
        let mut ids = HistoryIds::default();
        for entry in self.undo_stack.iter_mut().chain(self.redo_stack.iter_mut()) {
            ids.collect(&mut entry.undo_data);
        }
        ids.order_reinstalls(&self.undo_stack, &self.redo_stack);
        let map = history_remap(project, renamed, &ids);
        if map.is_empty() {
            return 0;
        }

        let mut changed = 0;
        for entry in self.undo_stack.iter_mut().chain(self.redo_stack.iter_mut()) {
            if remap_entry(entry, &map) {
                changed += 1;
            }
        }
        self.undo_bytes = self.undo_stack.iter().map(Self::estimated_size).sum();
        self.redo_bytes = self.redo_stack.iter().map(Self::estimated_size).sum();
        tracing::info!(
            entries = changed,
            renamed = map.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            purpose = "keep the loaded history addressing its shots",
            "Translated undo/redo history through the shot UUID heal"
        );
        changed
    }
}

/// What the loaded history holds, gathered in one pass.
#[derive(Default)]
pub(super) struct HistoryIds {
    /// Every id the history mentions, with the sections it is mentioned in.
    holders: BTreeMap<String, BTreeSet<SectionId>>,
    /// Ids two sections hold in the SAME recorded state: one project snapshot,
    /// or the sections one entry reinstalls. Only these are repeats; an id that
    /// moved from one section to another (a split) is held by both only at
    /// different times, and is no repeat.
    repeats: Vec<(String, BTreeSet<SectionId>)>,
    /// Every shot an entry reinstalls whole (an Add's sections, a re-created
    /// or deleted section), with the project it lands in when that entry is
    /// replayed: `None` for the live project, or an index into `walls`.
    reinstalled: Vec<(SectionId, String, Option<usize>)>,
    /// Who holds each id in a whole-project state an entry restores (an undo's
    /// `before`, a redo's `after`): stepping through that entry replaces the
    /// project, so it is what every entry beyond it replays into (#732).
    walls: Vec<BTreeMap<String, BTreeSet<SectionId>>>,
}

/// The (section, id) of every shot `data` reinstalls whole.
fn reinstalled_ids(data: &UndoData) -> Vec<(SectionId, &str)> {
    match data {
        UndoData::AddedProjectContent(content) => content
            .sections
            .iter()
            .flat_map(|section| {
                section
                    .shots
                    .iter()
                    .filter_map(move |s| Some((section.id, s.uuid.as_deref()?)))
            })
            .collect(),
        UndoData::SectionSnapshot {
            section_id,
            section_data,
        } => section_data
            .shots
            .iter()
            .filter_map(|s| Some((*section_id, s.uuid.as_deref()?)))
            .collect(),
        UndoData::CreatedSectionSnapshot {
            section_id,
            section_data,
            identity,
        } => section_data
            .shots
            .iter()
            .filter_map(|s| s.uuid.as_deref())
            .chain(identity.shot_uuids.iter().flatten().map(String::as_str))
            .map(|uuid| (*section_id, uuid))
            .collect(),
        _ => Vec::new(),
    }
}

/// Who holds each id in one serialized project.
fn holders_in(json: &str) -> BTreeMap<String, BTreeSet<SectionId>> {
    let mut held: BTreeMap<String, BTreeSet<SectionId>> = BTreeMap::new();
    probe_snapshot(json, &mut |section, uuid| {
        held.entry(uuid.to_string()).or_default().insert(section);
    });
    held
}

impl HistoryIds {
    fn collect(&mut self, data: &mut UndoData) {
        visit_entry(data, &mut |section, uuid| {
            if let Some(uuid) = uuid.as_deref().filter(|u| !u.is_empty()) {
                self.holders
                    .entry(uuid.to_string())
                    .or_default()
                    .insert(section);
            }
            false
        });
        let mut state: BTreeMap<String, BTreeSet<SectionId>> = BTreeMap::new();
        let note =
            |state: &mut BTreeMap<String, BTreeSet<SectionId>>, section: SectionId, uuid: &str| {
                state.entry(uuid.to_string()).or_default().insert(section);
            };
        match data {
            UndoData::ProjectSnapshot(json) => {
                probe_snapshot(json, &mut |section, uuid| note(&mut state, section, uuid));
                self.close_state(&mut state);
            }
            UndoData::ProjectSnapshotPair { before, after } => {
                probe_snapshot(before, &mut |section, uuid| note(&mut state, section, uuid));
                self.close_state(&mut state);
                probe_snapshot(after, &mut |section, uuid| note(&mut state, section, uuid));
                self.close_state(&mut state);
            }
            other => {
                let reinstalled = reinstalled_ids(other);
                if !reinstalled.is_empty() {
                    for (section, uuid) in reinstalled {
                        note(&mut state, section, uuid);
                    }
                    self.close_state(&mut state);
                }
            }
        }
    }

    /// Record, for every shot an entry reinstalls, the project it lands in when
    /// that entry is replayed. Each stack is walked from the live end outwards
    /// (`back()` is the entry next to the live project): an entry replays into
    /// the live project unless a whole-project snapshot lies between them, in
    /// which case it replays into the nearest one's state - the `before` an
    /// undo restores, or the `after` a redo restores. Comparing against the
    /// live project instead mistakes a shot a later combine or split moved for
    /// a repeat (#732).
    fn order_reinstalls(&mut self, undo: &VecDeque<UndoEntry>, redo: &VecDeque<UndoEntry>) {
        for (stack, undoing) in [(undo, true), (redo, false)] {
            if !stack
                .iter()
                .any(|entry| !reinstalled_ids(&entry.undo_data).is_empty())
            {
                continue;
            }
            let mut wall: Option<usize> = None;
            for entry in stack.iter().rev() {
                for (section, uuid) in reinstalled_ids(&entry.undo_data) {
                    self.reinstalled.push((section, uuid.to_string(), wall));
                }
                let restores = match (&entry.undo_data, undoing) {
                    (UndoData::ProjectSnapshot(json), true)
                    | (UndoData::ProjectSnapshotPair { before: json, .. }, true)
                    | (UndoData::ProjectSnapshotPair { after: json, .. }, false) => Some(json),
                    _ => None,
                };
                if let Some(json) = restores {
                    self.walls.push(holders_in(json));
                    wall = Some(self.walls.len() - 1);
                }
            }
        }
    }

    /// Fold one recorded state into the totals and clear it for the next.
    fn close_state(&mut self, state: &mut BTreeMap<String, BTreeSet<SectionId>>) {
        for (uuid, sections) in std::mem::take(state) {
            let holders = self.holders.entry(uuid.clone()).or_default();
            holders.extend(sections.iter().copied());
            if sections.len() > 1 {
                self.repeats.push((uuid, sections));
            }
        }
    }
}

/// The renames the loaded history needs, given the healed live project and what
/// the history holds. For each (section S, id X) the history mentions:
///
/// 1. the live heal renamed it -> that new id, verbatim;
/// 2. X is live in S -> unchanged;
/// 3. the derived replacement for X is what S holds, live or elsewhere in the
///    history (history saved against an earlier heal) -> that;
/// 4. S must yield X: another section holds X in the same recorded state and
///    keeps it (the one holding X live, else the lowest), or S is a section an
///    entry reinstalls while another section holds X in the project that entry
///    replays into (see [`HistoryIds::order_reinstalls`]) -> a derived
///    replacement, stepped past every id in use;
/// 5. otherwise unchanged - including an id that only MOVED between sections,
///    such as a shot a combine or a split moved out of a section an older entry
///    created or added: the move's own snapshot is what that entry replays
///    into, and it holds the shot in that section alone (#732).
pub(super) fn history_remap(
    project: &Project,
    renamed: &ShotUuidRemap,
    ids: &HistoryIds,
) -> ShotUuidRemap {
    let live = project.shot_uuids_by_section();
    let live_holder: HashMap<&str, SectionId> = live
        .iter()
        .flat_map(|(section, held)| held.iter().map(move |id| (id.as_str(), *section)))
        .collect();

    let mut yields: BTreeSet<(SectionId, &str)> = BTreeSet::new();
    for (uuid, sections) in &ids.repeats {
        let keeper = live_holder
            .get(uuid.as_str())
            .copied()
            .filter(|holder| sections.contains(holder))
            .or_else(|| sections.iter().next().copied());
        for &section in sections.iter().filter(|&&s| Some(s) != keeper) {
            yields.insert((section, uuid.as_str()));
        }
    }
    for (section, uuid, wall) in &ids.reinstalled {
        let elsewhere = match wall.and_then(|i| ids.walls.get(i)) {
            Some(held) => held
                .get(uuid)
                .is_some_and(|sections| sections.iter().any(|s| s != section)),
            None => live_holder
                .get(uuid.as_str())
                .is_some_and(|&holder| holder != *section),
        };
        if elsewhere {
            yields.insert((*section, uuid.as_str()));
        }
    }

    let mut in_use: Option<HashSet<String>> = None;
    let mut map = ShotUuidRemap::default();
    for (uuid, sections) in &ids.holders {
        for &section in sections {
            if let Some(new) = renamed.get(section, uuid) {
                map.insert(section, uuid.clone(), new.to_string());
                continue;
            }
            let live_here = live.get(&section);
            if live_here.is_some_and(|held| held.contains(uuid)) {
                continue;
            }
            let derived = reminted_shot_uuid(section, uuid);
            let derived_is_here = live_here.is_some_and(|held| held.contains(&derived))
                || ids
                    .holders
                    .get(&derived)
                    .is_some_and(|holders| holders.contains(&section));
            if derived_is_here {
                map.insert(section, uuid.clone(), derived);
                continue;
            }
            if yields.contains(&(section, uuid.as_str())) {
                let in_use = in_use.get_or_insert_with(|| {
                    live.values()
                        .flatten()
                        .cloned()
                        .chain(ids.holders.keys().cloned())
                        .collect()
                });
                let new = reminted_shot_uuid_avoiding(section, uuid, |c| in_use.contains(c));
                in_use.insert(new.clone());
                map.insert(section, uuid.clone(), new);
            }
        }
    }
    map
}

/// Apply `map` to one entry: its typed fields, then any project snapshot.
fn remap_entry(entry: &mut UndoEntry, map: &ShotUuidRemap) -> bool {
    let mut changed = visit_entry(&mut entry.undo_data, &mut |section, uuid| {
        map.rewrite_uuid(section, uuid)
    });
    match &mut entry.undo_data {
        UndoData::ProjectSnapshot(json) => changed |= remap_snapshot(json, map),
        UndoData::ProjectSnapshotPair { before, after } => {
            changed |= remap_snapshot(before, map);
            changed |= remap_snapshot(after, map);
        }
        _ => {}
    }
    changed
}

/// Rewrite a serialized project snapshot in place, through `serde_json::Value`
/// rather than `Project` (see `remap_project_json`). A snapshot that does not
/// parse is left as it is: the restore reports it, as it always has.
fn remap_snapshot(json: &mut String, map: &ShotUuidRemap) -> bool {
    let touches = {
        let mut hit = false;
        probe_snapshot(json, &mut |section, uuid| {
            hit |= map.get(section, uuid).is_some();
        });
        hit
    };
    if !touches {
        return false;
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    let rename = |section: SectionId, old: &str| map.get(section, old).map(str::to_string);
    if !remap_project_json(&mut value, &rename) {
        return false;
    }
    match serde_json::to_string(&value) {
        Ok(rewritten) => {
            *json = rewritten;
            true
        }
        Err(_) => false,
    }
}

/// The part of a serialized project a probe needs: every section's id and shot
/// ids. Everything else is skipped while parsing, so probing a history of
/// whole-project snapshots costs a scan, not a rebuild of each project.
#[derive(Deserialize)]
struct SnapshotProbe {
    #[serde(default)]
    sections: Vec<ProbeSection>,
}

#[derive(Deserialize)]
struct ProbeSection {
    id: SectionId,
    #[serde(default)]
    shots: Vec<ProbeShot>,
}

#[derive(Deserialize)]
struct ProbeShot {
    #[serde(default)]
    uuid: Option<String>,
}

fn probe_snapshot(json: &str, found: &mut dyn FnMut(SectionId, &str)) {
    let Ok(probe) = serde_json::from_str::<SnapshotProbe>(json) else {
        return;
    };
    for section in &probe.sections {
        for uuid in section.shots.iter().filter_map(|s| s.uuid.as_deref()) {
            if !uuid.is_empty() {
                found(section.id, uuid);
            }
        }
    }
}

/// Visit every shot id an entry holds OUTSIDE a project snapshot, with the
/// section it belongs to. `f` returns whether it changed the id; the visit
/// returns whether any call did.
///
/// Exhaustive on purpose (no `_ =>`): a new variant that carries a shot id has
/// to say here how it is translated, or the build fails. Where a variant holds
/// one shot's id twice, both copies are visited - `Shot::with_uuid` lets
/// `shot_data.uuid` win over the recorded `uuid`, and
/// `Section::restore_with_identity` lets `identity.shot_uuids` win over
/// `section_data`, so translating only one copy would restore the old id.
fn visit_entry(
    data: &mut UndoData,
    f: &mut dyn FnMut(SectionId, &mut Option<String>) -> bool,
) -> bool {
    let mut changed = false;
    match data {
        // A `Command` holds shot ids only in `CreateSection` / `CreateShot`,
        // which carry no section for `CreateSection` and are stripped of ids
        // whenever they execute: inert, so not translated.
        UndoData::InverseCommand(_)
        | UndoData::ZeroDatumInfo { .. }
        | UndoData::SegmentMoveInfo { .. }
        | UndoData::ProjectSnapshot(_)
        | UndoData::ProjectSnapshotPair { .. }
        | UndoData::None => {}
        UndoData::PreviousValues {
            section_id,
            shot_uuid,
            ..
        } => changed |= f(*section_id, shot_uuid),
        UndoData::SectionSnapshot {
            section_id,
            section_data,
        } => changed |= visit_section_data(*section_id, section_data, f),
        UndoData::CreatedSectionSnapshot {
            section_id,
            section_data,
            identity,
        } => {
            changed |= visit_section_data(*section_id, section_data, f);
            for uuid in &mut identity.shot_uuids {
                changed |= f(*section_id, uuid);
            }
            changed |= f(*section_id, &mut identity.gauge_shot_uuid);
            changed |= f(*section_id, &mut identity.gauge_neighbour_uuid);
        }
        UndoData::ShotSnapshot {
            section_id,
            shot_data,
            uuid,
            ..
        } => {
            changed |= f(*section_id, &mut shot_data.uuid);
            changed |= f(*section_id, uuid);
        }
        UndoData::CreatedShotSnapshot {
            section_id,
            shot_data,
            after_shot_uuid,
            before_shot_uuid,
            uuid,
            ..
        } => {
            changed |= f(*section_id, &mut shot_data.uuid);
            changed |= f(*section_id, after_shot_uuid);
            changed |= f(*section_id, before_shot_uuid);
            changed |= f(*section_id, uuid);
        }
        UndoData::MergeInfo {
            removed_closures, ..
        }
        | UndoData::UnmergeInfo {
            removed_closures, ..
        } => {
            for closure in removed_closures {
                changed |= visit_closure(closure, f);
            }
        }
        UndoData::ClosureInfo { closure, .. } => changed |= visit_closure(closure, f),
        UndoData::StationMoveInfo {
            station_ref,
            prev_shot_before,
            prev_shot_after,
            ..
        } => {
            // Inert (the restore copies geometry by shot id), translated anyway
            // so no copy of an old id outlives the heal.
            for (_, shot_data) in prev_shot_before
                .iter_mut()
                .chain(prev_shot_after.iter_mut())
            {
                changed |= f(station_ref.section_id, &mut shot_data.uuid);
            }
        }
        UndoData::ShotCredits { changes, .. } => {
            for change in changes {
                let mut uuid = Some(std::mem::take(&mut change.shot_uuid));
                changed |= f(change.section_id, &mut uuid);
                change.shot_uuid = uuid.unwrap_or_default();
            }
        }
        UndoData::AddedProjectContent(content) => {
            for section in &mut content.sections {
                let id = section.id;
                for shot in &mut section.shots {
                    changed |= f(id, &mut shot.uuid);
                }
                changed |= f(id, &mut section.gauge_shot_uuid);
                changed |= f(id, &mut section.gauge_neighbour_uuid);
            }
            for (key, _) in &mut content.station_offsets {
                changed |= visit_key(key, f);
            }
            for (key, _) in &mut content.station_comments {
                changed |= visit_key(key, f);
            }
            for (key, _) in &mut content.station_names {
                changed |= visit_key(key, f);
            }
            for datum in &mut content.zero_datums {
                changed |= visit_key(&mut datum.station_ref, f);
            }
            for merge in &mut content.station_merges {
                changed |= visit_key(&mut merge.station_a, f);
                changed |= visit_key(&mut merge.station_b, f);
            }
            for closure in &mut content.loop_closures {
                changed |= visit_closure(closure, f);
            }
            for ignore in &mut content.ignores {
                let mut slot = Some(std::mem::take(&mut ignore.shot_uuid));
                changed |= f(ignore.section_id, &mut slot);
                ignore.shot_uuid = slot.unwrap_or_default();
            }
        }
    }
    changed
}

fn visit_section_data(
    section_id: SectionId,
    data: &mut SectionData,
    f: &mut dyn FnMut(SectionId, &mut Option<String>) -> bool,
) -> bool {
    let mut changed = false;
    for shot in &mut data.shots {
        changed |= f(section_id, &mut shot.uuid);
    }
    changed
}

fn visit_closure(
    closure: &mut LoopClosure,
    f: &mut dyn FnMut(SectionId, &mut Option<String>) -> bool,
) -> bool {
    let a = visit_key(&mut closure.station_a, f);
    let b = visit_key(&mut closure.station_b, f);
    a || b
}

fn visit_key(
    key: &mut StationKey,
    f: &mut dyn FnMut(SectionId, &mut Option<String>) -> bool,
) -> bool {
    let StationAnchor::AfterShot(uuid) = &mut key.anchor else {
        return false;
    };
    let mut slot = Some(std::mem::take(uuid));
    let changed = f(key.section_id, &mut slot);
    *uuid = slot.unwrap_or_default();
    changed
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
#[path = "uuid_remap_tests.rs"]
mod tests;
