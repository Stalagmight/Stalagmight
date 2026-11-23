//! The model invariants (plan section 2), checked by [`Project::validate`].
//!
//! Every violation is typed and names exactly what is wrong. The checks are
//! total (they never panic on a broken model) and report in a fixed order:
//! ids, references, chains, orphans, forest, anchors, closures, names,
//! counters, numbers; within each, in list (or id) order.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{LineId, SectionId, StationId, MAX_LINE_ID, MAX_SECTION_ID, MAX_STATION_ID};

use super::{
    ClosureId, OwnerFault, Project, Topology, MAX_CLOSURE_ID, MAX_SHOTS_PER_LINE,
    MAX_SHOT_UUID_CHARS,
};

/// The largest magnitude of a measured distance: a shot's length, its two
/// depths and its passage dimensions (left, right, up, down), metres. No cave
/// survey comes near it; it keeps every derived position finite (review
/// finding B-derived-solver-3: two finite shots of 1e308 m summed to
/// infinity, and the solver turned that into NaN). `docs/file-format.md`
/// section 6.
pub const MAX_MEASUREMENT_M: f64 = 1_000_000.0;

/// The largest magnitude of a stored coordinate: a placement, a datum
/// correction, a legacy hand move, metres (two and a half times the earth's
/// circumference).
pub const MAX_COORDINATE_M: f64 = 100_000_000.0;

/// Which end of a closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClosureEnd {
    A,
    B,
}

impl fmt::Display for ClosureEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::A => "a",
            Self::B => "b",
        })
    }
}

/// Which id allocator or id space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterKind {
    Station,
    Line,
    Closure,
    Section,
}

impl fmt::Display for CounterKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Station => "station",
            Self::Line => "line",
            Self::Closure => "closure",
            Self::Section => "Section",
        })
    }
}

/// Where a number lives, for [`Violation::NonFinite`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberField {
    Shot {
        line: LineId,
        index: usize,
        field: &'static str,
    },
    LineQuality {
        line: LineId,
    },
    LineAzimuthCorrection {
        line: LineId,
    },
    HandMove {
        line: LineId,
        station: StationId,
        axis: usize,
    },
    DatumGps {
        datum: usize,
        field: &'static str,
    },
    DatumAccuracy {
        datum: usize,
    },
    DatumCorrection {
        datum: usize,
        axis: usize,
    },
    Placement {
        placement: usize,
        axis: usize,
    },
    SurfaceElevation,
    DefaultAzimuthCorrection,
    /// A validation threshold, by its settings key.
    ValidationThreshold {
        field: &'static str,
    },
}

impl fmt::Display for NumberField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const AXES: [&str; 3] = ["x", "y", "z"];
        let axis = |i: &usize| AXES.get(*i).copied().unwrap_or("?");
        match self {
            Self::Shot { line, index, field } => {
                write!(f, "{field} of shot {index} of line {line}")
            }
            Self::LineQuality { line } => write!(f, "quality of line {line}"),
            Self::LineAzimuthCorrection { line } => {
                write!(f, "azimuth correction of line {line}")
            }
            Self::HandMove {
                line,
                station,
                axis: a,
            } => write!(
                f,
                "{} of line {line}'s hand move at station {station}",
                axis(a)
            ),
            Self::DatumGps { datum, field } => write!(f, "GPS {field} of datum {datum}"),
            Self::DatumAccuracy { datum } => write!(f, "accuracy of datum {datum}"),
            Self::DatumCorrection { datum, axis: a } => {
                write!(f, "correction {} of datum {datum}", axis(a))
            }
            Self::Placement { placement, axis: a } => {
                write!(f, "{} of placement {placement}", axis(a))
            }
            Self::SurfaceElevation => f.write_str("the surface elevation"),
            Self::DefaultAzimuthCorrection => f.write_str("the default azimuth correction"),
            Self::ValidationThreshold { field } => write!(f, "validation threshold {field}"),
        }
    }
}

