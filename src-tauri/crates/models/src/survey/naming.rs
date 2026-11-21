//! Station names (plan 5.3, per line) and shot-uuid identity.
//!
//! A line shows each of its stations by the name that line knows it by
//! ([`Project::name_on`]): the station's own name, except on a line one of
//! its aliases names (the line that knew the station by that alias: a 0.6.0
//! merge member's line, a merge's moving line, a line an import joined onto
//! it, a half of a split), which shows the alias's name. A junction keeps one
//! Station ID; each line keeps its own name for it, so a merge, a
//! conversion, a detach or a delete changes no name any line shows (review
//! of the real-app demo, I-3; 0.6.0 kept each line's own name for its
//! member). Invariant 8: no line shows two stations by one name. Repeats a
//! command makes are resolved by the existing letter-suffix rule
//! ([`crate::station_naming::name_with_free_letter`]): `5` -> `5a`,
//! `Junction` -> `Junction-a`.
//!
//! A rename addresses a slot ([`Project::set_name_on`]): on a line that knows
//! the station by an alias it renames that alias, which only that line
//! shows; elsewhere it renames the station, which every line without an alias
//! of its own shows ([`Project::lines_sharing_name_on`]), so the new name is
//! checked on all of them ([`Project::conflicts_with_name_on`]).

use std::collections::BTreeSet;

use crate::station_naming::name_with_free_letter;
use crate::{LineId, StationId, StationRef};

use super::{Project, ShotLocation, StationAlias};

/// Another station holding the same name on a shared line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameConflict {
    /// A line both stations are on.
    pub line: LineId,
    /// The other station.
    pub other: StationId,
}

impl Project {
    /// The alias by which `line` knows `station`: the last of its aliases
    /// whose `line` names it (the latest merge), if any.
    pub fn line_alias(&self, station: StationId, line: LineId) -> Option<&StationAlias> {
        self.stations
            .get(&station)?
            .aliases
            .iter()
            .rev()
            .find(|a| a.line == Some(line))
    }

    /// Whether `line` knows `station` by one of its aliases
    /// ([`Self::line_alias`]).
    pub fn known_by_alias_on(&self, station: StationId, line: LineId) -> bool {
        self.line_alias(station, line).is_some()
    }

    /// The name `line` shows `station` by: the name of the alias the line
    /// knows it by when that alias has one, else the station's own name.
    /// `None` when the station does not exist.
    pub fn name_on(&self, station: StationId, line: LineId) -> Option<&str> {
        let s = self.stations.get(&station)?;
        let alias = s.aliases.iter().rev().find(|a| a.line == Some(line));
        Some(alias.and_then(|a| a.name.as_deref()).unwrap_or(&s.name))
    }

    /// The name the line of `slot` shows the station there by
    /// ([`Self::name_on`]); `None` for a slot that does not exist.
    pub fn slot_name(&self, slot: StationRef) -> Option<&str> {
        let station = self
            .line(slot.line_id)?
            .station(slot.station_index as usize)?;
        self.name_on(station, slot.line_id)
    }

    /// Set the name `line` shows `station` by: the alias the line knows it
    /// by when it has one (only that line shows it), else the station's own
    /// name (every line in [`Self::lines_sharing_name_on`] shows it). No
    /// checks.
    pub fn set_name_on(&mut self, station: StationId, line: LineId, name: String) {
        let Some(s) = self.stations.get_mut(&station) else {
            return;
        };
        match s.aliases.iter_mut().rev().find(|a| a.line == Some(line)) {
            Some(alias) => alias.name = Some(name),
            None => s.name = name,
        }
    }

    /// The lines of `station` that show its own name: every line it is on
    /// but those that know it by an alias, in list order.
    pub fn lines_showing_name(&self, station: StationId) -> Vec<LineId> {
        let topology = self.topology();
        topology
            .lines_of(station)
            .into_iter()
            .filter(|&line| !self.known_by_alias_on(station, line))
            .collect()
    }

    /// The lines that show the name `line` shows `station` by: `line` alone
    /// when it knows the station by an alias, else every line showing the
    /// station's own name ([`Self::lines_showing_name`]).
    pub fn lines_sharing_name_on(&self, station: StationId, line: LineId) -> Vec<LineId> {
        if self.known_by_alias_on(station, line) {
            vec![line]
        } else {
            self.lines_showing_name(station)
        }
    }

