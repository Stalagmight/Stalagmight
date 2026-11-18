//! Project-wide shot UUID uniqueness.
//!
//! A shot's `uuid` is its persistent identity: station cross-references anchor to
//! it (`StationAnchor::AfterShot`), the Ariane export emits it as `<UUID>`, and
//! undo entries address shots by it. It used to be unique only within a line,
//! because an Ariane `<UUID>` is carried verbatim (#423) and merge / Add .stalx
//! keep their sources' ids, so importing one file twice, or merging two projects
//! that share an ancestor, put the same id into two lines.
//!
//! It is now unique across the whole project:
//!
//! * the doors that bring foreign ids in (an import, `create_line_and_merge`,
//!   merge, Add .stalx) call [`Project::reassign_repeated_shot_uuids`] for the
//!   lines they created, so the project already holding an id keeps it;
//! * a file saved before this, which can hold repeats, is healed on load by
//!   [`Project::make_shot_uuids_project_unique`], and the load translates the
//!   persisted undo history through the same renames so every entry still
//!   addresses the shot it was recorded against.
//!
//! The replacement id is DERIVED, not random ([`reminted_shot_uuid`]): it is a
//! function of the line and the id it replaces. A file that is opened and
//! never saved therefore reads to the same ids every time, two independent loads
//! of one revision agree (SpeleoDB Unwind), and a key stored against the old id
//! elsewhere - an ignored warning, a dive-plan anchor - can still be recognised
//! ([`stored_shot_uuid_matches`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{Line, LineId, StationAnchor, StationKey};

use super::Project;

/// Namespace of the derived replacement ids. Changing it changes every derived
/// id, so a file healed by one build would be healed differently by the next.
pub const SHOT_UUID_REMINT_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x3c2a_91d4_5e7b_4f06_a1c8_6d0e_9b47_2f15);

/// The id a repeated shot gets in `line_id` in place of `old`.
///
/// A v5 hash of the line and the old id, re-stamped as a v4: the SpeleoDB
/// sidecar keeps only v4 values (`speleodb_uuid`), and the version nibble is all
/// that distinguishes the two. The old id is length-prefixed so no pair of
/// (line, id) inputs can produce the same hash input.
pub fn reminted_shot_uuid(line_id: LineId, old: &str) -> String {
    remint_attempt(line_id, old, 0)
}

/// [`reminted_shot_uuid`], stepped past any id `taken` reports as in use. The
/// first attempt is the plain derived id; a collision with a 122-bit hash is not
/// a practical case, so stepping exists only to keep uniqueness unconditional.
pub fn reminted_shot_uuid_avoiding(
    line_id: LineId,
    old: &str,
    taken: impl Fn(&str) -> bool,
) -> String {
    (0..u32::MAX)
        .map(|attempt| remint_attempt(line_id, old, attempt))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| remint_attempt(line_id, old, 0))
}

fn remint_attempt(line_id: LineId, old: &str, attempt: u32) -> String {
    let name = if attempt == 0 {
        format!("{line_id}:{}:{old}", old.len())
    } else {
        format!("{line_id}:{}:{old}#{attempt}", old.len())
    };
    let hashed = uuid::Uuid::new_v5(&SHOT_UUID_REMINT_NAMESPACE, name.as_bytes());
    uuid::Builder::from_random_bytes(*hashed.as_bytes())
        .into_uuid()
        .to_string()
}

/// Whether an id stored against `line_id` somewhere outside the project (an
/// ignored warning, a dive-plan anchor) names the live shot whose id is `live`:
/// either it is that id, or the shot was renamed from it by a uniqueness heal.
pub fn stored_shot_uuid_matches(line_id: LineId, stored: &str, live: &str) -> bool {
    stored == live || reminted_shot_uuid(line_id, stored) == live
}

/// The live id that a key stored against `stored` in `line_id` names, given
/// `holds` (whether the line holds a shot with that id now): `stored` itself,
/// else the derived replacement a uniqueness heal gave that shot, else `None`.
///
/// THE one rule every key stored outside the project follows (ignored warnings,
/// dive-plan anchors, an Add's carried ignores). It tries the FIRST derived
/// candidate only: a replacement stepped past an id already in use exists only
/// when a file holds a derived value itself, and then the key names whichever
/// shot holds that value - a shot the same file could have named directly.
pub fn live_shot_uuid_for_stored(
    line_id: LineId,
    stored: &str,
    holds: impl Fn(&str) -> bool,
) -> Option<String> {
    if holds(stored) {
        return Some(stored.to_string());
    }
    let renamed = reminted_shot_uuid(line_id, stored);
    holds(&renamed).then_some(renamed)
}

