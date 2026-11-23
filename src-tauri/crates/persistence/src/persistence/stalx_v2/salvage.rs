//! Salvage (plan 6.6): open a file that parses but breaks the model
//! invariants, and repair a copy as best it can. Every repair is reported;
//! nothing here runs on a normal open.
//!
//! Repairs run in stages, earliest first, re-validating after each pass: ids,
//! then references, then chains / orphans / cycles, then anchors and
//! closures, then names and counters. A later stage only sees a model whose
//! earlier invariants hold, so repairs never fight each other.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::models::survey::{
    ClosureEnd, ClosureId, ClosureStrategy, CounterKind, LoopClosure, Placement, Project, Station,
    Violation, MAX_CLOSURE_ID,
};
use crate::models::{LineId, SectionId, StationId, MAX_LINE_ID, MAX_SECTION_ID, MAX_STATION_ID};

use super::decode::parse;
use super::error::DecodeError;
use super::types::{DecodeNote, DecodeOptions, Extras};

/// The most repair passes before salvage gives up.
const MAX_PASSES: usize = 64;

/// One repair salvage made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// A second station record with the same id was dropped.
    DroppedDuplicateStation {
        id: StationId,
    },
    /// A station's id was out of range and got a fresh one.
    RenumberedStation {
        from: StationId,
        to: StationId,
    },
    /// A line with a repeated or out-of-range id got a fresh one.
    RenumberedLine {
        from: LineId,
        to: LineId,
    },
    /// A line had a repeated id and no fresh id was left.
    DroppedLine {
        id: LineId,
    },
    RenumberedClosure {
        from: ClosureId,
        to: ClosureId,
    },
    DroppedSection {
        id: SectionId,
    },
    /// An archived station (RS 3) whose id collides or is out of range was
    /// dropped: its name and comment are lost.
    DroppedArchived {
        station: StationId,
        archived: StationId,
    },
    /// A3: a later record of a station no longer names a line an earlier
    /// one names.
    UnnamedArchivedLine {
        station: StationId,
        archived: StationId,
        line: LineId,
    },
    /// A4: a station with no native line took back the archived identity
    /// naming its lines (RS 4.3); its own became history.
    RestoredArchived {
        from: StationId,
        to: StationId,
    },
    /// C1 or C2: a loop closure no longer archives an end (RS 5.4).
    UnarchivedClosure {
        id: ClosureId,
    },
    ReMintedShotUuid {
        line: LineId,
        index: usize,
        uuid: String,
    },
    /// A line referenced a station with no record; one was created.
    CreatedStation {
        id: StationId,
    },
    /// A chain station was replaced by a fresh one (a repeated station, or a
    /// reference no record can be created for).
    ReplacedChainStation {
        line: LineId,
        index: usize,
        from: StationId,
        to: StationId,
    },
    DroppedDatum {
        station: StationId,
    },
    DroppedPlacement {
        station: StationId,
    },
    DroppedClosure {
        id: ClosureId,
    },
    /// Stale `via` hints cleared and hand moves dropped.
    ClearedAttachments {
        vias: usize,
        hand_moves: usize,
    },
    ClearedSection {
        line: LineId,
        section: SectionId,
    },
    /// A station on no line was dropped.
    DroppedStation {
        id: StationId,
    },
    /// A shot that closed a loop now ends at a new station, tied to the old
    /// one by a `Batch` closure.
    BrokeCycle {
        line: LineId,
        index: usize,
        station: StationId,
        new_station: StationId,
        closure: ClosureId,
    },
    /// A piece without an anchor got a placement at the origin (where the
    /// solver lays an unanchored piece out, so nothing moves).
    AddedPlacement {
        station: StationId,
    },
    RenamedStation {
        id: StationId,
        name: String,
    },
    /// A Section named like an earlier one got the first free `name (n)`.
    RenamedSection {
        id: SectionId,
        name: String,
    },
    RaisedCounter {
        counter: CounterKind,
        to: u64,
    },
    /// A counter past its id space was set to the bound (ids used up).
    LoweredCounter {
        counter: CounterKind,
        to: u64,
    },
}

