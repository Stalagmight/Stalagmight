//! Overlap interpolation (the DMP wizard, `docs/dmp-import.md`): updates to
//! EXISTING shots that ride an import, applied before its lines are placed.
//!
//! A line whose updated shots sit between two anchored stations is skipped
//! (0.6.0's doubly-anchored guard): re-measuring the shots between two fixed
//! points would tear one of them. An anchor of line L is a station of L that
//! holds its piece's placement datum, or a junction or an aligned snap whose
//! other side reaches a placement datum without passing through L. The
//! updates of every other line are written, and the anchor algorithm then
//! keeps what 0.6.0 kept still: a datum-held piece stays on its datum, a free
//! piece keeps the edited line's first station (rule (a), a geometry edit).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::models::survey::{EditContext, Project};
use crate::models::{ExistingShotUpdate, LineId, StationId};

/// Apply `updates` to the project's existing shots; returns how many updates
/// the doubly-anchored guard skipped (the receipt says so).
pub(super) fn apply_overlap_updates(project: &mut Project, updates: &[ExistingShotUpdate]) -> u32 {
    if updates.is_empty() {
        return 0;
    }
    tracing::info!(
        count = updates.len(),
        purpose = "overlap interpolation",
        "Applying existing shot updates before placement",
    );
    let mut by_line: BTreeMap<LineId, Vec<&ExistingShotUpdate>> = BTreeMap::new();
    for update in updates {
        by_line.entry(update.line_id).or_default().push(update);
    }

    let blocked = doubly_anchored_lines(project, &by_line);
    let skipped = updates
        .iter()
        .filter(|u| blocked.contains(&u.line_id))
        .count() as u32;

    for (&line_id, line_updates) in &by_line {
        if blocked.contains(&line_id) {
            continue;
        }
        if project.line(line_id).is_none() {
            tracing::warn!(
                line_id,
                purpose = "overlap interpolation",
                "Line not found in project",
            );
            continue;
        }
        let before = project.anchor_snapshot();
        let Some(line) = project.line_mut(line_id) else {
            continue;
        };
        for update in line_updates {
            let Some(shot) = line.shots.iter_mut().find(|s| s.id == update.shot_id) else {
                tracing::warn!(
                    line_id,
                    shot_id = update.shot_id,
                    purpose = "overlap interpolation",
                    "Shot not found in line",
                );
                continue;
            };
            let u = &update.updates;
            let old_length = shot.length;
            if let Some(v) = u.azimuth_in {
                shot.azimuth_in = v;
            }
            if let Some(v) = u.azimuth_out {
                shot.azimuth_out = v;
            }
            if let Some(v) = u.length {
                shot.length = v;
            }
            if let Some(v) = u.depth_in {
                shot.depth_in = v;
            }
            if let Some(v) = u.depth_out {
                shot.depth_out = v;
            }
            if let Some(v) = u.pitch_in {
                shot.pitch_in = v;
            }
            if let Some(v) = u.pitch_out {
                shot.pitch_out = v;
            }
            if let Some(v) = u.left {
                shot.left = v;
            }
            if let Some(v) = u.right {
                shot.right = v;
            }
            if let Some(v) = u.up {
                shot.up = v;
            }
            if let Some(v) = u.down {
                shot.down = v;
            }
            tracing::debug!(
                line_id,
                shot_id = update.shot_id,
                old_length,
                new_length = shot.length,
                purpose = "overlap interpolation",
                "Updated existing shot",
            );
        }
        project.settle_anchors(&before, &EditContext::geometry(line_id));
    }
    skipped
}

/// The lines whose updated range lies between two anchors (module docs).
fn doubly_anchored_lines(
    project: &Project,
    by_line: &BTreeMap<LineId, Vec<&ExistingShotUpdate>>,
) -> BTreeSet<LineId> {
    let positions = project.raw_positions();
    let aligned = project.closure_alignment(&positions);
    let topology = project.topology();
    let placement_stations: BTreeSet<StationId> = project
        .placement_datums()
        .into_iter()
        .flatten()
        .filter_map(|i| project.datums.get(i).map(|d| d.station))
        .collect();
    let datum_lines: BTreeSet<LineId> = placement_stations
        .iter()
        .flat_map(|&s| topology.lines_of(s))
        .collect();
    // Lines adjacent through a junction or an aligned snap.
    let mut adjacent: BTreeMap<LineId, BTreeSet<LineId>> = BTreeMap::new();
    let mut link = |a: &[LineId], b: &[LineId]| {
        for &x in a {
            for &y in b {
                if x != y {
                    adjacent.entry(x).or_default().insert(y);
                    adjacent.entry(y).or_default().insert(x);
                }
            }
        }
    };
    for &station in topology.stations() {
        if topology.is_junction(station) {
            let lines = topology.lines_of(station);
            link(&lines, &lines);
        }
    }
    for (closure, &is_aligned) in project.closures.iter().zip(&aligned) {
        if is_aligned {
            link(&topology.lines_of(closure.a), &topology.lines_of(closure.b));
        }
    }
    let reaches_datum = |start: LineId, excluded: LineId| {
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([start]);
        while let Some(line) = queue.pop_front() {
            if line == excluded || !seen.insert(line) {
                continue;
            }
            if datum_lines.contains(&line) {
                return true;
            }
            if let Some(next) = adjacent.get(&line) {
                queue.extend(next.iter().copied().filter(|l| !seen.contains(l)));
            }
        }
        false
    };

    let mut blocked = BTreeSet::new();
    for (&line_id, line_updates) in by_line {
        let (Some(line), Some(chain)) = (project.line(line_id), topology.chain(line_id)) else {
            continue;
        };
        let modified: Vec<usize> = line_updates
            .iter()
            .filter_map(|u| line.shots.iter().position(|s| s.id == u.shot_id))
            .collect();
        let (Some(&min_shot), Some(&max_shot)) = (modified.iter().min(), modified.iter().max())
        else {
            continue;
        };
        let partner_reaches = |station: StationId| {
            topology
                .lines_of(station)
                .into_iter()
                .any(|partner| partner != line_id && reaches_datum(partner, line_id))
        };
        let mut anchors: Vec<usize> = Vec::new();
        for (index, &station) in chain.iter().enumerate() {
            if placement_stations.contains(&station)
                || (topology.is_junction(station) && partner_reaches(station))
            {
                anchors.push(index);
            }
        }
        for (closure, &is_aligned) in project.closures.iter().zip(&aligned) {
            if !is_aligned {
                continue;
            }
            for (end, other) in [(closure.a, closure.b), (closure.b, closure.a)] {
                if let Some(index) = chain.iter().position(|&s| s == end) {
                    if partner_reaches(other) {
                        anchors.push(index);
                    }
                }
            }
        }
        // Shot i joins station i to station i + 1: stations up to `min_shot`
        // are in front of the range, those past `max_shot` behind it.
        let front = anchors.iter().any(|&i| i <= min_shot);
        let back = anchors.iter().any(|&i| i > max_shot);
        if front && back {
            tracing::warn!(
                line_id,
                min_shot,
                max_shot,
                anchors = ?anchors,
                purpose = "doubly-anchored guard",
                "Skipping shot updates: modified range is between two anchored stations",
            );
            blocked.insert(line_id);
        }
    }
    blocked
}