/// Renames of shot ids, keyed by the line that held the old id. A key is the
/// (line, old id) pair because the same old id can stay in one line and be
/// renamed in another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShotUuidRemap(BTreeMap<(LineId, String), String>);

impl ShotUuidRemap {
    /// The new id of `old` in `line_id`, or `None` when it was not renamed.
    pub fn get(&self, line_id: LineId, old: &str) -> Option<&str> {
        self.0.get(&(line_id, old.to_string())).map(String::as_str)
    }

    pub fn insert(&mut self, line_id: LineId, old: String, new: String) {
        self.0.insert((line_id, old), new);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Every rename as `(line, old, new)`, ordered by line then old id.
    pub fn iter(&self) -> impl Iterator<Item = (LineId, &str, &str)> {
        self.0
            .iter()
            .map(|((line, old), new)| (*line, old.as_str(), new.as_str()))
    }

    /// The lines that had at least one id renamed.
    pub fn lines(&self) -> BTreeSet<LineId> {
        self.0.keys().map(|(line, _)| *line).collect()
    }

    /// Rewrite an id held in `line_id`. Returns whether it changed.
    pub fn rewrite_uuid(&self, line_id: LineId, uuid: &mut Option<String>) -> bool {
        let Some(new) = uuid.as_deref().and_then(|old| self.get(line_id, old)) else {
            return false;
        };
        *uuid = Some(new.to_string());
        true
    }

    /// Rewrite a station key anchored on a renamed shot. Returns whether it changed.
    pub fn rewrite_key(&self, key: &mut StationKey) -> bool {
        let StationAnchor::AfterShot(old) = &key.anchor else {
            return false;
        };
        let Some(new) = self.get(key.line_id, old) else {
            return false;
        };
        key.anchor = StationAnchor::AfterShot(new.to_string());
        true
    }

    /// Rewrite every shot id and both gauge anchors of `line`. Returns whether
    /// anything changed. The project's uuid index is NOT updated; a caller holding
    /// the line inside a project uses [`Project::apply_shot_uuid_remap`].
    pub fn rewrite_line(&self, line: &mut Line) -> bool {
        let id = line.id;
        let mut changed = false;
        for shot in &mut line.shots {
            changed |= self.rewrite_uuid(id, &mut shot.uuid);
        }
        changed |= self.rewrite_uuid(id, &mut line.gauge_shot_uuid);
        changed |= self.rewrite_uuid(id, &mut line.gauge_neighbour_uuid);
        changed
    }
}

impl Project {
    /// Rename every shot id that more than one line holds, so each id is held
    /// by one line only. The line with the LOWEST id keeps the id; every
    /// other holder gets [`reminted_shot_uuid`] for it. Shots, gauge anchors and
    /// every `AfterShot` station key in the renamed lines follow.
    ///
    /// Repeats WITHIN one line are left alone: that is
    /// [`Self::ensure_unique_shot_uuids`]'s job, which runs first on load.
    ///
    /// Never calls [`Self::touch`]: the heal runs on every read of a file that
    /// needs it (load, the stalx-reader), and two reads of one file must report
    /// the same project.
    pub fn make_shot_uuids_project_unique(&mut self) -> ShotUuidRemap {
        let mut order: Vec<usize> = (0..self.lines.len()).collect();
        order.sort_by_key(|&i| self.lines[i].id);
        let remap = self.rename_repeats_in_order(&order, HashMap::new());
        self.follow_counter_parity(&remap);
        remap
    }