impl fmt::Display for Repair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DroppedDuplicateStation { id } => write!(f, "dropped a second station {id}"),
            Self::RenumberedStation { from, to } => write!(f, "station {from} is now {to}"),
            Self::RenumberedLine { from, to } => write!(f, "a line {from} is now line {to}"),
            Self::DroppedLine { id } => write!(f, "dropped a second line {id}"),
            Self::RenumberedClosure { from, to } => write!(f, "a closure {from} is now {to}"),
            Self::DroppedSection { id } => write!(f, "dropped Section {id}"),
            Self::DroppedArchived { station, archived } => write!(
                f,
                "dropped archived station {archived} of station {station} (its name and comment \
                 are lost)"
            ),
            Self::UnnamedArchivedLine {
                station,
                archived,
                line,
            } => write!(
                f,
                "archived station {archived} of station {station} no longer names line {line}"
            ),
            Self::RestoredArchived { from, to } => write!(
                f,
                "station {from} shows its archived station {to} again (its own is kept as history)"
            ),
            Self::UnarchivedClosure { id } => {
                write!(f, "closure {id} no longer shows one end as the other")
            }
            Self::ReMintedShotUuid { line, index, uuid } => {
                write!(f, "shot {index} of line {line} has the new uuid {uuid}")
            }
            Self::CreatedStation { id } => write!(f, "created the missing station {id}"),
            Self::ReplacedChainStation {
                line,
                index,
                from,
                to,
            } => write!(
                f,
                "station {index} of line {line} is the new station {to} instead of {from}"
            ),
            Self::DroppedDatum { station } => write!(f, "dropped a datum at station {station}"),
            Self::DroppedPlacement { station } => {
                write!(f, "dropped a placement at station {station}")
            }
            Self::DroppedClosure { id } => write!(f, "dropped closure {id}"),
            Self::ClearedAttachments { vias, hand_moves } => write!(
                f,
                "cleared {vias} stale attachment(s) and {hand_moves} hand move(s)"
            ),
            Self::ClearedSection { line, section } => {
                write!(f, "line {line} left the missing Section {section}")
            }
            Self::DroppedStation { id } => write!(f, "dropped station {id}, which was on no line"),
            Self::BrokeCycle {
                line,
                index,
                station,
                new_station,
                closure,
            } => write!(
                f,
                "shot {index} of line {line} now ends at the new station {new_station}, tied to \
                 {station} by closure {closure} and shown as it"
            ),
            Self::AddedPlacement { station } => {
                write!(f, "placed the piece of station {station} at the origin")
            }
            Self::RenamedStation { id, name } => write!(f, "station {id} is now named {name:?}"),
            Self::RenamedSection { id, name } => write!(f, "Section {id} is now named {name:?}"),
            Self::RaisedCounter { counter, to } | Self::LoweredCounter { counter, to } => {
                write!(f, "the next {counter} id is now {to}")
            }
        }
    }
}

/// What salvage did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SalvageReport {
    /// Why the file could not be read at all (no project then).
    pub error: Option<DecodeError>,
    pub repairs: Vec<Repair>,
    /// Violations left when salvage gave up (no project then).
    pub remaining: Vec<Violation>,
    /// The file's extras, when it parsed.
    pub extras: Option<Extras>,
    pub notes: Vec<DecodeNote>,
    /// What the load-time upgrade did, for a file an earlier build of
    /// version 2 wrote (RS 8.2): the repaired project is the upgraded one.
    pub upgrade: Option<super::UpgradeReport>,
}

