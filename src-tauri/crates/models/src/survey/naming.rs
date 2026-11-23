//! Station names (RS, plan 2026-11-23) and shot-uuid identity.
//!
//! Every line shows a node by its **shown identity** ([`Project::shown`]):
//! the node's own identity, or, for a loop end a closure archives, the kept
//! end's. A merged junction shows its kept station on every line; the
//! stations a merge did not keep are records, shown nowhere, whose names
//! count toward no uniqueness. Invariant N8: on each line, slots that show
//! different identities have different names (two slots of one identity on
//! a line, a same-line closure, are allowed).
//!
//! A rename addresses the shown identity, so it is checked on every line
//! that identity is shown on ([`Project::conflicts_with_name`]). Repeats an
//! operation makes are settled by **rule L** ([`Project::letter_collisions`],
//! RS 2): the colliding identity shown on fewer lines takes the existing
//! letter suffix ([`crate::station_naming::name_with_free_letter`]: `5` ->
//! `5a`, `Junction` -> `Junction-a`); on a tie the one new on the line does;
//! an identity shown on more lines than the one it collides with is never
//! lettered, and chain order never decides.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::station_naming::name_with_free_letter;
use crate::{LineId, StationId, StationRef};

use super::{Project, ShotLocation, ShownIndex, Topology};

/// Another station holding the same name on a shared line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameConflict {
    /// A line both stations are shown on.
    pub line: LineId,
    /// The other station (the identity it shows).
    pub other: StationId,
}

/// An N8 collision: on `line`, `name` is shown by several identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameCollision {
    pub line: LineId,
    pub name: String,
    /// The identities (their Station IDs) shown by `name` on the line, in
    /// chain order of their first slot.
    pub identities: Vec<StationId>,
}

/// A station rule L gave a letter suffix (RS 2), for the op's notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetteredStation {
    /// The identity renamed (its Station ID).
    pub station: StationId,
    /// The line it collided on.
    pub line: LineId,
    /// The identity it collided with, which kept the name.
    pub other: StationId,
    pub from: String,
    pub to: String,
}

impl Project {
    /// The name `node` is shown by on every line through it: its shown
    /// identity's name. `None` when the station does not exist.
    pub fn shown_name(&self, node: StationId) -> Option<&str> {
        self.shown_station(node).map(|s| s.name.as_str())
    }

    /// The name the slot shows ([`Self::shown_name`] of its node); `None`
    /// for a slot that does not exist.
    pub fn slot_name(&self, slot: StationRef) -> Option<&str> {
        let station = self
            .line(slot.line_id)?
            .station(slot.station_index as usize)?;
        self.shown_name(station)
    }

    /// Rename the identity `node` shows (every line it is shown on shows
    /// the new name). No checks.
    pub fn set_shown_name(&mut self, node: StationId, name: String) {
        let identity = self.shown(node);
        if let Some(s) = self.stations.get_mut(&identity) {
            s.name = name;
        }
    }

    /// The comment `node` is shown with: its shown identity's (RS 5.1).
    pub fn shown_comment(&self, node: StationId) -> Option<&str> {
        self.shown_station(node)?.comment.as_deref()
    }

    /// Set the comment of the identity `node` shows (a loop end's edit goes
    /// to the kept end's identity, RS 5.4); an empty one is none. No checks.
    pub fn set_shown_comment(&mut self, node: StationId, comment: Option<String>) {
        let identity = self.shown(node);
        if let Some(s) = self.stations.get_mut(&identity) {
            s.comment = comment.filter(|text| !text.is_empty());
        }
    }

    /// Every `(shown identity, line)` pair of this state, for rule L's "new
    /// on the line".
    pub(super) fn shown_pairs(&self) -> BTreeSet<(StationId, LineId)> {
        let topology = self.topology();
        let index = self.shown_index();
        let mut out = BTreeSet::new();
        for (position, line) in self.lines.iter().enumerate() {
            for &node in topology.chain_at(position).unwrap_or(&[]) {
                out.insert((index.shown(node), line.id));
            }
        }
        out
    }

