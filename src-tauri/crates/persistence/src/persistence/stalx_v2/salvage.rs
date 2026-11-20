//! Salvage (plan 6.6): open a file that parses but breaks the model
//! invariants, and repair a copy as best it can. Every repair is reported;
//! nothing here runs on a normal open.
//!
//! Repairs run in stages, earliest first, re-validating after each pass: ids,
//! then references, then chains / orphans / cycles, then anchors and
//! closures, then names and counters. A later stage only sees a model whose
//! earlier invariants hold, so repairs never fight each other.

use std::collections::BTreeSet;
use std::fmt;

use crate::models::survey::{
    ClosureId, ClosureStrategy, CounterKind, LoopClosure, Placement, Project, Station, Violation,
};
use crate::models::{LineId, SectionId, StationId, MAX_LINE_ID, MAX_STATION_ID};

use super::decode::parse;
use super::error::DecodeError;
use super::types::{DecodeNote, DecodeOptions, Extras};

/// The most repair passes before salvage gives up.
const MAX_PASSES: usize = 64;

/// One repair salvage made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// A second station record with the same id was dropped.
    DroppedDuplicateStation { id: StationId },
    /// A station's id was out of range and got a fresh one.
    RenumberedStation { from: StationId, to: StationId },
    /// A line with a repeated or out-of-range id got a fresh one.
    RenumberedLine { from: LineId, to: LineId },
    /// A line had a repeated id and no fresh id was left.
    DroppedLine { id: LineId },
    RenumberedClosure { from: ClosureId, to: ClosureId },
    DroppedSection { id: SectionId },
    DroppedAlias { station: StationId, alias: StationId },
    ReMintedShotUuid { line: LineId, index: usize, uuid: String },
    /// A line referenced a station with no record; one was created.
    CreatedStation { id: StationId },
    /// A chain station was replaced by a fresh one (a repeated station, or a
    /// reference no record can be created for).
    ReplacedChainStation {
        line: LineId,
        index: usize,
        from: StationId,
        to: StationId,
    },
    DroppedDatum { station: StationId },
    DroppedPlacement { station: StationId },
    DroppedClosure { id: ClosureId },
    /// Stale `via` hints cleared and hand moves dropped.
    ClearedAttachments { vias: usize, hand_moves: usize },
    ClearedSection { line: LineId, section: SectionId },
    /// A station on no line was dropped.
    DroppedStation { id: StationId },
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
    AddedPlacement { station: StationId },
    RenamedStation { id: StationId, name: String },
    RaisedCounter { counter: CounterKind, to: u64 },
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
            Self::DroppedAlias { station, alias } => {
                write!(f, "dropped alias {alias} of station {station}")
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
                 {station} by closure {closure}"
            ),
            Self::AddedPlacement { station } => {
                write!(f, "placed the piece of station {station} at the origin")
            }
            Self::RenamedStation { id, name } => write!(f, "station {id} is now named {name:?}"),
            Self::RaisedCounter { counter, to } => {
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
    let (mut project, extras, notes) = match doc.assemble(Some(&mut report.repairs)) {
        Ok(parts) => parts,
        Err(error) => {
            report.error = Some(error);
            return (None, report);
        }
    };
    report.extras = Some(extras);
    report.notes = notes;
    for _ in 0..MAX_PASSES {
        let violations = project.validate();
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
        | Violation::AliasCollision { .. }
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
        | Violation::HandMoveOffLine { .. }
        | Violation::DanglingSection { .. } => 1,
        Violation::RepeatedStationInChain { .. }
        | Violation::OrphanStation { .. }
        | Violation::Cycle { .. } => 2,
        Violation::UnanchoredComponent { .. }
        | Violation::ExtraPlacement { .. }
        | Violation::PlacementInDatumComponent { .. }
        | Violation::ClosureSelfLoop { .. }
        | Violation::ClosureAcrossUnplacedPieces { .. } => 3,
        Violation::DuplicateStationName { .. }
        | Violation::CounterNotAbove { .. }
        | Violation::NonFinite { .. } => 4,
    }
}

