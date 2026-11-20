//! The Closures Computed View (plan 3.7): today's least-squares adjustment on
//! the station graph.
//!
//! Ported from `models/src/project/lsq.rs` (`adjust_least_squares`,
//! `gps_fix_observations`), `closure.rs` (`adjust_loop_closures_skipping`) and
//! `coordinates.rs` (`compute_adjusted_stations`, `plan_adjustment_anchors`):
//!
//! - **Solve components**: the shot components joined by any closure (today's
//!   `get_connected_line_ids` follows every closure). A component is solved
//!   when it holds an eligible closure the across-one-shot rule does not skip,
//!   or a GPS fix; otherwise it keeps its raw layout.
//! - **Unknowns**: one per constraint group, a group being the stations tied
//!   by the closures the solve ties (eligible ones and aligned snaps, minus
//!   the skip set); a junction is one station, so today's merge groups are
//!   built in. Groups are numbered by their lowest StationId.
//! - **Observations**: every shot between two groups, today's vector plus the
//!   difference of its two stations' hand moves on its line (#759), with
//!   today's per-axis weights from `base_sigma_length_m * quality` and
//!   `base_sigma_azimuth_deg * quality`, each axis's sigma floored at
//!   `sigma_len x sigma_az` ([`ShotSigmas`]); soft GPS fixes add a diagonal
//!   term.
//! - **Held**: each placement datum's group at its raw position (two in one
//!   group fail the component), hard GPS fixes (accuracy below 1 mm, the
//!   default) at their GPS, else the gauge: the component's anchor station
//!   (its `Placement` station for a datum-less component) at its raw position.
//! - **Spurs** (leaf-pruned, datums, fixes and a held gauge pinned) keep their
//!   measured shots: a spur station is translated with its attachment station.
//! - Horizontal: depths are the raw ones, except that the ends of every
//!   closure a successful solve adjusted share their tie group's mean depth
//!   (today's solve stamps them aligned on its working copy, so its depth
//!   co-location includes them).
//! - A failing component keeps its raw layout and is reported.
//!
//! Results are per station; the per-slot form adds each line's hand-move z.
//! Everything iterates in sorted order (lines by id, groups by lowest
//! station), so reordering lines or closures (without changing the skip set)
//! changes no bit.

use std::collections::{BTreeMap, VecDeque};

use crate::config::ClosureAdjustmentConfig;
use crate::station::DatumRole;
use crate::{
    gps_to_local, AdjustmentFailureReport, AdjustmentReport, LineId, StationId, StationRef,
    DEFAULT_GPS_ACCURACY_M,
};

use super::closures::ClosureAcrossOneShot;
use super::graph::{ShotEdge, StationPositions};
use super::lsq::solve_reduced_laplacian;
use super::topology::find;
use super::{Anchor, Project, RawPositions, SlotPosition};

/// An accuracy below this many metres is a hard pin (today's
/// `HARD_FIX_ACCURACY_M`).
const HARD_FIX_ACCURACY_M: f64 = 1e-3;

/// Why a component's solve failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustError {
    /// Closures tie two placement datums into one constraint group.
    MultiplePlacementDatums,
    /// The solve would move a placement datum's station (a guard; the datum's
    /// group is held, so it never fires on a valid model).
    WouldMoveDatum,
}

impl AdjustError {
    /// Today's internal error text.
    pub fn message(self) -> &'static str {
        match self {
            Self::MultiplePlacementDatums => {
                "Multiple placement zero datums in a single constraint group"
            }
            Self::WouldMoveDatum => "Adjustment would move a zero datum station",
        }
    }

    /// Today's plain-language reason (`AdjustmentFailure::user_reason`).
    pub fn user_reason(self) -> &'static str {
        match self {
            Self::MultiplePlacementDatums => {
                "loop closures tie two zero datums to the same point, and both positions are fixed. Remove one of those closures or one of the datums."
            }
            Self::WouldMoveDatum => {
                "the adjustment would have moved a zero datum. Check the closures and datums in this part of the survey."
            }
        }
    }
}

/// One component the solve could not adjust; it keeps its raw layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjustmentFailure {
    /// The station the component's solve was held at.
    pub anchor: StationId,
    /// That station's slot (a datum's `via` slot, else its first slot).
    pub anchor_slot: StationRef,
    /// Every line of the component, ascending.
    pub lines: Vec<LineId>,
    pub error: AdjustError,
}

impl AdjustmentFailure {
    /// The wire form.
    pub fn to_wire(&self) -> AdjustmentFailureReport {
        AdjustmentFailureReport {
            anchor: self.anchor_slot,
            line_ids: self.lines.clone(),
            reason: self.error.user_reason().to_string(),
        }
    }
}

