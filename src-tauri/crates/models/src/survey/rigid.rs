//! Rigid groups, anchored lines and immovable stations (plan 3.6, 3.8), the
//! sets the Move tools read.
//!
//! - **Rigid group**: stations joined by shots and aligned closures (snaps and
//!   ties), transitively: what moves together under Move Segment / Move Line
//!   (today's `get_rigid_connected_line_ids`, merges now being junctions).
//! - **Anchored lines**: every line of a rigid group holding a datum, GPS
//!   fixes included (today's `get_anchored_line_ids`); Move Segment refuses
//!   them.
//! - **Immovable stations**: every datum station (fixes included) and every
//!   station tied to one by aligned closures, transitively (today's
//!   frontend `computeImmovableStationKeys` over `merged_with` and aligned
//!   `closures_with`); Move Station refuses them.
//! - **Merge pairs** for the wire (plan 9.2): each junction's first slot
//!   paired with its other slots.
//! - **Co-located stations** of one station: its tie group
//!   ([`super::TieGroups`]); a Move Station moves them together.
//!
//! The Move tools (`ops_move.rs`) and the Merge refusal (`ops_merge.rs`)
//! read these; nothing else computes them.

use std::collections::BTreeSet;

use crate::{LineId, StationId, StationRef};

use super::closures::find;
use super::{Project, TieGroups};

/// The rigid groups of one state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigidGroups {
    stations: Vec<StationId>,
    group_of: Vec<usize>,
    /// Per group: its lines, ascending.
    lines: Vec<Vec<LineId>>,
    /// Per group: whether it holds a datum (placement or fix).
    has_datum: Vec<bool>,
}

impl RigidGroups {
    /// The group index of `station`.
    pub fn group_of(&self, station: StationId) -> Option<usize> {
        self.stations
            .binary_search(&station)
            .ok()
            .map(|d| self.group_of[d])
    }

    /// The lines of group `group`, ascending.
    pub fn lines(&self, group: usize) -> &[LineId] {
        self.lines.get(group).map_or(&[], Vec::as_slice)
    }

    /// Whether group `group` holds a datum.
    pub fn has_datum(&self, group: usize) -> bool {
        self.has_datum.get(group).copied().unwrap_or(false)
    }

    /// Whether `a` and `b` are in one rigid group (both on a line).
    pub fn tied(&self, a: StationId, b: StationId) -> bool {
        match (self.group_of(a), self.group_of(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }

    /// Number of groups.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether there is no group.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

impl Project {
    /// The rigid groups for the alignment `aligned` (parallel to `closures`).
    pub fn rigid_groups(&self, aligned: &[bool]) -> RigidGroups {
        let topology = self.topology();
        let n = topology.stations().len();
        let mut parent: Vec<usize> = (0..n).collect();
        let unite = |a: usize, b: usize, parent: &mut Vec<usize>| {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                parent[ra.max(rb)] = ra.min(rb);
            }
        };
        self.for_each_shot_edge(|edge| unite(edge.from, edge.to, &mut parent));
        for (closure, &is_aligned) in self.closures.iter().zip(aligned) {
            if is_aligned {
                if let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) {
                    unite(a, b, &mut parent);
                }
            }
        }
        let mut index_of_root = vec![usize::MAX; n];
        let mut group_of = vec![0; n];
        let mut count = 0;
        for (d, slot) in group_of.iter_mut().enumerate() {
            let root = find(&mut parent, d);
            if index_of_root[root] == usize::MAX {
                index_of_root[root] = count;
                count += 1;
            }
            *slot = index_of_root[root];
        }
        let mut lines: Vec<BTreeSet<LineId>> = vec![BTreeSet::new(); count];
        for line in self.lines() {
            if let Some(d) = topology.dense(line.start) {
                lines[group_of[d]].insert(line.id);
            }
        }
        let mut has_datum = vec![false; count];
        for datum in &self.datums {
            if let Some(d) = topology.dense(datum.station) {
                has_datum[group_of[d]] = true;
            }
        }
        RigidGroups {
            stations: topology.stations().to_vec(),
            group_of,
            lines: lines.into_iter().map(|s| s.into_iter().collect()).collect(),
            has_datum,
        }
    }

    /// The lines rigidly connected to `line` (itself included), ascending.
    pub fn rigid_lines_of(&self, line: LineId, groups: &RigidGroups) -> Vec<LineId> {
        self.line(line)
            .and_then(|l| groups.group_of(l.start))
            .map(|g| groups.lines(g).to_vec())
            .unwrap_or_default()
    }

    /// Every line of a rigid group that holds a datum, ascending.
    pub fn anchored_lines(&self, groups: &RigidGroups) -> Vec<LineId> {
        let mut out: BTreeSet<LineId> = BTreeSet::new();
        for g in 0..groups.len() {
            if groups.has_datum(g) {
                out.extend(groups.lines(g).iter().copied());
            }
        }
        out.into_iter().collect()
    }

    /// The wire's derived merge pairs (plan 9.2): for each junction, ascending
    /// by station, its first slot paired with each of its other slots.
    pub fn merge_pairs(&self) -> Vec<(StationRef, StationRef)> {
        let topology = self.topology();
        let mut out = Vec::new();
        for &station in topology.stations() {
            let slots = topology.slots_of(station);
            if let Some((&first, rest)) = slots.split_first() {
                out.extend(rest.iter().map(|&other| (first, other)));
            }
        }
        out
    }

    /// Every station the Move Station tool refuses: datum stations and the
    /// stations tied to them, ascending.
    pub fn immovable_stations(&self, ties: &TieGroups) -> Vec<StationId> {
        let mut out: BTreeSet<StationId> = BTreeSet::new();
        for datum in &self.datums {
            out.extend(ties.group(datum.station).iter().copied());
        }
        out.into_iter().collect()
    }
}