/// One broken invariant.
#[derive(Debug, Clone, PartialEq)]
pub enum Violation {
    // 1. ids
    /// `stations` holds a station under another id than its own.
    StationKeyMismatch {
        key: StationId,
        id: StationId,
    },
    /// An id at or above its id space's bound.
    IdOutOfRange {
        kind: CounterKind,
        id: u64,
    },
    /// A1: an archived record's id is a live station's id or another
    /// record's.
    ArchivedIdCollision {
        station: StationId,
        archived: StationId,
    },
    /// A1: an archived record's id at or above [`MAX_STATION_ID`] (outside
    /// the Station ID space).
    ArchivedIdOutOfRange {
        station: StationId,
        archived: StationId,
    },
    /// A line holds more than [`MAX_SHOTS_PER_LINE`] shots (session shot ids
    /// would run into the next line's).
    TooManyShots {
        line: LineId,
        count: usize,
    },
    DuplicateLineId {
        line: LineId,
    },
    DuplicateClosureId {
        closure: ClosureId,
    },
    DuplicateSectionId {
        section: SectionId,
    },
    /// A shot uuid is empty or longer than [`MAX_SHOT_UUID_CHARS`].
    InvalidShotUuid {
        line: LineId,
        index: usize,
        uuid: String,
    },
    /// Two shots share a uuid (`first` is the earlier in list order).
    DuplicateShotUuid {
        uuid: String,
        first: (LineId, usize),
        second: (LineId, usize),
    },
    // 2. references
    DanglingLineStart {
        line: LineId,
        station: StationId,
    },
    DanglingShotTarget {
        line: LineId,
        index: usize,
        station: StationId,
    },
    DanglingDatumStation {
        datum: usize,
        station: StationId,
    },
    DanglingPlacementStation {
        placement: usize,
        station: StationId,
    },
    DanglingClosureEnd {
        closure: ClosureId,
        end: ClosureEnd,
        station: StationId,
    },
    /// A datum's `via` names a line that does not pass through its station.
    StaleDatumVia {
        datum: usize,
        station: StationId,
        line: LineId,
    },
    /// A closure end's `via` names a line that does not pass through it.
    StaleClosureVia {
        closure: ClosureId,
        end: ClosureEnd,
        station: StationId,
        line: LineId,
    },
    /// A2: an archived record names a line that does not pass through the
    /// station holding it.
    StaleArchivedLine {
        station: StationId,
        archived: StationId,
        line: LineId,
    },
    /// A3: two records of one station name the same line.
    LineNamedTwice {
        station: StationId,
        line: LineId,
        first: StationId,
        second: StationId,
    },
    /// A4: a station on a line has no native line (records name every line
    /// through it).
    NoNativeLine {
        station: StationId,
    },
    /// A6: an archived record's owner (`into`) names no other record of the
    /// station holding it.
    DanglingArchivedOwner {
        station: StationId,
        archived: StationId,
        into: StationId,
    },
    /// A6 (with A5): an archived record is archived under a history record,
    /// which owns nothing.
    ArchivedUnderHistory {
        station: StationId,
        archived: StationId,
        into: StationId,
    },
    /// A6: following archived records' owners from this one comes back to
    /// it.
    ArchivedOwnerCycle {
        station: StationId,
        archived: StationId,
    },
    /// A hand move keyed on a station that is not on its line.
    HandMoveOffLine {
        line: LineId,
        station: StationId,
    },
    DanglingSection {
        line: LineId,
        section: SectionId,
    },
    // 3. simple chains
    RepeatedStationInChain {
        line: LineId,
        station: StationId,
        first: usize,
        second: usize,
    },
    // 4. every station on a line
    OrphanStation {
        station: StationId,
    },
    // 5. forest
    /// Shot `index` of `line` closes a cycle (stations already connected).
    Cycle {
        line: LineId,
        index: usize,
        from: StationId,
        to: StationId,
    },
    // 6. one anchor per component
    UnanchoredComponent {
        lowest_station: StationId,
    },
    /// A second placement in a datum-less component.
    ExtraPlacement {
        placement: usize,
        station: StationId,
        first: usize,
    },
    PlacementInDatumComponent {
        placement: usize,
        station: StationId,
    },
    // 7. closures
    ClosureSelfLoop {
        closure: ClosureId,
        station: StationId,
    },
    /// The ends are in different components and not both hold a datum.
    ClosureAcrossUnplacedPieces {
        closure: ClosureId,
        a: StationId,
        b: StationId,
    },
    /// C1: a station is the archived end of two closures (`first` is the
    /// earlier in list order).
    StationArchivedTwice {
        station: StationId,
        first: ClosureId,
        second: ClosureId,
    },
    /// C2: following archived end -> kept end from this closure's archived
    /// end comes back to it.
    ClosureArchiveCycle {
        closure: ClosureId,
        station: StationId,
    },
    // 8. names
    /// N8: two native slots of a line show two identities by one name
    /// (RS-FX5: a foreign slot or a loop end counts for no uniqueness,
    /// `NativeSlots`; `first` and `second` are the identities' Station IDs).
    DuplicateStationName {
        line: LineId,
        name: String,
        first: StationId,
        second: StationId,
    },
    /// Two Sections share a name (`first` is the earlier in panel order).
    /// Names are project-unique (`docs/file-format.md` 5.7); the Section
    /// commands and Add / Merge keep them so, and a file is held to it at
    /// load (review finding C-file-format-12).
    DuplicateSectionName {
        name: String,
        first: SectionId,
        second: SectionId,
    },
    // 9. counters
    CounterNotAbove {
        counter: CounterKind,
        next: u64,
        max_in_use: u64,
    },
    /// A stored counter past its id space's bound (at the bound the ids are
    /// used up; past it no id it could hand out is valid). Station, Section
    /// and closure counters: the line counter is derived.
    CounterOutOfRange {
        counter: CounterKind,
        next: u64,
    },
    // 10. numbers
    NonFinite {
        field: NumberField,
        value: f64,
    },
    /// A finite distance or coordinate past its bound ([`MAX_MEASUREMENT_M`],
    /// [`MAX_COORDINATE_M`]).
    OutOfBounds {
        field: NumberField,
        value: f64,
        bound: f64,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StationKeyMismatch { key, id } => {
                write!(f, "station {id} is stored under id {key}")
            }
            Self::IdOutOfRange { kind, id } => {
                let bound = match kind {
                    CounterKind::Station => u64::from(MAX_STATION_ID),
                    CounterKind::Line => u64::from(MAX_LINE_ID) + 1,
                    CounterKind::Section => u64::from(MAX_SECTION_ID),
                    CounterKind::Closure => MAX_CLOSURE_ID,
                };
                write!(f, "{kind} id {id} is not below {bound}")
            }
            Self::ArchivedIdCollision { station, archived } => write!(
                f,
                "station {station} keeps archived station {archived}, whose id is another \
                 station's or archived station's"
            ),
            Self::ArchivedIdOutOfRange { station, archived } => write!(
                f,
                "station {station} keeps archived station {archived}, which is not below \
                 {MAX_STATION_ID}"
            ),
            Self::TooManyShots { line, count } => write!(
                f,
                "line {line} has {count} shots; a line holds at most {MAX_SHOTS_PER_LINE}"
            ),
            Self::DuplicateLineId { line } => write!(f, "two lines have id {line}"),
            Self::DuplicateClosureId { closure } => write!(f, "two closures have id {closure}"),
            Self::DuplicateSectionId { section } => write!(f, "two Sections have id {section}"),
            Self::InvalidShotUuid { line, index, uuid } => write!(
                f,
                "shot {index} of line {line} has uuid {uuid:?}, which is empty or longer than \
                 {MAX_SHOT_UUID_CHARS} characters"
            ),
            Self::DuplicateShotUuid {
                uuid,
                first,
                second,
            } => write!(
                f,
                "shot {} of line {} repeats uuid {uuid:?} of shot {} of line {}",
                second.1, second.0, first.1, first.0
            ),
            Self::DanglingLineStart { line, station } => {
                write!(
                    f,
                    "line {line} starts at station {station}, which does not exist"
                )
            }
            Self::DanglingShotTarget {
                line,
                index,
                station,
            } => write!(
                f,
                "shot {index} of line {line} ends at station {station}, which does not exist"
            ),
            Self::DanglingDatumStation { datum, station } => write!(
                f,
                "datum {datum} is on station {station}, which is not on any line"
            ),
            Self::DanglingPlacementStation { placement, station } => write!(
                f,
                "placement {placement} is on station {station}, which is not on any line"
            ),
            Self::DanglingClosureEnd {
                closure,
                end,
                station,
            } => write!(
                f,
                "closure {closure} end {end} is station {station}, which is not on any line"
            ),
            Self::StaleDatumVia {
                datum,
                station,
                line,
            } => write!(
                f,
                "datum {datum} is attached via line {line}, which does not pass through \
                 station {station}"
            ),
            Self::StaleClosureVia {
                closure,
                end,
                station,
                line,
            } => write!(
                f,
                "closure {closure} end {end} is attached via line {line}, which does not pass \
                 through station {station}"
            ),
            Self::StaleArchivedLine {
                station,
                archived,
                line,
            } => write!(
                f,
                "archived station {archived} of station {station} names line {line}, which does \
                 not pass through it"
            ),
            Self::LineNamedTwice {
                station,
                line,
                first,
                second,
            } => write!(
                f,
                "archived stations {first} and {second} of station {station} both name line \
                 {line}"
            ),
            Self::NoNativeLine { station } => write!(
                f,
                "station {station} is shown on no line of its own: archived stations name every \
                 line through it"
            ),
            Self::DanglingArchivedOwner {
                station,
                archived,
                into,
            } => write!(
                f,
                "archived station {archived} of station {station} is archived under {into},                  which is no other archived station of it"
            ),
            Self::ArchivedUnderHistory {
                station,
                archived,
                into,
            } => write!(
                f,
                "archived station {archived} of station {station} is archived under {into},                  which names no line"
            ),
            Self::ArchivedOwnerCycle { station, archived } => write!(
                f,
                "archived station {archived} of station {station} is archived under itself                  through other archived stations"
            ),
            Self::HandMoveOffLine { line, station } => write!(
                f,
                "line {line} has a hand move at station {station}, which is not on it"
            ),
            Self::DanglingSection { line, section } => {
                write!(
                    f,
                    "line {line} is in Section {section}, which does not exist"
                )
            }
            Self::RepeatedStationInChain {
                line,
                station,
                first,
                second,
            } => write!(
                f,
                "line {line} passes station {station} twice (stations {first} and {second})"
            ),
            Self::OrphanStation { station } => write!(f, "station {station} is on no line"),
            Self::Cycle {
                line,
                index,
                from,
                to,
            } => write!(
                f,
                "shot {index} of line {line} ({from} -> {to}) closes a loop without a loop \
                 closure"
            ),
            Self::UnanchoredComponent { lowest_station } => write!(
                f,
                "the piece holding station {lowest_station} has no datum and no placement"
            ),
            Self::ExtraPlacement {
                placement,
                station,
                first,
            } => write!(
                f,
                "placement {placement} (station {station}) is a second placement of the piece \
                 placed by placement {first}"
            ),
            Self::PlacementInDatumComponent { placement, station } => write!(
                f,
                "placement {placement} (station {station}) is in a piece that holds a datum"
            ),
            Self::ClosureSelfLoop { closure, station } => {
                write!(f, "closure {closure} joins station {station} to itself")
            }
            Self::ClosureAcrossUnplacedPieces { closure, a, b } => write!(
                f,
                "closure {closure} joins stations {a} and {b}, which are in different pieces \
                 that do not both hold a datum"
            ),
            Self::StationArchivedTwice {
                station,
                first,
                second,
            } => write!(
                f,
                "station {station} is the archived end of closures {first} and {second}"
            ),
            Self::ClosureArchiveCycle { closure, station } => write!(
                f,
                "closure {closure} archives station {station}, whose kept ends lead back to it"
            ),
            Self::DuplicateStationName {
                line,
                name,
                first,
                second,
            } => write!(
                f,
                "stations {first} and {second} on line {line} are both shown as {name:?}"
            ),
            Self::DuplicateSectionName {
                name,
                first,
                second,
            } => write!(f, "Sections {first} and {second} are both named {name:?}"),
            Self::CounterNotAbove {
                counter,
                next,
                max_in_use,
            } => write!(
                f,
                "the next {counter} id {next} is not above {max_in_use}, an id in use"
            ),
            Self::CounterOutOfRange { counter, next } => {
                let bound = match counter {
                    CounterKind::Section => u64::from(MAX_SECTION_ID),
                    CounterKind::Closure => MAX_CLOSURE_ID,
                    _ => u64::from(MAX_STATION_ID),
                };
                write!(f, "the next {counter} id {next} is past {bound}")
            }
            Self::NonFinite { field, value } => write!(f, "{field} is {value}, not finite"),
            Self::OutOfBounds {
                field,
                value,
                bound,
            } => write!(f, "{field} is {value} m, past the {bound} m bound"),
        }
    }
}

