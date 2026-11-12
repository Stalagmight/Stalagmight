//! Project-wide shot UUID uniqueness.
//!
//! A shot's `uuid` is its persistent identity: station cross-references anchor to
//! it (`StationAnchor::AfterShot`), the Ariane export emits it as `<UUID>`, and
//! undo entries address shots by it. It used to be unique only within a section,
//! because an Ariane `<UUID>` is carried verbatim (#423) and merge / Add .stalx
//! keep their sources' ids, so importing one file twice, or merging two projects
//! that share an ancestor, put the same id into two sections.
//!
//! It is now unique across the whole project:
//!
//! * the doors that bring foreign ids in (an import, `create_section_and_merge`,
//!   merge, Add .stalx) call [`Project::reassign_repeated_shot_uuids`] for the
//!   sections they created, so the project already holding an id keeps it;
//! * a file saved before this, which can hold repeats, is healed on load by
//!   [`Project::make_shot_uuids_project_unique`], and the load translates the
//!   persisted undo history through the same renames so every entry still
//!   addresses the shot it was recorded against.
//!
//! The replacement id is DERIVED, not random ([`reminted_shot_uuid`]): it is a
//! function of the section and the id it replaces. A file that is opened and
//! never saved therefore reads to the same ids every time, two independent loads
//! of one revision agree (SpeleoDB Unwind), and a key stored against the old id
//! elsewhere - an ignored warning, a dive-plan anchor - can still be recognised
//! ([`stored_shot_uuid_matches`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::{Section, SectionId, StationAnchor, StationKey};

use super::Project;

/// Namespace of the derived replacement ids. Changing it changes every derived
/// id, so a file healed by one build would be healed differently by the next.
pub const SHOT_UUID_REMINT_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x3c2a_91d4_5e7b_4f06_a1c8_6d0e_9b47_2f15);

/// The id a repeated shot gets in `section_id` in place of `old`.
///
/// A v5 hash of the section and the old id, re-stamped as a v4: the SpeleoDB
/// sidecar keeps only v4 values (`speleodb_uuid`), and the version nibble is all
/// that distinguishes the two. The old id is length-prefixed so no pair of
/// (section, id) inputs can produce the same hash input.
pub fn reminted_shot_uuid(section_id: SectionId, old: &str) -> String {
    remint_attempt(section_id, old, 0)
}

/// [`reminted_shot_uuid`], stepped past any id `taken` reports as in use. The
/// first attempt is the plain derived id; a collision with a 122-bit hash is not
/// a practical case, so stepping exists only to keep uniqueness unconditional.
pub fn reminted_shot_uuid_avoiding(
    section_id: SectionId,
    old: &str,
    taken: impl Fn(&str) -> bool,
) -> String {
    (0..u32::MAX)
        .map(|attempt| remint_attempt(section_id, old, attempt))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| remint_attempt(section_id, old, 0))
}

fn remint_attempt(section_id: SectionId, old: &str, attempt: u32) -> String {
    let name = if attempt == 0 {
        format!("{section_id}:{}:{old}", old.len())
    } else {
        format!("{section_id}:{}:{old}#{attempt}", old.len())
    };
    let hashed = uuid::Uuid::new_v5(&SHOT_UUID_REMINT_NAMESPACE, name.as_bytes());
    uuid::Builder::from_random_bytes(*hashed.as_bytes())
        .into_uuid()
        .to_string()
}

/// Whether an id stored against `section_id` somewhere outside the project (an
/// ignored warning, a dive-plan anchor) names the live shot whose id is `live`:
/// either it is that id, or the shot was renamed from it by a uniqueness heal.
pub fn stored_shot_uuid_matches(section_id: SectionId, stored: &str, live: &str) -> bool {
    stored == live || reminted_shot_uuid(section_id, stored) == live
}

/// The live id that a key stored against `stored` in `section_id` names, given
/// `holds` (whether the section holds a shot with that id now): `stored` itself,
/// else the derived replacement a uniqueness heal gave that shot, else `None`.
///
/// THE one rule every key stored outside the project follows (ignored warnings,
/// dive-plan anchors, an Add's carried ignores). It tries the FIRST derived
/// candidate only: a replacement stepped past an id already in use exists only
/// when a file holds a derived value itself, and then the key names whichever
/// shot holds that value - a shot the same file could have named directly.
pub fn live_shot_uuid_for_stored(
    section_id: SectionId,
    stored: &str,
    holds: impl Fn(&str) -> bool,
) -> Option<String> {
    if holds(stored) {
        return Some(stored.to_string());
    }
    let renamed = reminted_shot_uuid(section_id, stored);
    holds(&renamed).then_some(renamed)
}

/// Renames of shot ids, keyed by the section that held the old id. A key is the
/// (section, old id) pair because the same old id can stay in one section and be
/// renamed in another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShotUuidRemap(BTreeMap<(SectionId, String), String>);