    /// The identities other than `identity` that `lines` show by `name`.
    fn conflicts_on_lines(
        &self,
        identity: StationId,
        lines: &[LineId],
        name: &str,
    ) -> Vec<NameConflict> {
        let topology = self.topology();
        let index = self.shown_index();
        let mut out = Vec::new();
        for &line in lines {
            for &node in topology.chain(line).unwrap_or(&[]) {
                let other = index.shown(node);
                if other != identity
                    && self.stations.get(&other).is_some_and(|s| s.name == name)
                    && !out.contains(&NameConflict { line, other })
                {
                    out.push(NameConflict { line, other });
                }
            }
        }
        out
    }

    /// The stations sharing the name `node` shows, on a line that shows it,
    /// by line (list order), then chain order.
    pub fn name_conflicts(&self, node: StationId) -> Vec<NameConflict> {
        match self.shown_station(node) {
            Some(s) => self.conflicts_with_name(node, &s.name),
            None => Vec::new(),
        }
    }

    /// What renaming the identity `node` shows to `name` would collide with:
    /// the other identities shown by `name` on a line that identity is shown
    /// on.
    pub fn conflicts_with_name(&self, node: StationId, name: &str) -> Vec<NameConflict> {
        let identity = self.shown(node);
        self.conflicts_on_lines(identity, &self.lines_shown_on(identity), name)
    }

    /// Every name the identity `node` shows must not repeat: the names the
    /// lines it is shown on show other identities by.
    pub fn names_around(&self, node: StationId) -> BTreeSet<String> {
        let identity = self.shown(node);
        let topology = self.topology();
        let index = self.shown_index();
        let mut taken = BTreeSet::new();
        for line in self.lines_shown_on(identity) {
            for &other in topology.chain(line).unwrap_or(&[]) {
                let shown = index.shown(other);
                if shown != identity {
                    if let Some(s) = self.stations.get(&shown) {
                        taken.insert(s.name.clone());
                    }
                }
            }
        }
        taken
    }

    /// If the name `node` shows collides on a line it is shown on, give its
    /// identity the first free letter suffix (free on every such line) and
    /// return the new name; `None` when it was already unique.
    pub fn uniquify_station_name(&mut self, node: StationId) -> Option<String> {
        let current = self.shown_name(node)?.to_string();
        let taken = self.names_around(node);
        if !taken.contains(&current) {
            return None;
        }
        let fresh = name_with_free_letter(&current, &|n| taken.contains(n));
        self.set_shown_name(node, fresh.clone());
        Some(fresh)
    }

    /// Make the names line `line` shows unique in chain order: the first
    /// identity showing a name keeps it, a later one gets a letter suffix
    /// free on every line it is shown on. Returns the renamed identities in
    /// chain order. No operation uses this chain-order choice any more:
    /// Combine, the 0.6.0 migration and the earlier version 2 upgrade all
    /// settle repeats by rule L ([`Self::letter_collisions`], RS 4.4, 8.2,
    /// 8.3).
    pub fn uniquify_line_names(&mut self, line: LineId) -> Vec<StationId> {
        let Some(chain) = self.topology().chain(line).map(<[StationId]>::to_vec) else {
            return Vec::new();
        };
        let mut seen: BTreeMap<String, StationId> = BTreeMap::new();
        let mut renamed = Vec::new();
        for node in chain {
            let identity = self.shown(node);
            let Some(name) = self.shown_name(identity).map(str::to_string) else {
                continue;
            };
            match seen.get(&name) {
                None => {
                    seen.insert(name, identity);
                    continue;
                }
                Some(&first) if first == identity => continue,
                Some(_) => {}
            }
            let mut taken = self.names_around(identity);
            taken.extend(seen.keys().cloned());
            let fresh = name_with_free_letter(&name, &|n| taken.contains(n));
            self.set_shown_name(identity, fresh.clone());
            seen.insert(fresh, identity);
            renamed.push(identity);
        }
        renamed
    }

