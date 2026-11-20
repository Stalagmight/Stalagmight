//! The Ariane rows of a station-graph project (plan 10.5): the front end of
//! the TML/TMLU export on `models::survey`. Everything after the rows are
//! built (GPS on the STARTs, the geometry repair, the SpeleoDB hardening, the
//! orphan guard and the XML) is the parent module's.
//!
//! The walk is 0.6.0's, read off the station graph instead of the merge list
//! (plan Deviations X2):
//!
//! - **Entrances.** The frame datum's line roots the document; with no datum,
//!   the first line in list order that is no other line's child (a line is a
//!   child when it shares a station with a line of lower id). Every other
//!   connected component is rooted at its first line in list order that holds
//!   a placement datum, else its first line. An entrance line gets a START on
//!   the station its datum stands on (station 0 without one).
//! - **Edges oriented away from the entrance.** Lines are visited breadth first
//!   from each component's root through their junctions (neighbours by line
//!   id, then chain index); each line is walked once, outwards from the
//!   station it was reached at: its shots after that station forward, the
//!   ones before it reversed (`"<line> (reversed)"`, or `"(pre-zero-datum)"`
//!   on an entrance), exactly 0.6.0's blocks and row order.
//! - **Ids.** A row's `ID` is the Station ID of the station it ends at and its
//!   `FromID` that of the station it starts from, so a line hangs off its
//!   parent's row for the junction with no alias and no placeholder pass, and
//!   ids are stable across exports. A START's id is its station's. CLOSURE
//!   rows and the GPS-fix STARTs are numbered above every station.
//! - **Closures** become CLOSURE rows between their two stations (`Tie`s
//!   included: a migrated cycle-closing merge is a loop like any other).
//! - **Station comments** fold into the one row that ends at the station (a
//!   START drops it, as before).

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::Result;

use crate::models::survey::{Line, Project, Shot, Topology};
use crate::models::{comment_split, LineId, StationId, EMPTY_CREDIT};

use super::super::survey::shot_credit;
use super::datums::{closure_shot, stamp_starts_at, start_shot, DatumNodes};
use super::{
    color_to_ariane, enforce_ariane_geometry_invariants, export_uuid, harden_for_speleodb,
    names_for_path, pitch_to_inclination, remint_duplicate_uuids, reverse_pre_zero_azimuth,
    reversed_pitch, row_date, row_explorer, write_document, write_output, AzimuthExport,
    DocumentHead, ExportShot, Names, TmlProfile,
};

/// Export a station-graph project to Ariane TML (`.tml` = ZIP) or TMLU
/// (`.tmlu` = plain XML). Returns the notes the user has to be told about
/// this document (the azimuth compromises, unwritten GPS anchors). Notes,
/// not warnings: nothing failed, but something about the file is not what
/// the reader would assume, and the export command is the only moment the
/// user is looking.
pub fn export_tml(project: &Project, path: &Path) -> Result<Vec<String>> {
    export_with_profile(project, path, TmlProfile::User)
}

/// The SpeleoDB co-push sidecar (#196) of a station-graph project: the same
/// document, hardened for `openspeleo_lib`'s GeoJSON conversion (see
/// `TmlProfile` and `harden_for_speleodb`). Its notes go to the log with the
/// rest of the push: no user is watching it.
pub fn export_tml_for_speleodb(project: &Project, path: &Path) -> Result<()> {
    export_with_profile(project, path, TmlProfile::SpeleoDbSidecar).map(|_| ())
}

fn export_with_profile(project: &Project, path: &Path, profile: TmlProfile) -> Result<Vec<String>> {
    let (xml, notes) = build_xml(project, names_for_path(path), profile)?;
    write_output(path, &xml)?;
    Ok(notes)
}

