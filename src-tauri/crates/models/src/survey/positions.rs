//! Solver v1, raw (measured) positions (plan 3.2-3.5).
//!
//! - **Frame**: the first resolvable datum in list order; local (x, y) is ENU
//!   on the tangent plane at its GPS ([`crate::gps_to_local`]).
//! - **Anchor** of each component: its first datum in list order (the
//!   placement datum) at `gps_to_local(gps, frame) + correction` (the frame
//!   datum at `(0, 0) + correction`), else its [`Placement`].
//! - **Traversal**: a BFS from the anchor station; each station's lines in
//!   line-id order (not display order, so reordering lines never changes a
//!   bit); a line is walked once, outwards from the station it was reached
//!   at, by today's shot vector ([`Shot::vector`]). Hand moves are applied
//!   per line: `chain = shown - hand`.
//! - **Depth**: absolute, never walked: the mean of the depth readings at the
//!   station over every line through it, plus those of every station tied to
//!   it transitively by `Tie` closures and aligned snaps, summed in sorted
//!   slot order (#469).
//!
//! Everything iterates in sorted or list order; no hash order reaches a
//! result. Total on invalid models (a component without an anchor is laid out
//! from its lowest station at the origin and reported).
//!
//! [`Placement`]: super::Placement
//! [`Shot::vector`]: super::Shot::vector

use std::collections::VecDeque;

use crate::{gps_to_local, StationId, StationRef};

use super::{ClosureStrategy, LoopClosure, Project, Topology};

/// A snap (`Move` closure) is rigid while its ends are closer than this,
/// horizontally (today's `MERGE_ALIGNMENT_INVARIANT_M`).
pub const ALIGNMENT_TOLERANCE_M: f64 = 0.01;

/// A datum correction smaller than this (metres, magnitude) is stored as
/// `None` (plan 4.3).
pub const CORRECTION_EPSILON_M: f64 = 1e-9;

/// What anchors one component.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    /// Its placement datum (the first datum of the component in list order).
    Datum {
        datum: usize,
        station: StationId,
        at: [f64; 2],
    },
    /// Its placement (a datum-less component).
    Placement {
        placement: usize,
        station: StationId,
        at: [f64; 2],
    },
    /// Neither (an invalid model): laid out from its lowest station at the
    /// origin.
    Unanchored { station: StationId },
}

impl Anchor {
    /// The station the component is laid out from.
    pub fn station(&self) -> StationId {
        match *self {
            Self::Datum { station, .. }
            | Self::Placement { station, .. }
            | Self::Unanchored { station } => station,
        }
    }

    /// Where that station sits.
    pub fn at(&self) -> [f64; 2] {
        match *self {
            Self::Datum { at, .. } | Self::Placement { at, .. } => at,
            Self::Unanchored { .. } => [0.0, 0.0],
        }
    }

    /// The datum index, for a datum-anchored component.
    pub fn datum(&self) -> Option<usize> {
        match *self {
            Self::Datum { datum, .. } => Some(datum),
            _ => None,
        }
    }
}

/// Something the solver had to work around (only on invalid models).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionIssue {
    /// A component with no datum and no placement.
    UnanchoredComponent { lowest_station: StationId },
}

/// One slot's wire position: XY of its station, Z of the station plus this
/// line's hand-move z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotPosition {
    pub slot: StationRef,
    pub station: StationId,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Raw positions of every station of one state of a project.
///
/// Only meaningful for the project state it was computed from: the per-slot
/// accessors read that project's hand moves.
#[derive(Debug, Clone, PartialEq)]
pub struct RawPositions {
    frame: Option<usize>,
    anchors: Vec<Anchor>,
    stations: Vec<StationId>,
    xy: Vec<[f64; 2]>,
    z: Vec<f64>,
    issues: Vec<PositionIssue>,
}

impl RawPositions {
    /// The frame datum's index.
    pub fn frame(&self) -> Option<usize> {
        self.frame
    }

    /// The anchor of each component (indexed like [`Topology::components`]).
    pub fn anchors(&self) -> &[Anchor] {
        &self.anchors
    }

    /// What the solver had to work around.
    pub fn issues(&self) -> &[PositionIssue] {
        &self.issues
    }