impl Project {
    /// Every broken invariant (plan section 2), empty for a valid model.
    pub fn validate(&self) -> Vec<Violation> {
        let topology = self.topology();
        let mut out = Vec::new();
        self.check_ids(&mut out);
        self.check_references(topology, &mut out);
        self.check_chains(&mut out);
        self.check_orphans(topology, &mut out);
        self.check_forest(topology, &mut out);
        self.check_anchors(topology, &mut out);
        self.check_closures(topology, &mut out);
        self.check_names(topology, &mut out);
        self.check_counters(&mut out);
        self.check_numbers(&mut out);
        out
    }

    /// Whether [`Self::validate`] finds nothing.
    pub fn is_valid(&self) -> bool {
        self.validate().is_empty()
    }

    fn check_ids(&self, out: &mut Vec<Violation>) {
        let mut archived: BTreeSet<StationId> = BTreeSet::new();
        for (&key, station) in &self.stations {
            if key != station.id {
                out.push(Violation::StationKeyMismatch {
                    key,
                    id: station.id,
                });
            }
            if key >= MAX_STATION_ID {
                out.push(Violation::IdOutOfRange {
                    kind: CounterKind::Station,
                    id: u64::from(key),
                });
            }
            for record in &station.archived {
                if record.id >= MAX_STATION_ID {
                    out.push(Violation::ArchivedIdOutOfRange {
                        station: key,
                        archived: record.id,
                    });
                }
                if self.stations.contains_key(&record.id) || !archived.insert(record.id) {
                    out.push(Violation::ArchivedIdCollision {
                        station: key,
                        archived: record.id,
                    });
                }
            }
        }
        let mut lines = BTreeSet::new();
        for line in &self.lines {
            if !lines.insert(line.id) {
                out.push(Violation::DuplicateLineId { line: line.id });
            }
            if line.id > MAX_LINE_ID {
                out.push(Violation::IdOutOfRange {
                    kind: CounterKind::Line,
                    id: u64::from(line.id),
                });
            }
            if line.shots.len() > MAX_SHOTS_PER_LINE {
                out.push(Violation::TooManyShots {
                    line: line.id,
                    count: line.shots.len(),
                });
            }
        }
        let mut closures = BTreeSet::new();
        for closure in &self.closures {
            if !closures.insert(closure.id) {
                out.push(Violation::DuplicateClosureId {
                    closure: closure.id,
                });
            }
            if closure.id >= MAX_CLOSURE_ID {
                out.push(Violation::IdOutOfRange {
                    kind: CounterKind::Closure,
                    id: closure.id,
                });
            }
        }
        let mut sections = BTreeSet::new();
        for section in &self.sections {
            if !sections.insert(section.id) {
                out.push(Violation::DuplicateSectionId {
                    section: section.id,
                });
            }
            if section.id >= MAX_SECTION_ID {
                out.push(Violation::IdOutOfRange {
                    kind: CounterKind::Section,
                    id: u64::from(section.id),
                });
            }
        }
        // Hashed: the report order is the shots' list order either way.
        let shot_count = self.lines.iter().map(|l| l.shots.len()).sum();
        let mut first_seen: HashMap<&str, (LineId, usize)> = HashMap::with_capacity(shot_count);
        for line in &self.lines {
            for (index, shot) in line.shots.iter().enumerate() {
                if shot.uuid.is_empty() || shot.uuid.chars().count() > MAX_SHOT_UUID_CHARS {
                    out.push(Violation::InvalidShotUuid {
                        line: line.id,
                        index,
                        uuid: shot.uuid.clone(),
                    });
                }
                if let Some(&first) = first_seen.get(shot.uuid.as_str()) {
                    out.push(Violation::DuplicateShotUuid {
                        uuid: shot.uuid.clone(),
                        first,
                        second: (line.id, index),
                    });
                } else {
                    first_seen.insert(&shot.uuid, (line.id, index));
                }
            }
        }
    }

