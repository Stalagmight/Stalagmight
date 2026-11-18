//! Station identity: every station has a name unique within its line and a
//! project-unique Station ID, and every line a project-unique name.
//!
//! The names and ids live in `Project::station_names` / `Project::station_ids`,
//! `StationKey`-keyed maps carried through every structural edit by the same
//! walkers as `station_comments` (`for_each_station_ref`, `map_station_refs`,
//! the reverse remap, the prepend/first-shot fixups, `station_rekey`). An
//! operation that creates a station names it by the creation rules
//! ([`Project::name_new_station`]); ids are only ever allocated here, by
//! [`Project::ensure_station_identities`], the one normalization entry point.
//! It runs at the load boundary, after an import or an Add/Merge, after every
//! mutating command and after every undo/redo step, and it never touches the
//! name or id of a station that already has a valid one.

use std::collections::{HashMap, HashSet};

use crate::{LineId, ShotId, StationAnchor, StationId, StationKey, StationRef, MAX_STATION_ID};

use super::station_naming::{
    dedupe_line_name, dedupe_line_name_from, fill_line_station_names, name_after_end,
    name_before_start, name_characters_refusal, name_with_free_letter, plan_combine_prefixes,
    seed_line_name_from, settle_line_station_names, CombinePrefixReason, CombinePrefixes,
};
use super::{station_key_order, Project};

/// What a line without a name is called in user-facing text
/// ([`Project::line_label`]).
pub const UNNAMED_LINE: &str = "unnamed line";

/// What one [`Project::ensure_station_identities`] pass changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IdentityHeal {
    /// Stations that got a new Station ID (no id, or an id an earlier station
    /// already held).
    pub ids_allocated: usize,
    /// Stored ids dropped because their station is gone, or because another
    /// key of the same station holds one.
    pub ids_dropped: usize,
    /// Stations that got a seeded name.
    pub names_seeded: usize,
    /// Stations renamed because an earlier station of their line holds the
    /// name.
    pub names_deduped: usize,
    /// Lines that got a seeded or a de-duplicated name.
    pub line_names: usize,
}

impl IdentityHeal {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// FNV-1a, for the per-pass maps below: their keys are short uuid strings and
/// small integers, never attacker-chosen in a way that matters (a crafted file
/// can only slow its own open), and the default SipHash dominated the
/// no-change pass that runs after every command.
#[derive(Default)]
struct Fnv(u64);

impl std::hash::Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut hash = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = hash;
    }
}

type FastHash = std::hash::BuildHasherDefault<Fnv>;
type FastMap<K, V> = HashMap<K, V, FastHash>;
type FastSet<K> = HashSet<K, FastHash>;

/// Resolves stored keys against the current shots in O(1), without
/// allocating, for the whole-project passes below (`Project::resolve_key`
/// clones the uuid on its cache lookup).
struct KeyResolver<'a> {
    /// `(line, shot uuid)` -> the station after that shot. First occurrence
    /// wins, as in `resolve_key`.
    after: FastMap<(LineId, &'a str), u32>,
    /// Shot count per line.
    shots: FastMap<LineId, u32>,
}

impl<'a> KeyResolver<'a> {
    fn new(lines: &'a [crate::Line]) -> Self {
        let total: usize = lines.iter().map(|l| l.shots.len()).sum();
        let mut after = FastMap::with_capacity_and_hasher(total, FastHash::default());
        let mut shots = FastMap::with_capacity_and_hasher(lines.len(), FastHash::default());
        for line in lines {
            shots.insert(line.id, line.shots.len() as u32);
            for (i, shot) in line.shots.iter().enumerate() {
                if let Some(uuid) = shot.uuid.as_deref() {
                    after.entry((line.id, uuid)).or_insert(i as u32 + 1);
                }
            }
        }
        Self { after, shots }
    }

    /// The station `key` names and whether `key` is its canonical form.
    fn resolve(&self, key: &StationKey) -> Option<(u32, bool)> {
        let count = *self.shots.get(&key.line_id)?;
        match &key.anchor {
            StationAnchor::LineStart => Some((0, true)),
            StationAnchor::AfterShot(uuid) => self
                .after
                .get(&(key.line_id, uuid.as_str()))
                .map(|index| (*index, true)),
            StationAnchor::Index(index) => (*index <= count).then_some((*index, false)),
        }
    }
}