pub(super) fn run(bytes: &[u8]) -> (Option<Project>, SalvageReport) {
    let mut report = SalvageReport::default();
    let doc = match parse(bytes, &DecodeOptions::default()) {
        Ok(doc) => doc,
        Err(error) => {
            report.error = Some(error);
            return (None, report);
        }
    };
    let mut earlier = doc.earlier;
    let (mut project, extras, notes) = match doc.assemble(Some(&mut report.repairs)) {
        Ok(parts) => parts,
        Err(error) => {
            report.error = Some(error);
            return (None, report);
        }
    };
    // A file an earlier build of version 2 wrote: its own damage is repaired
    // first, then the load-time upgrade runs (it settles what such a file
    // shows per line, RS 8.2), then whatever is left is repaired.
    report.extras = Some(extras);
    report.notes = notes;
    for _ in 0..MAX_PASSES {
        let mut violations = project.validate();
        if earlier {
            violations.retain(|v| !super::upgrade::settles(v));
            if violations.is_empty() {
                earlier = false;
                report.upgrade = Some(super::upgrade::run(&mut project));
                continue;
            }
        }
        if violations.is_empty() {
            return (Some(project), report);
        }
        let before = report.repairs.len();
        repair_pass(&mut project, &violations, &mut report.repairs);
        if report.repairs.len() == before {
            report.remaining = violations;
            return (None, report);
        }
    }
    report.remaining = project.validate();
    if report.remaining.is_empty() {
        (Some(project), report)
    } else {
        (None, report)
    }
}

/// The repair stage a violation belongs to.
fn stage(v: &Violation) -> u8 {
    match v {
        Violation::StationKeyMismatch { .. }
        | Violation::IdOutOfRange { .. }
        | Violation::ArchivedIdCollision { .. }
        | Violation::ArchivedIdOutOfRange { .. }
        | Violation::DuplicateLineId { .. }
        | Violation::DuplicateClosureId { .. }
        | Violation::DuplicateSectionId { .. }
        | Violation::InvalidShotUuid { .. }
        | Violation::DuplicateShotUuid { .. } => 0,
        Violation::DanglingLineStart { .. }
        | Violation::DanglingShotTarget { .. }
        | Violation::DanglingDatumStation { .. }
        | Violation::DanglingPlacementStation { .. }
        | Violation::DanglingClosureEnd { .. }
        | Violation::StaleDatumVia { .. }
        | Violation::StaleClosureVia { .. }
        | Violation::StaleArchivedLine { .. }
        | Violation::LineNamedTwice { .. }
        | Violation::NoNativeLine { .. }
        | Violation::HandMoveOffLine { .. }
        | Violation::DanglingSection { .. } => 1,
        Violation::RepeatedStationInChain { .. }
        | Violation::OrphanStation { .. }
        | Violation::Cycle { .. } => 2,
        Violation::UnanchoredComponent { .. }
        | Violation::ExtraPlacement { .. }
        | Violation::PlacementInDatumComponent { .. }
        | Violation::ClosureSelfLoop { .. }
        | Violation::ClosureAcrossUnplacedPieces { .. }
        | Violation::StationArchivedTwice { .. }
        | Violation::ClosureArchiveCycle { .. } => 3,
        Violation::DuplicateStationName { .. }
        | Violation::DuplicateSectionName { .. }
        | Violation::CounterNotAbove { .. }
        | Violation::CounterOutOfRange { .. }
        | Violation::NonFinite { .. }
        | Violation::OutOfBounds { .. }
        | Violation::TooManyShots { .. } => 4,
    }
}

/// Fresh ids and lookups for one pass, computed once so a pass stays linear
/// however many violations it repairs.
struct Pass {
    next_station: StationId,
    next_closure: ClosureId,
    next_line: LineId,
    archived: BTreeSet<StationId>,
    uuids: HashSet<String>,
    /// Positions of each line id in the line list.
    positions: HashMap<LineId, Vec<usize>>,
}