    /// The stations other than `station` that `lines` show by `name`.
    fn conflicts_on_lines(
        &self,
        station: StationId,
        lines: &[LineId],
        name: &str,
    ) -> Vec<NameConflict> {
        let topology = self.topology();
        let mut out = Vec::new();
        for &line in lines {
            for &other in topology.chain(line).unwrap_or(&[]) {
                if other != station
                    && self.name_on(other, line) == Some(name)
                    && !out.contains(&NameConflict { line, other })
                {
                    out.push(NameConflict { line, other });
                }
            }
        }
        out
    }

    /// The stations that share `station`'s own name on a line showing it,
    /// by line (list order), then chain order.
    pub fn name_conflicts(&self, station: StationId) -> Vec<NameConflict> {
        match self.stations.get(&station) {
            Some(s) => self.conflicts_with_name(station, &s.name),
            None => Vec::new(),
        }
    }

    /// The stations already shown by `name` on a line that shows `station`'s
    /// own name: what renaming the station itself to `name` would collide
    /// with.
    pub fn conflicts_with_name(&self, station: StationId, name: &str) -> Vec<NameConflict> {
        self.conflicts_on_lines(station, &self.lines_showing_name(station), name)
    }

    /// What renaming `station` through its slot on `line` to `name` would
    /// collide with: the stations shown by `name` on the lines that show the
    /// renamed name ([`Self::lines_sharing_name_on`]).
    pub fn conflicts_with_name_on(
        &self,
        station: StationId,
        line: LineId,
        name: &str,
    ) -> Vec<NameConflict> {
        self.conflicts_on_lines(station, &self.lines_sharing_name_on(station, line), name)
    }

    /// Every name shown on `lines` by a station other than `station`.
    fn names_on_lines(&self, station: StationId, lines: &[LineId]) -> BTreeSet<String> {
        let topology = self.topology();
        let mut taken = BTreeSet::new();
        for &line in lines {
            for &other in topology.chain(line).unwrap_or(&[]) {
                if other != station {
                    if let Some(name) = self.name_on(other, line) {
                        taken.insert(name.to_string());
                    }
                }
            }
        }
        taken
    }

    /// Every name `station`'s own name must not repeat: the names the lines
    /// showing it ([`Self::lines_showing_name`]) show their other stations
    /// by.
    pub fn names_around(&self, station: StationId) -> BTreeSet<String> {
        self.names_on_lines(station, &self.lines_showing_name(station))
    }

    /// Every name the name `line` shows `station` by must not repeat
    /// ([`Self::lines_sharing_name_on`]).
    pub fn names_around_on(&self, station: StationId, line: LineId) -> BTreeSet<String> {
        self.names_on_lines(station, &self.lines_sharing_name_on(station, line))
    }

    /// If `station`'s own name collides on a line showing it, give it the
    /// first free letter suffix of its name (free on every such line) and
    /// return the new name; `None` when it was already unique (or does not
    /// exist).
    pub fn uniquify_station_name(&mut self, station: StationId) -> Option<String> {
        let current = self.stations.get(&station)?.name.clone();
        let taken = self.names_around(station);
        if !taken.contains(&current) {
            return None;
        }
        let fresh = name_with_free_letter(&current, &|n| taken.contains(n));
        if let Some(s) = self.stations.get_mut(&station) {
            s.name.clone_from(&fresh);
        }
        Some(fresh)
    }

    /// If the name `line` shows `station` by collides on a line that shows
    /// it, give it the first free letter suffix (free on every such line)
    /// and return the new name; `None` when it was already unique.
    pub fn uniquify_name_on(&mut self, station: StationId, line: LineId) -> Option<String> {
        let current = self.name_on(station, line)?.to_string();
        let taken = self.names_around_on(station, line);
        if !taken.contains(&current) {
            return None;
        }
        let fresh = name_with_free_letter(&current, &|n| taken.contains(n));
        self.set_name_on(station, line, fresh.clone());
        Some(fresh)
    }

    /// Make the names line `line` shows unique in chain order: the first
    /// holder of a name keeps it, a later one gets a letter suffix that is
    /// free on every line showing the renamed name. Returns the renamed
    /// stations in chain order.
    pub fn uniquify_line_names(&mut self, line: LineId) -> Vec<StationId> {
        let Some(chain) = self.topology().chain(line).map(<[StationId]>::to_vec) else {
            return Vec::new();
        };
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut renamed = Vec::new();
        for station in chain {
            let Some(name) = self.name_on(station, line).map(str::to_string) else {
                continue;
            };
            if seen.insert(name.clone()) {
                continue;
            }
            let mut taken = self.names_around_on(station, line);
            taken.extend(seen.iter().cloned());
            let fresh = name_with_free_letter(&name, &|n| taken.contains(n));
            self.set_name_on(station, line, fresh.clone());
            seen.insert(fresh);
            renamed.push(station);
        }
        renamed
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
