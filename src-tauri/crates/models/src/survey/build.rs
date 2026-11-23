//! Low-level construction API: create a project, mint ids, add stations and
//! lines, push datums, placements and closures, resolve slots.
//!
//! Not command-level: nothing here refuses a user action, moves an anchor or
//! re-checks the invariants. Callers (the command layer, the file decoder, the
//! migration, tests) build a state and then run [`Project::validate`]; a
//! command also runs [`Project::settle_anchors`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, Utc};

use crate::station_naming::name_with_free_letter;
use crate::{
    AzimuthReference, LineId, SectionId, ShotData, StationId, StationRef, MAX_LINE_ID,
    MAX_SECTION_ID, MAX_STATION_ID,
};

use super::{
    Caches, ClosureEnd, ClosureId, ClosureStrategy, Counters, Line, LineMeta, LoopClosure,
    Placement, Project, ProjectSettings, Shot, Station, ZeroDatum,
};

/// Where a new line starts ([`Project::add_line`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartAt {
    /// A new station, named by the seed rule.
    New,
    /// An existing station: the new line joins it (a junction).
    Station(StationId),
}

/// What [`Project::drop_stale_attachments`] cleared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaleAttachments {
    /// Datums whose `via` was cleared.
    pub datum_vias: Vec<usize>,
    /// Closure ends whose `via` was cleared.
    pub closure_vias: Vec<(ClosureId, ClosureEnd)>,
    /// Hand moves dropped, by `(line, station)`.
    pub hand_moves: Vec<(LineId, StationId)>,
    /// Archived record lines cleared (A2), by `(station, record id, line)`.
    /// A record left with no line is history (A5).
    pub archived_lines: Vec<(StationId, StationId, LineId)>,
}

/// Why a construction call was refused. Nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// The station does not exist.
    UnknownStation(StationId),
    /// The line does not exist.
    UnknownLine(LineId),
    /// A station with this id already exists, or the id is an archived
    /// record's.
    StationIdTaken(StationId),
    /// The id is at or above [`MAX_STATION_ID`].
    StationIdOutOfRange(StationId),
    /// No Station ID is left below [`MAX_STATION_ID`].
    StationIdsExhausted,
    /// No Section id is left below [`MAX_SECTION_ID`].
    SectionIdsExhausted,
    /// The next line id would exceed [`MAX_LINE_ID`].
    LineIdsExhausted,
    /// A line with this id already exists.
    LineIdTaken(LineId),
    /// The shot uuid is already used by another shot (or twice in the input).
    ShotUuidTaken(String),
    /// The chain would pass this station twice.
    RepeatedStation(StationId),
    /// A chain needs exactly one station more than it has shots.
    ChainLength { stations: usize, shots: usize },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownStation(id) => write!(f, "station {id} does not exist"),
            Self::UnknownLine(id) => write!(f, "line {id} does not exist"),
            Self::StationIdTaken(id) => write!(f, "station id {id} is already in use"),
            Self::StationIdOutOfRange(id) => {
                write!(f, "station id {id} is not below {MAX_STATION_ID}")
            }
            Self::StationIdsExhausted => write!(f, "no station id is left"),
            Self::SectionIdsExhausted => write!(f, "no Section id is left"),
            Self::LineIdsExhausted => write!(f, "no line id is left (limit {MAX_LINE_ID})"),
            Self::LineIdTaken(id) => write!(f, "line id {id} is already in use"),
            Self::ShotUuidTaken(uuid) => write!(f, "shot uuid {uuid:?} is already in use"),
            Self::RepeatedStation(id) => write!(f, "the chain passes station {id} twice"),
            Self::ChainLength { stations, shots } => write!(
                f,
                "a chain of {shots} shots needs {} stations, got {stations}",
                shots + 1
            ),
        }
    }
}

impl std::error::Error for BuildError {}

impl Project {
    /// An empty project created now.
    pub fn new(name: impl Into<String>, location: Option<String>) -> Self {
        Self::new_at(name, location, Utc::now())
    }