    /// A station's horizontal position.
    pub fn xy(&self, station: StationId) -> Option<[f64; 2]> {
        self.index(station).map(|d| self.xy[d])
    }

    /// A station's depth (without any line's hand-move z).
    pub fn z(&self, station: StationId) -> Option<f64> {
        self.index(station).map(|d| self.z[d])
    }

    /// Every positioned station with its `[x, y, z]`, ascending by id.
    pub fn iter(&self) -> impl Iterator<Item = (StationId, [f64; 3])> + '_ {
        self.stations
            .iter()
            .zip(self.xy.iter().zip(&self.z))
            .map(|(&s, (xy, &z))| (s, [xy[0], xy[1], z]))
    }

    /// A slot's position: its station's XY, and its station's Z plus the
    /// line's hand-move z.
    pub fn slot(&self, project: &Project, slot: StationRef) -> Option<[f64; 3]> {
        let station = project.station_at(slot)?;
        let [x, y] = self.xy(station)?;
        let z = self.z(station)?;
        let hand_z = project
            .line(slot.line_id)
            .and_then(|line| line.hand_moves.get(&station))
            .map(|hand| hand[2]);
        Some([x, y, hand_z.map_or(z, |h| z + h)])
    }

    /// Every slot's position, in slot order (lines in list order, then chain
    /// index): the wire's per-slot list (plan 3.5).
    pub fn slots(&self, project: &Project) -> Vec<SlotPosition> {
        let mut out = Vec::new();
        for line in project.lines() {
            for (index, station) in line.stations().enumerate() {
                let (Some([x, y]), Some(z)) = (self.xy(station), self.z(station)) else {
                    continue;
                };
                let z = line.hand_moves.get(&station).map_or(z, |hand| z + hand[2]);
                out.push(SlotPosition {
                    slot: StationRef::new(line.id, index as u32),
                    station,
                    x,
                    y,
                    z,
                });
            }
        }
        out
    }

    fn index(&self, station: StationId) -> Option<usize> {
        self.stations.binary_search(&station).ok()
    }
}

/// Horizontal layout state shared by the traversal.
pub(super) struct Layout {
    pub(super) xy: Vec<[f64; 2]>,
    pub(super) placed: Vec<bool>,
    walked: Vec<bool>,
}

impl Layout {
    pub(super) fn new(topology: &Topology, line_count: usize) -> Self {
        let n = topology.stations().len();
        Self {
            xy: vec![[0.0, 0.0]; n],
            placed: vec![false; n],
            walked: vec![false; line_count],
        }
    }
}

impl Project {
    /// The frame datum: the first datum, in list order, whose station is on a
    /// line.
    pub fn frame_datum(&self) -> Option<usize> {
        let topology = self.topology();
        self.datums
            .iter()
            .position(|datum| topology.contains(datum.station))
    }

    /// The placement datum of each component (its first datum in list order),
    /// indexed like [`Topology::components`].
    pub fn placement_datums(&self) -> Vec<Option<usize>> {
        let topology = self.topology();
        let mut out = vec![None; topology.components().len()];
        for (index, datum) in self.datums.iter().enumerate() {
            if let Some(c) = topology.component_of(datum.station) {
                out[c].get_or_insert(index);
            }
        }
        out
    }

    /// Where placement datum `datum` puts its station: `gps_to_local(gps,
    /// frame) + correction`, the frame datum at `(0, 0) + correction`. `None`
    /// when there is no frame or no such datum.
    pub fn datum_anchor_position(&self, datum: usize) -> Option<[f64; 2]> {
        let frame = self.frame_datum()?;
        let d = self.datums.get(datum)?;
        let base = if datum == frame {
            [0.0, 0.0]
        } else {
            let (x, y) = gps_to_local(&d.gps, &self.datums[frame].gps);
            [x, y]
        };
        Some(match d.correction {
            Some([cx, cy]) => [base[0] + cx, base[1] + cy],
            None => base,
        })
    }