/// The canonical key of every station of `line` that has one, in station
/// order: `LineStart`, then `AfterShot(uuid)` for each shot with a uuid that
/// no earlier shot of the line holds (a repeated uuid's second station cannot
/// be keyed apart from the first).
fn canonical_keys(line: &crate::Line) -> Vec<(u32, StationKey)> {
    let mut out = Vec::with_capacity(line.shots.len() + 1);
    out.push((0, StationKey::new(line.id, StationAnchor::LineStart)));
    let mut seen: HashSet<&str> = HashSet::with_capacity(line.shots.len());
    for (i, shot) in line.shots.iter().enumerate() {
        if let Some(uuid) = shot.uuid.as_deref() {
            if seen.insert(uuid) {
                out.push((
                    i as u32 + 1,
                    StationKey::new(line.id, StationAnchor::AfterShot(uuid.to_string())),
                ));
            }
        }
    }
    out
}

/// Number of stations the canonical walk keys, assuming no repeated uuid
/// (the fast checks fall to the full pass when that is wrong).
fn keyable_stations(lines: &[crate::Line]) -> usize {
    lines
        .iter()
        .map(|l| 1 + l.shots.iter().filter(|s| s.uuid.is_some()).count())
        .sum()
}

/// A stored map split by [`entries_by_station`]: the one entry per station,
/// the dangling entries, and how many entries lost to another key of their
/// station.
type ByStation<V> = (
    HashMap<(LineId, u32), (StationKey, V)>,
    Vec<(StationKey, V)>,
    usize,
);

/// For each station, the one stored entry of `map` that names it: the
/// canonical key when present, else the lowest key. Dangling keys and the
/// losing keys of a station are returned apart.
fn entries_by_station<V>(resolver: &KeyResolver<'_>, map: HashMap<StationKey, V>) -> ByStation<V> {
    let mut by_station: HashMap<(LineId, u32), (StationKey, V, bool)> =
        HashMap::with_capacity(map.len());
    let mut dangling = Vec::new();
    let mut losers = 0;
    for (key, value) in map {
        let Some((index, canonical)) = resolver.resolve(&key) else {
            dangling.push((key, value));
            continue;
        };
        let slot = (key.line_id, index);
        let replace = match by_station.get(&slot) {
            None => true,
            Some((held, _, held_canonical)) => {
                losers += 1;
                (canonical && !held_canonical)
                    || (canonical == *held_canonical
                        && station_key_order(&key) < station_key_order(held))
            }
        };
        if replace {
            by_station.insert(slot, (key, value, canonical));
        }
    }
    let by_station = by_station
        .into_iter()
        .map(|(slot, (key, value, _))| (slot, (key, value)))
        .collect();
    (by_station, dangling, losers)
}

impl Project {
    /// THE normalization entry point for station and line identity:
    ///
    /// - every line gets a non-empty name, unique in the project
    ///   ([`Self::ensure_line_names`]);
    /// - every station gets a Station ID: one without (or with an id an earlier
    ///   station holds, in line order, station order) gets `next_station_id`,
    ///   allocated in line order, station order, so a file an older build
    ///   rewrote without the ids re-mints the same ids on every open;
    /// - every station gets a name: a missing one is seeded from its index,
    ///   and a name an earlier station of the line holds gets a letter suffix
    ///   (`5`, `5` -> `5`, `5a`) - see `station_naming`.
    ///
    /// A station that already has a valid id and name keeps both. Ids whose
    /// station is gone are dropped (an id is identity, not data); a name whose
    /// station is gone is left for the integrity check, like a comment.
    ///
    /// Cost when nothing is missing: one pass over the shots to build the
    /// `(line, uuid)` index the keys resolve against (a few allocations sized
    /// to the project, none per station), then one pass over each map. That
    /// is O(stations) per call, so `execute()` skips the call entirely for a
    /// command that cannot change station identity
    /// (`Command::can_change_station_identity` in the core crate).
    pub fn ensure_station_identities(&mut self) -> IdentityHeal {
        let mut heal = IdentityHeal {
            line_names: self.ensure_line_names(),
            ..IdentityHeal::default()
        };
        // The common case - nothing missing - is decided by one resolver
        // built once for both maps.
        let (ids_settled, names_settled) = {
            let resolver = KeyResolver::new(&self.lines);
            let keyable = keyable_stations(&self.lines);
            (
                self.station_ids_settled(&resolver, keyable),
                self.station_names_settled(&resolver, keyable),
            )
        };
        if !ids_settled {
            self.settle_station_ids(&mut heal);
        }
        if !names_settled {
            self.settle_station_names(&mut heal);
        }
        if !heal.is_empty() {
            tracing::debug!(?heal, "normalized station identities");
        }
        heal
    }