pub(super) fn build_xml(
    project: &Project,
    n: &Names,
    profile: TmlProfile,
) -> Result<(Vec<u8>, Vec<String>)> {
    let azimuth_export = AzimuthExport::decide_from(
        project
            .lines()
            .iter()
            .filter(|line| !line.shots.is_empty())
            .map(|line| project.azimuth_reference_for(line)),
    );
    let (rows, unwritten_datums) = build_rows(project, profile, azimuth_export);
    write_document(
        &DocumentHead {
            cave_name: &project.name,
            surface_elevation: project.settings.surface_elevation,
            azimuth_export,
        },
        &rows,
        unwritten_datums,
        n,
        profile,
    )
}

/// The finished rows, plus how many datums ended up on no START (the
/// `gps-anchors-not-written` note).
pub(super) fn build_rows(
    project: &Project,
    profile: TmlProfile,
    azimuth_export: AzimuthExport,
) -> (Vec<ExportShot>, usize) {
    if project.lines().is_empty() {
        return (Vec::new(), 0);
    }
    let topology = project.topology();
    let frame = project.frame_datum();
    let root_line = frame
        .and_then(|datum| datum_line(project, topology, datum))
        .unwrap_or_else(|| default_root_line(project, topology));
    let entrance_datums = entrance_datums(project, topology);
    let entries = entry_stations(project, topology, root_line, &entrance_datums);

    let mut rows: Vec<ExportShot> = Vec::new();
    for line in project.lines() {
        let entrance_datum = if line.id == root_line {
            frame.filter(|&datum| datum_line(project, topology, datum) == Some(line.id))
        } else if !entries.contains_key(&line.id) {
            entrance_datums.get(&line.id).copied()
        } else {
            None
        };
        let entry = match entries.get(&line.id) {
            Some(&index) if line.id != root_line => Entry::Junction(index),
            _ => Entry::Start(
                entrance_datum
                    .and_then(|datum| topology.slot_on(project.datums[datum].station, line.id))
                    .map_or(0, |slot| slot.station_index as usize),
            ),
        };
        let emit = azimuth_export.emit(project.azimuth_reference_for(line));
        line_rows(line, entry, emit, &mut rows);
    }

    fold_station_comments(project, &mut rows);

    // CLOSURE rows (then the GPS-fix pairs) are numbered above every station.
    let mut next_id = project
        .stations
        .keys()
        .next_back()
        .map_or(0, |&id| i64::from(id) + 1);
    let closures = closure_rows(project, topology, &rows, &mut next_id);
    rows.extend(closures);

    // GPS, on START rows only: every datum is an entrance (#741).
    let raw = project.raw_positions();
    let with_row: HashSet<i64> = rows.iter().map(|row| row.id).collect();
    let nodes = DatumNodes {
        gps: project.datums.iter().map(|datum| datum.gps).collect(),
        nodes: project
            .datums
            .iter()
            .map(|datum| Some(i64::from(datum.station)).filter(|id| with_row.contains(id)))
            .collect(),
        frame,
        frame_xy: frame
            .and_then(|datum| raw.xy(project.datums[datum].station))
            .map(|[x, y]| (x, y)),
    };
    let start_xy = |start: i64| {
        let station = StationId::try_from(start).ok()?;
        raw.xy(station).map(|[x, y]| (x, y))
    };
    let datum_gps = stamp_starts_at(nodes, start_xy, &mut rows, profile);
    enforce_ariane_geometry_invariants(&mut rows, profile);
    let unwritten = datum_gps.append_fix_starts(&mut rows, next_id);

    match profile {
        TmlProfile::SpeleoDbSidecar => harden_for_speleodb(&mut rows),
        TmlProfile::User => remint_duplicate_uuids(&mut rows),
    }
    (rows, unwritten)
}

/// Where a line is entered.
#[derive(Debug, Clone, Copy)]
enum Entry {
    /// An entrance: a START on this chain index (its datum's station, else 0).
    Start(usize),
    /// A child line: it hangs off the junction at this chain index.
    Junction(usize),
}