    /// The anchor of every component (plan 3.3), indexed like
    /// [`Topology::components`].
    pub fn component_anchors(&self) -> Vec<Anchor> {
        let topology = self.topology();
        let placement_datums = self.placement_datums();
        let mut placements: Vec<Option<usize>> = vec![None; topology.components().len()];
        for (index, placement) in self.placements.iter().enumerate() {
            if let Some(c) = topology.component_of(placement.station) {
                placements[c].get_or_insert(index);
            }
        }
        topology
            .components()
            .iter()
            .enumerate()
            .map(|(c, component)| {
                if let Some(datum) = placement_datums[c] {
                    let station = self.datums[datum].station;
                    let at = self.datum_anchor_position(datum).unwrap_or([0.0, 0.0]);
                    Anchor::Datum { datum, station, at }
                } else if let Some(placement) = placements[c] {
                    let p = self.placements[placement];
                    Anchor::Placement {
                        placement,
                        station: p.station,
                        at: [p.x, p.y],
                    }
                } else {
                    Anchor::Unanchored {
                        station: component.lowest_station().unwrap_or_default(),
                    }
                }
            })
            .collect()
    }

    /// Raw positions and depths of every station (plan 3.3-3.5).
    pub fn raw_positions(&self) -> RawPositions {
        let topology = self.topology();
        let anchors = self.component_anchors();
        let mut layout = Layout::new(topology, self.lines.len());
        let mut issues = Vec::new();
        for anchor in &anchors {
            if let Anchor::Unanchored { station } = anchor {
                issues.push(PositionIssue::UnanchoredComponent {
                    lowest_station: *station,
                });
            }
            self.walk_component(topology, anchor.station(), anchor.at(), &mut layout);
        }
        let z = self.depths(topology, &layout.xy, &anchors);
        RawPositions {
            frame: self.frame_datum(),
            anchors,
            stations: topology.stations().to_vec(),
            xy: layout.xy,
            z,
            issues,
        }
    }

    /// Whether a closure is a rigid tie in this state (plan 3.6): `Tie`
    /// always; `Move` while its ends are within [`ALIGNMENT_TOLERANCE_M`]
    /// horizontally and the tie would not join two datum-held components;
    /// anything else never.
    pub fn closure_is_aligned(&self, positions: &RawPositions, closure: &LoopClosure) -> bool {
        let topology = self.topology();
        let held: Vec<bool> = positions
            .anchors()
            .iter()
            .map(|anchor| anchor.datum().is_some())
            .collect();
        is_aligned(topology, &held, closure, |s| positions.xy(s))
    }

    /// Lay out the component holding `start` with `start` at `at`, walking
    /// every line once. Stations already placed are left alone.
    pub(super) fn walk_component(
        &self,
        topology: &Topology,
        start: StationId,
        at: [f64; 2],
        layout: &mut Layout,
    ) {
        let Some(d0) = topology.dense(start) else {
            return;
        };
        if layout.placed[d0] {
            return;
        }
        layout.xy[d0] = at;
        layout.placed[d0] = true;
        let mut queue = VecDeque::from([start]);
        while let Some(station) = queue.pop_front() {
            let Some(d) = topology.dense(station) else {
                continue;
            };
            let p = layout.xy[d];
            // Lines in id order, not display order: reordering the line list
            // must not change a single bit of a position.
            let mut slots = topology.slots_of(station).to_vec();
            if slots.len() > 1 {
                slots.sort_unstable();
            }
            for slot in slots {
                let Some(position) = topology.line_position(slot.line_id) else {
                    continue;
                };
                if layout.walked[position] {
                    continue;
                }
                layout.walked[position] = true;
                self.walk_line(
                    topology,
                    position,
                    slot.station_index as usize,
                    p,
                    layout,
                    &mut queue,
                );
            }
        }
    }

