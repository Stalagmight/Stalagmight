//! Placements and the move tools (plan 4.4), plus the derived sets the
//! frontend's move tools read: rigid groups, anchored lines, co-located
//! stations and immovable stations.
//!
//! - **Rigid group** of a line: its piece, plus every piece tied to it by an
//!   aligned closure (a `Tie`, or a snap within 1 cm that does not
//!   double-anchor), transitively. A junction is one station, so merges need
//!   no walk any more.
//! - **Move Segment / Move Line** translates the piece's `Placement` (every
//!   placement in the rigid group); refused, for a non-zero offset, when the
//!   rigid group holds any datum, placement or fix (as today).
//! - **Move Station** rewrites every shot incident to the station, on every
//!   line through it, so the station lands at its position plus the offset
//!   and every neighbour stays: the shot into it gets `v + hand + offset`, the
//!   shot out of it `v - hand - offset`, where `hand` is that line's legacy
//!   hand move at the station, which is folded in and deleted (as today). The
//!   depth readings at the station change by `hand z + offset z`. When the
//!   station is its free piece's placement station, the placement moves with
//!   it (today's station-0 case). Refused on a station holding a datum, and
//!   when a rewritten shot would have no length.
//! - **Co-located stations**: a station and every station tied to it by
//!   aligned closures, transitively (the drag preview's set; one Move Station
//!   per member).
//! - **Immovable stations**: the co-located sets of every datum station.
//! - **Anchored lines**: the rigid groups of every datum's piece.

use std::collections::BTreeSet;

use crate::{average_azimuth_of, normalize_azimuth, LineId, StationId};

use super::ops::{OpError, OpOutcome, Sweep};
use super::ops_merge::find;
use super::{EditContext, Project};

/// An offset smaller than this on every axis is no move (today's guard).
const NO_MOVE_M: f64 = 1e-9;

/// A shot bearing below this horizontal length keeps its old azimuth.
const MIN_BEARING_LENGTH_M: f64 = 1e-9;

/// What Move Segment did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentMoved {
    /// Every line of the rigid group, ascending.
    pub lines: Vec<LineId>,
    /// The placement stations that were translated.
    pub placements: Vec<StationId>,
}

/// What Move Station did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationMoved {
    pub station: StationId,
    /// The shots rewritten, as `(line, shot index)`, in slot order.
    pub rewritten: Vec<(LineId, usize)>,
    /// Whether the station's piece placement moved with it.
    pub placement_moved: bool,
    /// Lines whose hand move at the station was folded in and deleted.
    pub hand_moves_folded: Vec<LineId>,
}

/// One planned shot rewrite.
#[derive(Debug, Clone, Copy)]
struct Rewrite {
    line: LineId,
    index: usize,
    length: f64,
    azimuth: f64,
    pitch: f64,
    /// The new `depth_out` (shot into the station) or `depth_in` (out of it).
    depth: f64,
    into_station: bool,
}

impl Project {
    /// Translate the piece of `line` (its rigid group) by `offset` (plan
    /// 4.4; the z component only counts for the zero-move test, as today).
    pub fn move_segment(
        &mut self,
        line: LineId,
        offset: [f64; 3],
    ) -> Result<OpOutcome<SegmentMoved>, OpError> {
        if offset.iter().any(|v| !v.is_finite()) {
            return Err(OpError::NonFiniteOffset);
        }
        let start = self.line(line).ok_or(OpError::UnknownLine(line))?.start;
        let components = self.rigid_group_components(start);
        let topology = self.topology_arc();
        let held = self.placement_datums();
        let moves = offset.iter().any(|v| v.abs() > NO_MOVE_M);
        if moves && components.iter().any(|&c| held[c].is_some()) {
            return Err(OpError::SegmentHoldsDatum);
        }
        let mut lines: Vec<LineId> = components
            .iter()
            .filter_map(|&c| topology.component(c))
            .flat_map(|component| component.lines.iter().copied())
            .collect();
        lines.sort_unstable();
        self.run_op(Sweep::Skip, |p| {
            let mut ctx = EditContext::none();
            let mut placements = Vec::new();
            for placement in &mut p.placements {
                let in_group = topology
                    .component_of(placement.station)
                    .is_some_and(|c| components.contains(&c));
                if in_group {
                    placement.x += offset[0];
                    placement.y += offset[1];
                    placements.push(placement.station);
                    ctx = ctx.with_placed(placement.station);
                }
            }
            Ok((SegmentMoved { lines, placements }, ctx))
        })
    }