    /// Seed every unnamed line `Line n` (smallest free `n`, in line order)
    /// and make every repeated line name unique with ` (2)`, ` (3)`, ... on the
    /// later holder. Returns how many lines it named.
    pub fn ensure_line_names(&mut self) -> usize {
        let named = |name: &Option<String>| name.as_ref().is_some_and(|n| !n.trim().is_empty());
        let mut seen: HashSet<&str> = HashSet::with_capacity(self.lines.len());
        let settled = self.lines.iter().all(|line| {
            line.name
                .as_deref()
                .is_some_and(|n| !n.is_empty() && n.trim().len() == n.len() && seen.insert(n))
        });
        if settled {
            return 0;
        }
        // Names are stored and compared trimmed (`Main ` is `Main`), so the
        // repeats below are found on the trimmed key; a blank one is unnamed.
        let mut changed = 0;
        for line in &mut self.lines {
            if let Some(name) = &line.name {
                if name.trim().len() != name.len() {
                    let bare = name.trim();
                    line.name = (!bare.is_empty()).then(|| bare.to_string());
                    changed += 1;
                }
            }
        }
        let mut taken: HashSet<String> = self
            .lines
            .iter()
            .filter(|line| named(&line.name))
            .filter_map(|line| line.name.clone())
            .collect();
        let mut kept: HashSet<String> = HashSet::with_capacity(self.lines.len());
        // `taken` only grows, so each search resumes at a cursor (one per
        // repeated name, one for the seeds): N lines sharing a name, or N
        // unnamed ones, cost O(N) probes rather than O(N^2).
        let mut dedupe_from: HashMap<String, u64> = HashMap::new();
        let mut seed_from: u64 = 1;
        // Repeats first, in line order, then the seeds, so a seed never takes
        // a name a later repeat would have been renamed to.
        for line in &mut self.lines {
            let Some(name) = line.name.clone().filter(|n| !n.trim().is_empty()) else {
                continue;
            };
            if !kept.insert(name.clone()) {
                let from = dedupe_from.entry(name.clone()).or_insert(2);
                let fresh = dedupe_line_name_from(&name, from, &|n| taken.contains(n));
                taken.insert(fresh.clone());
                kept.insert(fresh.clone());
                line.name = Some(fresh);
                changed += 1;
            }
        }
        for line in &mut self.lines {
            if !named(&line.name) {
                let fresh = seed_line_name_from(&mut seed_from, &|n| taken.contains(n));
                taken.insert(fresh.clone());
                line.name = Some(fresh);
                changed += 1;
            }
        }
        changed
    }

    /// Every station keyed canonically with an id, ids unique, and
    /// `next_station_id` above them all.
    fn station_ids_settled(&self, resolver: &KeyResolver<'_>, keyable: usize) -> bool {
        if self.station_ids.len() != keyable {
            return false;
        }
        let mut seen: FastSet<StationId> =
            FastSet::with_capacity_and_hasher(self.station_ids.len(), FastHash::default());
        self.next_station_id <= MAX_STATION_ID
            && self.station_ids.iter().all(|(key, id)| {
                matches!(resolver.resolve(key), Some((_, true)))
                    && *id < self.next_station_id
                    && seen.insert(*id)
            })
    }

