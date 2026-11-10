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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    pub fn translate_shot_uuids(&mut self, project: &Project, renamed: &ShotUuidRemap) -> usize {
        let started = Instant::now();
        let mut holders: BTreeMap<String, BTreeSet<SectionId>> = BTreeMap::new();
        for entry in self.undo_stack.iter_mut().chain(self.redo_stack.iter_mut()) {
            visit_entry(&mut entry.undo_data, &mut |section, uuid| {
                if let Some(uuid) = uuid.as_deref().filter(|u| !u.is_empty()) {
                    holders.entry(uuid.to_string()).or_default().insert(section);
                }
                false
            });
            visit_snapshots(&entry.undo_data, &mut |section, uuid| {
                holders.entry(uuid.to_string()).or_default().insert(section);
            });
        }
        let map = history_remap(project, renamed, &holders);
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

/// The renames the loaded history needs, given the healed live project and
/// every `(id -> sections holding it)` the history mentions. For each
/// (section S, id X) the history holds:
///
/// 1. the live heal renamed it -> that new id, verbatim;
/// 2. X is live in S -> unchanged;
/// 3. the derived replacement for X is live in S -> that (history saved
///    against an older heal);
/// 4. X is held live by another section, or by a lower section elsewhere in the
///    history -> a derived replacement, stepped past every id in use;
/// 5. otherwise unchanged.
///
/// The same id never ends up in two sections of one recorded state: rule 4
/// renames every holder but the one that keeps it.
pub(super) fn history_remap(
    project: &Project,
    renamed: &ShotUuidRemap,
    holders: &BTreeMap<String, BTreeSet<SectionId>>,
) -> ShotUuidRemap {
    let live = project.shot_uuids_by_section();
    let live_holder: HashMap<&str, SectionId> = live
        .iter()
        .flat_map(|(section, ids)| ids.iter().map(move |id| (id.as_str(), *section)))
        .collect();
    let mut in_use: HashSet<String> = live.values().flatten().cloned().collect();
    in_use.extend(holders.keys().cloned());

    let mut map = ShotUuidRemap::default();
    for (uuid, sections) in holders {
        let lowest = sections.iter().next().copied();
        for &section in sections {
            if let Some(new) = renamed.get(section, uuid) {
                map.insert(section, uuid.clone(), new.to_string());
                continue;
            }
            let live_here = live.get(&section);
            if live_here.is_some_and(|ids| ids.contains(uuid)) {
                continue;
            }
            let derived = reminted_shot_uuid(section, uuid);
            if live_here.is_some_and(|ids| ids.contains(&derived)) {
                map.insert(section, uuid.clone(), derived);
                continue;
            }
            let held_live_elsewhere = live_holder
                .get(uuid.as_str())
                .is_some_and(|&holder| holder != section);
            let lower_holder_in_history = lowest.is_some_and(|first| first < section);
            if held_live_elsewhere || lower_holder_in_history {
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

fn visit_snapshots(data: &UndoData, found: &mut dyn FnMut(SectionId, &str)) {
    match data {
        UndoData::ProjectSnapshot(json) => probe_snapshot(json, found),
        UndoData::ProjectSnapshotPair { before, after } => {
            probe_snapshot(before, found);
            probe_snapshot(after, found);
        }
        _ => {}
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