/// How the Closures Computed View treats the solve component (the shot
/// components joined by any closure) holding a station: whether it is
/// solved, and the station it is held at when nothing else is. Edits that
/// must not shift the view (Combine Lines' choice of the side that moves)
/// read it, so they and the solve share one rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolveHold {
    /// The held station: the station of the component's first placement
    /// datum in list order, else the `Placement` station of its piece with
    /// the lowest station (the gauge).
    pub anchor: StationId,
    /// Whether `anchor` is a placement datum's station.
    pub datum: bool,
    /// Whether the solve adjusts the component (it holds an eligible closure
    /// the across-one-shot rule does not skip, or a GPS fix); otherwise it
    /// keeps its raw layout.
    pub solved: bool,
}

/// The solve components of one state (module docs).
struct SolvePartition {
    /// Per shot component: the root shot component of its solve component.
    solve_of: Vec<usize>,
    /// Per root: whether the solve adjusts it.
    wanted: Vec<bool>,
    /// Per closure (list order): skipped as across one shot.
    skipped: Vec<bool>,
}

/// The shot component whose anchor holds the solve of `members`: the one
/// whose placement datum is first in list order, else the lowest.
fn held_component(anchors: &[Anchor], members: &[usize], fallback: usize) -> usize {
    members
        .iter()
        .copied()
        .min_by_key(|&c| (anchors[c].datum().unwrap_or(usize::MAX), c))
        .unwrap_or(fallback)
}

/// The Closures Computed View of one project state: per-station positions
/// plus what the solve skipped and could not do.
#[derive(Debug, Clone, PartialEq)]
pub struct AdjustedPositions {
    stations: Vec<StationId>,
    xy: Vec<[f64; 2]>,
    z: Vec<f64>,
    failures: Vec<AdjustmentFailure>,
    skipped: Vec<ClosureAcrossOneShot>,
}

impl AdjustedPositions {
    /// A station's adjusted horizontal position.
    pub fn xy(&self, station: StationId) -> Option<[f64; 2]> {
        self.index(station).map(|d| self.xy[d])
    }

    /// A station's depth: the raw one, except at the ends of every closure a
    /// successful solve adjusted, which share their tie group's mean depth
    /// (D12, `co_locate_adjusted_depths`). The solve itself is horizontal.
    pub fn z(&self, station: StationId) -> Option<f64> {
        self.index(station).map(|d| self.z[d])
    }

    /// Every station with its `[x, y, z]`, ascending by id.
    pub fn iter(&self) -> impl Iterator<Item = (StationId, [f64; 3])> + '_ {
        self.stations
            .iter()
            .zip(self.xy.iter().zip(&self.z))
            .map(|(&s, (xy, &z))| (s, [xy[0], xy[1], z]))
    }

    /// A slot's adjusted position (its station's, plus the line's hand z).
    pub fn slot(&self, project: &Project, slot: StationRef) -> Option<[f64; 3]> {
        let station = project.station_at(slot)?;
        let [x, y] = self.xy(station)?;
        let z = self.z(station)?;
        let hand_z = project
            .line(slot.line_id)
            .and_then(|line| line.hand_moves.get(&station))
            .map_or(0.0, |hand| hand[2]);
        Some([x, y, z + hand_z])
    }

    /// Every slot's adjusted position in slot order (the wire's list).
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

    /// The components the solve could not adjust.
    pub fn failures(&self) -> &[AdjustmentFailure] {
        &self.failures
    }

    /// The closures the solve skipped (across one shot).
    pub fn skipped(&self) -> &[ClosureAcrossOneShot] {
        &self.skipped
    }

    fn index(&self, station: StationId) -> Option<usize> {
        self.stations.binary_search(&station).ok()
    }
}

impl StationPositions for AdjustedPositions {
    fn station_xy(&self, station: StationId) -> Option<[f64; 2]> {
        self.xy(station)
    }

    fn station_z(&self, station: StationId) -> Option<f64> {
        self.z(station)
    }
}