impl Pass {
    fn of(p: &Project) -> Self {
        let mut positions: HashMap<LineId, Vec<usize>> = HashMap::new();
        for (i, line) in p.lines().iter().enumerate() {
            positions.entry(line.id).or_default().push(i);
        }
        Self {
            next_station: fresh_station_id(p),
            next_closure: fresh_closure_id(p),
            next_line: p
                .lines()
                .iter()
                .filter(|l| l.id <= MAX_LINE_ID)
                .map(|l| l.id.saturating_add(1))
                .max()
                .unwrap_or(0),
            archived: p
                .stations
                .values()
                .flat_map(|s| s.archived.iter().map(|r| r.id))
                .collect(),
            uuids: p
                .lines()
                .iter()
                .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
                .collect(),
            positions,
        }
    }

    fn station(&mut self, p: &mut Project, name: String) -> StationId {
        let id = self.next_station;
        self.next_station = id.saturating_add(1);
        p.stations.insert(id, Station::new(id, name));
        p.observe_station_id(id);
        id
    }

    fn closure(&mut self, p: &mut Project) -> ClosureId {
        let id = self.next_closure;
        self.next_closure = id.saturating_add(1);
        p.counters.next_closure_id = p.counters.next_closure_id.max(self.next_closure);
        id
    }

    fn uuid(&mut self) -> String {
        loop {
            let uuid = uuid::Uuid::new_v4().to_string();
            if self.uuids.insert(uuid.clone()) {
                return uuid;
            }
        }
    }

    /// The `nth` position of line `id` (0: the first).
    fn position(&self, id: LineId, nth: usize) -> Option<usize> {
        self.positions.get(&id).and_then(|v| v.get(nth)).copied()
    }
}

/// Edits to the line list, applied together at the end of a pass.
#[derive(Default)]
struct LineEdits {
    uuids: Vec<(usize, usize, String)>,
    /// (position, chain index, station)
    slots: Vec<(usize, usize, StationId)>,
    renumber: Vec<(usize, LineId)>,
    drop: BTreeSet<usize>,
    clear_section: Vec<usize>,
}