    fn check_references(&self, topology: &Topology, out: &mut Vec<Violation>) {
        let section_ids: BTreeSet<SectionId> = self.sections.iter().map(|s| s.id).collect();
        // Every chain station is a topology station: when all of those are
        // stored, no start or shot target can dangle (one sorted merge
        // instead of a map lookup per shot).
        let chains_resolve = sorted_subset(topology.stations(), self.stations.keys());
        for line in &self.lines {
            if !chains_resolve && !self.stations.contains_key(&line.start) {
                out.push(Violation::DanglingLineStart {
                    line: line.id,
                    station: line.start,
                });
            }
            for (index, shot) in line.shots.iter().enumerate() {
                if !chains_resolve && !self.stations.contains_key(&shot.to) {
                    out.push(Violation::DanglingShotTarget {
                        line: line.id,
                        index,
                        station: shot.to,
                    });
                }
            }
            for &station in line.hand_moves.keys() {
                if topology.slot_on(station, line.id).is_none() {
                    out.push(Violation::HandMoveOffLine {
                        line: line.id,
                        station,
                    });
                }
            }
            if let Some(section) = line.section_id {
                if !section_ids.contains(&section) {
                    out.push(Violation::DanglingSection {
                        line: line.id,
                        section,
                    });
                }
            }
        }
        let resolves =
            |station: StationId| self.stations.contains_key(&station) && topology.contains(station);
        let via_holds =
            |station: StationId, line: LineId| topology.slot_on(station, line).is_some();
        for (datum, d) in self.datums.iter().enumerate() {
            if !resolves(d.station) {
                out.push(Violation::DanglingDatumStation {
                    datum,
                    station: d.station,
                });
            } else if let Some(line) = d.via.filter(|&line| !via_holds(d.station, line)) {
                out.push(Violation::StaleDatumVia {
                    datum,
                    station: d.station,
                    line,
                });
            }
        }
        for (placement, p) in self.placements.iter().enumerate() {
            if !resolves(p.station) {
                out.push(Violation::DanglingPlacementStation {
                    placement,
                    station: p.station,
                });
            }
        }
        for c in &self.closures {
            for (end, station, via) in
                [(ClosureEnd::A, c.a, c.a_via), (ClosureEnd::B, c.b, c.b_via)]
            {
                if !resolves(station) {
                    out.push(Violation::DanglingClosureEnd {
                        closure: c.id,
                        end,
                        station,
                    });
                } else if let Some(line) = via.filter(|&line| !via_holds(station, line)) {
                    out.push(Violation::StaleClosureVia {
                        closure: c.id,
                        end,
                        station,
                        line,
                    });
                }
            }
        }
        for (&station, s) in &self.stations {
            for (i, fault) in super::records::owner_faults(&s.archived) {
                let record = &s.archived[i];
                let into = record.into.unwrap_or(record.id);
                out.push(match fault {
                    OwnerFault::Dangling => Violation::DanglingArchivedOwner {
                        station,
                        archived: record.id,
                        into,
                    },
                    OwnerFault::History => Violation::ArchivedUnderHistory {
                        station,
                        archived: record.id,
                        into,
                    },
                    OwnerFault::Cycle => Violation::ArchivedOwnerCycle {
                        station,
                        archived: record.id,
                    },
                });
            }
            if s.archived.is_empty() || !topology.contains(station) {
                continue;
            }
            let mut named: BTreeMap<LineId, StationId> = BTreeMap::new();
            for record in &s.archived {
                for &line in &record.lines {
                    if !via_holds(station, line) {
                        out.push(Violation::StaleArchivedLine {
                            station,
                            archived: record.id,
                            line,
                        });
                    }
                    match named.get(&line) {
                        Some(&first) => out.push(Violation::LineNamedTwice {
                            station,
                            line,
                            first,
                            second: record.id,
                        }),
                        None => {
                            named.insert(line, record.id);
                        }
                    }
                }
            }
            let lines = topology.lines_of(station);
            if !lines.is_empty() && lines.iter().all(|l| named.contains_key(l)) {
                out.push(Violation::NoNativeLine { station });
            }
        }
    }