    /// Move station `station` by `offset` (x, y east/north metres, z depth
    /// metres, +down) by rewriting its incident shots (see the module docs).
    pub fn move_station(
        &mut self,
        station: StationId,
        offset: [f64; 3],
    ) -> Result<OpOutcome<StationMoved>, OpError> {
        if offset.iter().any(|v| !v.is_finite()) {
            return Err(OpError::NonFiniteOffset);
        }
        self.existing_station(station)?;
        let moves = offset.iter().any(|v| v.abs() > NO_MOVE_M);
        if moves && self.datums.iter().any(|d| d.station == station) {
            return Err(OpError::StationHoldsDatum);
        }
        let rewrites = self.plan_station_move(station, offset)?;
        let placement_moved = self.placements.iter().any(|p| p.station == station);
        self.run_op(Sweep::Skip, |p| {
            for rewrite in &rewrites {
                let Some(shot) = p
                    .line_mut(rewrite.line)
                    .and_then(|l| l.shots.get_mut(rewrite.index))
                else {
                    continue;
                };
                shot.length = rewrite.length;
                shot.azimuth_in = rewrite.azimuth;
                shot.azimuth_out = rewrite.azimuth;
                shot.pitch_in = Some(rewrite.pitch);
                shot.pitch_out = Some(rewrite.pitch);
                if rewrite.into_station {
                    shot.depth_out = rewrite.depth;
                } else {
                    shot.depth_in = rewrite.depth;
                }
            }
            let mut hand_moves_folded = Vec::new();
            for line in p.topology_arc().lines_of(station) {
                if let Some(l) = p.line_mut(line) {
                    if l.hand_moves.remove(&station).is_some() {
                        hand_moves_folded.push(line);
                    }
                }
            }
            let mut ctx = EditContext::none();
            if placement_moved {
                for placement in &mut p.placements {
                    if placement.station == station {
                        placement.x += offset[0];
                        placement.y += offset[1];
                    }
                }
                ctx = ctx.with_placed(station);
            }
            let rewritten = rewrites.iter().map(|r| (r.line, r.index)).collect();
            Ok((
                StationMoved {
                    station,
                    rewritten,
                    placement_moved,
                    hand_moves_folded,
                },
                ctx,
            ))
        })
    }

    /// The shot rewrites of a Move Station, or the refusal.
    fn plan_station_move(
        &self,
        station: StationId,
        offset: [f64; 3],
    ) -> Result<Vec<Rewrite>, OpError> {
        let mut rewrites = Vec::new();
        for slot in self.slots_of(station) {
            let Some(line) = self.line(slot.line_id) else {
                continue;
            };
            let reference = self.azimuth_reference_for(line);
            let hand = line.hand(station);
            let total = [
                hand[0] + offset[0],
                hand[1] + offset[1],
                hand[2] + offset[2],
            ];
            let index = slot.station_index as usize;
            if index > 0 {
                if let Some(shot) = line.shots.get(index - 1) {
                    let v = shot.vector(reference);
                    let depth_out = shot.depth_out + total[2];
                    let (length, azimuth, pitch) = rewritten(
                        [v[0] + total[0], v[1] + total[1]],
                        depth_out - shot.depth_in,
                        reference,
                        average_azimuth_of(shot.azimuth_in, shot.azimuth_out),
                    )?;
                    rewrites.push(Rewrite {
                        line: line.id,
                        index: index - 1,
                        length,
                        azimuth,
                        pitch,
                        depth: depth_out,
                        into_station: true,
                    });
                }
            }
            if let Some(shot) = line.shots.get(index) {
                let v = shot.vector(reference);
                let depth_in = shot.depth_in + total[2];
                let (length, azimuth, pitch) = rewritten(
                    [v[0] - total[0], v[1] - total[1]],
                    shot.depth_out - depth_in,
                    reference,
                    average_azimuth_of(shot.azimuth_in, shot.azimuth_out),
                )?;
                rewrites.push(Rewrite {
                    line: line.id,
                    index,
                    length,
                    azimuth,
                    pitch,
                    depth: depth_in,
                    into_station: false,
                });
            }
        }
        Ok(rewrites)
    }

    /// The component indices of `station`'s rigid group (its piece plus the
    /// pieces tied to it by aligned closures), ascending.
    pub fn rigid_group_components(&self, station: StationId) -> Vec<usize> {
        let roots = self.rigid_roots();
        let Some(own) = self.topology().component_of(station) else {
            return Vec::new();
        };
        (0..roots.len())
            .filter(|&c| roots[c] == roots[own])
            .collect()
    }

    /// The lines of `line`'s rigid group, ascending (today's
    /// `get_rigid_connected_line_ids`).
    pub fn rigid_group_lines(&self, line: LineId) -> Vec<LineId> {
        let Some(start) = self.line(line).map(|l| l.start) else {
            return Vec::new();
        };
        let topology = self.topology();
        let mut lines: Vec<LineId> = self
            .rigid_group_components(start)
            .into_iter()
            .filter_map(|c| topology.component(c))
            .flat_map(|component| component.lines.iter().copied())
            .collect();
        lines.sort_unstable();
        lines
    }