fn repair_pass(p: &mut Project, violations: &[Violation], repairs: &mut Vec<Repair>) {
    let Some(first) = violations.iter().map(stage).min() else {
        return;
    };
    let mut pass = Pass::of(p);
    let mut edits = LineEdits::default();
    let mut remap: BTreeMap<StationId, StationId> = BTreeMap::new();
    let mut drop_datums = BTreeSet::new();
    let mut drop_placements = BTreeSet::new();
    let mut drop_closures = BTreeSet::new();
    let mut stale = false;
    let (mut normalised, mut restored, mut lettered) = (false, false, false);
    // Within a pass, act on each line at most once for chain repairs (later
    // violations about it are re-checked next pass).
    let mut touched_lines = BTreeSet::new();
    for v in violations.iter().filter(|v| stage(v) == first) {
        match v {
            Violation::StationKeyMismatch { key, id } => {
                if let Some(station) = p.stations.get_mut(key) {
                    station.id = *key;
                    repairs.push(Repair::RenumberedStation {
                        from: *id,
                        to: *key,
                    });
                }
            }
            Violation::IdOutOfRange { kind, id } => match kind {
                CounterKind::Station => {
                    if let Ok(from) = StationId::try_from(*id) {
                        if let std::collections::btree_map::Entry::Vacant(slot) = remap.entry(from)
                        {
                            let to = pass.next_station;
                            pass.next_station = to.saturating_add(1);
                            slot.insert(to);
                            repairs.push(Repair::RenumberedStation { from, to });
                        }
                    }
                }
                CounterKind::Line => {
                    if let Ok(from) = LineId::try_from(*id) {
                        for position in pass.positions.get(&from).cloned().unwrap_or_default() {
                            renumber_line(&mut pass, &mut edits, position, from, repairs);
                        }
                    }
                }
                CounterKind::Section => {
                    if let Ok(section) = SectionId::try_from(*id) {
                        p.sections.retain(|s| s.id != section);
                        for (position, line) in p.lines().iter().enumerate() {
                            if line.section_id == Some(section) {
                                edits.clear_section.push(position);
                            }
                        }
                        repairs.push(Repair::DroppedSection { id: section });
                    }
                }
                CounterKind::Closure => {
                    // Past the format's integer bound: no counter above it
                    // can be written, so the closure goes.
                    let before = p.closures.len();
                    p.closures.retain(|c| c.id != *id);
                    if p.closures.len() < before {
                        repairs.push(Repair::DroppedClosure { id: *id });
                    }
                }
            },
            Violation::ArchivedIdCollision { station, archived }
            | Violation::ArchivedIdOutOfRange { station, archived } => {
                if let Some(s) = p.stations.get_mut(station) {
                    if let Some(i) = s.archived.iter().position(|r| r.id == *archived) {
                        s.archived.remove(i);
                        repairs.push(Repair::DroppedArchived {
                            station: *station,
                            archived: *archived,
                        });
                    }
                }
            }
            Violation::DuplicateLineId { line } => {
                if touched_lines.insert(*line) {
                    if let Some(position) = pass.position(*line, 1) {
                        renumber_line(&mut pass, &mut edits, position, *line, repairs);
                    }
                }
            }
            Violation::DuplicateClosureId { closure } => {
                let next = pass.closure(p);
                if let Some(c) = p.closures.iter_mut().filter(|c| c.id == *closure).nth(1) {
                    c.id = next;
                    repairs.push(Repair::RenumberedClosure {
                        from: *closure,
                        to: next,
                    });
                }
            }
            Violation::DuplicateSectionId { section } => {
                if let Some(i) = p
                    .sections
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.id == *section)
                    .nth(1)
                    .map(|(i, _)| i)
                {
                    p.sections.remove(i);
                    repairs.push(Repair::DroppedSection { id: *section });
                }
            }
            Violation::InvalidShotUuid { line, index, .. }
            | Violation::DuplicateShotUuid {
                second: (line, index),
                ..
            } => {
                if let Some(position) = pass.position(*line, 0) {
                    let uuid = pass.uuid();
                    edits.uuids.push((position, *index, uuid.clone()));
                    repairs.push(Repair::ReMintedShotUuid {
                        line: *line,
                        index: *index,
                        uuid,
                    });
                }
            }
            Violation::DanglingLineStart { line, station } => {
                restore_chain_station(p, &mut pass, &mut edits, (*line, 0), *station, repairs);
            }
            Violation::DanglingShotTarget {
                line,
                index,
                station,
            } => {
                restore_chain_station(
                    p,
                    &mut pass,
                    &mut edits,
                    (*line, index + 1),
                    *station,
                    repairs,
                );
            }
            Violation::DanglingDatumStation { datum, .. } => {
                drop_datums.insert(*datum);
            }
            Violation::DanglingPlacementStation { placement, .. }
            | Violation::ExtraPlacement { placement, .. }
            | Violation::PlacementInDatumComponent { placement, .. } => {
                drop_placements.insert(*placement);
            }
            Violation::DanglingClosureEnd { closure, .. }
            | Violation::ClosureSelfLoop { closure, .. }
            | Violation::ClosureAcrossUnplacedPieces { closure, .. } => {
                drop_closures.insert(*closure);
            }
            Violation::StationArchivedTwice { .. } | Violation::ClosureArchiveCycle { .. } => {
                if !normalised {
                    normalised = true;
                    for id in p.normalise_closure_archives() {
                        repairs.push(Repair::UnarchivedClosure { id });
                    }
                }
            }
            Violation::StaleDatumVia { .. }
            | Violation::StaleClosureVia { .. }
            | Violation::StaleArchivedLine { .. }
            | Violation::HandMoveOffLine { .. } => stale = true,
            // A3: the later record naming a line no longer names it.
            Violation::LineNamedTwice {
                station,
                line,
                second,
                ..
            } => {
                if let Some(s) = p.stations.get_mut(station) {
                    for record in &mut s.archived {
                        if record.id == *second && record.lines.contains(line) {
                            record.lines.retain(|l| l != line);
                            repairs.push(Repair::UnnamedArchivedLine {
                                station: *station,
                                archived: *second,
                                line: *line,
                            });
                        }
                    }
                }
            }
            // A4: the station takes back the identity of the record naming
            // its lines (RS 4.3, rule K when several do), once per pass for
            // every such station.
            Violation::NoNativeLine { .. } => {
                if !restored {
                    restored = true;
                    for r in p.restore_archived_identities() {
                        repairs.push(Repair::RestoredArchived {
                            from: r.old,
                            to: r.new,
                        });
                    }
                }
            }
            Violation::DanglingSection { line, section } => {
                if let Some(position) = pass.position(*line, 0) {
                    edits.clear_section.push(position);
                    repairs.push(Repair::ClearedSection {
                        line: *line,
                        section: *section,
                    });
                }
            }
            Violation::RepeatedStationInChain {
                line,
                station,
                second,
                ..
            } => {
                if touched_lines.insert(*line) {
                    if let Some(position) = pass.position(*line, 0) {
                        let name = p
                            .shown_name(*station)
                            .map_or_else(|| station.to_string(), str::to_string);
                        let to = pass.station(p, name);
                        edits.slots.push((position, *second, to));
                        repairs.push(Repair::ReplacedChainStation {
                            line: *line,
                            index: *second,
                            from: *station,
                            to,
                        });
                    }
                }
            }
            Violation::OrphanStation { station } => {
                p.stations.remove(station);
                repairs.push(Repair::DroppedStation { id: *station });
            }
            Violation::Cycle {
                line, index, to, ..
            } => {
                if touched_lines.insert(*line) {
                    if let Some(position) = pass.position(*line, 0) {
                        let name = p
                            .shown_name(*to)
                            .map_or_else(|| to.to_string(), str::to_string);
                        let new_station = pass.station(p, name);
                        edits.slots.push((position, index + 1, new_station));
                        let closure = pass.closure(p);
                        p.closures.push(LoopClosure {
                            id: closure,
                            a: new_station,
                            a_via: Some(*line),
                            b: *to,
                            b_via: None,
                            strategy: ClosureStrategy::Batch,
                            // The new end shows the old one (RS 5.1), as
                            // the shot did: nothing a line shows changes.
                            archived: Some(ClosureEnd::A),
                        });
                        repairs.push(Repair::BrokeCycle {
                            line: *line,
                            index: *index,
                            station: *to,
                            new_station,
                            closure,
                        });
                    }
                }
            }
            Violation::UnanchoredComponent { lowest_station } => {
                p.placements.push(Placement {
                    station: *lowest_station,
                    x: 0.0,
                    y: 0.0,
                });
                repairs.push(Repair::AddedPlacement {
                    station: *lowest_station,
                });
            }
            Violation::DuplicateStationName { .. } => {
                // Rule L, once per pass for every collision: of the
                // identities sharing a name on a line, the ones shown on
                // fewer lines take a letter free on every line they are
                // shown on (never the junction shown on more lines); a tie
                // keeps the lowest Station ID.
                if !lettered {
                    lettered = true;
                    for l in p.letter_collisions_with(BTreeSet::new) {
                        repairs.push(Repair::RenamedStation {
                            id: l.station,
                            name: l.to,
                        });
                    }
                }
            }
            Violation::DuplicateSectionName { name, second, .. } => {
                rename_section(p, *second, name, repairs);
            }
            Violation::CounterNotAbove { counter, .. } => raise_counter(p, *counter, repairs),
            Violation::CounterOutOfRange { counter, .. } => lower_counter(p, *counter, repairs),
            // Splitting a line is not a repair: such a file stays refused.
            Violation::TooManyShots { .. } => {}
            // No JSON number is non-finite; nothing to repair.
            Violation::NonFinite { .. } => {}
            // A distance or coordinate past its bound (B-derived-solver-3) is
            // not guessed at: such a file stays refused.
            Violation::OutOfBounds { .. } => {}
        }
    }
    apply_line_edits(p, edits);
    if !remap.is_empty() {
        remap_stations(p, &remap);
    }
    if stale {
        let dropped = p.drop_stale_attachments();
        repairs.push(Repair::ClearedAttachments {
            vias: dropped.datum_vias.len()
                + dropped.closure_vias.len()
                + dropped.archived_lines.len(),
            hand_moves: dropped.hand_moves.len(),
        });
    }
    for index in drop_datums.into_iter().rev() {
        if index < p.datums.len() {
            let datum = p.datums.remove(index);
            repairs.push(Repair::DroppedDatum {
                station: datum.station,
            });
        }
    }
    for index in drop_placements.into_iter().rev() {
        if index < p.placements.len() {
            let placement = p.placements.remove(index);
            repairs.push(Repair::DroppedPlacement {
                station: placement.station,
            });
        }
    }
    if !drop_closures.is_empty() {
        p.closures.retain(|c| {
            if drop_closures.contains(&c.id) {
                repairs.push(Repair::DroppedClosure { id: c.id });
                false
            } else {
                true
            }
        });
    }
}