/// The per-axis standard deviations (metres) one shot's horizontal vector
/// is weighed with in the Closures Computed View (weight `1 / sigma^2` per
/// axis, x east, y north).
///
/// **First order (0.6.0's).** With `sigma_len = base_sigma_length_m *
/// quality` and `sigma_az = base_sigma_azimuth_deg * quality` (radians), the
/// shot vector `len * (sin az, cos az)` has
///
/// ```text
/// sigma_x^2 = (sin(az) * sigma_len)^2 + (len * cos(az) * sigma_az)^2
/// sigma_y^2 = (cos(az) * sigma_len)^2 + (len * sin(az) * sigma_az)^2
/// ```
///
/// **The floor (plan Deviations G9, C24 finding F2).** On a grid axis
/// (az = 0, 90, 180 or 270 degrees) the length term of the axis across the
/// shot vanishes, and so does the azimuth term as `len -> 0`. A zero-length
/// connector at azimuth 0 then has sigma_x = 0 (no weight on x at all), and
/// a 1 mm shot there sigma_x = 1.7e-5 m (a weight of 3e9, where ordinary
/// shots weigh tens to hundreds), so a short shot near an axis left the
/// normal equations singular or so ill-conditioned that the solve opened a
/// connector by the whole misclosure or moved stations by kilometres. The
/// first-order propagation drops the product of the two errors: the azimuth
/// error turns the true length, which differs from the measured one by the
/// length error, so the axis across the shot also carries `e_len * e_az`
/// (times `cos(az)` on x, `sin(az)` on y), with variance
/// `sigma_len^2 * sigma_az^2` there. No axis of any shot is known better than
/// that (for `sigma_az` under 1 radian), so each axis's sigma is at least
/// `sigma_len * sigma_az` (8.7e-4 m with the defaults). The floor binds only
/// for a shot shorter than `sigma_len` (5 cm by default) within about
/// `sigma_az` (1 degree) of a grid axis, where the first-order terms vanish;
/// every other shot keeps 0.6.0's weights bit for bit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotSigmas {
    /// 0.6.0's first-order sigmas, `[x, y]`.
    pub first_order: [f64; 2],
    /// What the solve weighs with: each first-order sigma raised to
    /// `floor` when below it (a NaN stays NaN and the axis is not observed).
    pub axes: [f64; 2],
    /// `sigma_len * sigma_az`.
    pub floor: f64,
}

impl ShotSigmas {
    /// The sigmas of a shot of slope `length` at true azimuth `azimuth_rad`
    /// on a line of `quality` (a quality that is not a positive number
    /// counts as 1, as in 0.6.0).
    pub fn of(
        config: &ClosureAdjustmentConfig,
        quality: f64,
        length: f64,
        azimuth_rad: f64,
    ) -> Self {
        let quality = if quality.is_finite() && quality > 0.0 {
            quality
        } else {
            1.0
        };
        let sigma_len = config.base_sigma_length_m * quality;
        let sigma_az = config.base_sigma_azimuth_deg.to_radians() * quality;
        let (sin, cos) = (azimuth_rad.sin(), azimuth_rad.cos());
        let first_order = [
            ((sin * sigma_len).powi(2) + (length * cos * sigma_az).powi(2)).sqrt(),
            ((cos * sigma_len).powi(2) + (length * sin * sigma_az).powi(2)).sqrt(),
        ];
        let floor = sigma_len * sigma_az;
        let axes = first_order.map(|sigma| if sigma < floor { floor } else { sigma });
        Self {
            first_order,
            axes,
            floor,
        }
    }

    /// Whether the floor raised an axis: 0.6.0 weighed this shot with a
    /// degenerate (huge, infinite or zero) weight there.
    pub fn floored(&self) -> bool {
        self.first_order
            .iter()
            .zip(&self.axes)
            .any(|(first, axis)| first.to_bits() != axis.to_bits())
    }
}

/// One GPS fix as the solve sees it (today's `FixObservation`).
#[derive(Debug, Clone, Copy)]
struct Fix {
    /// Dense station.
    station: usize,
    east: f64,
    north: f64,
    /// `1 / accuracy^2`; `None` for a hard fix.
    weight: Option<f64>,
}

/// What one component's solve needs.
struct SolveInput<'a> {
    /// The component's shots, lines in id order.
    shots: Vec<ShotEdge<'a>>,
    /// Dense stations, ascending.
    stations: Vec<usize>,
    /// Dense end pairs of the closures the solve ties, list order.
    ties: Vec<(usize, usize)>,
    /// Dense stations of the placement datums, datum list order.
    placement: Vec<usize>,
    fixes: Vec<Fix>,
    /// Dense station held when nothing else is.
    anchor: usize,
    /// List positions of the eligible, unskipped closures of the component:
    /// a successful solve makes their ends one point, depth included.
    adjusted: Vec<usize>,
}

impl Project {
    /// The Closures Computed View of this state.
    pub fn adjusted_positions(&self, config: &ClosureAdjustmentConfig) -> AdjustedPositions {
        self.adjusted_positions_from(&self.raw_positions(), config)
    }