    fn settle_station_ids(&mut self, heal: &mut IdentityHeal) {
        let map = std::mem::take(&mut self.station_ids);
        let (by_station, dangling, losers) = {
            let resolver = KeyResolver::new(&self.lines);
            entries_by_station(&resolver, map)
        };
        heal.ids_dropped += dangling.len() + losers;
        let mut ids: HashMap<StationKey, StationId> = HashMap::with_capacity(by_station.len());
        let mut seen: HashSet<StationId> = HashSet::with_capacity(by_station.len());
        let mut missing: Vec<StationKey> = Vec::new();
        for line in &self.lines {
            for (index, key) in canonical_keys(line) {
                match by_station.get(&(line.id, index)) {
                    // An id past the bound is invalid: the station is
                    // re-minted like one that has none.
                    Some((_, id)) if *id < MAX_STATION_ID && seen.insert(*id) => {
                        ids.insert(key, *id);
                    }
                    _ => missing.push(key),
                }
            }
        }
        // A counter past the bound is as invalid as such an id: start again
        // above the valid ids.
        if self.next_station_id > MAX_STATION_ID {
            self.next_station_id = 0;
        }
        if let Some(max) = seen.iter().max() {
            self.next_station_id = self.next_station_id.max(max + 1);
        }
        heal.ids_allocated += missing.len();
        let room = u64::from(MAX_STATION_ID - self.next_station_id);
        if missing.len() as u64 > room {
            // Allocating would run past the bound: renumber every station
            // compactly, in line order, station order (only a project that
            // has used up 2^31 ids, or a crafted one, gets here).
            tracing::warn!(
                stations = ids.len() + missing.len(),
                next_station_id = self.next_station_id,
                purpose = "keep Station IDs unique and bounded",
                "Station IDs would overflow; renumbering every station"
            );
            heal.ids_allocated += ids.len();
            ids.clear();
            self.next_station_id = 0;
            for line in &self.lines {
                for (_, key) in canonical_keys(line) {
                    ids.insert(key, self.next_station_id);
                    self.next_station_id += 1;
                }
            }
        } else {
            for key in missing {
                ids.insert(key, self.next_station_id);
                self.next_station_id += 1;
            }
        }
        self.station_ids = ids;
    }

    /// Every station named under its canonical key, no name blank or padded
    /// with whitespace, names unique within each line. Keys that do not
    /// resolve are ignored.
    fn station_names_settled(&self, resolver: &KeyResolver<'_>, keyable: usize) -> bool {
        let mut resolving = 0;
        let mut unique: FastSet<(LineId, &str)> =
            FastSet::with_capacity_and_hasher(self.station_names.len(), FastHash::default());
        for (key, name) in &self.station_names {
            match resolver.resolve(key) {
                None => {}
                Some((_, true))
                    if !name.is_empty()
                        && name.trim().len() == name.len()
                        && unique.insert((key.line_id, name)) =>
                {
                    resolving += 1;
                }
                Some(_) => return false,
            }
        }
        resolving == keyable
    }

    fn settle_station_names(&mut self, heal: &mut IdentityHeal) {
        let map = std::mem::take(&mut self.station_names);
        let (mut by_station, dangling, _) = {
            let resolver = KeyResolver::new(&self.lines);
            entries_by_station(&resolver, map)
        };
        let mut names: HashMap<StationKey, String> =
            HashMap::with_capacity(by_station.len() + dangling.len());
        names.extend(dangling);
        for line in &self.lines {
            let keys = canonical_keys(line);
            let mut slots: Vec<Option<String>> = keys
                .iter()
                .map(|(index, _)| by_station.remove(&(line.id, *index)).map(|(_, n)| n))
                .collect();
            let (seeded, renamed) = settle_line_station_names(&mut slots);
            heal.names_seeded += seeded;
            heal.names_deduped += renamed;
            for ((_, key), name) in keys.into_iter().zip(slots) {
                if let Some(name) = name {
                    names.insert(key, name);
                }
            }
        }
        self.station_names = names;
    }

    // === Lookups ===

    /// The Station ID of a positional station, if it has one.
    pub fn station_id_at(&self, station: StationRef) -> Option<StationId> {
        let key = self.key_for_ref(station)?;
        self.station_ids.get(&key).copied()
    }

    /// The name of a positional station, if it has one.
    pub fn station_name_at(&self, station: StationRef) -> Option<&str> {
        let key = self.key_for_ref(station)?;
        self.station_names.get(&key).map(String::as_str)
    }

    /// The names of `line_id`'s stations by index (`None` for an unnamed or
    /// unkeyable station), read through the canonical keys.
    pub fn line_station_names(&self, line_id: LineId) -> Vec<Option<String>> {
        let Some(line) = self.get_line(line_id) else {
            return Vec::new();
        };
        let mut out = vec![None; line.shots.len() + 1];
        for (index, key) in canonical_keys(line) {
            if let Some(slot) = out.get_mut(index as usize) {
                *slot = self.station_names.get(&key).cloned();
            }
        }
        out
    }