fn repair_pass(p: &mut Project, violations: &[Violation], repairs: &mut Vec<Repair>) {
    let Some(first) = violations.iter().map(stage).min() else {
        return;
    };
    let mut drop_datums = BTreeSet::new();
    let mut drop_placements = BTreeSet::new();
    let mut drop_closures = BTreeSet::new();
    let mut stale = false;
    // Within a stage, act on each line, closure id or station at most once
    // per pass (later violations about it are re-checked next pass).
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
                        let to = fresh_station_id(p);
                        remap_station(p, from, to);
                        repairs.push(Repair::RenumberedStation { from, to });
                    }
                }
                CounterKind::Line => {
                    if let Ok(from) = LineId::try_from(*id) {
                        renumber_line(p, from, true, repairs);
                    }
                }
                CounterKind::Section => {
                    if let Ok(section) = SectionId::try_from(*id) {
                        drop_section(p, section, repairs);
                    }
                }
                CounterKind::Closure => {}
            },
            Violation::AliasCollision { station, alias } => {
                if let Some(s) = p.stations.get_mut(station) {
                    if let Some(i) = s.aliases.iter().position(|a| a.id == *alias) {
                        s.aliases.remove(i);
                        repairs.push(Repair::DroppedAlias {
                            station: *station,
                            alias: *alias,
                        });
                    }
                }
            }
            Violation::DuplicateLineId { line } => {
                if touched_lines.insert(*line) {
                    renumber_line(p, *line, false, repairs);
                }
            }
            Violation::DuplicateClosureId { closure } => {
                let next = fresh_closure_id(p);
                if let Some(c) = p.closures.iter_mut().filter(|c| c.id == *closure).nth(1) {
                    c.id = next;
                    p.counters.next_closure_id = next.saturating_add(1);
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
                let uuid = p.mint_shot_uuid();
                if let Some(shot) = p.line_mut(*line).and_then(|l| l.shots.get_mut(*index)) {
                    shot.uuid.clone_from(&uuid);
                    repairs.push(Repair::ReMintedShotUuid {
                        line: *line,
                        index: *index,
                        uuid,
                    });
                }
            }
            Violation::DanglingLineStart { line, station } => {
                restore_chain_station(p, *line, 0, *station, repairs);
            }
            Violation::DanglingShotTarget {
                line,
                index,
                station,
            } => {
                restore_chain_station(p, *line, index + 1, *station, repairs);
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
            Violation::StaleDatumVia { .. }
            | Violation::StaleClosureVia { .. }
            | Violation::HandMoveOffLine { .. } => stale = true,
            Violation::DanglingSection { line, section } => {
                if let Some(l) = p.line_mut(*line) {
                    l.section_id = None;
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
                    let name = p
                        .station(*station)
                        .map_or_else(|| station.to_string(), |s| s.name.clone());
                    let to = new_station(p, name);
                    if set_chain_station(p, *line, *second, to) {
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
                    break_cycle(p, *line, *index, *to, repairs);
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
            Violation::DuplicateStationName { second, .. } => {
                if let Some(name) = p.uniquify_station_name(*second) {
                    repairs.push(Repair::RenamedStation { id: *second, name });
                }
            }
            Violation::CounterNotAbove { counter, .. } => raise_counter(p, *counter, repairs),
            // No JSON number is non-finite; nothing to repair.
            Violation::NonFinite { .. } => {}
        }
    }
    if stale {
        let dropped = p.drop_stale_attachments();
        repairs.push(Repair::ClearedAttachments {
            vias: dropped.datum_vias.len() + dropped.closure_vias.len(),
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
    for id in drop_closures {
        if let Some(i) = p.closures.iter().position(|c| c.id == id) {
            p.closures.remove(i);
            repairs.push(Repair::DroppedClosure { id });
        }
    }
}

/// A station id above every id and alias in use (and the counter).
fn fresh_station_id(p: &Project) -> StationId {
    p.stations
        .values()
        .flat_map(|s| std::iter::once(s.id).chain(s.aliases.iter().map(|a| a.id)))
        .filter(|&id| id < MAX_STATION_ID)
        .map(|id| id.saturating_add(1))
        .chain(std::iter::once(p.counters.next_station_id.min(MAX_STATION_ID)))
        .max()
        .unwrap_or(0)
}

/// A new station named `name` (uniquified later if it clashes).
fn new_station(p: &mut Project, name: String) -> StationId {
    let id = fresh_station_id(p);
    p.stations.insert(id, Station::new(id, name));
    p.observe_station_id(id);
    id
}

fn fresh_closure_id(p: &Project) -> ClosureId {
    p.closures
        .iter()
        .map(|c| c.id.saturating_add(1))
        .chain(std::iter::once(p.counters.next_closure_id))
        .max()
        .unwrap_or(1)
}

/// Replace station `from` by `to` everywhere it is referenced.
fn remap_station(p: &mut Project, from: StationId, to: StationId) {
    if let Some(mut station) = p.stations.remove(&from) {
        station.id = to;
        p.stations.insert(to, station);
    }
    p.observe_station_id(to);
    for line in p.lines_mut() {
        if line.start == from {
            line.start = to;
        }
        for shot in &mut line.shots {
            if shot.to == from {
                shot.to = to;
            }
        }
        if let Some(hand) = line.hand_moves.remove(&from) {
            line.hand_moves.insert(to, hand);
        }
    }
    for datum in &mut p.datums {
        if datum.station == from {
            datum.station = to;
        }
    }
    for placement in &mut p.placements {
        if placement.station == from {
            placement.station = to;
        }
    }
    for closure in &mut p.closures {
        if closure.a == from {
            closure.a = to;
        }
        if closure.b == from {
            closure.b = to;
        }
    }
}

/// Give the second line with id `id` (the first when `any`) a fresh id.
fn renumber_line(p: &mut Project, id: LineId, any: bool, repairs: &mut Vec<Repair>) {
    let next = p
        .lines()
        .iter()
        .filter(|l| l.id <= MAX_LINE_ID)
        .map(|l| l.id.saturating_add(1))
        .max()
        .unwrap_or(0);
    let skip = usize::from(!any);
    let Some(position) = p
        .lines()
        .iter()
        .enumerate()
        .filter(|(_, l)| l.id == id)
        .nth(skip)
        .map(|(i, _)| i)
    else {
        return;
    };
    if next > MAX_LINE_ID {
        p.lines_mut().remove(position);
        repairs.push(Repair::DroppedLine { id });
        return;
    }
    if let Some(line) = p.lines_mut().get_mut(position) {
        line.id = next;
        line.assign_session_shot_ids();
    }
    repairs.push(Repair::RenumberedLine { from: id, to: next });
}

fn drop_section(p: &mut Project, section: SectionId, repairs: &mut Vec<Repair>) {
    p.sections.retain(|s| s.id != section);
    for line in p.lines_mut() {
        if line.section_id == Some(section) {
            line.section_id = None;
        }
    }
    repairs.push(Repair::DroppedSection { id: section });
}

/// A chain references station `id`, which has no record: create it when the
/// id can be a station, else put a fresh station in that chain slot.
fn restore_chain_station(
    p: &mut Project,
    line: LineId,
    index: usize,
    id: StationId,
    repairs: &mut Vec<Repair>,
) {
    if p.stations.contains_key(&id) {
        return;
    }
    if id < MAX_STATION_ID && !p.is_alias_id(id) {
        p.stations.insert(id, Station::new(id, id.to_string()));
        p.observe_station_id(id);
        repairs.push(Repair::CreatedStation { id });
        return;
    }
    let to = new_station(p, id.to_string());
    if set_chain_station(p, line, index, to) {
        repairs.push(Repair::ReplacedChainStation {
            line,
            index,
            from: id,
            to,
        });
    }
}

/// Set chain slot `index` of `line` (0 is the start) to `station`.
fn set_chain_station(p: &mut Project, line: LineId, index: usize, station: StationId) -> bool {
    let Some(l) = p.line_mut(line) else {
        return false;
    };
    if index == 0 {
        l.start = station;
        return true;
    }
    match l.shots.get_mut(index - 1) {
        Some(shot) => {
            shot.to = station;
            true
        }
        None => false,
    }
}

/// Shot `index` of `line` closes a loop at `to`: end it at a new station and
/// tie that station to `to` with a `Batch` closure.
fn break_cycle(p: &mut Project, line: LineId, index: usize, to: StationId, repairs: &mut Vec<Repair>) {
    let name = p
        .station(to)
        .map_or_else(|| to.to_string(), |s| s.name.clone());
    let new_station = new_station(p, name);
    if !set_chain_station(p, line, index + 1, new_station) {
        return;
    }
    let id = fresh_closure_id(p);
    p.counters.next_closure_id = id.saturating_add(1);
    p.closures.push(LoopClosure {
        id,
        a: new_station,
        a_via: Some(line),
        b: to,
        b_via: None,
        strategy: ClosureStrategy::Batch,
    });
    repairs.push(Repair::BrokeCycle {
        line,
        index,
        station: to,
        new_station,
        closure: id,
    });
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
