//! Line topology for the Ariane TML export: which line roots the
//! document, how child lines hang off their parents via station merges,
//! and which zero datum an entrance line stands on. Extracted from `tml.rs`
//! (#157 size policy); the writer itself stays there.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::models::{Line, LineId, Project, StationRef, ZeroDatum};

#[derive(Debug, Clone, Copy)]
pub(super) struct ParentLink {
    pub(super) parent_station: StationRef,
    pub(super) child_station_index: u32,
}

/// Pick a deterministic root using the legacy "lowest line ID wins parent"
/// heuristic so exports without zero datums remain stable.
pub(super) fn choose_default_root_line_id(project: &Project) -> LineId {
    let mut child_lines: HashSet<LineId> = HashSet::new();
    for merge in &project.station_merges {
        let child = if merge.station_a.line_id <= merge.station_b.line_id {
            merge.station_b.line_id
        } else {
            merge.station_a.line_id
        };
        child_lines.insert(child);
    }

    project
        .lines
        .iter()
        .map(|s| s.id)
        .find(|id| !child_lines.contains(id))
        .unwrap_or(project.lines[0].id)
}

pub(super) fn station_depth(line: &Line, station_index: usize) -> f64 {
    if line.shots.is_empty() {
        return 0.0;
    }
    if station_index == 0 {
        line.shots[0].depth_in
    } else if station_index >= line.shots.len() {
        line.shots[line.shots.len() - 1].depth_out
    } else {
        line.shots[station_index].depth_in
    }
}

/// The datum each line's entrance START stands on when that line is an
/// entrance: the line's first PLACEMENT datum (mirrors `Project::frame_datum`'s
/// first-that-resolves rule, scoped to one line). A GPS fix never roots an
/// entrance (#741): its merge component already holds a placement datum, which
/// does, and the fix gets a START of its own (`tml_datums`).
pub(super) fn entrance_datums(project: &Project) -> HashMap<LineId, &ZeroDatum> {
    let mut by_line: HashMap<LineId, &ZeroDatum> = HashMap::new();
    for datum in project.placement_datums() {
        if let Some(sr) = project.resolve_key(&datum.station_ref) {
            by_line.entry(sr.line_id).or_insert(datum);
        }
    }
    by_line
}

/// Build a parent map from station_merges.
///
/// Returns: child_line_id → (parent_station_ref, child_station_index)
///
/// Parent links are chosen by traversing the merge graph from `root_line_id`.
pub(super) fn build_parent_map(
    project: &Project,
    root_line_id: LineId,
    entrance_datums: &HashMap<LineId, &ZeroDatum>,
) -> HashMap<LineId, ParentLink> {
    #[derive(Debug, Clone, Copy)]
    struct NeighborLink {
        neighbor_line_id: LineId,
        parent_station: StationRef,
        child_station_index: u32,
    }

    let mut adjacency: HashMap<LineId, Vec<NeighborLink>> = HashMap::new();
    for merge in &project.station_merges {
        let (Some(a), Some(b)) = (
            project.resolve_key(&merge.station_a),
            project.resolve_key(&merge.station_b),
        ) else {
            continue;
        };
        adjacency.entry(a.line_id).or_default().push(NeighborLink {
            neighbor_line_id: b.line_id,
            parent_station: a,
            child_station_index: b.station_index,
        });
        adjacency.entry(b.line_id).or_default().push(NeighborLink {
            neighbor_line_id: a.line_id,
            parent_station: b,
            child_station_index: a.station_index,
        });
    }

    for links in adjacency.values_mut() {
        links.sort_by_key(|l| {
            (
                l.neighbor_line_id,
                l.child_station_index,
                l.parent_station.station_index,
            )
        });
    }

    let mut map: HashMap<LineId, ParentLink> = HashMap::new();
    let mut visited: HashSet<LineId> = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(root_line_id);
    queue.push_back(root_line_id);

    // Traverse from the export root, then from one line per REMAINING merge-connected
    // component. Seeding only from `root_line_id` leaves every line outside that
    // component absent from `map`, and the caller reads absence as "this is an entrance"
    // - so a gallery of lines merged to each other exported as that many disconnected
    // STARTs stacked on one anchor, with the merges between them never applied. One root
    // per component instead: that component's first line is the entrance, and the rest
    // resolve their parent links normally.
    loop {
        while let Some(parent_line_id) = queue.pop_front() {
            let Some(links) = adjacency.get(&parent_line_id) else {
                continue;
            };
            for link in links {
                if visited.insert(link.neighbor_line_id) {
                    map.insert(
                        link.neighbor_line_id,
                        ParentLink {
                            parent_station: link.parent_station,
                            child_station_index: link.child_station_index,
                        },
                    );
                    queue.push_back(link.neighbor_line_id);
                }
            }
        }

        // A component's entrance prefers a line holding a placement datum -
        // the same rule the MAIN root follows via `frame_datum` (#378 review):
        // GPS lands only on entrance STARTs, so rooting a datum-carrying
        // component at some other line would export it anchored to the frame
        // datum's coordinates while its own datum went unwritten. Within each
        // preference tier, `project.lines` order keeps the choice
        // deterministic and the export byte-stable. A line with no merges at
        // all is not in `adjacency` and is left out of the map deliberately: it
        // is an entrance in its own right.
        let candidate = |id: &LineId| !visited.contains(id) && adjacency.contains_key(id);
        let Some(next_root) = project
            .lines
            .iter()
            .map(|line| line.id)
            .filter(candidate)
            .find(|id| entrance_datums.contains_key(id))
            .or_else(|| project.lines.iter().map(|line| line.id).find(candidate))
        else {
            break;
        };

        tracing::debug!(
            line_id = next_root,
            root_line_id,
            "Rooting a merge component that is disconnected from the export root"
        );
        visited.insert(next_root);
        queue.push_back(next_root);
    }

    map
}