    /// Walk the line at list position `position` outwards from chain index
    /// `index`, whose station is shown at `p`.
    fn walk_line(
        &self,
        topology: &Topology,
        position: usize,
        index: usize,
        p: [f64; 2],
        layout: &mut Layout,
        queue: &mut VecDeque<StationId>,
    ) {
        let (Some(line), Some(chain)) = (self.lines.get(position), topology.chain_at(position))
        else {
            return;
        };
        let Some(&from) = chain.get(index) else {
            return;
        };
        let reference = self.azimuth_reference_for(line);
        let origin = match line.hand_moves.get(&from) {
            Some(hand) => [p[0] - hand[0], p[1] - hand[1]],
            None => p,
        };
        let mut place = |k: usize, chain_xy: [f64; 2]| {
            let station = chain[k];
            let shown = match line.hand_moves.get(&station) {
                Some(hand) => [chain_xy[0] + hand[0], chain_xy[1] + hand[1]],
                None => chain_xy,
            };
            if let Some(d) = topology.dense(station) {
                if !layout.placed[d] {
                    layout.xy[d] = shown;
                    layout.placed[d] = true;
                    queue.push_back(station);
                }
            }
        };
        let mut c = origin;
        for k in index + 1..chain.len() {
            let v = line.shots[k - 1].vector(reference);
            c = [c[0] + v[0], c[1] + v[1]];
            place(k, c);
        }
        let mut c = origin;
        for k in (0..index).rev() {
            let v = line.shots[k].vector(reference);
            c = [c[0] - v[0], c[1] - v[1]];
            place(k, c);
        }
    }

    /// Depth of every station (plan 3.4), indexed like `topology.stations()`.
    fn depths(&self, topology: &Topology, xy: &[[f64; 2]], anchors: &[Anchor]) -> Vec<f64> {
        let n = topology.stations().len();
        let held: Vec<bool> = anchors.iter().map(|a| a.datum().is_some()).collect();
        let position_of = |s: StationId| topology.dense(s).map(|d| xy[d]);
        let mut parent: Vec<usize> = (0..n).collect();
        for closure in &self.closures {
            if !is_aligned(topology, &held, closure, position_of) {
                continue;
            }
            if let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
        // Members of each tie group, by root (ascending dense index).
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); n];
        for d in 0..n {
            let root = find(&mut parent, d);
            members[root].push(d);
        }
        let mut z = vec![0.0; n];
        for group in members.iter().filter(|group| !group.is_empty()) {
            let mut slots: Vec<StationRef> = group
                .iter()
                .flat_map(|&d| topology.slots_of(topology.stations()[d]).iter().copied())
                .collect();
            if slots.len() > 1 {
                slots.sort_unstable();
            }
            let depth = self.mean_depth(&slots);
            for &d in group {
                z[d] = depth;
            }
        }
        z
    }

    /// The mean of the depth readings at `slots` (incoming `depth_out`, then
    /// outgoing `depth_in`, per slot), summed in the given order; `0.0` with no
    /// reading. Today's `compute_station_depth`.
    fn mean_depth(&self, slots: &[StationRef]) -> f64 {
        let mut depths: Vec<f64> = Vec::new();
        for slot in slots {
            let Some(line) = self.line(slot.line_id) else {
                continue;
            };
            let index = slot.station_index as usize;
            if index > 0 {
                if let Some(shot) = line.shots.get(index - 1) {
                    depths.push(shot.depth_out);
                }
            }
            if let Some(shot) = line.shots.get(index) {
                depths.push(shot.depth_in);
            }
        }
        if depths.is_empty() {
            0.0
        } else {
            depths.iter().sum::<f64>() / depths.len() as f64
        }
    }
}

/// [`Project::closure_is_aligned`] over precomputed state: `held[c]` says
/// component `c` holds a datum.
fn is_aligned(
    topology: &Topology,
    held: &[bool],
    closure: &LoopClosure,
    position_of: impl Fn(StationId) -> Option<[f64; 2]>,
) -> bool {
    match closure.strategy {
        ClosureStrategy::Tie => true,
        ClosureStrategy::Move => {
            let (Some([ax, ay]), Some([bx, by])) = (position_of(closure.a), position_of(closure.b))
            else {
                return false;
            };
            let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
            // A non-finite gap is unmeasurable, and an unmeasurable snap is torn.
            if gap.is_nan() || gap >= ALIGNMENT_TOLERANCE_M {
                return false;
            }
            match (
                topology.component_of(closure.a),
                topology.component_of(closure.b),
            ) {
                (Some(ca), Some(cb)) if ca != cb => {
                    let both = held.get(ca).copied().unwrap_or(false)
                        && held.get(cb).copied().unwrap_or(false);
                    !both
                }
                (Some(_), Some(_)) => true,
                _ => false,
            }
        }
        ClosureStrategy::Batch
        | ClosureStrategy::LeastSquaresWeighted
        | ClosureStrategy::ProportionalByLength
        | ClosureStrategy::EqualDistribution => false,
    }
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}
