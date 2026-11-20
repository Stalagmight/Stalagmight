//! The planned route's physical readouts (#38, #299): swum line length, depths, cumulative
//! distance and the round-trip dive profile.
//!
//! Copied from `models/src/project/gas.rs` (`cumulative_distances`, `route_metrics`,
//! `ExitLegRates`, `build_route_profile`) for the station-graph engine (`survey::gas`). The
//! arithmetic is unchanged. What changed is how a route is described: the old helpers took one
//! coordinate per route SLOT plus a swim mask, and a junction appeared as two slots joined by a
//! zero-distance merge hop. On the station graph a junction is one route node, and its two slots
//! can sit at different depths when a legacy hand move carries a z on one line (plan 5.6). So:
//!
//! - each leg carries its own geometry ([`LegEnds`]): the two stations at their slot coordinates
//!   on the leg shot's line, which is exactly the pair of points the old swim leg measured;
//! - each node carries one depth: its slot z on the line of the arriving leg (the departing leg
//!   for the first node, see `route_node_lines` in the engine).

use crate::{ShotKind, StationId};

use super::physics::edge_gas;

/// One route leg as the readouts measure it: `Some([from, to])`, the leg shot's two stations at
/// their slot coordinates (`[x, y, z]`) on that shot's own line, for a swum leg (a shot, including a
/// reference/jump shot the diver swims); `None` for a loop-closure tie, a zero-distance graph edge
/// whose endpoints are the *same* surveyed point - the measured gap there (a closure's misclosure)
/// was never swum, so it must not count toward route distance or depth.
pub(super) type LegEnds = Option<[[f64; 3]; 2]>;

/// The 3-D length of a swum leg.
fn leg_length(ends: &[[f64; 3]; 2]) -> f64 {
    let (a, b) = (ends[0], ends[1]);
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
}

/// Cumulative swum distance (metres) at each of the route's `nodes` stations: 3-D segment lengths
/// accumulated over swum legs only, with zero-distance closure ties excluded exactly as
/// [`route_metrics`] does. A missing leg entry counts as a tie (it has no geometry to measure).
pub(super) fn cumulative_distances(nodes: usize, legs: &[LegEnds]) -> Vec<f64> {
    let mut cum = vec![0.0_f64; nodes];
    for i in 1..nodes {
        let seg = legs
            .get(i - 1)
            .copied()
            .flatten()
            .map_or(0.0, |ends| leg_length(&ends));
        cum[i] = cum[i - 1] + seg;
    }
    cum
}

/// Physical metrics of the planned route from its per-node depths and its per-leg geometry
/// ([`LegEnds`]): the one-way swum line length (sum of 3-D segment lengths over swum legs only),
/// the deepest point, and the distance-weighted average depth over the swum legs. `z` is depth in
/// metres (positive below surface, `station.rs`), so the deepest point is the max depth over *all*
/// route stations. Max-depth is deliberately over all stations, not just swum legs: both ends of a
/// tie are real surveyed stations the route passes through, so the deeper one is a genuine depth the
/// diver reaches - only the un-swum tie *distance* (and its depth-weight) is excluded, from
/// length/avg. An empty route → `(0.0, None, None)`; a single station (or all-tie route) → zero
/// length with the depth at the first station.
pub(super) fn route_metrics(depths: &[f64], legs: &[LegEnds]) -> (f64, Option<f64>, Option<f64>) {
    let Some(&first) = depths.first() else {
        return (0.0, None, None);
    };
    let max_depth = depths.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut length = 0.0;
    let mut depth_len_sum = 0.0;
    // Skip zero-distance ties (loop closures): their measured-coord gap was not swum.
    for ends in legs.iter().flatten() {
        let (a, b) = (ends[0], ends[1]);
        let seg = leg_length(ends);
        length += seg;
        // Weight each segment's mean depth by its length → the depth-along-distance average.
        depth_len_sum += seg * 0.5 * (a[2] + b[2]);
    }
    let avg_depth = if length > 0.0 {
        depth_len_sum / length
    } else {
        first
    };
    (length, Some(max_depth), Some(avg_depth))
}

/// One point of the anticipated dive profile (#299) on the station graph: the wire's
/// [`crate::gas::RouteProfilePoint`] with the route node's station in place of its slot
/// (the wire names the slot, see `GasReach::to_wire`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GasProfilePoint {
    /// Index of the route node this point reports (a route that doubles back visits a station
    /// more than once, so the index, not the station, says which pass).
    pub route_index: usize,
    pub station: StationId,
    /// Swum distance from the origin along the profile (out, then back), metres.
    pub distance_m: f64,
    pub depth_m: f64,
    /// Cumulative gas, litres.
    pub gas_l: f64,
    pub elapsed_min: f64,
    /// `true` on the way in (up to and including the turnaround), `false` on the way back.
    pub outbound: bool,
}