    fn check_chains(&self, out: &mut Vec<Violation>) {
        let mut sorted: Vec<StationId> = Vec::new();
        for line in &self.lines {
            // Fast path: no station twice, nothing to report.
            sorted.clear();
            sorted.extend(line.stations());
            sorted.sort_unstable();
            if sorted.windows(2).all(|w| w[0] != w[1]) {
                continue;
            }
            let mut seen: BTreeMap<StationId, usize> = BTreeMap::new();
            for (index, station) in line.stations().enumerate() {
                if let Some(&first) = seen.get(&station) {
                    out.push(Violation::RepeatedStationInChain {
                        line: line.id,
                        station,
                        first,
                        second: index,
                    });
                } else {
                    seen.insert(station, index);
                }
            }
        }
    }

    fn check_orphans(&self, topology: &Topology, out: &mut Vec<Violation>) {
        if sorted_subset(self.stations.keys(), topology.stations()) {
            return;
        }
        for &station in self.stations.keys() {
            if !topology.contains(station) {
                out.push(Violation::OrphanStation { station });
            }
        }
    }

    fn check_forest(&self, topology: &Topology, out: &mut Vec<Violation>) {
        // Union-find over the topology's dense index (ascending station id,
        // so "the lower id is the root" is the lower index), edges in list
        // order. Every chain station has a dense index.
        let mut parent: Vec<usize> = (0..topology.stations().len()).collect();
        fn find(parent: &mut [usize], x: usize) -> usize {
            let mut root = x;
            while parent[root] != root {
                root = parent[root];
            }
            let mut cur = x;
            while cur != root {
                let next = parent[cur];
                parent[cur] = root;
                cur = next;
            }
            root
        }
        for line in &self.lines {
            let mut from = line.start;
            for (index, shot) in line.shots.iter().enumerate() {
                if let (Some(a), Some(b)) = (topology.dense(from), topology.dense(shot.to)) {
                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                    if ra == rb {
                        out.push(Violation::Cycle {
                            line: line.id,
                            index,
                            from,
                            to: shot.to,
                        });
                    } else {
                        parent[ra.max(rb)] = ra.min(rb);
                    }
                }
                from = shot.to;
            }
        }
    }