    /// [`Self::adjusted_positions`] from the raw layout of this state.
    pub fn adjusted_positions_from(
        &self,
        raw: &RawPositions,
        config: &ClosureAdjustmentConfig,
    ) -> AdjustedPositions {
        let topology = self.topology();
        let stations = topology.stations().to_vec();
        let mut xy: Vec<[f64; 2]> = Vec::with_capacity(stations.len());
        let mut z: Vec<f64> = Vec::with_capacity(stations.len());
        for &s in &stations {
            xy.push(raw.xy(s).unwrap_or([0.0, 0.0]));
            z.push(raw.z(s).unwrap_or(0.0));
        }
        let aligned = self.closure_alignment(raw);
        let skipped = self.closures_across_one_shot_with(&aligned);
        let roles = self.datum_roles();
        let has_fix = roles.contains(&Some(DatumRole::GpsFix));
        let has_eligible = self
            .closures
            .iter()
            .any(|c| Self::closure_is_eligible(c.strategy));
        let mut failures = Vec::new();
        if has_eligible || has_fix {
            let raw_xy = xy.clone();
            let mut adjusted_closures = vec![false; self.closures.len()];
            for (input, lines, anchor_slot) in self.solve_inputs(raw, &aligned, &skipped, &roles) {
                let anchor = topology.stations()[input.anchor];
                match self.solve_component(&raw_xy, &input, config) {
                    Ok(solved) => {
                        for (&d, p) in input.stations.iter().zip(solved) {
                            xy[d] = p;
                        }
                        for &i in &input.adjusted {
                            adjusted_closures[i] = true;
                        }
                    }
                    Err(error) => failures.push(AdjustmentFailure {
                        anchor,
                        anchor_slot,
                        lines,
                        error,
                    }),
                }
            }
            self.co_locate_adjusted_depths(&aligned, &adjusted_closures, &mut z);
        }
        AdjustedPositions {
            stations,
            xy,
            z,
            failures,
            skipped,
        }
    }

    /// The report of the Closures Computed View (today's
    /// `get_adjustment_report`): the skipped closures and the far GPS fixes
    /// (always current), and the failures of `adjusted` when given.
    pub fn adjustment_report(&self, adjusted: Option<&AdjustedPositions>) -> AdjustmentReport {
        let skipped = match adjusted {
            Some(a) => a.skipped().to_vec(),
            None => self.closures_across_one_shot(),
        };
        AdjustmentReport {
            skipped_closures: skipped.iter().filter_map(|c| c.to_wire(self)).collect(),
            failures: adjusted
                .map(|a| {
                    a.failures()
                        .iter()
                        .map(AdjustmentFailure::to_wire)
                        .collect()
                })
                .unwrap_or_default(),
            far_gps_fixes: self
                .far_gps_fixes()
                .iter()
                .map(super::FarFix::to_wire)
                .collect(),
        }
    }

    /// How the Closures Computed View treats the solve component holding
    /// `station` ([`SolveHold`]); `None` for a station on no line.
    pub fn solve_hold(&self, station: StationId) -> Option<SolveHold> {
        let raw = self.raw_positions();
        let aligned = self.closure_alignment(&raw);
        let skipped = self.closures_across_one_shot_with(&aligned);
        let partition = self.solve_partition(&skipped, &self.datum_roles());
        let root = partition.solve_of[self.topology().component_of(station)?];
        let members: Vec<usize> = (0..partition.solve_of.len())
            .filter(|&c| partition.solve_of[c] == root)
            .collect();
        let anchors = raw.anchors();
        let held = held_component(anchors, &members, root);
        Some(SolveHold {
            anchor: anchors[held].station(),
            datum: anchors[held].datum().is_some(),
            solved: partition.wanted[root],
        })
    }