    /// An empty project created (and last modified) at `created_at`.
    pub fn new_at(
        name: impl Into<String>,
        location: Option<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            name: name.into(),
            location,
            created_at,
            modified_at: created_at,
            stations: BTreeMap::new(),
            lines: Vec::new(),
            datums: Vec::new(),
            placements: Vec::new(),
            closures: Vec::new(),
            sections: Vec::new(),
            settings: ProjectSettings::default(),
            imported_file_hashes: Vec::new(),
            imported_line_hashes: Vec::new(),
            counters: Counters::default(),
            file_path: None,
            caches: Caches::default(),
        }
    }

    /// Mark the project modified now.
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
    }

    // ----- ids --------------------------------------------------------------

    /// The id the next line gets: one above the highest line id, `0` for a
    /// project without lines (derived, never stored).
    pub fn next_line_id(&self) -> LineId {
        self.lines
            .iter()
            .map(|line| line.id.saturating_add(1))
            .max()
            .unwrap_or(0)
    }

    /// Hand out a fresh Station ID (no station is created).
    pub fn mint_station_id(&mut self) -> Result<StationId, BuildError> {
        let id = self.counters.next_station_id;
        if id >= MAX_STATION_ID {
            return Err(BuildError::StationIdsExhausted);
        }
        self.counters.next_station_id = id + 1;
        Ok(id)
    }

    /// Create a station with a fresh id and `name`.
    pub fn mint_station(&mut self, name: impl Into<String>) -> Result<StationId, BuildError> {
        let id = self.mint_station_id()?;
        self.stations.insert(id, Station::new(id, name));
        Ok(id)
    }

    /// Insert a station with its own id (a decoder, a migration, an Add), and
    /// raise the station counter past it.
    pub fn insert_station(&mut self, station: Station) -> Result<(), BuildError> {
        let id = station.id;
        if id >= MAX_STATION_ID {
            return Err(BuildError::StationIdOutOfRange(id));
        }
        if self.stations.contains_key(&id) || self.is_archived_id(id) {
            return Err(BuildError::StationIdTaken(id));
        }
        self.observe_station_id(id);
        for record in &station.archived {
            self.observe_station_id(record.id);
        }
        self.stations.insert(id, station);
        Ok(())
    }

    /// Raise the station counter above `id` (never lowers it).
    pub fn observe_station_id(&mut self, id: StationId) {
        let next = id.saturating_add(1);
        if self.counters.next_station_id < next {
            self.counters.next_station_id = next;
        }
    }

    /// Hand out a fresh closure id.
    pub fn mint_closure_id(&mut self) -> ClosureId {
        let id = self.counters.next_closure_id;
        self.counters.next_closure_id = id.saturating_add(1);
        id
    }

    /// Hand out a fresh Section id.
    pub fn mint_section_id(&mut self) -> Result<SectionId, BuildError> {
        let id = self.counters.next_section_id;
        if id >= MAX_SECTION_ID {
            return Err(BuildError::SectionIdsExhausted);
        }
        self.counters.next_section_id = id + 1;
        Ok(id)
    }

    // ----- lookups ----------------------------------------------------------

    /// Station `id`.
    pub fn station(&self, id: StationId) -> Option<&Station> {
        self.stations.get(&id)
    }

    /// Station `id`, mutably (names, comments, archived records; no cache
    /// depends on them).
    pub fn station_mut(&mut self, id: StationId) -> Option<&mut Station> {
        self.stations.get_mut(&id)
    }

    /// The station at a slot ("the i-th station along line L").
    pub fn station_at(&self, slot: StationRef) -> Option<StationId> {
        self.topology().station_at(slot)
    }

    /// Every slot of a station, sorted by (line list order, index).
    pub fn slots_of(&self, station: StationId) -> &[StationRef] {
        self.topology().slots_of(station)
    }

    /// Line `line`'s chain of stations.
    pub fn line_stations(&self, line: LineId) -> Option<&[StationId]> {
        self.topology().chain(line)
    }

    /// The azimuth reference line `line` walks with: its own, else the
    /// project default, else "nothing recorded" (as today).
    pub fn azimuth_reference_for(&self, line: &Line) -> AzimuthReference {
        line.azimuth_reference
            .or(self.settings.default_azimuth_reference)
            .unwrap_or_default()
    }

    // ----- lines ------------------------------------------------------------

    /// Add a line at the end of the list: a chain from `start` through one new
    /// station per shot. New stations are named by the seed rule (their chain
    /// index, letter-suffixed when the line already has that name).
    ///
    /// A shot's uuid is its data's, else a fresh v4; a uuid some shot already
    /// has is refused. No anchor is created: a line that starts a new
    /// component needs a [`Placement`] (or a datum) before the model validates.
    pub fn add_line(
        &mut self,
        meta: LineMeta,
        start: StartAt,
        shots: Vec<ShotData>,
    ) -> Result<LineId, BuildError> {
        let id = self.checked_next_line_id()?;
        self.check_new_uuids(&shots)?;
        if let StartAt::Station(station) = start {
            if !self.stations.contains_key(&station) {
                return Err(BuildError::UnknownStation(station));
            }
        }
        let needed = shots.len() + usize::from(start == StartAt::New);
        let first = self.counters.next_station_id;
        if ((MAX_STATION_ID - first.min(MAX_STATION_ID)) as usize) < needed {
            return Err(BuildError::StationIdsExhausted);
        }

        let mut taken: BTreeSet<String> = BTreeSet::new();
        let seed = |taken: &mut BTreeSet<String>, index: usize| {
            let base = index.to_string();
            let name = if taken.contains(&base) {
                name_with_free_letter(&base, &|n| taken.contains(n))
            } else {
                base
            };
            taken.insert(name.clone());
            name
        };
        let start_id = match start {
            StartAt::Station(station) => {
                if let Some(existing) = self.stations.get(&station) {
                    taken.insert(existing.name.clone());
                }
                station
            }
            StartAt::New => {
                let name = seed(&mut taken, 0);
                self.mint_station(name)?
            }
        };
        let mut built = Vec::with_capacity(shots.len());
        for (index, data) in shots.into_iter().enumerate() {
            let name = seed(&mut taken, index + 1);
            let to = self.mint_station(name)?;
            built.push(Shot::from_data(0, data, to));
        }
        self.push_line(id, meta, start_id, built);
        Ok(id)
    }

    /// Add a line at the end of the list over existing stations: `chain[0]` is
    /// the start, shot `i` ends at `chain[i + 1]`. For junctions anywhere on
    /// the chain (tests, importers, the migration).
    pub fn add_line_on_stations(
        &mut self,
        meta: LineMeta,
        chain: &[StationId],
        shots: Vec<ShotData>,
    ) -> Result<LineId, BuildError> {
        if chain.len() != shots.len() + 1 {
            return Err(BuildError::ChainLength {
                stations: chain.len(),
                shots: shots.len(),
            });
        }
        let mut seen = BTreeSet::new();
        for &station in chain {
            if !self.stations.contains_key(&station) {
                return Err(BuildError::UnknownStation(station));
            }
            if !seen.insert(station) {
                return Err(BuildError::RepeatedStation(station));
            }
        }
        let id = self.checked_next_line_id()?;
        self.check_new_uuids(&shots)?;
        let built = shots
            .into_iter()
            .zip(&chain[1..])
            .map(|(data, &to)| Shot::from_data(0, data, to))
            .collect();
        self.push_line(id, meta, chain[0], built);
        Ok(id)
    }

    /// Append a fully built line (a decoder, a migration). Only the id is
    /// checked; session shot ids are assigned. Run [`Self::validate`] after.
    pub fn insert_line(&mut self, mut line: Line) -> Result<LineId, BuildError> {
        if line.id > MAX_LINE_ID {
            return Err(BuildError::LineIdsExhausted);
        }
        if self.line_position(line.id).is_some() {
            return Err(BuildError::LineIdTaken(line.id));
        }
        line.assign_session_shot_ids();
        let id = line.id;
        self.lines_mut().push(line);
        Ok(id)
    }

    /// Re-number every shot's session id as a load does
    /// (`line_id * FEATURE_ID_STRIDE + index`). Call after deserializing.
    pub fn assign_session_shot_ids(&mut self) {
        for line in self.lines_mut() {
            line.assign_session_shot_ids();
        }
    }

    fn checked_next_line_id(&self) -> Result<LineId, BuildError> {
        let id = self.next_line_id();
        if id > MAX_LINE_ID {
            return Err(BuildError::LineIdsExhausted);
        }
        Ok(id)
    }

    /// Refuse a uuid already in the project or repeated within `shots`.
    fn check_new_uuids(&self, shots: &[ShotData]) -> Result<(), BuildError> {
        let index = self.shot_uuids();
        let mut fresh = BTreeSet::new();
        for uuid in shots.iter().filter_map(|data| data.uuid.as_deref()) {
            if index.contains(uuid) || !fresh.insert(uuid) {
                return Err(BuildError::ShotUuidTaken(uuid.to_string()));
            }
        }
        Ok(())
    }

    fn push_line(&mut self, id: LineId, meta: LineMeta, start: StationId, shots: Vec<Shot>) {
        let mut line = Line {
            id,
            name: meta.name,
            color: meta.color,
            direction: meta.direction,
            quality: meta.quality,
            azimuth_reference: meta.azimuth_reference,
            section_id: meta.section_id,
            legacy_surveyor: meta.legacy_surveyor,
            legacy_date: meta.legacy_date,
            start,
            shots,
            hand_moves: BTreeMap::new(),
            next_shot_id: 0,
        };
        line.assign_session_shot_ids();
        self.lines_mut().push(line);
    }

    // ----- anchors and closures (low level) -------------------------------

    /// Append a datum as given (no placement logic) and return its index.
    pub fn push_datum(&mut self, datum: ZeroDatum) -> usize {
        self.datums.push(datum);
        self.datums.len() - 1
    }

    /// Set the placement of `station`'s component to `(station, x, y)`,
    /// replacing the component's existing placement in its list position, or
    /// appending one. Returns its index. Does not check for a datum.
    pub fn set_placement(
        &mut self,
        station: StationId,
        x: f64,
        y: f64,
    ) -> Result<usize, BuildError> {
        let topology = self.topology_arc();
        let Some(component) = topology.component_of(station) else {
            return Err(BuildError::UnknownStation(station));
        };
        let placement = Placement { station, x, y };
        let existing = self
            .placements
            .iter()
            .position(|p| topology.component_of(p.station) == Some(component));
        if let Some(index) = existing {
            self.placements[index] = placement;
            Ok(index)
        } else {
            self.placements.push(placement);
            Ok(self.placements.len() - 1)
        }
    }

    /// Clear what an edit left pointing at a line that no longer passes the
    /// station: datum and closure-end `via` hints (they become unset), hand
    /// moves (dropped) and archived records' lines (A2; a record left with
    /// no line is history, A5). Invariant 2 refuses them, so a command that
    /// removes a station from a line (delete shot or range, delete line,
    /// split, combine) runs this in its epilogue, before [`Self::validate`].
    pub fn drop_stale_attachments(&mut self) -> StaleAttachments {
        let topology = self.topology_arc();
        let holds = |station: StationId, line: LineId| topology.slot_on(station, line).is_some();
        let mut dropped = StaleAttachments::default();
        for (index, datum) in self.datums.iter_mut().enumerate() {
            if datum.via.is_some_and(|line| !holds(datum.station, line)) {
                datum.via = None;
                dropped.datum_vias.push(index);
            }
        }
        for closure in &mut self.closures {
            if closure.a_via.is_some_and(|line| !holds(closure.a, line)) {
                closure.a_via = None;
                dropped.closure_vias.push((closure.id, ClosureEnd::A));
            }
            if closure.b_via.is_some_and(|line| !holds(closure.b, line)) {
                closure.b_via = None;
                dropped.closure_vias.push((closure.id, ClosureEnd::B));
            }
        }
        for line in &self.lines {
            for &station in line.hand_moves.keys() {
                if !holds(station, line.id) {
                    dropped.hand_moves.push((line.id, station));
                }
            }
        }
        for (&station, s) in &mut self.stations {
            for record in &mut s.archived {
                record.lines.retain(|&line| {
                    let stays = holds(station, line);
                    if !stays {
                        dropped.archived_lines.push((station, record.id, line));
                    }
                    stays
                });
            }
        }
        if !dropped.hand_moves.is_empty() {
            let stale = dropped.hand_moves.clone();
            for line in self.lines_mut() {
                line.hand_moves
                    .retain(|station, _| !stale.contains(&(line.id, *station)));
            }
        }
        dropped
    }

    /// Append a closure with a fresh id that archives nothing (no refusal
    /// checks) and return the id.
    pub fn push_closure(
        &mut self,
        a: StationId,
        a_via: Option<LineId>,
        b: StationId,
        b_via: Option<LineId>,
        strategy: ClosureStrategy,
    ) -> ClosureId {
        let id = self.mint_closure_id();
        self.closures.push(LoopClosure {
            id,
            a,
            a_via,
            b,
            b_via,
            strategy,
            archived: None,
        });
        id
    }
}
