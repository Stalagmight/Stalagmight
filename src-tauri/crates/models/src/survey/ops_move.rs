//! Placements and the move tools (plan 4.4).
//!
//! - **Move Segment / Move Line** translates the piece's `Placement` (every
//!   placement in the rigid group, [`Project::rigid_groups`]); refused, for a
//!   non-zero offset, when the rigid group holds any datum, placement or fix
//!   (as today).
//! - **Move Station** rewrites every shot incident to the station, on every
//!   line through it, so the station lands at its position plus the offset
//!   and every neighbour stays: the shot into it gets `v + hand + offset`, the
//!   shot out of it `v - hand - offset`, where `hand` is that line's legacy
//!   hand move at the station, which is folded in and deleted (as today). The
//!   depth readings at the station change by `hand z + offset z`. When the
//!   station is its free piece's placement station, the placement moves with
//!   it (today's station-0 case). Refused on a station holding a datum, and
//!   when a rewritten shot would have no length.
//!
//! The sets the frontend's move tools read (rigid groups, anchored lines,
//! co-located and immovable stations) are derived views (`rigid.rs`,
//! `closures.rs`).

use crate::{average_azimuth_of, normalize_azimuth, LineId, StationId};

use super::ops::{OpError, OpOutcome, Sweep};
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
        let groups = self.rigid_groups(&self.closure_alignment(&self.raw_positions()));
        let group = groups.group_of(start);
        let moves = offset.iter().any(|v| v.abs() > NO_MOVE_M);
        if moves && group.is_some_and(|g| groups.has_datum(g)) {
            return Err(OpError::SegmentHoldsDatum);
        }
        let lines = group.map_or_else(Vec::new, |g| groups.lines(g).to_vec());
        self.run_op(Sweep::Skip, |p| {
            let mut ctx = EditContext::none();
            let mut placements = Vec::new();
            for placement in &mut p.placements {
                let in_group = group.is_some() && groups.group_of(placement.station) == group;
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

    /// Move Line ("Adjust Position" on a line): today it moves the line's
    /// whole rigid group, exactly as [`Self::move_segment`].
    pub fn move_line(
        &mut self,
        line: LineId,
        offset: [f64; 3],
    ) -> Result<OpOutcome<SegmentMoved>, OpError> {
        self.move_segment(line, offset)
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