    /// The name a line shows the user: its name, or `unnamed line` for a
    /// line a normalization has not reached yet - never `Line <id>`, which
    /// reads as (and can collide with) a seeded name like `Line 3`.
    pub fn line_label(&self, line_id: LineId) -> Option<String> {
        let line = self.get_line(line_id)?;
        Some(
            line.name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| UNNAMED_LINE.to_string()),
        )
    }

    /// A station as the user reads it, with its line: `Main Line / 17`.
    /// `None` when the station is not in the project.
    pub fn station_label(&self, station: StationRef) -> Option<String> {
        let line = self.line_label(station.line_id)?;
        let line_ref = self.get_line(station.line_id)?;
        if station.station_index as usize > line_ref.shots.len() {
            return None;
        }
        let name = self
            .station_name_at(station)
            .map_or_else(|| station.station_index.to_string(), str::to_string);
        Some(format!("{line} / {name}"))
    }

    /// [`Self::station_label`] for user-facing text that must say something
    /// even when the station cannot be named: `a station of Main Line`, or
    /// `a station` when its line is gone too. Never the positional
    /// `line:index` handle.
    pub fn station_text(&self, station: StationRef) -> String {
        self.station_label(station).unwrap_or_else(|| {
            self.line_label(station.line_id)
                .map_or_else(|| "a station".to_string(), |l| format!("a station of {l}"))
        })
    }

    /// A shot as the user reads it, with its line: `Main Line: 4 → 5`.
    /// `None` when the shot is not in the project.
    pub fn shot_label(&self, line_id: LineId, shot_id: ShotId) -> Option<String> {
        let line = self.get_line(line_id)?;
        let index = line.shots.iter().position(|s| s.id == shot_id)?;
        self.shot_label_at(line_id, index)
    }

    /// [`Self::shot_label`] for the shot at `shot_index` of the line.
    pub fn shot_label_at(&self, line_id: LineId, shot_index: usize) -> Option<String> {
        let line = self.get_line(line_id)?;
        if shot_index >= line.shots.len() {
            return None;
        }
        let index = u32::try_from(shot_index).ok()?;
        let name = |i: u32| {
            self.station_name_at(StationRef::new(line_id, i))
                .map_or_else(|| i.to_string(), str::to_string)
        };
        let label = self.line_label(line_id)?;
        Some(format!(
            "{label}: {} \u{2192} {}",
            name(index),
            name(index + 1)
        ))
    }

    /// The shot labels of a whole line by shot index (`Main Line: 4 → 5`),
    /// reading the line's names once: what a pass that names many shots of
    /// one line (validation warnings) should use instead of
    /// [`Self::shot_label_at`] per shot.
    pub fn line_shot_labels(&self, line_id: LineId) -> Vec<String> {
        let Some(label) = self.line_label(line_id) else {
            return Vec::new();
        };
        let names: Vec<String> = self
            .line_station_names(line_id)
            .into_iter()
            .enumerate()
            .map(|(i, n)| n.unwrap_or_else(|| i.to_string()))
            .collect();
        names
            .windows(2)
            .map(|pair| format!("{label}: {} \u{2192} {}", pair[0], pair[1]))
            .collect()
    }

    /// The identity `line_id` carries right now, with its stations' ids and
    /// names ([`crate::LineIdentity`]), for an undo entry that re-creates the
    /// line.
    pub fn line_identity(&self, line_id: LineId) -> Option<crate::LineIdentity> {
        let line = self.get_line(line_id)?;
        let mut identity = crate::LineIdentity::of(line);
        identity.station_names = self.line_station_names(line_id);
        identity.station_ids = (0..=line.shots.len() as u32)
            .map(|index| self.station_id_at(StationRef::new(line_id, index)))
            .collect();
        Some(identity)
    }

    /// Put a re-created line's recorded station ids and names back
    /// ([`Self::line_identity`]). An id another station holds by now is left
    /// to the normalization, which keeps ids unique.
    pub fn restore_line_station_identity(
        &mut self,
        line_id: LineId,
        identity: &crate::LineIdentity,
    ) {
        let held: HashSet<StationId> = self.station_ids.values().copied().collect();
        for (index, id) in identity.station_ids.iter().enumerate() {
            let (Some(id), Some(key)) =
                (id, self.key_for_ref(StationRef::new(line_id, index as u32)))
            else {
                continue;
            };
            if !held.contains(id) {
                self.station_ids.insert(key, *id);
                self.next_station_id = self.next_station_id.max(id.saturating_add(1));
            }
        }
        for (index, name) in identity.station_names.iter().enumerate() {
            let (Some(name), Some(key)) = (
                name,
                self.key_for_ref(StationRef::new(line_id, index as u32)),
            ) else {
                continue;
            };
            self.station_names.insert(key, name.clone());
        }
    }

    // === Naming new stations ===

    /// Name the station an operation just created at `station_index` of
    /// `line_id` by the creation rules (`station_naming`): the End rule when it
    /// is the line's last station, the Prepend rule when it is the first, the
    /// Mid rule otherwise. A station that already has a name keeps it. Its
    /// Station ID is allocated by the next [`Self::ensure_station_identities`].
    pub fn name_new_station(&mut self, line_id: LineId, station_index: u32) {
        let Some(key) = self.key_for_ref(StationRef::new(line_id, station_index)) else {
            return;
        };
        if self.station_names.contains_key(&key) {
            return;
        }
        let names = self.line_station_names(line_id);
        let last = names.len().saturating_sub(1) as u32;
        // A neighbour without a name reads as its seed, as the
        // normalization would name it.
        let neighbour = |index: u32| -> String {
            names
                .get(index as usize)
                .cloned()
                .flatten()
                .unwrap_or_else(|| index.to_string())
        };
        let taken = names.iter().flatten().map(String::as_str);
        let name = if station_index == 0 {
            if last == 0 {
                "0".to_string()
            } else {
                name_before_start(&neighbour(1), taken)
            }
        } else if station_index == last {
            name_after_end(&neighbour(station_index - 1), taken)
        } else {
            let held: HashSet<&str> = taken.collect();
            name_with_free_letter(&neighbour(station_index - 1), &|n| held.contains(n))
        };
        self.station_names.insert(key, name);
    }

    /// Fill the unnamed stations of a partly named line by the creation rules
    /// (`station_naming::fill_line_station_names`): an import whose file names
    /// some stations, or rows the user added in an import wizard. A line with
    /// no name at all is left to the seeding rule. Returns how many names it
    /// filled.
    pub fn fill_station_names(&mut self, line_id: LineId) -> usize {
        let mut names = self.line_station_names(line_id);
        let filled = fill_line_station_names(&mut names);
        if filled == 0 {
            return 0;
        }
        for (index, name) in names.into_iter().enumerate() {
            let (Some(name), Some(key)) = (
                name,
                self.key_for_ref(StationRef::new(line_id, index as u32)),
            ) else {
                continue;
            };
            self.station_names.entry(key).or_insert(name);
        }
        filled
    }

    /// Make the names of one line unique: the later holder of a repeated
    /// name gets a letter suffix. Missing names stay missing (the next
    /// normalization seeds them). The final safety of a Combine Lines.
    pub fn uniquify_station_names_in(&mut self, line_id: LineId) {
        let Some(line) = self.get_line(line_id) else {
            return;
        };
        let keys = canonical_keys(line);
        let held: Vec<Option<String>> = keys
            .iter()
            .map(|(_, key)| self.station_names.get(key).cloned())
            .collect();
        let mut slots = held.clone();
        let (_, renamed) = settle_line_station_names(&mut slots);
        if renamed == 0 {
            return;
        }
        for (((_, key), before), after) in keys.into_iter().zip(held).zip(slots) {
            if let (Some(before), Some(after)) = (before, after) {
                if before != after {
                    self.station_names.insert(key, after);
                }
            }
        }
    }

    /// The Combine Lines prefixing plan (`station_naming::plan_combine_prefixes`)
    /// for combining `other` into `kept`, `other_join` being the other line's
    /// join station (its name is retired, so it collides with nothing).
    fn combine_prefix_plan(
        &self,
        kept: LineId,
        other: LineId,
        other_join: StationRef,
    ) -> CombinePrefixes {
        let kept_names: Vec<String> = self
            .line_station_names(kept)
            .into_iter()
            .flatten()
            .collect();
        let other_names: Vec<String> = self
            .line_station_names(other)
            .into_iter()
            .flatten()
            .collect();
        let join_name = self.station_name_at(other_join).map(str::to_string);
        plan_combine_prefixes(
            &kept_names.iter().map(String::as_str).collect::<Vec<_>>(),
            &other_names.iter().map(String::as_str).collect::<Vec<_>>(),
            join_name.as_deref(),
        )
    }

    /// Combine Lines' prefixing: rename the station names of `kept` and
    /// `other` in place, before the join, when the other line's names (its
    /// join station `other_join` left out) collide with the kept line's, or
    /// the kept line continues an earlier lettered combine
    /// ([`Self::combine_prefix_plan`]).
    pub(super) fn prefix_combined_station_names(
        &mut self,
        kept: LineId,
        other: LineId,
        other_join: StationRef,
    ) {
        let plan = self.combine_prefix_plan(kept, other, other_join);
        if plan.is_empty() {
            return;
        }
        for (key, name) in &mut self.station_names {
            let renames = if key.line_id == kept {
                &plan.kept
            } else if key.line_id == other {
                &plan.other
            } else {
                continue;
            };
            if let Some(renamed) = renames.get(name.as_str()) {
                name.clone_from(renamed);
            }
        }
    }

    /// What combining `other` into `kept` (joined at `kept_join` and
    /// `other_join`) will rename, for the Combine Lines dialog to say before
    /// the user confirms: `None` when no station is renamed. Each line's
    /// lettering is summarized by how many stations are lettered and how many
    /// keep their names, and the first and the last lettered station in its
    /// own station order; the other line's names made unique by the Mid rule
    /// are listed one by one (`suffixed`), and its join station is left out
    /// of everything (its name is retired: it becomes the joined station).
    /// `join_name` is the joined station's name after the combine (the kept
    /// join station's, prefixed when it is).
    pub fn combine_station_renames(
        &self,
        kept: LineId,
        other: LineId,
        kept_join: StationRef,
        other_join: StationRef,
    ) -> Option<crate::CombineStationRenames> {
        let plan = self.combine_prefix_plan(kept, other, other_join);
        let reason = match plan.reason? {
            CombinePrefixReason::Collision => crate::CombineRenameReason::Collision,
            CombinePrefixReason::Lettered => crate::CombineRenameReason::Lettered,
        };
        let retired = self.station_name_at(other_join).map(str::to_string);
        let no_suffixes = HashSet::new();
        let summarize = |line: LineId,
                         renames: &HashMap<String, String>,
                         suffixed: &HashSet<String>,
                         skip: Option<&str>|
         -> Option<crate::CombineLineRenames> {
            let names: Vec<String> = self
                .line_station_names(line)
                .into_iter()
                .flatten()
                .filter(|name| Some(name.as_str()) != skip)
                .collect();
            let lettered: Vec<(&String, &String)> = names
                .iter()
                .filter(|old| !suffixed.contains(*old))
                .filter_map(|old| renames.get(old).map(|new| (old, new)))
                .collect();
            let (first, last) = (lettered.first()?, lettered.last()?);
            let renamed = lettered.len().saturating_add(suffixed.len());
            Some(crate::CombineLineRenames {
                count: u32::try_from(lettered.len()).unwrap_or(u32::MAX),
                unchanged: u32::try_from(names.len().saturating_sub(renamed)).unwrap_or(u32::MAX),
                first_from: first.0.clone(),
                first_to: first.1.clone(),
                last_from: last.0.clone(),
                last_to: last.1.clone(),
            })
        };
        let kept_renames = summarize(kept, &plan.kept, &no_suffixes, None);
        let other_renames = summarize(other, &plan.other, &plan.other_suffixed, retired.as_deref());
        let suffixed: Vec<crate::CombineStationRename> = self
            .line_station_names(other)
            .into_iter()
            .flatten()
            .filter(|old| plan.other_suffixed.contains(old))
            .filter_map(|old| {
                let to = plan.other.get(&old)?.clone();
                Some(crate::CombineStationRename { from: old, to })
            })
            .collect();
        if kept_renames.is_none() && other_renames.is_none() && suffixed.is_empty() {
            return None;
        }
        let join = self.station_name_at(kept_join)?.to_string();
        let join_name = plan.kept.get(&join).cloned().unwrap_or(join);
        Some(crate::CombineStationRenames {
            reason,
            kept: kept_renames,
            other: other_renames,
            suffixed: (!suffixed.is_empty()).then_some(suffixed),
            join_name,
        })
    }

    // === User edits ===

    /// Validate a station name the user typed for `station`: trimmed, not
    /// empty, and not a name another station of its line holds. Returns the
    /// trimmed name.
    pub fn validate_station_name(&self, station: StationRef, name: &str) -> Result<String, String> {
        let line = self
            .get_line(station.line_id)
            .ok_or_else(|| crate::LINE_GONE.to_string())?;
        if station.station_index as usize > line.shots.len() {
            return Err("Station not found".to_string());
        }
        let name = name.trim();
        if name.is_empty() {
            return Err("A station name cannot be empty".to_string());
        }
        if let Some(refusal) = name_characters_refusal("station", name) {
            return Err(refusal);
        }
        let names = self.line_station_names(station.line_id);
        let clash = names
            .iter()
            .enumerate()
            .any(|(i, n)| i as u32 != station.station_index && n.as_deref() == Some(name));
        if clash {
            let line_name = self.line_label(station.line_id).unwrap_or_default();
            return Err(format!(
                "Line \"{line_name}\" already has a station named \"{name}\""
            ));
        }
        Ok(name.to_string())
    }

    /// Set a station's name after [`Self::validate_station_name`]. Returns the
    /// name it replaced.
    pub fn set_station_name(
        &mut self,
        station: StationRef,
        name: &str,
    ) -> Result<Option<String>, String> {
        let name = self.validate_station_name(station, name)?;
        let key = self
            .key_for_ref(station)
            .ok_or_else(|| "Station cannot be named: its shot has no identity".to_string())?;
        let previous = self.station_names.insert(key, name);
        self.touch();
        Ok(previous)
    }

    /// Validate a line name the user typed: trimmed, not empty, and not the
    /// name of another line (`except` is the line being renamed). Returns
    /// the trimmed name.
    pub fn validate_line_name(&self, name: &str, except: Option<LineId>) -> Result<String, String> {
        let except: &[LineId] = match &except {
            Some(id) => std::slice::from_ref(id),
            None => &[],
        };
        self.validate_line_name_replacing(name, except)
    }

    /// [`Self::validate_line_name`] for a name that replaces the lines
    /// `replacing` (a Split's halves replace the line split, a Combine's line
    /// replaces both lines): their names are no collision.
    pub fn validate_line_name_replacing(
        &self,
        name: &str,
        replacing: &[LineId],
    ) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A line name cannot be empty".to_string());
        }
        if let Some(refusal) = name_characters_refusal("line", name) {
            return Err(refusal);
        }
        let taken = self.lines.iter().any(|line| {
            !replacing.contains(&line.id) && line.name.as_deref().map(str::trim) == Some(name)
        });
        if taken {
            return Err(format!("A line named \"{name}\" already exists"));
        }
        Ok(name.to_string())
    }

    /// The names of the lines an operation makes in place of `replacing`
    /// (a Split's two halves, a Combine's one line), each validated as
    /// [`Self::validate_line_name_replacing`] and distinct from the ones
    /// before it: the same refusals as a rename. Returns them trimmed.
    pub fn validate_replacement_line_names(
        &self,
        names: &[&str],
        replacing: &[LineId],
    ) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = Vec::with_capacity(names.len());
        for name in names {
            let name = self.validate_line_name_replacing(name, replacing)?;
            if out.contains(&name) {
                return Err(format!("A line named \"{name}\" already exists"));
            }
            out.push(name);
        }
        Ok(out)
    }

    /// Make the names of the lines an operation just touched (`touched`: a
    /// split's halves, a combined line) unique, renaming THEM rather than an
    /// unrelated line: a touched line whose (trimmed) name another line
    /// holds, or an earlier touched one, gets ` (2)`, ` (3)`, ...; an unnamed
    /// one is left to the seeding rule. The backstop behind the validation of
    /// a name the user typed, run before the normalization (which would
    /// otherwise rename the later holder in line order, whichever it is).
    /// Returns how many lines it renamed.
    pub fn dedupe_touched_line_names(&mut self, touched: &[LineId]) -> usize {
        let mut taken: HashSet<String> = self
            .lines
            .iter()
            .filter(|line| !touched.contains(&line.id))
            .filter_map(|line| line.name.as_deref())
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let mut renamed = 0;
        for id in touched {
            let Some(line) = self.get_line_mut(*id) else {
                continue;
            };
            let Some(name) = line
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
            else {
                continue;
            };
            let name = name.to_string();
            let fresh = if taken.contains(&name) {
                renamed += 1;
                dedupe_line_name(&name, &|n| taken.contains(n))
            } else {
                name
            };
            taken.insert(fresh.clone());
            line.name = Some(fresh);
        }
        renamed
    }

    /// Offset every Station ID, and `next_station_id`, by `by`. Add .stalx /
    /// Merge projects shift the incoming project's ids past the target's
    /// `next_station_id`.
    pub fn offset_station_ids(&mut self, by: StationId) {
        // Saturating at the bound: an id pushed past it is invalid, and the
        // normalization re-mints it (renumbering when there is no room).
        let shift = |id: StationId| id.saturating_add(by).min(MAX_STATION_ID);
        for id in self.station_ids.values_mut() {
            *id = shift(*id);
        }
        self.next_station_id = shift(self.next_station_id);
    }
}
