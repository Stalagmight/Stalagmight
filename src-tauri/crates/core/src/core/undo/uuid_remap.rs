//! Translating a loaded undo/redo history through the load's shot UUID heal.
//!
//! A file saved before shot UUIDs were unique across the project can hold one
//! id in several lines. The load renames the repeats
//! (`Project::make_shot_uuids_project_unique`), but the persisted history was
//! recorded against the OLD ids: an entry that addresses `(line, old id)`
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
    remap_project_json, reminted_shot_uuid, reminted_shot_uuid_avoiding, LineData, LineId,
    LoopClosure, Project, ShotUuidRemap, StationAnchor, StationKey,
};

use super::history_walk::HolderWalk;
use super::{HistoryDropped, UndoData, UndoEntry, UndoStack};

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
    ) -> (usize, Option<HistoryDropped>) {
        if let Some(dropped) = self.drop_history_that_does_not_fit(project) {
            return (0, Some(dropped));
        }
        let translated = self.translate_shot_uuids(project, renamed);
        self.drop_entries_addressing_a_missing_shot(project);
        (translated, None)
    }

    /// Drop the whole history when a compact entry in it does not fit the
    /// state the entries above it leave: its recorded line count, a line
    /// it edits by id, a run past the end. Every row parsed and passed
    /// `check_shape`, so the file's rows are well-formed - but such an entry
    /// would refuse every step for good, and the entries under it with it. A
    /// compact history is usable only whole, so it goes whole, with the banner
    /// a damaged history gets (`HistoryDropped::Unreadable`).
    ///
    /// Judged by walking each stack from the loaded project, the walk the uuid
    /// remap takes. A walk that loses track at a TYPED entry proves nothing
    /// about the deltas under it, and keeps the history.
    fn drop_history_that_does_not_fit(&mut self, project: &Project) -> Option<HistoryDropped> {
        let mut unfit = 0;
        for (stack, undoing) in [(&self.undo_stack, true), (&self.redo_stack, false)] {
            let mut walk = HolderWalk::of(project);
            for entry in stack.iter().rev() {
                if !walk.known {
                    break;
                }
                walk.step(entry, undoing);
            }
            unfit += usize::from(walk.unfit_delta);
        }
        if unfit == 0 {
            return None;
        }
        let rows = self.undo_stack.len() + self.redo_stack.len();
        tracing::warn!(
            rows,
            purpose = "load persisted undo history",
            "A compact undo entry does not fit the project it would be applied to; the whole \
             history is dropped rather than wedged at that entry"
        );
        self.clear();
        Some(HistoryDropped::Unreadable)
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
        ids.walk_recorded_states(project, &self.undo_stack, &self.redo_stack);
        let map = history_remap(project, renamed, &ids);
        if map.is_empty() {
            return 0;
        }

        let mut changed = 0;
        let rename = |line: LineId, old: &str| map.get(line, old).map(str::to_string);
        // Walked from the live project outwards again, so a compact entry's
        // copies can be renamed against the state it applies to (see
        // `ProjectDelta::rename_copies`).
        for (stack, undoing) in [(&mut self.undo_stack, true), (&mut self.redo_stack, false)] {
            let mut walk = HolderWalk::of(project);
            for entry in stack.iter_mut().rev() {
                let mut touched = remap_entry(entry, &map);
                if let UndoData::Delta(delta) = &mut entry.undo_data {
                    if walk.known {
                        touched |= delta.delta.rename_copies(walk.view(), &rename);
                    }
                }
                if touched {
                    changed += 1;
                }
                walk.step(entry, undoing);
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
    /// Every id the history mentions, with the lines it is mentioned in.
    holders: BTreeMap<String, BTreeSet<LineId>>,
    /// Ids two lines hold in the SAME recorded state: one project snapshot,
    /// or the lines one entry reinstalls. Only these are repeats; an id that
    /// moved from one line to another (a split) is held by both only at
    /// different times, and is no repeat.
    repeats: Vec<(String, BTreeSet<LineId>)>,
    /// Every shot an entry reinstalls whole (an Add's lines, a re-created
    /// line) while ANOTHER line holds its id in the project that entry
    /// replays into - the state the walk from the live project has reached by
    /// then ([`HistoryIds::walk_recorded_states`], #732). Each must yield.
    reinstall_yields: Vec<(LineId, String)>,
}

/// The (line, id) of every shot `data` reinstalls whole. A compact
/// [`UndoData::Delta`] is not here: the walk steps through it to the whole
/// state it records ([`HistoryIds::walk_recorded_states`]).
fn reinstalled_ids(data: &UndoData) -> Vec<(LineId, &str)> {
    match data {
        UndoData::AddedProjectContent(content) => content
            .lines
            .iter()
            .flat_map(|line| {
                line.shots
                    .iter()
                    .filter_map(move |s| Some((line.id, s.uuid.as_deref()?)))
            })
            .collect(),
        UndoData::CreatedLineSnapshot {
            line_id,
            line_data,
            identity,
        } => line_data
            .shots
            .iter()
            .filter_map(|s| s.uuid.as_deref())
            .chain(identity.shot_uuids.iter().flatten().map(String::as_str))
            .map(|uuid| (*line_id, uuid))
            .collect(),
        _ => Vec::new(),
    }
}

impl HistoryIds {
    fn collect(&mut self, data: &mut UndoData) {
        visit_entry(data, &mut |line, uuid| {
            if let Some(uuid) = uuid.as_deref().filter(|u| !u.is_empty()) {
                self.holders
                    .entry(uuid.to_string())
                    .or_default()
                    .insert(line);
            }
            false
        });
        let mut state: BTreeMap<String, BTreeSet<LineId>> = BTreeMap::new();
        let note = |state: &mut BTreeMap<String, BTreeSet<LineId>>, line: LineId, uuid: &str| {
            state.entry(uuid.to_string()).or_default().insert(line);
        };
        match data {
            UndoData::ProjectSnapshotPair { before, after } => {
                probe_snapshot(before, &mut |line, uuid| note(&mut state, line, uuid));
                self.close_state(&mut state);
                probe_snapshot(after, &mut |line, uuid| note(&mut state, line, uuid));
                self.close_state(&mut state);
            }
            other => {
                let reinstalled = reinstalled_ids(other);
                if !reinstalled.is_empty() {
                    for (line, uuid) in reinstalled {
                        note(&mut state, line, uuid);
                    }
                    self.close_state(&mut state);
                }
            }
        }
    }

    /// Walk each stack from the live project outwards, stepping a holder view
    /// ([`HolderWalk`]) through every entry the way undo (or redo) steps the
    /// project. Along the way:
    ///
    /// - every shot an entry reinstalls is judged against the state the walk
    ///   has reached - the project that entry replays into - and must yield
    ///   when another line holds its id there. Judging it against the live
    ///   project instead mistakes a shot a later combine or split moved for a
    ///   repeat (#732);
    /// - every state a compact delta records (its other side) is folded in
    ///   like a snapshot half: the ids it holds, and its repeats.
    ///
    /// A step the walk cannot follow leaves the rest of that stack judged
    /// against the live project.
    fn walk_recorded_states(
        &mut self,
        project: &Project,
        undo: &VecDeque<UndoEntry>,
        redo: &VecDeque<UndoEntry>,
    ) {
        let live = HolderWalk::of(project);
        for (stack, undoing) in [(undo, true), (redo, false)] {
            let mut walk = HolderWalk::of(project);
            for entry in stack.iter().rev() {
                let judge = if walk.known { &walk } else { &live };
                for (line, uuid) in reinstalled_ids(&entry.undo_data) {
                    if judge.held_elsewhere(line, uuid) {
                        self.reinstall_yields.push((line, uuid.to_string()));
                    }
                }
                let Some(changed) = walk.step(entry, undoing) else {
                    continue;
                };
                for (line, shots) in &changed {
                    for uuid in shots.iter().flatten().filter(|u| !u.is_empty()) {
                        self.holders
                            .entry(uuid.to_string())
                            .or_default()
                            .insert(*line);
                        let held = walk.holders_of(uuid);
                        if held.len() > 1 {
                            self.repeats
                                .push((uuid.to_string(), held.into_iter().collect()));
                        }
                    }
                }
            }
        }
    }

    /// Fold one recorded state into the totals and clear it for the next.
    fn close_state(&mut self, state: &mut BTreeMap<String, BTreeSet<LineId>>) {
        for (uuid, lines) in std::mem::take(state) {
            let holders = self.holders.entry(uuid.clone()).or_default();
            holders.extend(lines.iter().copied());
            if lines.len() > 1 {
                self.repeats.push((uuid, lines));
            }
        }
    }
}

/// The renames the loaded history needs, given the healed live project and what
/// the history holds. For each (line S, id X) the history mentions:
///
/// 1. the live heal renamed it -> that new id, verbatim;
/// 2. X is live in S -> unchanged;
/// 3. the derived replacement for X is what S holds, live or elsewhere in the
///    history (history saved against an earlier heal) -> that;
/// 4. S must yield X: another line holds X in the same recorded state and
///    keeps it (the one holding X live, else the lowest), or S is a line an
///    entry reinstalls while another line holds X in the project that entry
///    replays into (see [`HistoryIds::walk_recorded_states`]) -> a derived
///    replacement, stepped past every id in use;
/// 5. otherwise unchanged - including an id that only MOVED between lines,
///    such as a shot a combine or a split moved out of a line an older entry
///    created or added: the move's own snapshot is what that entry replays
///    into, and it holds the shot in that line alone (#732).
pub(super) fn history_remap(
    project: &Project,
    renamed: &ShotUuidRemap,
    ids: &HistoryIds,
) -> ShotUuidRemap {
    let live = project.shot_uuids_by_line();
    let live_holder: HashMap<&str, LineId> = live
        .iter()
        .flat_map(|(line, held)| held.iter().map(move |id| (id.as_str(), *line)))
        .collect();

    let mut yields: BTreeSet<(LineId, &str)> = BTreeSet::new();
    for (uuid, lines) in &ids.repeats {
        let keeper = live_holder
            .get(uuid.as_str())
            .copied()
            .filter(|holder| lines.contains(holder))
            .or_else(|| lines.iter().next().copied());
        for &line in lines.iter().filter(|&&s| Some(s) != keeper) {
            yields.insert((line, uuid.as_str()));
        }
    }
    for (line, uuid) in &ids.reinstall_yields {
        yields.insert((*line, uuid.as_str()));
    }

    let mut in_use: Option<HashSet<String>> = None;
    let mut map = ShotUuidRemap::default();
    for (uuid, lines) in &ids.holders {
        for &line in lines {
            if let Some(new) = renamed.get(line, uuid) {
                map.insert(line, uuid.clone(), new.to_string());
                continue;
            }
            let live_here = live.get(&line);
            if live_here.is_some_and(|held| held.contains(uuid)) {
                continue;
            }
            let derived = reminted_shot_uuid(line, uuid);
            let derived_is_here = live_here.is_some_and(|held| held.contains(&derived))
                || ids
                    .holders
                    .get(&derived)
                    .is_some_and(|holders| holders.contains(&line));
            if derived_is_here {
                map.insert(line, uuid.clone(), derived);
                continue;
            }
            if yields.contains(&(line, uuid.as_str())) {
                let in_use = in_use.get_or_insert_with(|| {
                    live.values()
                        .flatten()
                        .cloned()
                        .chain(ids.holders.keys().cloned())
                        .collect()
                });
                let new = reminted_shot_uuid_avoiding(line, uuid, |c| in_use.contains(c));
                in_use.insert(new.clone());
                map.insert(line, uuid.clone(), new);
            }
        }
    }
    map
}

/// Apply `map` to one entry: its typed fields, then any project snapshot.
fn remap_entry(entry: &mut UndoEntry, map: &ShotUuidRemap) -> bool {
    let mut changed = visit_entry(&mut entry.undo_data, &mut |line, uuid| {
        map.rewrite_uuid(line, uuid)
    });
    match &mut entry.undo_data {
        UndoData::ProjectSnapshotPair { before, after } => {
            changed |= remap_snapshot(before, map);
            changed |= remap_snapshot(after, map);
        }
        UndoData::Delta(delta) => {
            let rename = |line: LineId, old: &str| map.get(line, old).map(str::to_string);
            changed |= delta.delta.remap(&rename);
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
        probe_snapshot(json, &mut |line, uuid| {
            hit |= map.get(line, uuid).is_some();
        });
        hit
    };
    if !touches {
        return false;
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    let rename = |line: LineId, old: &str| map.get(line, old).map(str::to_string);
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

/// The part of a serialized project a probe needs: every line's id and shot
/// ids. Everything else is skipped while parsing, so probing a history of
/// whole-project snapshots costs a scan, not a rebuild of each project.
#[derive(Deserialize)]
struct SnapshotProbe {
    #[serde(default)]
    lines: Vec<ProbeLine>,
}

#[derive(Deserialize)]
struct ProbeLine {
    id: LineId,
    #[serde(default)]
    shots: Vec<ProbeShot>,
}

#[derive(Deserialize)]
struct ProbeShot {
    #[serde(default)]
    uuid: Option<String>,
}

fn probe_snapshot(json: &str, found: &mut dyn FnMut(LineId, &str)) {
    let Ok(probe) = serde_json::from_str::<SnapshotProbe>(json) else {
        return;
    };
    for line in &probe.lines {
        for uuid in line.shots.iter().filter_map(|s| s.uuid.as_deref()) {
            if !uuid.is_empty() {
                found(line.id, uuid);
            }
        }
    }
}

/// Visit every shot id an entry holds OUTSIDE a project snapshot, with the
/// line it belongs to. `f` returns whether it changed the id; the visit
/// returns whether any call did.
///
/// Exhaustive on purpose (no `_ =>`): a new variant that carries a shot id has
/// to say here how it is translated, or the build fails. Where a variant holds
/// one shot's id twice, both copies are visited - `Shot::with_uuid` lets
/// `shot_data.uuid` win over the recorded `uuid`, and
/// `Line::restore_with_identity` lets `identity.shot_uuids` win over
/// `line_data`, so translating only one copy would restore the old id.
fn visit_entry(
    data: &mut UndoData,
    f: &mut dyn FnMut(LineId, &mut Option<String>) -> bool,
) -> bool {
    let mut changed = false;
    match data {
        // A `Command` holds shot ids only in `CreateLine` / `CreateShot`,
        // which carry no line for `CreateLine` and are stripped of ids
        // whenever they execute: inert, so not translated.
        UndoData::InverseCommand(_)
        | UndoData::ZeroDatumInfo { .. }
        | UndoData::SegmentMoveInfo { .. }
        | UndoData::ProjectSnapshotPair { .. }
        // Section memberships and colours, keyed by line: no shot id.
        | UndoData::CaveSections(_)
        // Rewritten as a whole by `remap_entry`, like a snapshot.
        | UndoData::Delta(_)
        | UndoData::None => {}
        UndoData::PreviousValues {
            line_id, shot_uuid, ..
        } => changed |= f(*line_id, shot_uuid),
        UndoData::CreatedLineSnapshot {
            line_id,
            line_data,
            identity,
        } => {
            changed |= visit_line_data(*line_id, line_data, f);
            for uuid in &mut identity.shot_uuids {
                changed |= f(*line_id, uuid);
            }
            changed |= f(*line_id, &mut identity.gauge_shot_uuid);
            changed |= f(*line_id, &mut identity.gauge_neighbour_uuid);
        }
        UndoData::CreatedShotSnapshot {
            line_id,
            shot_data,
            after_shot_uuid,
            before_shot_uuid,
            uuid,
            ..
        } => {
            changed |= f(*line_id, &mut shot_data.uuid);
            changed |= f(*line_id, after_shot_uuid);
            changed |= f(*line_id, before_shot_uuid);
            changed |= f(*line_id, uuid);
        }
        UndoData::ClosureInfo { closure, .. } => changed |= visit_closure(closure, f),
        UndoData::ShotCredits { changes, .. } => {
            for change in changes {
                let mut uuid = Some(std::mem::take(&mut change.shot_uuid));
                changed |= f(change.line_id, &mut uuid);
                change.shot_uuid = uuid.unwrap_or_default();
            }
        }
        UndoData::AddedProjectContent(content) => {
            for line in &mut content.lines {
                let id = line.id;
                for shot in &mut line.shots {
                    changed |= f(id, &mut shot.uuid);
                }
                changed |= f(id, &mut line.gauge_shot_uuid);
                changed |= f(id, &mut line.gauge_neighbour_uuid);
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
            for (key, _) in &mut content.station_ids {
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
                changed |= f(ignore.line_id, &mut slot);
                ignore.shot_uuid = slot.unwrap_or_default();
            }
        }
    }
    changed
}

fn visit_line_data(
    line_id: LineId,
    data: &mut LineData,
    f: &mut dyn FnMut(LineId, &mut Option<String>) -> bool,
) -> bool {
    let mut changed = false;
    for shot in &mut data.shots {
        changed |= f(line_id, &mut shot.uuid);
    }
    changed
}

fn visit_closure(
    closure: &mut LoopClosure,
    f: &mut dyn FnMut(LineId, &mut Option<String>) -> bool,
) -> bool {
    let a = visit_key(&mut closure.station_a, f);
    let b = visit_key(&mut closure.station_b, f);
    a || b
}

fn visit_key(key: &mut StationKey, f: &mut dyn FnMut(LineId, &mut Option<String>) -> bool) -> bool {
    let StationAnchor::AfterShot(uuid) = &mut key.anchor else {
        return false;
    };
    let mut slot = Some(std::mem::take(uuid));
    let changed = f(key.line_id, &mut slot);
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