/// The line a datum names: its `via` line, else its station's first line.
fn datum_line(project: &Project, topology: &Topology, datum: usize) -> Option<LineId> {
    let datum = project.datums.get(datum)?;
    datum
        .via
        .filter(|&line| topology.slot_on(datum.station, line).is_some())
        .or_else(|| topology.first_slot(datum.station).map(|slot| slot.line_id))
}

/// The root when no datum frames the survey: the first line in list order
/// that shares no station with a line of lower id, else the first line.
fn default_root_line(project: &Project, topology: &Topology) -> LineId {
    let mut children: HashSet<LineId> = HashSet::new();
    for &station in topology.stations() {
        let lines = topology.lines_of(station);
        if let Some(&lowest) = lines.iter().min() {
            children.extend(lines.iter().copied().filter(|&line| line != lowest));
        }
    }
    project
        .lines()
        .iter()
        .map(|line| line.id)
        .find(|id| !children.contains(id))
        .unwrap_or(project.lines()[0].id)
}

/// The first placement datum of each line (by [`datum_line`]), in datum list
/// order: the datum an entrance on that line stands on.
fn entrance_datums(project: &Project, topology: &Topology) -> HashMap<LineId, usize> {
    let placement: BTreeSet<usize> = project.placement_datums().into_iter().flatten().collect();
    let mut by_line: HashMap<LineId, usize> = HashMap::new();
    for datum in placement {
        if let Some(line) = datum_line(project, topology, datum) {
            by_line.entry(line).or_insert(datum);
        }
    }
    by_line
}

/// The chain index each non-entrance line is entered at (0.6.0's parent
/// map): a breadth-first walk over lines through their junctions from
/// `root`, then from one root per remaining connected piece (a line holding a
/// placement datum first, else the first in list order). A line with no
/// junction is in no piece and absent, an entrance of its own.
///
/// A line's children are the unvisited lines it shares a station with, in
/// line-id order, each entered at the lowest chain index of the stations the
/// two share. Linear in the slots (I-import-export-2): a station is expanded
/// once, by the first line that reaches it, which visits every line through
/// it; a later line through that station finds nothing new there, so it skips
/// it. Building every ordered pair of lines at a station instead cost
/// `k * (k - 1)` entries at a hub of `k` lines (gigabytes for a crafted file
/// with tens of thousands of lines off one station).
fn entry_stations(
    project: &Project,
    topology: &Topology,
    root: LineId,
    entrance_datums: &HashMap<LineId, usize>,
) -> HashMap<LineId, usize> {
    // The lines through some junction: the ones a walk can reach or start.
    let mut joined: HashSet<LineId> = HashSet::new();
    for &station in topology.stations() {
        if topology.is_junction(station) {
            joined.extend(topology.slots_of(station).iter().map(|slot| slot.line_id));
        }
    }
    let mut expanded: HashSet<StationId> = HashSet::new();
    let mut entries: HashMap<LineId, usize> = HashMap::new();
    let mut visited: HashSet<LineId> = HashSet::from([root]);
    let mut queue: VecDeque<LineId> = VecDeque::from([root]);
    // Both cursors only move forward: a line once visited stays visited.
    let lines: Vec<LineId> = project.lines().iter().map(|line| line.id).collect();
    let (mut datum_cursor, mut cursor) = (0, 0);
    loop {
        while let Some(parent) = queue.pop_front() {
            // (child, its chain index) for each station the two share.
            let mut children: Vec<(LineId, u32)> = Vec::new();
            for &station in topology.chain(parent).unwrap_or(&[]) {
                if !expanded.insert(station) {
                    continue;
                }
                children.extend(
                    topology
                        .slots_of(station)
                        .iter()
                        .filter(|slot| !visited.contains(&slot.line_id))
                        .map(|slot| (slot.line_id, slot.station_index)),
                );
            }
            children.sort_unstable();
            for (child, index) in children {
                if visited.insert(child) {
                    entries.insert(child, index as usize);
                    queue.push_back(child);
                }
            }
        }
        let candidate = |id: LineId| !visited.contains(&id) && joined.contains(&id);
        while datum_cursor < lines.len()
            && !(entrance_datums.contains_key(&lines[datum_cursor])
                && candidate(lines[datum_cursor]))
        {
            datum_cursor += 1;
        }
        while cursor < lines.len() && !candidate(lines[cursor]) {
            cursor += 1;
        }
        let Some(&next) = lines.get(datum_cursor).or_else(|| lines.get(cursor)) else {
            break;
        };
        visited.insert(next);
        queue.push_back(next);
    }
    entries
}