fn apply_line_edits(p: &mut Project, edits: LineEdits) {
    let lines = p.lines_mut();
    for (position, index, uuid) in edits.uuids {
        if let Some(shot) = lines.get_mut(position).and_then(|l| l.shots.get_mut(index)) {
            shot.uuid = uuid;
        }
    }
    for (position, index, station) in edits.slots {
        if let Some(line) = lines.get_mut(position) {
            if index == 0 {
                line.start = station;
            } else if let Some(shot) = line.shots.get_mut(index - 1) {
                shot.to = station;
            }
        }
    }
    for (position, id) in edits.renumber {
        if let Some(line) = lines.get_mut(position) {
            line.id = id;
            line.assign_session_shot_ids();
        }
    }
    for position in edits.clear_section {
        if let Some(line) = lines.get_mut(position) {
            line.section_id = None;
        }
    }
    for position in edits.drop.into_iter().rev() {
        if position < lines.len() {
            lines.remove(position);
        }
    }
}

/// A station id above every id and alias in use (and the counter).
fn fresh_station_id(p: &Project) -> StationId {
    p.stations
        .values()
        .flat_map(|s| std::iter::once(s.id).chain(s.archived.iter().map(|r| r.id)))
        .filter(|&id| id < MAX_STATION_ID)
        .map(|id| id.saturating_add(1))
        .chain(std::iter::once(
            p.counters.next_station_id.min(MAX_STATION_ID),
        ))
        .max()
        .unwrap_or(0)
}