    /// Rename the ids of `new_lines` that another line of the project
    /// already holds (the project keeps its ids; the newcomers yield), and ids
    /// repeated between two newcomers (the lower line id keeps it). Called by
    /// every door that creates lines from data carrying ids: an import, Add
    /// .stalx, merge, `create_line_and_merge`. Station keys built against the
    /// old ids in those lines are rewritten with them.
    pub fn reassign_repeated_shot_uuids(&mut self, new_lines: &[LineId]) -> ShotUuidRemap {
        let newcomers: HashSet<LineId> = new_lines.iter().copied().collect();
        let mut claimed: HashMap<String, LineId> = HashMap::new();
        for line in self.lines.iter().filter(|s| !newcomers.contains(&s.id)) {
            for uuid in line.shots.iter().filter_map(|s| s.uuid.as_deref()) {
                if !uuid.is_empty() {
                    claimed.entry(uuid.to_string()).or_insert(line.id);
                }
            }
        }
        let mut order: Vec<usize> = (0..self.lines.len())
            .filter(|&i| newcomers.contains(&self.lines[i].id))
            .collect();
        order.sort_by_key(|&i| self.lines[i].id);
        // Not `follow_counter_parity`: the set is the project's own, and a
        // newcomer's record parity comes from the data it was built from (Add
        // .stalx carries its source's set), never from a project shot that
        // happened to share its old id.
        self.rename_repeats_in_order(&order, claimed)
    }

    /// Walk the lines at `order`, letting each claim its ids; an id already
    /// claimed by a DIFFERENT line (in `pre_claimed` or earlier in the walk) is
    /// renamed. Then apply the renames to the whole project.
    fn rename_repeats_in_order(
        &mut self,
        order: &[usize],
        mut claimed: HashMap<String, LineId>,
    ) -> ShotUuidRemap {
        // Every id in the project, for stepping past one already in use. Built
        // only once a repeat is found: the common case has none.
        let mut in_use: Option<HashSet<String>> = None;
        let mut remap = ShotUuidRemap::default();
        for &i in order {
            let line_id = self.lines[i].id;
            for shot in &self.lines[i].shots {
                let Some(old) = shot.uuid.as_deref().filter(|u| !u.is_empty()) else {
                    continue;
                };
                match claimed.get(old) {
                    None => {
                        claimed.insert(old.to_string(), line_id);
                    }
                    Some(&holder) if holder == line_id => {}
                    Some(_) => {
                        if remap.get(line_id, old).is_some() {
                            continue;
                        }
                        let in_use = in_use.get_or_insert_with(|| {
                            self.lines
                                .iter()
                                .flat_map(|s| s.shots.iter().filter_map(|shot| shot.uuid.clone()))
                                .collect()
                        });
                        let new = reminted_shot_uuid_avoiding(line_id, old, |c| in_use.contains(c));
                        in_use.insert(new.clone());
                        claimed.insert(new.clone(), line_id);
                        remap.insert(line_id, old.to_string(), new);
                    }
                }
            }
        }
        self.apply_shot_uuid_remap(&remap);
        remap
    }

    /// Give every shot `remap` renamed the counter-parity entry its old id had
    /// (`counter_parity_shots`), so it keeps its record parity.
    ///
    /// The old id's entry STAYS. When the rename resolved a repeat, the
    /// line that kept the old id still holds a shot with it, and the set,
    /// keyed by id, cannot say which of the two copies the entry was written
    /// for: before the rename both read it. Keeping it on both changes no
    /// shot's record parity. When the old id is held by no one any more the
    /// leftover entry is inert, and the next save prunes it. Only for a
    /// project's OWN repeats (the load heal, or a caller renaming a sole
    /// holder): a newcomer's parity comes from its own source.
    pub fn follow_counter_parity(&mut self, remap: &ShotUuidRemap) {
        let renamed: Vec<String> = remap
            .iter()
            .filter(|(_, old, _)| self.counter_parity_shots.contains(*old))
            .map(|(_, _, new)| new.to_string())
            .collect();
        self.counter_parity_shots.extend(renamed);
    }

    /// Apply `remap` to the live project: shot ids, gauge anchors, held gauges
    /// and every `AfterShot` station key in a renamed line, then refresh the uuid index
    /// of those lines. Positional (`Index`) keys need nothing: a rename does
    /// not move a shot. Never calls [`Self::touch`]. The counter-parity set is
    /// left to the caller ([`Self::follow_counter_parity`]).
    pub fn apply_shot_uuid_remap(&mut self, remap: &ShotUuidRemap) {
        if remap.is_empty() {
            return;
        }
        for line in &mut self.lines {
            remap.rewrite_line(line);
        }
        // A held gauge names its shots in its line, like a station key.
        for held in &mut self.held_gauges {
            if let Some(new) = remap.get(held.line_id, &held.shot) {
                held.shot = new.to_string();
            }
            if let Some(new) = held
                .neighbour
                .as_deref()
                .and_then(|n| remap.get(held.line_id, n))
            {
                held.neighbour = Some(new.to_string());
            }
        }
        self.map_station_refs(|mut key| {
            remap.rewrite_key(&mut key);
            key
        });
        for line_id in remap.lines() {
            self.reindex_line_shot_uuids(line_id);
        }
        tracing::info!(
            renamed = remap.len(),
            lines = remap.lines().len(),
            purpose = "keep shot UUIDs unique across the project",
            "Renamed shot UUIDs that more than one line held"
        );
    }

