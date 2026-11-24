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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use crate::models::survey::{EditContext, Project, Topology};
use crate::models::{ExistingShotUpdate, LineId, ShotId, StationId};

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
        let index_of: HashMap<ShotId, usize> = line
            .shots
            .iter()
            .enumerate()
            .map(|(index, shot)| (shot.id, index))
            .collect();
        for update in line_updates {
            let Some(shot) = index_of
                .get(&update.shot_id)
                .and_then(|&index| line.shots.get_mut(index))
            else {
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
///
/// Linear in the survey per updated line (I-import-export-2): one
/// breadth-first pass marks the lines that reach a placement datum without
/// passing through the updated line, instead of a walk per partner over an
/// adjacency that held every pair of lines at a junction (`k * (k - 1)` at a
/// hub of `k` lines).
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
    // The other ends of the aligned snaps at each station.
    let mut snapped: HashMap<StationId, Vec<StationId>> = HashMap::new();
    for (closure, &is_aligned) in project.closures.iter().zip(&aligned) {
        if is_aligned {
            snapped.entry(closure.a).or_default().push(closure.b);
            snapped.entry(closure.b).or_default().push(closure.a);
        }
    }
    let graph = LineGraph {
        topology,
        snapped: &snapped,
    };

    let mut blocked = BTreeSet::new();
    for (&line_id, line_updates) in by_line {
        let (Some(line), Some(chain)) = (project.line(line_id), topology.chain(line_id)) else {
            continue;
        };
        let index_of: HashMap<ShotId, usize> = line
            .shots
            .iter()
            .enumerate()
            .map(|(index, shot)| (shot.id, index))
            .collect();
        let modified: Vec<usize> = line_updates
            .iter()
            .filter_map(|u| index_of.get(&u.shot_id).copied())
            .collect();
        let (Some(&min_shot), Some(&max_shot)) = (modified.iter().min(), modified.iter().max())
        else {
            continue;
        };
        let reaching = graph.reaching(&datum_lines, line_id);
        let partner_reaches = |station: StationId| {
            topology
                .slots_of(station)
                .iter()
                .any(|slot| slot.line_id != line_id && reaching.contains(&slot.line_id))
        };
        let mut anchors: Vec<usize> = Vec::new();
        for (index, &station) in chain.iter().enumerate() {
            if placement_stations.contains(&station)
                || (topology.is_junction(station) && partner_reaches(station))
            {
                anchors.push(index);
            }
            for &other in snapped.get(&station).into_iter().flatten() {
                if partner_reaches(other) {
                    anchors.push(index);
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

/// Lines joined through their junctions and the aligned snaps between their
/// stations: two lines are adjacent when they share a station, or when an
/// aligned snap joins a station of one to a station of the other.
struct LineGraph<'a> {
    topology: &'a Topology,
    snapped: &'a HashMap<StationId, Vec<StationId>>,
}

impl LineGraph<'_> {
    /// The lines that reach one of `sources` without passing through
    /// `excluded` (`excluded` itself never counts). A station is expanded
    /// once, by the first line that reaches it, and a station's lines are
    /// queued once, so the pass is linear in the slots and snaps.
    fn reaching(&self, sources: &BTreeSet<LineId>, excluded: LineId) -> HashSet<LineId> {
        let mut reached: HashSet<LineId> = HashSet::new();
        let mut queue: VecDeque<LineId> = VecDeque::new();
        for &line in sources {
            if line != excluded && reached.insert(line) {
                queue.push_back(line);
            }
        }
        let mut expanded: HashSet<StationId> = HashSet::new();
        let mut queued: HashSet<StationId> = HashSet::new();
        let mut queue_lines_of =
            |station: StationId, reached: &mut HashSet<LineId>, queue: &mut VecDeque<LineId>| {
                if queued.insert(station) {
                    for slot in self.topology.slots_of(station) {
                        if slot.line_id != excluded && reached.insert(slot.line_id) {
                            queue.push_back(slot.line_id);
                        }
                    }
                }
            };
        while let Some(line) = queue.pop_front() {
            for &station in self.topology.chain(line).unwrap_or(&[]) {
                if !expanded.insert(station) {
                    continue;
                }
                queue_lines_of(station, &mut reached, &mut queue);
                for &other in self.snapped.get(&station).into_iter().flatten() {
                    queue_lines_of(other, &mut reached, &mut queue);
                }
            }
        }
        reached
    }
}

#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point"
)]
#[path = "import_overlap_tests.rs"]
mod tests;