fn fresh_closure_id(p: &Project) -> ClosureId {
    p.closures
        .iter()
        .map(|c| c.id.saturating_add(1))
        .chain(std::iter::once(p.counters.next_closure_id))
        .max()
        .unwrap_or(1)
}

/// Replace every station of `remap` by its new id, in one sweep.
fn remap_stations(p: &mut Project, remap: &BTreeMap<StationId, StationId>) {
    let to = |s: StationId| remap.get(&s).copied().unwrap_or(s);
    for (&from, &new) in remap {
        if let Some(mut station) = p.stations.remove(&from) {
            station.id = new;
            p.stations.insert(new, station);
        }
        p.observe_station_id(new);
    }
    for line in p.lines_mut() {
        line.start = to(line.start);
        for shot in &mut line.shots {
            shot.to = to(shot.to);
        }
        let hand = std::mem::take(&mut line.hand_moves);
        line.hand_moves = hand.into_iter().map(|(s, h)| (to(s), h)).collect();
    }
    for datum in &mut p.datums {
        datum.station = to(datum.station);
    }
    for placement in &mut p.placements {
        placement.station = to(placement.station);
    }
    for closure in &mut p.closures {
        closure.a = to(closure.a);
        closure.b = to(closure.b);
    }
}

/// Give the line at `position` (id `from`) a fresh id, or drop it when no id
/// is left.
fn renumber_line(
    pass: &mut Pass,
    edits: &mut LineEdits,
    position: usize,
    from: LineId,
    repairs: &mut Vec<Repair>,
) {
    let next = pass.next_line;
    if next > MAX_LINE_ID {
        edits.drop.insert(position);
        repairs.push(Repair::DroppedLine { id: from });
        return;
    }
    pass.next_line = next + 1;
    edits.renumber.push((position, next));
    repairs.push(Repair::RenumberedLine { from, to: next });
}

