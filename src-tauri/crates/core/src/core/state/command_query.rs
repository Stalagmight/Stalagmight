//! Read-only topology queries the Merge, closure, datum and Move tools ask
//! before a command (none of them is a Command): the merge check, the datum
//! preview, rigid groups, anchored lines and the segment geometry, on the
//! station-graph model. The layouts come from the memoised raw positions of
//! the current revision ([`AppState::raw_positions_of`]).

use std::collections::BTreeSet;

use super::AppState;
use crate::models::survey::Project;
use crate::models::{
    ConnectivityResult, GpsCoordinates, LineId, SegmentGeometry, SegmentLineGeometry, StationRef,
    ZeroDatumPreview, LINE_GONE,
};

impl AppState {
    /// What setting a zero datum with `coordinates` on `station_ref` would do
    /// (read-only, NOT a Command): its role, and for a GPS fix the gap between
    /// the GPS and the surveyed position (#741). `Ok(None)` when the station does
    /// not exist; `Err` when no project is open or the GPS is invalid.
    pub fn preview_zero_datum(
        &self,
        station_ref: StationRef,
        coordinates: GpsCoordinates,
    ) -> Result<Option<ZeroDatumPreview>, String> {
        GpsCoordinates::validate(
            coordinates.latitude,
            coordinates.longitude,
            coordinates.depth_reference,
        )?;
        self.with_project(|project| Ok(project.preview_zero_datum_at(station_ref, &coordinates)))
    }

    /// Check merge connectivity between two stations (read-only): connected
    /// pairs go to the closure dialog with its figures (each end's depth on
    /// the picked slot's line) and its across-one-shot refusal (#737);
    /// unconnected ones may merge, with the datum shift when both pieces are
    /// placed (#741). Refused, in 0.6.0's words, for a slot that names no
    /// station, a pair that is one station, or a pair a closure joins.
    pub fn check_merge_connectivity(
        &self,
        station_a: StationRef,
        station_b: StationRef,
    ) -> Result<ConnectivityResult, String> {
        self.with_project(|project| project.check_merge_connectivity_wire(station_a, station_b))
    }

    /// Get all line IDs connected through merges and aligned closures (rigid
    /// groups): the lines of `line_id`'s rigid group, itself included,
    /// ascending.
    pub fn get_rigid_connected_line_ids(&self, line_id: LineId) -> Result<Vec<LineId>, String> {
        // Serialize with command execution to avoid observing intermediate batch state
        let _cmd_guard = self
            .command_lock
            .lock()
            .map_err(|_| "Failed to acquire command lock".to_string())?;
        self.with_project(|project| {
            if project.line(line_id).is_none() {
                return Err(LINE_GONE.to_string());
            }
            let raw = self.raw_positions_of(project);
            let groups = project.rigid_groups(&project.closure_alignment(&raw));
            Ok(project.rigid_lines_of(line_id, &groups))
        })
    }

    /// Get shot-line geometry for all lines in the segment containing the
    /// given line: the lines joined to it by shots, junctions and loop
    /// closures of any kind (0.6.0's `get_connected_line_ids`), ascending by
    /// id. Coordinates are relative to the given line's station 0.
    pub fn get_segment_geometry(&self, line_id: LineId) -> Result<SegmentGeometry, String> {
        // Serialize with command execution to avoid observing intermediate batch state
        let _cmd_guard = self
            .command_lock
            .lock()
            .map_err(|_| "Failed to acquire command lock".to_string())?;
        self.with_project(|project| {
            let Some(line) = project.line(line_id) else {
                return Err(LINE_GONE.to_string());
            };
            let raw = self.raw_positions_of(project);
            let [ax, ay] = raw.xy(line.start).unwrap_or([0.0, 0.0]);
            let mut lines = Vec::new();
            for id in segment_lines(project, line_id) {
                let Some(line) = project.line(id) else {
                    continue;
                };
                let points: Vec<[f64; 2]> = line
                    .stations()
                    .filter_map(|s| raw.xy(s))
                    .map(|[x, y]| [x - ax, y - ay])
                    .collect();
                lines.push(SegmentLineGeometry {
                    line_id: id,
                    color: line.color.clone(),
                    shot_lines: points
                        .iter()
                        .zip(points.iter().skip(1))
                        .map(|(from, to)| [*from, *to])
                        .collect(),
                });
            }
            Ok(SegmentGeometry { lines })
        })
    }

    /// Get all line IDs anchored by a zero datum.
    /// A zero datum anchors every line reachable through merges AND aligned closures,
    /// since unaligned closures are not rigidly constrained yet.
    pub fn get_anchored_line_ids(&self) -> Result<Vec<LineId>, String> {
        // Serialize with command execution to avoid observing intermediate batch state
        let _cmd_guard = self
            .command_lock
            .lock()
            .map_err(|_| "Failed to acquire command lock".to_string())?;
        self.with_project(|project| {
            if project.datums.is_empty() {
                return Ok(Vec::new());
            }
            let raw = self.raw_positions_of(project);
            let groups = project.rigid_groups(&project.closure_alignment(&raw));
            Ok(project.anchored_lines(&groups))
        })
    }
}

/// The lines of `line`'s piece and of every piece a loop closure (of any
/// strategy, aligned or not) joins to it, transitively, ascending by id.
fn segment_lines(project: &Project, line: LineId) -> BTreeSet<LineId> {
    let topology = project.topology();
    let Some(home) = project
        .line(line)
        .and_then(|l| topology.component_of(l.start))
    else {
        return BTreeSet::new();
    };
    let mut reached: BTreeSet<usize> = BTreeSet::from([home]);
    let mut queue = vec![home];
    while let Some(component) = queue.pop() {
        for closure in &project.closures {
            let ends = (
                topology.component_of(closure.a),
                topology.component_of(closure.b),
            );
            let other = match ends {
                (Some(a), Some(b)) if a == component => b,
                (Some(a), Some(b)) if b == component => a,
                _ => continue,
            };
            if reached.insert(other) {
                queue.push(other);
            }
        }
    }
    reached
        .into_iter()
        .filter_map(|c| topology.component(c))
        .flat_map(|c| c.lines.iter().copied())
        .collect()
}