impl ShotUuidRemap {
    /// The new id of `old` in `section_id`, or `None` when it was not renamed.
    pub fn get(&self, section_id: SectionId, old: &str) -> Option<&str> {
        self.0
            .get(&(section_id, old.to_string()))
            .map(String::as_str)
    }

    pub fn insert(&mut self, section_id: SectionId, old: String, new: String) {
        self.0.insert((section_id, old), new);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Every rename as `(section, old, new)`, ordered by section then old id.
    pub fn iter(&self) -> impl Iterator<Item = (SectionId, &str, &str)> {
        self.0
            .iter()
            .map(|((section, old), new)| (*section, old.as_str(), new.as_str()))
    }

    /// The sections that had at least one id renamed.
    pub fn sections(&self) -> BTreeSet<SectionId> {
        self.0.keys().map(|(section, _)| *section).collect()
    }

    /// Rewrite an id held in `section_id`. Returns whether it changed.
    pub fn rewrite_uuid(&self, section_id: SectionId, uuid: &mut Option<String>) -> bool {
        let Some(new) = uuid.as_deref().and_then(|old| self.get(section_id, old)) else {
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
        let Some(new) = self.get(key.section_id, old) else {
            return false;
        };
        key.anchor = StationAnchor::AfterShot(new.to_string());
        true
    }

    /// Rewrite every shot id and both gauge anchors of `section`. Returns whether
    /// anything changed. The project's uuid index is NOT updated; a caller holding
    /// the section inside a project uses [`Project::apply_shot_uuid_remap`].
    pub fn rewrite_section(&self, section: &mut Section) -> bool {
        let id = section.id;
        let mut changed = false;
        for shot in &mut section.shots {
            changed |= self.rewrite_uuid(id, &mut shot.uuid);
        }
        changed |= self.rewrite_uuid(id, &mut section.gauge_shot_uuid);
        changed |= self.rewrite_uuid(id, &mut section.gauge_neighbour_uuid);
        changed
    }
}

impl Project {
    /// Rename every shot id that more than one section holds, so each id is held
    /// by one section only. The section with the LOWEST id keeps the id; every
    /// other holder gets [`reminted_shot_uuid`] for it. Shots, gauge anchors and
    /// every `AfterShot` station key in the renamed sections follow.
    ///
    /// Repeats WITHIN one section are left alone: that is
    /// [`Self::ensure_unique_shot_uuids`]'s job, which runs first on load.
    ///
    /// Never calls [`Self::touch`]: the heal runs on every read of a file that
    /// needs it (load, the stalx-reader), and two reads of one file must report
    /// the same project.
    pub fn make_shot_uuids_project_unique(&mut self) -> ShotUuidRemap {
        let mut order: Vec<usize> = (0..self.sections.len()).collect();
        order.sort_by_key(|&i| self.sections[i].id);
        self.rename_repeats_in_order(&order, HashMap::new())
    }

    /// Rename the ids of `new_sections` that another section of the project
    /// already holds (the project keeps its ids; the newcomers yield), and ids
    /// repeated between two newcomers (the lower section id keeps it). Called by
    /// every door that creates sections from data carrying ids: an import, Add
    /// .stalx, merge, `create_section_and_merge`. Station keys built against the
    /// old ids in those sections are rewritten with them.
    pub fn reassign_repeated_shot_uuids(&mut self, new_sections: &[SectionId]) -> ShotUuidRemap {
        let newcomers: HashSet<SectionId> = new_sections.iter().copied().collect();
        let mut claimed: HashMap<String, SectionId> = HashMap::new();
        for section in self.sections.iter().filter(|s| !newcomers.contains(&s.id)) {
            for uuid in section.shots.iter().filter_map(|s| s.uuid.as_deref()) {
                if !uuid.is_empty() {
                    claimed.entry(uuid.to_string()).or_insert(section.id);
                }
            }
        }
        let mut order: Vec<usize> = (0..self.sections.len())
            .filter(|&i| newcomers.contains(&self.sections[i].id))
            .collect();
        order.sort_by_key(|&i| self.sections[i].id);
        self.rename_repeats_in_order(&order, claimed)
    }

    /// Walk the sections at `order`, letting each claim its ids; an id already
    /// claimed by a DIFFERENT section (in `pre_claimed` or earlier in the walk) is
    /// renamed. Then apply the renames to the whole project.
    fn rename_repeats_in_order(
        &mut self,
        order: &[usize],
        mut claimed: HashMap<String, SectionId>,
    ) -> ShotUuidRemap {
        // Every id in the project, for stepping past one already in use. Built
        // only once a repeat is found: the common case has none.
        let mut in_use: Option<HashSet<String>> = None;
        let mut remap = ShotUuidRemap::default();
        for &i in order {
            let section_id = self.sections[i].id;
            for shot in &self.sections[i].shots {
                let Some(old) = shot.uuid.as_deref().filter(|u| !u.is_empty()) else {
                    continue;
                };
                match claimed.get(old) {
                    None => {
                        claimed.insert(old.to_string(), section_id);
                    }
                    Some(&holder) if holder == section_id => {}
                    Some(_) => {
                        if remap.get(section_id, old).is_some() {
                            continue;
                        }
                        let in_use = in_use.get_or_insert_with(|| {
                            self.sections
                                .iter()
                                .flat_map(|s| s.shots.iter().filter_map(|shot| shot.uuid.clone()))
                                .collect()
                        });
                        let new =
                            reminted_shot_uuid_avoiding(section_id, old, |c| in_use.contains(c));
                        in_use.insert(new.clone());
                        claimed.insert(new.clone(), section_id);
                        remap.insert(section_id, old.to_string(), new);
                    }
                }
            }
        }
        self.apply_shot_uuid_remap(&remap);
        remap
    }

    /// Apply `remap` to the live project: shot ids, gauge anchors and every
    /// `AfterShot` station key in a renamed section, then refresh the uuid index
    /// of those sections. Positional (`Index`) keys need nothing: a rename does
    /// not move a shot. Never calls [`Self::touch`].
    pub fn apply_shot_uuid_remap(&mut self, remap: &ShotUuidRemap) {
        if remap.is_empty() {
            return;
        }
        for section in &mut self.sections {
            remap.rewrite_section(section);
        }
        // A renamed shot keeps its record parity (`counter_parity_shots`).
        let renamed: Vec<String> = remap
            .iter()
            .filter(|(_, old, _)| self.counter_parity_shots.contains(*old))
            .map(|(_, _, new)| new.to_string())
            .collect();
        self.counter_parity_shots.extend(renamed);
        self.map_station_refs(|mut key| {
            remap.rewrite_key(&mut key);
            key
        });
        for section_id in remap.sections() {
            self.reindex_section_shot_uuids(section_id);
        }
        tracing::info!(
            renamed = remap.len(),
            sections = remap.sections().len(),
            purpose = "keep shot UUIDs unique across the project",
            "Renamed shot UUIDs that more than one section held"
        );
    }

    /// Every shot id in the project with the section holding it.
    pub fn shot_uuids_by_section(&self) -> HashMap<SectionId, HashSet<String>> {
        self.sections
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
/// Rewrites each shot's `uuid` and both gauge anchors under `sections`, keyed by
/// the section's `id`, and every station key - an object holding `section_id`
/// and an `anchor` of `{"AfterShot": id}` - anywhere in the document. Returns
/// whether anything changed.
pub fn remap_project_json(
    value: &mut serde_json::Value,
    rename: &dyn Fn(SectionId, &str) -> Option<String>,
) -> bool {
    let mut changed = false;
    if let Some(sections) = value
        .get_mut("sections")
        .and_then(serde_json::Value::as_array_mut)
    {
        for section in sections {
            changed |= remap_section_json(section, rename);
        }
    }
    changed |= remap_station_keys_json(value, rename);
    changed
}

fn remap_section_json(
    section: &mut serde_json::Value,
    rename: &dyn Fn(SectionId, &str) -> Option<String>,
) -> bool {
    let Some(section_id) = json_section_id(section.get("id")) else {
        return false;
    };
    let mut changed = false;
    if let Some(shots) = section
        .get_mut("shots")
        .and_then(serde_json::Value::as_array_mut)
    {
        for shot in shots {
            changed |= remap_string_field(shot, "uuid", section_id, rename);
        }
    }
    changed |= remap_string_field(section, "gauge_shot_uuid", section_id, rename);
    changed |= remap_string_field(section, "gauge_neighbour_uuid", section_id, rename);
    changed
}

fn remap_string_field(
    object: &mut serde_json::Value,
    field: &str,
    section_id: SectionId,
    rename: &dyn Fn(SectionId, &str) -> Option<String>,
) -> bool {
    let Some(slot) = object.get_mut(field) else {
        return false;
    };
    let Some(new) = slot.as_str().and_then(|old| rename(section_id, old)) else {
        return false;
    };
    *slot = serde_json::Value::String(new);
    true
}

fn remap_station_keys_json(
    value: &mut serde_json::Value,
    rename: &dyn Fn(SectionId, &str) -> Option<String>,
) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            let mut changed = false;
            let section_id = json_section_id(map.get("section_id"));
            if let (Some(section_id), Some(anchor)) = (section_id, map.get_mut("anchor")) {
                if let Some(slot) = anchor.get_mut("AfterShot") {
                    if let Some(new) = slot.as_str().and_then(|old| rename(section_id, old)) {
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

fn json_section_id(value: Option<&serde_json::Value>) -> Option<SectionId> {
    value
        .and_then(serde_json::Value::as_u64)
        .and_then(|id| SectionId::try_from(id).ok())
}