    fn check_anchors(&self, topology: &Topology, out: &mut Vec<Violation>) {
        let count = topology.components().len();
        let mut has_datum = vec![false; count];
        for d in &self.datums {
            if let Some(c) = topology.component_of(d.station) {
                has_datum[c] = true;
            }
        }
        let mut first_placement: Vec<Option<usize>> = vec![None; count];
        for (placement, p) in self.placements.iter().enumerate() {
            let Some(c) = topology.component_of(p.station) else {
                continue;
            };
            if has_datum[c] {
                out.push(Violation::PlacementInDatumComponent {
                    placement,
                    station: p.station,
                });
            } else if let Some(first) = first_placement[c] {
                out.push(Violation::ExtraPlacement {
                    placement,
                    station: p.station,
                    first,
                });
            } else {
                first_placement[c] = Some(placement);
            }
        }
        for (c, component) in topology.components().iter().enumerate() {
            if !has_datum[c] && first_placement[c].is_none() {
                if let Some(lowest_station) = component.lowest_station() {
                    out.push(Violation::UnanchoredComponent { lowest_station });
                }
            }
        }
    }

    fn check_closures(&self, topology: &Topology, out: &mut Vec<Violation>) {
        let mut datum_components = BTreeSet::new();
        for d in &self.datums {
            if let Some(c) = topology.component_of(d.station) {
                datum_components.insert(c);
            }
        }
        for c in &self.closures {
            if c.a == c.b {
                out.push(Violation::ClosureSelfLoop {
                    closure: c.id,
                    station: c.a,
                });
                continue;
            }
            let (Some(ca), Some(cb)) = (topology.component_of(c.a), topology.component_of(c.b))
            else {
                continue;
            };
            if ca != cb && !(datum_components.contains(&ca) && datum_components.contains(&cb)) {
                out.push(Violation::ClosureAcrossUnplacedPieces {
                    closure: c.id,
                    a: c.a,
                    b: c.b,
                });
            }
        }
        // C1: one archiving closure per station; C2: no cycle of archives
        // (the cycles found by one walk of the archives, so a crafted file
        // reports instead of hanging, in time linear in its closures).
        let mut kept_of: BTreeMap<StationId, StationId> = BTreeMap::new();
        let mut archived_by: BTreeMap<StationId, ClosureId> = BTreeMap::new();
        for c in &self.closures {
            let Some((archived, kept)) = c.archive() else {
                continue;
            };
            if let Some(&first) = archived_by.get(&archived) {
                out.push(Violation::StationArchivedTwice {
                    station: archived,
                    first,
                    second: c.id,
                });
            } else {
                archived_by.insert(archived, c.id);
                kept_of.insert(archived, kept);
            }
        }
        let on_cycle = super::archive::archive_cycle_nodes(&kept_of);
        for c in &self.closures {
            let Some((archived, _)) = c.archive() else {
                continue;
            };
            if archived_by.get(&archived) == Some(&c.id) && on_cycle.contains(&archived) {
                out.push(Violation::ClosureArchiveCycle {
                    closure: c.id,
                    station: archived,
                });
            }
        }
    }