    /// The N8 repeats of the line at list `position`, in chain order: each
    /// `(name, first, second)` where a slot shows identity `second` by the
    /// name identity `first` showed at an earlier slot (each pair once).
    pub(super) fn line_name_repeats<'a>(
        &'a self,
        topology: &'a Topology,
        index: &ShownIndex,
        position: usize,
    ) -> Vec<(&'a str, StationId, StationId)> {
        let Some(chain) = topology.chain_at(position) else {
            return Vec::new();
        };
        let named = |node: &StationId| {
            let identity = index.shown(*node);
            self.stations
                .get(&identity)
                .map(|s| (s.name.as_str(), identity))
        };
        // Fast path: no name on two identities of the chain.
        let mut sorted: Vec<(&str, StationId)> = chain.iter().filter_map(named).collect();
        sorted.sort_unstable();
        if sorted
            .windows(2)
            .all(|w| w[0].0 != w[1].0 || w[0].1 == w[1].1)
        {
            return Vec::new();
        }
        let mut seen: BTreeMap<&str, StationId> = BTreeMap::new();
        // Each pair once (a same-line closure shows one identity at two
        // slots), by a set: a crafted line of one name repeated by every
        // station must not cost a scan of the repeats per slot.
        let mut reported: HashSet<(StationId, StationId)> = HashSet::new();
        let mut out: Vec<(&str, StationId, StationId)> = Vec::new();
        for node in chain {
            let Some((name, identity)) = named(node) else {
                continue;
            };
            match seen.get(name) {
                Some(&first) if first != identity => {
                    if reported.insert((first, identity)) {
                        out.push((name, first, identity));
                    }
                }
                Some(_) => {}
                None => {
                    seen.insert(name, identity);
                }
            }
        }
        out
    }

    /// The N8 collisions of the line at list `position`: each name its slots
    /// show more than one identity by, with those identities.
    fn line_collisions(
        &self,
        topology: &Topology,
        index: &ShownIndex,
        position: usize,
    ) -> Vec<NameCollision> {
        let Some(line) = self.lines.get(position) else {
            return Vec::new();
        };
        let mut out: Vec<NameCollision> = Vec::new();
        for (name, first, second) in self.line_name_repeats(topology, index, position) {
            match out.iter_mut().find(|c| c.name == name) {
                Some(collision) => {
                    if !collision.identities.contains(&second) {
                        collision.identities.push(second);
                    }
                }
                None => out.push(NameCollision {
                    line: line.id,
                    name: name.to_string(),
                    identities: vec![first, second],
                }),
            }
        }
        out
    }

    /// Every N8 collision (RS 3), by line (list order), then chain order.
    pub fn name_collisions(&self) -> Vec<NameCollision> {
        let topology = self.topology();
        let index = self.shown_index();
        (0..self.lines.len())
            .flat_map(|position| self.line_collisions(topology, &index, position))
            .collect()
    }

    /// Rule L (RS 2): settle every N8 collision by lettering, of the
    /// identities sharing a name on a line, all but the one shown on the
    /// most lines (on a tie: the one that already showed on the line in
    /// `before`, then the lowest Station ID). Each letter is free on every
    /// line the lettered identity is shown on, so no letter makes a new
    /// collision and the pass ends. Returns each letter, in the order given.
    pub fn letter_collisions(&mut self, before: &Project) -> Vec<LetteredStation> {
        self.letter_collisions_with(|| before.shown_pairs())
    }

    /// [`Self::letter_collisions`] where "already shown on the line" is
    /// given as the `(identity, line)` pairs the state before showed,
    /// computed only once a collision is found: for an upgrade whose state
    /// before is not a model of this kind (a 0.6.0 file's slots, an earlier
    /// version 2 file's per-line names, plan 2026-11-23 RS 8.2 and 8.3).
    pub fn letter_collisions_with(
        &mut self,
        before: impl FnOnce() -> BTreeSet<(StationId, LineId)>,
    ) -> Vec<LetteredStation> {
        let mut out = Vec::new();
        let mut before = Some(before);
        let mut before_pairs: Option<BTreeSet<(StationId, LineId)>> = None;
        loop {
            let collisions = self.name_collisions();
            if collisions.is_empty() {
                break;
            }
            if before_pairs.is_none() {
                before_pairs = before.take().map(|f| f());
            }
            let Some(pairs) = before_pairs.as_ref() else {
                break;
            };
            let mut changed = false;
            for collision in collisions {
                // An earlier letter of this pass may have settled it.
                let holders: Vec<StationId> = collision
                    .identities
                    .iter()
                    .copied()
                    .filter(|&id| self.shown_name(id) == Some(collision.name.as_str()))
                    .collect();
                if holders.len() < 2 {
                    continue;
                }
                let rank = |id: StationId| {
                    (
                        self.lines_shown_on(id).len(),
                        pairs.contains(&(id, collision.line)),
                        std::cmp::Reverse(id),
                    )
                };
                let Some(keeper) = holders.iter().copied().max_by_key(|&id| rank(id)) else {
                    continue;
                };
                for identity in holders.into_iter().filter(|&id| id != keeper) {
                    let taken = self.names_around(identity);
                    let fresh = name_with_free_letter(&collision.name, &|n| taken.contains(n));
                    self.set_shown_name(identity, fresh.clone());
                    changed = true;
                    out.push(LetteredStation {
                        station: identity,
                        line: collision.line,
                        other: keeper,
                        from: collision.name.clone(),
                        to: fresh,
                    });
                }
            }
            if !changed {
                break;
            }
        }
        out
    }

    /// The dialogs' other answer to each of `letters` (RS 4.1 step 6): the
    /// identity that kept the name is lettered instead, with a letter free on
    /// every line it is shown on once the lettered identity has its name
    /// back. Called on the state rule L left (`letters` applied); each entry
    /// names the identity to rename (`station`), the one keeping the name
    /// (`other`), and the line, in the order of `letters`.
    pub fn alternative_letters(&self, letters: &[LetteredStation]) -> Vec<LetteredStation> {
        if letters.is_empty() {
            return Vec::new();
        }
        let mut unlettered = self.clone();
        for letter in letters.iter().rev() {
            if let Some(s) = unlettered.stations.get_mut(&letter.station) {
                s.name.clone_from(&letter.from);
            }
        }
        letters
            .iter()
            .map(|letter| {
                let mut taken = unlettered.names_around(letter.other);
                taken.insert(letter.from.clone());
                let to = name_with_free_letter(&letter.from, &|n| taken.contains(n));
                LetteredStation {
                    station: letter.other,
                    line: letter.line,
                    other: letter.station,
                    from: letter.from.clone(),
                    to,
                }
            })
            .collect()
    }

    /// Whether some shot already has `uuid`.
    pub fn is_shot_uuid_taken(&self, uuid: &str) -> bool {
        self.shot_uuids().contains(uuid)
    }

    /// Where the shot with `uuid` is.
    pub fn shot_by_uuid(&self, uuid: &str) -> Option<ShotLocation> {
        self.shot_uuids().get(uuid)
    }

    /// Every shot uuid held by more than one shot, with all its places.
    pub fn duplicate_shot_uuids(&self) -> Vec<(String, Vec<ShotLocation>)> {
        self.shot_uuids()
            .duplicates()
            .map(|(uuid, locations)| (uuid.to_string(), locations.to_vec()))
            .collect()
    }

    /// A fresh v4 shot uuid no shot of the project has.
    pub fn mint_shot_uuid(&self) -> String {
        loop {
            let candidate = crate::Shot::uuid_or_mint(None);
            if !self.is_shot_uuid_taken(&candidate) {
                return candidate;
            }
        }
    }
}