/// A station's depth as its line reads it (0.6.0's `station_depth`).
fn station_depth(line: &Line, index: usize) -> f64 {
    match (line.shots.first(), line.shots.last()) {
        (Some(first), Some(last)) => {
            if index == 0 {
                first.depth_in
            } else if index >= line.shots.len() {
                last.depth_out
            } else {
                line.shots[index].depth_in
            }
        }
        _ => 0.0,
    }
}

/// Append the rows of `line`, entered at `entry`: a START first on an
/// entrance, then the shots after the entry station forwards, then the ones
/// before it reversed.
fn line_rows(
    line: &Line,
    entry: Entry,
    emit: crate::models::AzimuthReference,
    rows: &mut Vec<ExportShot>,
) {
    let name = line
        .name
        .clone()
        .unwrap_or_else(|| format!("Line {}", line.id));
    let color = color_to_ariane(&line.color);
    let chain: Vec<StationId> = line.stations().collect();
    let (at, entrance) = match entry {
        Entry::Start(at) => (at.min(line.shots.len()), true),
        Entry::Junction(at) => (at.min(line.shots.len()), false),
    };
    let entry_depth = station_depth(line, at);
    if entrance {
        // A START takes the people and date of the shot it leads into.
        let credit = line
            .shots
            .get(at)
            .or(line.shots.first())
            .map_or(&EMPTY_CREDIT, shot_credit);
        rows.push(start_shot(
            i64::from(chain[at]),
            name.clone(),
            entry_depth,
            row_explorer(credit),
            row_date(credit),
            color.clone(),
        ));
    }

    let mut previous = entry_depth;
    for index in at..line.shots.len() {
        let shot = &line.shots[index];
        let mut row = shot_row(shot, &name, &color, chain[index], chain[index + 1]);
        row.azimuth = emit.to_true(shot.azimuth_in);
        row.depth = shot.depth_out;
        row.depth_in = shot.depth_in;
        row.depth_in_same_as_prev = (shot.depth_in - previous).abs() < 1e-9;
        row.inclination = pitch_to_inclination(shot.pitch_in);
        row.left = shot.left.unwrap_or(0.0);
        row.right = shot.right.unwrap_or(0.0);
        previous = shot.depth_out;
        rows.push(row);
    }

    let reversed_name = if entrance {
        format!("{name} (pre-zero-datum)")
    } else {
        format!("{name} (reversed)")
    };
    let mut previous = entry_depth;
    for index in (0..at).rev() {
        let shot = &line.shots[index];
        let mut row = shot_row(shot, &reversed_name, &color, chain[index + 1], chain[index]);
        // Ariane carries one forward azimuth per shot: a reversed leg is the
        // reciprocal heading, its depths swapped, its pitch negated and its
        // left and right exchanged.
        row.azimuth = reverse_pre_zero_azimuth(emit.to_true(shot.azimuth_in));
        row.depth = shot.depth_in;
        row.depth_in = shot.depth_out;
        row.depth_in_same_as_prev = (shot.depth_out - previous).abs() < 1e-9;
        row.inclination = pitch_to_inclination(reversed_pitch(shot.pitch_in, shot.pitch_out));
        row.left = shot.right.unwrap_or(0.0);
        row.right = shot.left.unwrap_or(0.0);
        previous = shot.depth_in;
        rows.push(row);
    }
}