    fn check_names(&self, topology: &Topology, out: &mut Vec<Violation>) {
        // N8: the native slots of each line (RS-FX5: a foreign slot or a
        // loop end shows another station's identity and is exempt; the shown
        // index never loops, so a broken C2 cannot hang this), reported in
        // chain order.
        let native = super::NativeSlots::of(self);
        for (position, line) in self.lines.iter().enumerate() {
            for (name, first, second) in self.line_name_repeats(topology, &native, position) {
                out.push(Violation::DuplicateStationName {
                    line: line.id,
                    name: name.to_string(),
                    first,
                    second,
                });
            }
        }
        let mut sections: HashMap<&str, SectionId> = HashMap::with_capacity(self.sections.len());
        for section in &self.sections {
            match sections.get(section.name.as_str()) {
                Some(&first) => out.push(Violation::DuplicateSectionName {
                    name: section.name.clone(),
                    first,
                    second: section.id,
                }),
                None => {
                    sections.insert(section.name.as_str(), section.id);
                }
            }
        }
    }

    fn check_counters(&self, out: &mut Vec<Violation>) {
        for (counter, next, bound) in [
            (
                CounterKind::Station,
                self.counters.next_station_id,
                MAX_STATION_ID,
            ),
            (
                CounterKind::Section,
                self.counters.next_section_id,
                MAX_SECTION_ID,
            ),
        ] {
            if next > bound {
                out.push(Violation::CounterOutOfRange {
                    counter,
                    next: u64::from(next),
                });
            }
        }
        if self.counters.next_closure_id > MAX_CLOSURE_ID {
            out.push(Violation::CounterOutOfRange {
                counter: CounterKind::Closure,
                next: self.counters.next_closure_id,
            });
        }
        let station_max = self
            .stations
            .values()
            .flat_map(|s| std::iter::once(s.id).chain(s.archived.iter().map(|r| r.id)))
            .chain(self.stations.keys().copied())
            .max();
        if let Some(max) = station_max {
            if self.counters.next_station_id <= max {
                out.push(Violation::CounterNotAbove {
                    counter: CounterKind::Station,
                    next: u64::from(self.counters.next_station_id),
                    max_in_use: u64::from(max),
                });
            }
        }
        if let Some(max) = self.closures.iter().map(|c| c.id).max() {
            if self.counters.next_closure_id <= max {
                out.push(Violation::CounterNotAbove {
                    counter: CounterKind::Closure,
                    next: self.counters.next_closure_id,
                    max_in_use: max,
                });
            }
        }
        if let Some(max) = self.sections.iter().map(|s| s.id).max() {
            if self.counters.next_section_id <= max {
                out.push(Violation::CounterNotAbove {
                    counter: CounterKind::Section,
                    next: u64::from(self.counters.next_section_id),
                    max_in_use: u64::from(max),
                });
            }
        }
    }