    /// Whether `line`'s rigid group holds a datum (placement or fix).
    pub fn rigid_group_holds_datum(&self, line: LineId) -> bool {
        let Some(start) = self.line(line).map(|l| l.start) else {
            return false;
        };
        let held = self.placement_datums();
        self.rigid_group_components(start)
            .into_iter()
            .any(|c| held[c].is_some())
    }

    /// Whether `a` and `b` are in one rigid group.
    pub fn rigidly_tied(&self, a: StationId, b: StationId) -> bool {
        let topology = self.topology();
        let (Some(ca), Some(cb)) = (topology.component_of(a), topology.component_of(b)) else {
            return false;
        };
        let roots = self.rigid_roots();
        roots[ca] == roots[cb]
    }

    /// Every line of the rigid group of every datum's piece, ascending
    /// (today's `get_anchored_line_ids`): the lines Move Segment refuses.
    pub fn anchored_lines(&self) -> Vec<LineId> {
        let topology = self.topology();
        let roots = self.rigid_roots();
        let held_roots: BTreeSet<usize> = self
            .placement_datums()
            .iter()
            .enumerate()
            .filter(|(_, datum)| datum.is_some())
            .map(|(c, _)| roots[c])
            .collect();
        let mut lines: Vec<LineId> = topology
            .components()
            .iter()
            .enumerate()
            .filter(|(c, _)| held_roots.contains(&roots[*c]))
            .flat_map(|(_, component)| component.lines.iter().copied())
            .collect();
        lines.sort_unstable();
        lines
    }

    /// `station` and every station tied to it by aligned closures,
    /// transitively, ascending: the stations a drag of `station` moves.
    pub fn colocated_stations(&self, station: StationId) -> Vec<StationId> {
        let topology = self.topology();
        let Some(own) = topology.dense(station) else {
            return Vec::new();
        };
        let mut parent = self.aligned_station_roots();
        let root = find(&mut parent, own);
        topology
            .stations()
            .iter()
            .enumerate()
            .filter(|&(d, _)| find(&mut parent, d) == root)
            .map(|(_, &s)| s)
            .collect()
    }

    /// Every station a move tool may not drag: each datum station (placement
    /// or fix) and the stations co-located with it.
    pub fn immovable_stations(&self) -> BTreeSet<StationId> {
        let topology = self.topology();
        let mut parent = self.aligned_station_roots();
        let roots: BTreeSet<usize> = self
            .datums
            .iter()
            .filter_map(|d| topology.dense(d.station))
            .map(|d| find(&mut parent, d))
            .collect();
        topology
            .stations()
            .iter()
            .enumerate()
            .filter(|&(d, _)| roots.contains(&find(&mut parent, d)))
            .map(|(_, &s)| s)
            .collect()
    }

    /// Union-find parents over dense stations joined by aligned closures.
    fn aligned_station_roots(&self) -> Vec<usize> {
        let topology = self.topology();
        let positions = self.raw_positions();
        let mut parent: Vec<usize> = (0..topology.stations().len()).collect();
        for closure in &self.closures {
            if !self.closure_is_aligned(&positions, closure) {
                continue;
            }
            if let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
        parent
    }

    /// For each component, the root of its rigid group (components joined by
    /// aligned closures).
    fn rigid_roots(&self) -> Vec<usize> {
        let topology = self.topology();
        let positions = self.raw_positions();
        let mut parent: Vec<usize> = (0..topology.components().len()).collect();
        for closure in &self.closures {
            if !self.closure_is_aligned(&positions, closure) {
                continue;
            }
            if let (Some(a), Some(b)) = (
                topology.component_of(closure.a),
                topology.component_of(closure.b),
            ) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
        (0..parent.len()).map(|c| find(&mut parent, c)).collect()
    }
}

/// A shot's new `(length, measured azimuth, pitch)` for horizontal vector
/// `xy` and depth change `dz` (today's Move Station formulas): the bearing is
/// solved in true north and stored as a measured reading (#438); a vertical
/// shot keeps `old_azimuth`.
fn rewritten(
    xy: [f64; 2],
    dz: f64,
    reference: crate::AzimuthReference,
    old_azimuth: f64,
) -> Result<(f64, f64, f64), OpError> {
    // Spelled as today's formula (not `hypot`), so the bits agree.
    let horizontal = (xy[0] * xy[0] + xy[1] * xy[1]).sqrt();
    let length = (horizontal * horizontal + dz * dz).sqrt();
    if length <= 0.0 || !length.is_finite() {
        return Err(OpError::ZeroLengthShot);
    }
    let azimuth = if horizontal > MIN_BEARING_LENGTH_M {
        reference.to_measured(normalize_azimuth(xy[0].atan2(xy[1]).to_degrees()))
    } else {
        old_azimuth
    };
    Ok((length, azimuth, dz.atan2(horizontal).to_degrees()))
}