/// A chain references station `id`, which has no record: create it when the
/// id can be a station, else put a fresh station in that chain slot.
fn restore_chain_station(
    p: &mut Project,
    pass: &mut Pass,
    edits: &mut LineEdits,
    (line, index): (LineId, usize),
    id: StationId,
    repairs: &mut Vec<Repair>,
) {
    if p.stations.contains_key(&id) {
        return;
    }
    if id < MAX_STATION_ID && !pass.archived.contains(&id) {
        p.stations.insert(id, Station::new(id, id.to_string()));
        p.observe_station_id(id);
        pass.next_station = pass.next_station.max(id.saturating_add(1));
        repairs.push(Repair::CreatedStation { id });
        return;
    }
    let Some(position) = pass.position(line, 0) else {
        return;
    };
    let to = pass.station(p, id.to_string());
    edits.slots.push((position, index, to));
    repairs.push(Repair::ReplacedChainStation {
        line,
        index,
        from: id,
        to,
    });
}

/// Give Section `id` (named `name`, like an earlier one) the first free
/// `name (n)`, the suffix Add .stalx gives a colliding name.
fn rename_section(p: &mut Project, id: SectionId, name: &str, repairs: &mut Vec<Repair>) {
    let taken: BTreeSet<&str> = p.sections.iter().map(|s| s.name.as_str()).collect();
    // `taken` is finite, so some suffix is free.
    let mut n = 2u64;
    let fresh = loop {
        let candidate = format!("{name} ({n})");
        if !taken.contains(candidate.as_str()) {
            break candidate;
        }
        n += 1;
    };
    let Some(section) = p.sections.iter_mut().find(|s| s.id == id) else {
        return;
    };
    section.name.clone_from(&fresh);
    repairs.push(Repair::RenamedSection { id, name: fresh });
}

fn lower_counter(p: &mut Project, counter: CounterKind, repairs: &mut Vec<Repair>) {
    let to = match counter {
        CounterKind::Station => {
            p.counters.next_station_id = MAX_STATION_ID;
            u64::from(MAX_STATION_ID)
        }
        CounterKind::Section => {
            p.counters.next_section_id = MAX_SECTION_ID;
            u64::from(MAX_SECTION_ID)
        }
        CounterKind::Closure => {
            p.counters.next_closure_id = MAX_CLOSURE_ID;
            MAX_CLOSURE_ID
        }
        CounterKind::Line => return,
    };
    repairs.push(Repair::LoweredCounter { counter, to });
}

fn raise_counter(p: &mut Project, counter: CounterKind, repairs: &mut Vec<Repair>) {
    let to = match counter {
        CounterKind::Station => {
            let next = fresh_station_id(p);
            p.counters.next_station_id = next;
            u64::from(next)
        }
        CounterKind::Closure => {
            let next = fresh_closure_id(p);
            p.counters.next_closure_id = next;
            next
        }
        CounterKind::Section => {
            let next = p
                .sections
                .iter()
                .map(|s| s.id.saturating_add(1))
                .chain(std::iter::once(p.counters.next_section_id))
                .max()
                .unwrap_or(0);
            p.counters.next_section_id = next;
            u64::from(next)
        }
        CounterKind::Line => return,
    };
    repairs.push(Repair::RaisedCounter { counter, to });
}