/// What the diver breathes and how fast they move on the way **out** - the leg the reach field
/// never prices. `GasConfig::normalize` guarantees `speed_m_per_min` is finite, positive and no
/// slower than the survey speed, and `jump_multiplier` is >= 1.0.
pub(super) struct ExitLegRates {
    /// The swum exit pace - every leg the diver was not riding comes home at this.
    pub(super) speed_m_per_min: f64,
    pub(super) rmv_l_per_min: f64,
    pub(super) jump_multiplier: f64,
    /// Per route leg, aligned with `leg_kinds`: `Some(pace)` where the span was scootered - the
    /// backup rides the same line home, so that leg exits at the span's own pace
    /// (`resolve_segments`-guaranteed finite and positive) - else `None` for the swum exit. A
    /// missing entry falls back to the swum exit, the over-stating (conservative) direction.
    pub(super) scooter_speed_by_leg: Vec<Option<f64>>,
}

/// Build the round-trip dive profile (#299): depth, swum distance, runtime and cumulative gas at
/// every route station, out to the turnaround and back.
///
/// Every OUTBOUND number is read straight off vectors this compute already produced, so the profile
/// cannot disagree with `route_line_length_m`, `stage_drop_distances_m` or the overlay's gas.
/// Runtime is one of them: `route_time` comes off the SAME walk riders as `route_runtime_min`, so
/// the last outbound point is that readout exactly.
///
/// ⚠ It is NOT `cumulative distance / in_speed`, which is what it was until #317 and what made a
/// scootered plan export as if it had been swum: with per-segment speeds there is no single speed to
/// divide by, and the panel's own runtime readout was already per-segment - so the chart and both
/// dive-profile exports contradicted the number beside them, in a document the diver carries.
///
/// The RETURN leg is derived here and nowhere else. The reach field only ever prices the way IN
/// (`in_rmv` / the per-segment speeds in the caller), because the reserve fraction is what pays for
/// the exit - so this is the single place that knows the exit-leg rates. Distance mirrors exactly;
/// time and gas do not: a swum leg comes home at the one swum exit speed on the diver's base RMV
/// (`GasConfig::normalize`-guaranteed no slower than the survey speed, so a swum plan's return is
/// never over-stated), while a SCOOTERED span rides home at the span's own pace - the mandatory
/// backup makes the ride out a modelled certainty (`DpvRig`), and the one-way range cap already
/// assumes the backup covers the same line back. Charging a ridden span the swim pace instead
/// would bill runtime and exit gas the plan does not spend, in the export the diver carries.
///
/// The turnaround is emitted once, the origin twice (departure and return), giving `2n - 1` points.
pub(super) fn build_route_profile(
    stations: &[StationId],
    depths: &[f64],
    leg_kinds: &[Option<ShotKind>],
    cum_distance: &[f64],
    route_gas: &[f64],
    route_time: &[f64],
    exit: &ExitLegRates,
) -> Vec<GasProfilePoint> {
    if stations.is_empty() {
        return Vec::new();
    }
    let outbound: Vec<GasProfilePoint> = stations
        .iter()
        .enumerate()
        .map(|(i, &station)| {
            let distance_m = cum_distance.get(i).copied().unwrap_or(0.0);
            GasProfilePoint {
                route_index: i,
                station,
                distance_m,
                depth_m: depths.get(i).copied().unwrap_or(0.0),
                gas_l: route_gas.get(i).copied().unwrap_or(0.0),
                elapsed_min: route_time.get(i).copied().unwrap_or(0.0),
                outbound: true,
            }
        })
        .collect();

    let total_m = outbound.last().map_or(0.0, |p| p.distance_m);
    let mut gas_l = outbound.last().map_or(0.0, |p| p.gas_l);
    let mut elapsed_min = outbound.last().map_or(0.0, |p| p.elapsed_min);

    // Leg `i` joins outbound points `i` and `i+1`, so stepping back from `i+1` to `i` re-swims it.
    let mut profile = outbound.clone();
    for i in (0..outbound.len().saturating_sub(1)).rev() {
        let (from, to) = (&outbound[i + 1], &outbound[i]);
        let seg_m = from.distance_m - to.distance_m;
        // A tie is the same physical point twice: no distance, so no time and no gas. Skipping it
        // here is what keeps the return a distance-exact mirror of the way in.
        if seg_m > 0.0 {
            let virtual_leg = leg_kinds
                .get(i)
                .copied()
                .flatten()
                .is_some_and(ShotKind::is_virtual);
            let rmv = if virtual_leg {
                exit.rmv_l_per_min * exit.jump_multiplier
            } else {
                exit.rmv_l_per_min
            };
            let speed = exit
                .scooter_speed_by_leg
                .get(i)
                .copied()
                .flatten()
                .unwrap_or(exit.speed_m_per_min);
            elapsed_min += seg_m / speed;
            gas_l += edge_gas(rmv, speed, seg_m, from.depth_m, to.depth_m);
        }
        profile.push(GasProfilePoint {
            route_index: to.route_index,
            station: to.station,
            distance_m: total_m + (total_m - to.distance_m),
            depth_m: to.depth_m,
            gas_l,
            elapsed_min,
            outbound: false,
        });
    }
    profile
}