/// The row of `shot` walked from station `from` to station `to`, with the
/// direction-dependent fields (azimuth, depths, pitch, left/right) left for
/// the caller.
fn shot_row(shot: &Shot, line: &str, color: &str, from: StationId, to: StationId) -> ExportShot {
    let credit = shot_credit(shot);
    ExportShot {
        id: i64::from(to),
        uuid: Some(export_uuid(Some(shot.uuid.as_str()))),
        from_id: i64::from(from),
        shot_type: if shot.kind.is_virtual() {
            "VIRTUAL"
        } else {
            "REAL"
        },
        line: line.to_string(),
        azimuth: 0.0,
        length: shot.length,
        depth: 0.0,
        depth_in: 0.0,
        depth_in_same_as_prev: false,
        inclination: 0.0,
        left: 0.0,
        right: 0.0,
        up: shot.up.unwrap_or(0.0),
        down: shot.down.unwrap_or(0.0),
        comment: shot.comment.clone(),
        explorer: row_explorer(credit),
        date: row_date(credit),
        color: color.to_string(),
        latitude: 0.0,
        longitude: 0.0,
        closure_to_id: -1,
        locked: false,
    }
}

/// Fold each station comment into the one REAL / VIRTUAL row that ends at the
/// station, and ZWSP-mark every shot comment (`comment_split`), as 0.6.0's
/// fold did. A station whose node is a START keeps no comment in the file.
fn fold_station_comments(project: &Project, rows: &mut [ExportShot]) {
    let mut by_row: HashMap<i64, &str> = HashMap::new();
    for station in project.stations.values() {
        let Some(text) = station.comment.as_deref().filter(|t| !t.is_empty()) else {
            continue;
        };
        by_row.insert(i64::from(station.id), text);
    }
    for row in rows.iter_mut() {
        if row.shot_type != "REAL" && row.shot_type != "VIRTUAL" {
            continue;
        }
        let station = by_row.remove(&row.id);
        row.comment = comment_split::merge(row.comment.as_deref(), station);
    }
    let mut dropped: Vec<i64> = by_row.into_keys().collect();
    dropped.sort_unstable();
    for station in dropped {
        tracing::debug!(
            station,
            "Station comment dropped from .tml export: the Ariane row for this station is a \
             START, which takes no station comment"
        );
    }
}

/// One CLOSURE row per closure whose two stations have rows, numbered from
/// `next_id`, named and coloured after end `a`'s line.
fn closure_rows(
    project: &Project,
    topology: &Topology,
    rows: &[ExportShot],
    next_id: &mut i64,
) -> Vec<ExportShot> {
    let node_rows: HashMap<i64, &ExportShot> = rows.iter().map(|row| (row.id, row)).collect();
    let mut out = Vec::new();
    for closure in &project.closures {
        let (from_id, to_id) = (i64::from(closure.a), i64::from(closure.b));
        if from_id == to_id || !node_rows.contains_key(&from_id) || !node_rows.contains_key(&to_id)
        {
            tracing::warn!(
                closure_id = closure.id,
                "Skipping closure export: an end has no row in this file"
            );
            continue;
        }
        let line = closure
            .a_via
            .filter(|&line| topology.slot_on(closure.a, line).is_some())
            .or_else(|| topology.first_slot(closure.a).map(|slot| slot.line_id))
            .and_then(|id| project.line(id));
        let line_name = line.map_or_else(
            || "Line".to_string(),
            |line| {
                line.name
                    .clone()
                    .unwrap_or_else(|| format!("Line {}", line.id))
            },
        );
        let color = line.map_or_else(
            || "0x4ecdc4ff".to_string(),
            |line| color_to_ariane(&line.color),
        );
        let depth_b = node_rows.get(&to_id).map_or(0.0, |row| row.depth);
        out.push(closure_shot(
            *next_id,
            from_id,
            to_id,
            line_name,
            color,
            node_rows.get(&from_id).copied(),
            depth_b,
        ));
        *next_id += 1;
    }
    out
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
#[path = "tml_survey_tests.rs"]
mod tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
#[path = "tml_survey_walk_tests.rs"]
mod walk_tests;