    fn check_numbers(&self, out: &mut Vec<Violation>) {
        // A distance or coordinate is finite and within its bound; any other
        // number only finite.
        let mut check_within = |field: NumberField, value: f64, bound: Option<f64>| {
            if !value.is_finite() {
                out.push(Violation::NonFinite { field, value });
            } else if let Some(bound) = bound.filter(|b| value.abs() > *b) {
                out.push(Violation::OutOfBounds {
                    field,
                    value,
                    bound,
                });
            }
        };
        let measurement = Some(MAX_MEASUREMENT_M);
        let coordinate = Some(MAX_COORDINATE_M);
        let mut check = |field: NumberField, value: f64| match &field {
            NumberField::Shot { field: name, .. } if is_measurement(name) => {
                check_within(field, value, measurement);
            }
            NumberField::HandMove { .. }
            | NumberField::DatumCorrection { .. }
            | NumberField::Placement { .. } => check_within(field, value, coordinate),
            _ => check_within(field, value, None),
        };
        for line in &self.lines {
            check(NumberField::LineQuality { line: line.id }, line.quality);
            if let Some(reference) = line.azimuth_reference {
                check(
                    NumberField::LineAzimuthCorrection { line: line.id },
                    reference.correction,
                );
            }
            for (&station, hand) in &line.hand_moves {
                for (axis, &value) in hand.iter().enumerate() {
                    check(
                        NumberField::HandMove {
                            line: line.id,
                            station,
                            axis,
                        },
                        value,
                    );
                }
            }
            for (index, shot) in line.shots.iter().enumerate() {
                for (field, value) in shot.numbers() {
                    if let Some(value) = value {
                        check(
                            NumberField::Shot {
                                line: line.id,
                                index,
                                field,
                            },
                            value,
                        );
                    }
                }
            }
        }
        for (datum, d) in self.datums.iter().enumerate() {
            for (field, value) in [
                ("latitude", d.gps.latitude),
                ("longitude", d.gps.longitude),
                ("depth reference", d.gps.depth_reference),
            ] {
                check(NumberField::DatumGps { datum, field }, value);
            }
            if let Some(accuracy) = d.accuracy_m {
                check(NumberField::DatumAccuracy { datum }, accuracy);
            }
            if let Some(correction) = d.correction {
                for (axis, value) in correction.into_iter().enumerate() {
                    check(NumberField::DatumCorrection { datum, axis }, value);
                }
            }
        }
        for (placement, p) in self.placements.iter().enumerate() {
            check(NumberField::Placement { placement, axis: 0 }, p.x);
            check(NumberField::Placement { placement, axis: 1 }, p.y);
        }
        if let Some(elevation) = self.settings.surface_elevation {
            check(NumberField::SurfaceElevation, elevation);
        }
        if let Some(reference) = self.settings.default_azimuth_reference {
            check(NumberField::DefaultAzimuthCorrection, reference.correction);
        }
        let v = &self.settings.validation;
        for (field, value) in [
            ("warn_shot_length_min", v.warn_shot_length_min),
            ("warn_shot_length_max", v.warn_shot_length_max),
            (
                "warn_depth_change_per_shot_max",
                v.warn_depth_change_per_shot_max,
            ),
            ("warn_azimuth_deviation_max", v.warn_azimuth_deviation_max),
            ("azimuth_diff_threshold", v.azimuth_diff_threshold),
            ("pitch_diff_threshold", v.pitch_diff_threshold),
            (
                "inclination_depth_diff_threshold",
                v.inclination_depth_diff_threshold,
            ),
        ] {
            check(NumberField::ValidationThreshold { field }, value);
        }
    }
}

/// Whether every element of the ascending `small` is in the ascending
/// `large` (one merge walk).
fn sorted_subset<'a>(
    small: impl IntoIterator<Item = &'a StationId>,
    large: impl IntoIterator<Item = &'a StationId>,
) -> bool {
    let mut large = large.into_iter().peekable();
    for x in small {
        loop {
            match large.peek() {
                Some(y) if *y < x => {
                    large.next();
                }
                Some(y) if *y == x => break,
                _ => return false,
            }
        }
    }
    true
}

/// Whether a shot field is a distance bounded by [`MAX_MEASUREMENT_M`] (the
/// angles and the temperature are not).
fn is_measurement(field: &str) -> bool {
    matches!(
        field,
        "length" | "depth_in" | "depth_out" | "left" | "right" | "up" | "down"
    )
}