    /// The solve components (shot components joined by every closure) and
    /// which of them the solve adjusts: an eligible closure it does not skip
    /// with both ends in the component, or a GPS fix.
    fn solve_partition(
        &self,
        skipped: &[ClosureAcrossOneShot],
        roles: &[Option<DatumRole>],
    ) -> SolvePartition {
        let topology = self.topology();
        let components = topology.components();
        let mut parent: Vec<usize> = (0..components.len()).collect();
        for closure in &self.closures {
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
        let solve_of: Vec<usize> = (0..components.len())
            .map(|c| find(&mut parent, c))
            .collect();
        let mut skipped_mask = vec![false; self.closures.len()];
        for s in skipped {
            if let Some(m) = skipped_mask.get_mut(s.index) {
                *m = true;
            }
        }
        let solve_of_station = |s: StationId| topology.component_of(s).map(|c| solve_of[c]);
        let mut wanted = vec![false; components.len()];
        for (i, closure) in self.closures.iter().enumerate() {
            if Self::closure_is_eligible(closure.strategy) && !skipped_mask[i] {
                if let (Some(a), Some(b)) =
                    (solve_of_station(closure.a), solve_of_station(closure.b))
                {
                    if a == b {
                        wanted[a] = true;
                    }
                }
            }
        }
        for (datum, role) in self.datums.iter().zip(roles) {
            if *role == Some(DatumRole::GpsFix) {
                if let Some(s) = solve_of_station(datum.station) {
                    wanted[s] = true;
                }
            }
        }
        SolvePartition {
            solve_of,
            wanted,
            skipped: skipped_mask,
        }
    }

    /// The components to solve, in the order of their first line in the list,
    /// with their sorted line ids and anchor slot.
    fn solve_inputs(
        &self,
        raw: &RawPositions,
        aligned: &[bool],
        skipped: &[ClosureAcrossOneShot],
        roles: &[Option<DatumRole>],
    ) -> Vec<(SolveInput<'_>, Vec<LineId>, StationRef)> {
        let topology = self.topology();
        let components = topology.components();
        let anchors = raw.anchors();
        let SolvePartition {
            solve_of,
            wanted,
            skipped: skipped_mask,
        } = self.solve_partition(skipped, roles);
        let solve_of_station = |s: StationId| topology.component_of(s).map(|c| solve_of[c]);

        // Order: by the list position of the first line of the component.
        let mut first_line: Vec<usize> = vec![usize::MAX; components.len()];
        for (c, component) in components.iter().enumerate() {
            let s = solve_of[c];
            for line in &component.lines {
                if let Some(position) = topology.line_position(*line) {
                    first_line[s] = first_line[s].min(position);
                }
            }
        }
        let mut order: Vec<usize> = (0..components.len())
            .filter(|&s| solve_of[s] == s && wanted[s])
            .collect();
        order.sort_by_key(|&s| (first_line[s], s));

        // Bucket everything by solve component in one pass each, so the cost
        // stays linear however many components are solved.
        let mut index_of = vec![usize::MAX; components.len()];
        for (k, &s) in order.iter().enumerate() {
            index_of[s] = k;
        }
        let bucket = |station: StationId| -> Option<usize> {
            let k = index_of[solve_of_station(station)?];
            (k != usize::MAX).then_some(k)
        };
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
        for c in 0..components.len() {
            let k = index_of[solve_of[c]];
            if k != usize::MAX {
                members[k].push(c);
            }
        }
        let mut ties: Vec<Vec<(usize, usize)>> = vec![Vec::new(); order.len()];
        let mut adjusted: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
        for (i, c) in self.closures.iter().enumerate() {
            if skipped_mask[i] {
                continue;
            }
            let (Some(a), Some(b)) = (topology.dense(c.a), topology.dense(c.b)) else {
                continue;
            };
            let (Some(k), Some(kb)) = (bucket(c.a), bucket(c.b)) else {
                continue;
            };
            if k != kb {
                continue;
            }
            if Self::closure_ties_in_solve(c, aligned.get(i).copied().unwrap_or(false)) {
                ties[k].push((a, b));
            }
            if Self::closure_is_eligible(c.strategy) {
                adjusted[k].push(i);
            }
        }
        let frame = self
            .frame_datum()
            .and_then(|f| self.datums.get(f))
            .map(|d| d.gps);
        let mut shots: Vec<Vec<ShotEdge<'_>>> = vec![Vec::new(); order.len()];
        self.for_each_shot_edge(|edge| {
            if let Some(k) = bucket(edge.from_station) {
                shots[k].push(edge);
            }
        });
        let mut fixes: Vec<Vec<Fix>> = vec![Vec::new(); order.len()];
        if let Some(frame) = frame {
            for (d, role) in self.datums.iter().zip(roles) {
                if *role != Some(DatumRole::GpsFix) {
                    continue;
                }
                let (Some(k), Some(station)) = (bucket(d.station), topology.dense(d.station))
                else {
                    continue;
                };
                let (east, north) = gps_to_local(&d.gps, &frame);
                // A GPS that is no position is no observation (today).
                if !east.is_finite() || !north.is_finite() {
                    continue;
                }
                let accuracy = match d.accuracy_m.unwrap_or(DEFAULT_GPS_ACCURACY_M) {
                    a if a.is_finite() && a >= 0.0 => a,
                    _ => DEFAULT_GPS_ACCURACY_M,
                };
                let weight = (accuracy >= HARD_FIX_ACCURACY_M).then(|| 1.0 / (accuracy * accuracy));
                fixes[k].push(Fix {
                    station,
                    east,
                    north,
                    weight,
                });
            }
        }

        let mut out = Vec::with_capacity(order.len());
        for (k, (((members, ties), (adjusted, fixes)), shots)) in members
            .into_iter()
            .zip(ties)
            .zip(adjusted.into_iter().zip(fixes))
            .zip(shots)
            .enumerate()
        {
            let s = order[k];
            let mut stations: Vec<usize> = Vec::new();
            let mut lines: Vec<LineId> = Vec::new();
            for &c in &members {
                stations.extend(
                    components[c]
                        .stations
                        .iter()
                        .filter_map(|&st| topology.dense(st)),
                );
                lines.extend(components[c].lines.iter().copied());
            }
            stations.sort_unstable();
            lines.sort_unstable();
            lines.dedup();
            // The anchor: the component whose placement datum is first in list
            // order, else the (single) placement station.
            let anchor_component = held_component(anchors, &members, s);
            let anchor_station = anchors[anchor_component].station();
            let anchor_slot = match anchors[anchor_component] {
                Anchor::Datum { datum, .. } => {
                    self.datums.get(datum).and_then(|d| self.datum_slot(d))
                }
                _ => topology.first_slot(anchor_station),
            }
            .or_else(|| topology.first_slot(anchor_station))
            .unwrap_or(StationRef::new(0, 0));
            let mut placement: Vec<(usize, usize)> = members
                .iter()
                .filter_map(|&c| {
                    let d = anchors[c].datum()?;
                    Some((d, topology.dense(self.datums[d].station)?))
                })
                .collect();
            placement.sort_unstable();
            let input = SolveInput {
                shots,
                adjusted,
                stations,
                ties,
                placement: placement.into_iter().map(|(_, d)| d).collect(),
                fixes,
                anchor: topology.dense(anchor_station).unwrap_or(0),
            };
            out.push((input, lines, anchor_slot));
        }
        out
    }

    /// Depth in the view: today's solve stamps every closure it adjusted as
    /// aligned on its working copy, so the view's depth co-locates the ends
    /// of those closures as well as the aligned ones (the mean of the depth
    /// readings over the tie group, summed in slot order, as plan 3.4).
    /// Groups with no adjusted closure keep their raw depth.
    fn co_locate_adjusted_depths(&self, aligned: &[bool], adjusted: &[bool], z: &mut [f64]) {
        if !adjusted.contains(&true) {
            return;
        }
        let topology = self.topology();
        let n = topology.stations().len();
        let mut parent: Vec<usize> = (0..n).collect();
        let mut touched: Vec<usize> = Vec::new();
        for (i, closure) in self.closures.iter().enumerate() {
            let tie = aligned.get(i).copied().unwrap_or(false);
            let stamped = adjusted.get(i).copied().unwrap_or(false);
            if !(tie || stamped) {
                continue;
            }
            if let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
                if stamped {
                    touched.push(a);
                }
            }
        }
        let mut roots: Vec<usize> = touched.iter().map(|&a| find(&mut parent, a)).collect();
        roots.sort_unstable();
        roots.dedup();
        let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for d in 0..n {
            let root = find(&mut parent, d);
            if roots.binary_search(&root).is_ok() {
                members.entry(root).or_default().push(d);
            }
        }
        for group in members.values() {
            let mut slots: Vec<StationRef> = group
                .iter()
                .flat_map(|&d| topology.slots_of(topology.stations()[d]).iter().copied())
                .collect();
            slots.sort_unstable();
            let mut readings: Vec<f64> = Vec::new();
            for slot in &slots {
                let Some(line) = self.line(slot.line_id) else {
                    continue;
                };
                let index = slot.station_index as usize;
                if index > 0 {
                    if let Some(shot) = line.shots.get(index - 1) {
                        readings.push(shot.depth_out);
                    }
                }
                if let Some(shot) = line.shots.get(index) {
                    readings.push(shot.depth_in);
                }
            }
            let depth = if readings.is_empty() {
                0.0
            } else {
                readings.iter().sum::<f64>() / readings.len() as f64
            };
            for &d in group {
                z[d] = depth;
            }
        }
    }

