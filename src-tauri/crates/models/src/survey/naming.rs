//! Station-name uniqueness (plan 5.3) and shot-uuid identity.
//!
//! A station's name must be unique among the stations of every line it is on;
//! a junction is checked on all of its lines. Repeats are resolved by the
//! existing letter-suffix rule
//! ([`crate::station_naming::name_with_free_letter`]): `5` -> `5a`,
//! `Junction` -> `Junction-a`.

use std::collections::BTreeSet;

use crate::station_naming::name_with_free_letter;
use crate::{LineId, StationId};

use super::{Project, ShotLocation};

/// Another station holding the same name on a shared line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameConflict {
    /// A line both stations are on.
    pub line: LineId,
    /// The other station.
    pub other: StationId,
}

impl Project {
    /// The stations that share `station`'s name on a line it is on, by line
    /// (list order), then chain order.
    pub fn name_conflicts(&self, station: StationId) -> Vec<NameConflict> {
        match self.stations.get(&station) {
            Some(s) => self.conflicts_with_name(station, &s.name),
            None => Vec::new(),
        }
    }

    /// The stations that already hold `name` on a line `station` is on: what
    /// renaming `station` to `name` would collide with.
    pub fn conflicts_with_name(&self, station: StationId, name: &str) -> Vec<NameConflict> {
        let topology = self.topology();
        let mut out = Vec::new();
        for line in topology.lines_of(station) {
            for &other in topology.chain(line).unwrap_or(&[]) {
                if other != station
                    && self.stations.get(&other).is_some_and(|s| s.name == name)
                    && !out.contains(&NameConflict { line, other })
                {
                    out.push(NameConflict { line, other });
                }
            }
        }
        out
    }

    /// Every name held by another station on a line `station` is on.
    pub fn names_around(&self, station: StationId) -> BTreeSet<String> {
        let topology = self.topology();
        let mut taken = BTreeSet::new();
        for line in topology.lines_of(station) {
            for &other in topology.chain(line).unwrap_or(&[]) {
                if other != station {
                    if let Some(s) = self.stations.get(&other) {
                        taken.insert(s.name.clone());
                    }
                }
            }
        }
        taken
    }

    /// If `station`'s name collides on one of its lines, give it the first free
    /// letter suffix of its name (free on every line it is on) and return the
    /// new name; `None` when it was already unique (or does not exist).
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

    /// Make the names on line `line` unique in chain order: the first holder of
    /// a name keeps it, a later one gets a letter suffix that is free on every
    /// line it is on. Returns the renamed stations in chain order.
    pub fn uniquify_line_names(&mut self, line: LineId) -> Vec<StationId> {
        let Some(chain) = self.topology().chain(line).map(<[StationId]>::to_vec) else {
            return Vec::new();
        };
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut renamed = Vec::new();
        for station in chain {
            let Some(name) = self.stations.get(&station).map(|s| s.name.clone()) else {
                continue;
            };
            if seen.insert(name.clone()) {
                continue;
            }
            let mut taken = self.names_around(station);
            taken.extend(seen.iter().cloned());
            let fresh = name_with_free_letter(&name, &|n| taken.contains(n));
            if let Some(s) = self.stations.get_mut(&station) {
                s.name.clone_from(&fresh);
            }
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