    /// Every shot id in the project with the line holding it.
    pub fn shot_uuids_by_line(&self) -> HashMap<LineId, HashSet<String>> {
        self.lines
            .iter()
            .map(|s| {
                let ids = s
                    .shots
                    .iter()
                    .filter_map(|shot| shot.uuid.clone())
                    .collect();
                (s.id, ids)
            })
            .collect()
    }
}

/// Apply id renames to a project serialized as JSON (an undo snapshot), without
/// a round trip through [`Project`]: the snapshot restore tells a legacy blob
/// from a current one by which keys are PRESENT (`"validation":`,
/// `"surface_elevation":`), and a typed round trip would add them.
///
/// Rewrites each shot's `uuid` and both gauge anchors under `lines`, keyed by
/// the line's `id`, and every station key - an object holding `line_id`
/// and an `anchor` of `{"AfterShot": id}` - anywhere in the document. Returns
/// whether anything changed.
pub fn remap_project_json(
    value: &mut serde_json::Value,
    rename: &dyn Fn(LineId, &str) -> Option<String>,
) -> bool {
    let mut changed = false;
    if let Some(lines) = value
        .get_mut("lines")
        .and_then(serde_json::Value::as_array_mut)
    {
        for line in lines {
            changed |= remap_line_json(line, rename);
        }
    }
    changed |= remap_station_keys_json(value, rename);
    changed
}

fn remap_line_json(
    line: &mut serde_json::Value,
    rename: &dyn Fn(LineId, &str) -> Option<String>,
) -> bool {
    let Some(line_id) = json_line_id(line.get("id")) else {
        return false;
    };
    let mut changed = false;
    if let Some(shots) = line
        .get_mut("shots")
        .and_then(serde_json::Value::as_array_mut)
    {
        for shot in shots {
            changed |= remap_string_field(shot, "uuid", line_id, rename);
        }
    }
    changed |= remap_string_field(line, "gauge_shot_uuid", line_id, rename);
    changed |= remap_string_field(line, "gauge_neighbour_uuid", line_id, rename);
    changed
}

fn remap_string_field(
    object: &mut serde_json::Value,
    field: &str,
    line_id: LineId,
    rename: &dyn Fn(LineId, &str) -> Option<String>,
) -> bool {
    let Some(slot) = object.get_mut(field) else {
        return false;
    };
    let Some(new) = slot.as_str().and_then(|old| rename(line_id, old)) else {
        return false;
    };
    *slot = serde_json::Value::String(new);
    true
}

/// The station-key half of [`remap_project_json`], for a JSON fragment that
/// is not a whole project (a compact undo payload's literal values): every
/// object holding `line_id` and an `anchor` of `{"AfterShot": id}`,
/// anywhere in `value`.
pub fn remap_station_keys_json(
    value: &mut serde_json::Value,
    rename: &dyn Fn(LineId, &str) -> Option<String>,
) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            let mut changed = false;
            let line_id = json_line_id(map.get("line_id"));
            if let (Some(line_id), Some(anchor)) = (line_id, map.get_mut("anchor")) {
                if let Some(slot) = anchor.get_mut("AfterShot") {
                    if let Some(new) = slot.as_str().and_then(|old| rename(line_id, old)) {
                        *slot = serde_json::Value::String(new);
                        changed = true;
                    }
                }
            }
            for (_, child) in map.iter_mut() {
                changed |= remap_station_keys_json(child, rename);
            }
            changed
        }
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= remap_station_keys_json(item, rename);
            }
            changed
        }
        _ => false,
    }
}

fn json_line_id(value: Option<&serde_json::Value>) -> Option<LineId> {
    value
        .and_then(serde_json::Value::as_u64)
        .and_then(|id| LineId::try_from(id).ok())
}