    /// Solve one component; the adjusted XY of `input.stations`, in order.
    fn solve_component(
        &self,
        raw_xy: &[[f64; 2]],
        input: &SolveInput<'_>,
        config: &ClosureAdjustmentConfig,
    ) -> Result<Vec<[f64; 2]>, AdjustError> {
        let m = input.stations.len();
        let local = |d: usize| input.stations.binary_search(&d).ok();

        // Constraint groups, rooted at their lowest member.
        let mut parent: Vec<usize> = (0..m).collect();
        for &(a, b) in &input.ties {
            if let (Some(la), Some(lb)) = (local(a), local(b)) {
                let (ra, rb) = (find(&mut parent, la), find(&mut parent, lb));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
        let group: Vec<usize> = (0..m).map(|i| find(&mut parent, i)).collect();

        let mut fixed: BTreeMap<usize, [f64; 2]> = BTreeMap::new();
        for &d in &input.placement {
            let Some(l) = local(d) else {
                continue;
            };
            if fixed.insert(group[l], raw_xy[d]).is_some() {
                return Err(AdjustError::MultiplePlacementDatums);
            }
        }
        for fix in &input.fixes {
            if fix.weight.is_some() {
                continue;
            }
            if let Some(l) = local(fix.station) {
                fixed.entry(group[l]).or_insert([fix.east, fix.north]);
            }
        }
        let anchor_held = fixed.is_empty();
        if anchor_held {
            if let Some(l) = local(input.anchor) {
                fixed.insert(group[l], raw_xy[input.anchor]);
            }
        }

        // Unknowns: the free groups, by lowest member (= lowest StationId).
        let mut unknown: Vec<Option<usize>> = vec![None; m];
        let mut n = 0;
        for root in 0..m {
            if group[root] == root && !fixed.contains_key(&root) {
                unknown[root] = Some(n);
                n += 1;
            }
        }

        let mut atb_x = vec![0.0; n];
        let mut atb_y = vec![0.0; n];
        let mut diag_x = vec![0.0; n];
        let mut diag_y = vec![0.0; n];
        let mut adj: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
        let mut shot_nbrs: Vec<Vec<usize>> = vec![Vec::new(); m];
        let mut degree_nbrs: Vec<Vec<usize>> = vec![Vec::new(); m];
        for &edge in &input.shots {
            let (Some(li), Some(lj)) = (local(edge.from), local(edge.to)) else {
                continue;
            };
            shot_nbrs[li].push(lj);
            shot_nbrs[lj].push(li);
            degree_nbrs[li].push(lj);
            degree_nbrs[lj].push(li);
            let (root_i, root_j) = (group[li], group[lj]);
            if root_i == root_j {
                continue;
            }
            let line = edge.line;
            let shot = &line.shots[edge.index];
            let reference = self.azimuth_reference_for(line);
            let mut vec = shot.vector(reference);
            let (hand_i, hand_j) = (
                line.hand_moves.get(&edge.from_station),
                line.hand_moves.get(&edge.to_station),
            );
            if hand_i.is_some() || hand_j.is_some() {
                let hi = hand_i.copied().unwrap_or([0.0; 3]);
                let hj = hand_j.copied().unwrap_or([0.0; 3]);
                vec[0] += hj[0] - hi[0];
                vec[1] += hj[1] - hi[1];
            }
            let az = shot.average_azimuth_true_rad(reference);
            let [sigma_dx, sigma_dy] = ShotSigmas::of(config, line.quality, shot.length, az).axes;
            let w_x = if sigma_dx > 0.0 {
                1.0 / (sigma_dx * sigma_dx)
            } else {
                0.0
            };
            let w_y = if sigma_dy > 0.0 {
                1.0 / (sigma_dy * sigma_dy)
            } else {
                0.0
            };
            let (i_idx, j_idx) = (unknown[root_i], unknown[root_j]);
            let held = |root: usize| fixed.get(&root).copied().unwrap_or([0.0, 0.0]);
            let rhs = |axis: usize| match (i_idx, j_idx) {
                (Some(_), Some(_)) => vec[axis],
                (Some(_), None) => vec[axis] - held(root_j)[axis],
                (None, Some(_)) => vec[axis] + held(root_i)[axis],
                (None, None) => 0.0,
            };
            for (axis, w) in [(0, w_x), (1, w_y)] {
                if !(w.is_finite() && w > 0.0) {
                    continue;
                }
                let b = rhs(axis);
                let (diag, atb) = if axis == 0 {
                    (&mut diag_x, &mut atb_x)
                } else {
                    (&mut diag_y, &mut atb_y)
                };
                if let Some(ii) = i_idx {
                    diag[ii] += w;
                    atb[ii] += -w * b;
                }
                if let Some(jj) = j_idx {
                    diag[jj] += w;
                    atb[jj] += w * b;
                }
            }
            if let (Some(ii), Some(jj)) = (i_idx, j_idx) {
                accumulate(&mut adj[ii], jj, w_x, w_y);
                accumulate(&mut adj[jj], ii, w_x, w_y);
            }
        }
        for &(a, b) in &input.ties {
            if let (Some(la), Some(lb)) = (local(a), local(b)) {
                degree_nbrs[la].push(lb);
                degree_nbrs[lb].push(la);
            }
        }
        for fix in &input.fixes {
            let Some(w) = fix.weight else {
                continue;
            };
            let Some(i) = local(fix.station).and_then(|l| unknown[group[l]]) else {
                continue;
            };
            diag_x[i] += w;
            diag_y[i] += w;
            atb_x[i] += w * fix.east;
            atb_y[i] += w * fix.north;
        }

        // Warm start: each free group's lowest member's raw position.
        let mut x0_x = vec![0.0; n];
        let mut x0_y = vec![0.0; n];
        for root in 0..m {
            if let Some(i) = unknown[root] {
                let p = raw_xy[input.stations[root]];
                x0_x[i] = p[0];
                x0_y[i] = p[1];
            }
        }
        let (sol_x, sol_y) = if n == 0 {
            (Vec::new(), Vec::new())
        } else {
            solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0_x, &x0_y)
        };
        let group_xy = |root: usize| -> [f64; 2] {
            match unknown[root] {
                Some(i) => [sol_x[i], sol_y[i]],
                None => fixed
                    .get(&root)
                    .copied()
                    .unwrap_or(raw_xy[input.stations[root]]),
            }
        };

        // Spurs: leaf pruning over shots and ties, pinned stations kept.
        let mut pinned = vec![false; m];
        for &d in &input.placement {
            if let Some(l) = local(d) {
                pinned[l] = true;
            }
        }
        for fix in &input.fixes {
            if let Some(l) = local(fix.station) {
                pinned[l] = true;
            }
        }
        if anchor_held {
            if let Some(l) = local(input.anchor) {
                pinned[l] = true;
            }
        }
        let mut degree: Vec<usize> = degree_nbrs.iter().map(Vec::len).collect();
        let mut stack: Vec<usize> = (0..m).filter(|&l| degree[l] == 1 && !pinned[l]).collect();
        let mut pruned = vec![false; m];
        while let Some(l) = stack.pop() {
            pruned[l] = true;
            for &v in &degree_nbrs[l] {
                if !pruned[v] {
                    degree[v] = degree[v].saturating_sub(1);
                    if degree[v] == 1 && !pinned[v] {
                        stack.push(v);
                    }
                }
            }
        }

        // Positions: a non-spur station sits on its group's solution; a spur
        // keeps its measured shots, translated with its attachment station.
        let mut out: Vec<Option<[f64; 2]>> = vec![None; m];
        let mut queue: VecDeque<usize> = VecDeque::new();
        for l in 0..m {
            if !pruned[l] {
                out[l] = Some(group_xy(group[l]));
                queue.push_back(l);
            }
        }
        while let Some(u) = queue.pop_front() {
            let Some(pu) = out[u] else {
                continue;
            };
            let ru = raw_xy[input.stations[u]];
            for &v in &shot_nbrs[u] {
                if out[v].is_none() && pruned[v] {
                    let rv = raw_xy[input.stations[v]];
                    out[v] = Some([pu[0] + (rv[0] - ru[0]), pu[1] + (rv[1] - ru[1])]);
                    queue.push_back(v);
                }
            }
        }
        let solved: Vec<[f64; 2]> = (0..m)
            .map(|l| out[l].unwrap_or_else(|| group_xy(group[l])))
            .collect();

        // Placement datums must not have moved (today's guard).
        for &d in &input.placement {
            if let Some(l) = local(d) {
                let (before, after) = (raw_xy[d], solved[l]);
                if (before[0] - after[0]).abs() >= 1e-6 || (before[1] - after[1]).abs() >= 1e-6 {
                    return Err(AdjustError::WouldMoveDatum);
                }
            }
        }
        Ok(solved)
    }
}

/// Add `-w` to the off-diagonal entry `(j, ..)` of a row, appending it when
/// new (today's linear-scan accumulation, so the float sums run in shot
/// order).
fn accumulate(row: &mut Vec<(usize, f64, f64)>, j: usize, w_x: f64, w_y: f64) {
    for entry in row.iter_mut() {
        if entry.0 == j {
            entry.1 -= w_x;
            entry.2 -= w_y;
            return;
        }
    }
    row.push((j, -w_x, -w_y));
}
