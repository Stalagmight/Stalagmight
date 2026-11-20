//! Gas & route-planning engine on the station graph (issue #38; plan 3.8).
//!
//! Ported from `models/src/project/gas.rs` (`compute_gas_reach_field_with_overrides`, the
//! Plan-mode engine) and its pure helpers. The algorithms are the old ones; the node space is new:
//!
//! - a node is a station of [`super::Topology::stations`] (dense index), so a junction (what a
//!   merge was) is ONE node: the old zero-cost merge edges and the merge hops in the route are gone;
//! - loop closures stay zero-cost edges, aligned or not, as today;
//! - coordinates are the MEASURED raw positions ([`Project::raw_positions`]); reachability is
//!   coordinate-independent, so the engine never triggers the closure-adjusted solve;
//! - each shot edge's gas basis uses the slot z of its two stations on the shot's own line
//!   (`slot_coords`), which is the old per-slot z, legacy per-line hand moves included;
//! - a route leg is a swum leg iff a shot joins its two stations, and its kind is that shot's kind.
//!
//! The result is per station ([`GasReach`]); [`GasReach::to_wire`] expands it to the wire's
//! per-slot [`GasReachField`], and [`Project::gas_reach_field_wire`] keeps the old entry point's
//! signature (slots in, slots out).
//!
//! Layout: `physics` and `scooter` are copies of the old pure helpers; `budget` (reserve
//! strategies, budget and gauges), `drops` (stage drops and overrides), `walk` (way-point walk,
//! budget view, route target) and `profile` (route readouts and the dive profile) hold the old
//! engine's helpers; `wire` holds the slot boundary.

mod budget;
mod drops;
mod physics;
mod profile;
mod scooter;
mod walk;
mod wire;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    reason = "test module: tests construct from known-valid literals, a failed unwrap should fail \
              the test, and bit-exact float equality is what some assertions check"
)]
mod tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and exact equality \
              short-cuts the relative tolerance (infinities included)"
)]
mod differential_tests;

use std::collections::VecDeque;

use crate::config::{DpvRig, GasConfig, ReservePreset};
use crate::project::gas::{Cylinder, Diver, PlanSegment};
use crate::{LineId, ShotKind, StationId};

use super::graph::{dense_coords, slot_coords};
use super::{Project, ShotLocation, Topology};

use budget::{
    all_thirds_backgas_turn, backgas_fraction, backgas_turn_pressure, cylinder_usable_bars,
    effective_preset, free_backgas_turn, penetration_budget,
};
use drops::{
    apply_stage_drop_overrides, cumulative, default_drop_stations, place_stage_drops,
    stage_drop_plans,
};
use physics::edge_gas_basis;
pub use profile::GasProfilePoint;
use profile::{build_route_profile, cumulative_distances, route_metrics, ExitLegRates, LegEnds};
use scooter::{resolve_segments, scooter_drop_index, scooter_range_limit};
use walk::{budget_view, waypoint_walk, BudgetView};

/// The gas reach field on the station graph: the station-level twin of [`GasReachField`]
/// (`crate::project::gas::GasReachField`). Every per-station vector is indexed like
/// [`Self::stations`] (the dense order of [`super::Topology::stations`], ascending by id); route
/// vectors are indexed by route node (`route`) or by route leg (`route_shots`, `route_leg_kinds`,
/// leg `i` joining nodes `i` and `i + 1`). See the wire field for what each readout means.
///
/// [`GasReachField`]: crate::project::gas::GasReachField
#[derive(Debug, Clone, PartialEq)]
pub struct GasReach {
    /// Every station on a line, ascending: the index space of the per-station vectors.
    pub stations: Vec<StationId>,
    /// Gas to reach each station (the overlay's pass-through price), litres; `None` when
    /// unreachable.
    pub gas_to_reach: Vec<Option<f64>>,
    pub reachable: Vec<bool>,
    /// Stations the dive could END at that border one it could not, ascending.
    pub turn_frontier: Vec<StationId>,
    pub max_reachable_gas: f64,
    pub budget_litres: f64,
    /// The planned route origin -> way points -> target, one entry per node. A junction is one
    /// node, so there are no merge hops; a route that doubles back repeats a station.
    pub route: Vec<StationId>,
    /// Per route node: the line whose slot names the node on the wire and whose slot z is its
    /// depth (the arriving swum leg's line, else the departing one's, else the station's lowest
    /// line id).
    pub route_lines: Vec<Option<LineId>>,
    /// Per route leg: the shot that joins its two stations (a swum leg), or `None` for a closure
    /// tie.
    pub route_shots: Vec<Option<ShotLocation>>,
    /// Per route leg: that shot's kind (`None` for a tie).
    pub route_leg_kinds: Vec<Option<ShotKind>>,
    /// The route target (the last route node), if the plan has one.
    pub final_turn: Option<StationId>,
    pub turn_toward_endpoint: bool,
    pub backgas_turn_bar: Option<f64>,
    /// Route index of each stage drop, aligned with the per-stage gauges below.
    pub stage_drop_route_indices: Vec<usize>,
    pub stage_drop_bars: Vec<f64>,
    pub stage_drop_over_reserve: Vec<bool>,
    pub stage_drop_distances_m: Vec<f64>,
    pub cylinder_usable_bars: Vec<f64>,
    pub route_line_length_m: f64,
    pub route_max_depth_m: Option<f64>,
    pub route_avg_depth_m: Option<f64>,
    pub waypoint_gas_litres: Vec<Option<f64>>,
    /// Route index of each way point, in order; `None` from the first one that stalls.
    pub waypoint_route_indices: Vec<Option<usize>>,
    pub route_profile: Vec<GasProfilePoint>,
    pub scooter_distance_m: f64,
    pub scooter_range_limit_m: Option<f64>,
    /// The way point (index into the caller's list) the scooters are parked at, if any.
    pub scooter_drop_waypoint: Option<usize>,
    pub route_runtime_min: Option<f64>,
    pub turn_on_scooter_range: bool,
}

impl GasReach {
    /// The dense index of `station` in this result.
    fn index(&self, station: StationId) -> Option<usize> {
        self.stations.binary_search(&station).ok()
    }

    /// Gas to reach `station`, litres; `None` when unreachable or unknown.
    pub fn gas_at(&self, station: StationId) -> Option<f64> {
        self.index(station)
            .and_then(|i| self.gas_to_reach.get(i).copied().flatten())
    }

    /// Whether `station` is in reach.
    pub fn is_reachable(&self, station: StationId) -> bool {
        self.index(station)
            .and_then(|i| self.reachable.get(i).copied())
            .unwrap_or(false)
    }
}

impl Project {
    /// Single-diver open-circuit reachability field on the station graph (PRD §4/§5.1): today's
    /// `compute_gas_reach_field_with_overrides` with stations in place of slots. See that function
    /// for the full contract; in short:
    ///
    /// Seeds a gas-weighted Dijkstra from `origin` (or the entrance datums when `None`) over the
    /// survey graph, weighting each shot by the **in-leg** gas it costs
    /// (`edge_gas(rmv·survey_mult, in_speed, length, avg_depth)`), then marks every station whose
    /// one-way gas-to-reach is within the **turn budget** (`penetration_budget` for the active
    /// strategy; the diver holds the rest for the exit). The turn frontier is the reachable
    /// stations bordering an unreachable one. It walks the route to `endpoint` (see
    /// `walk::route_target` for the out-of-budget rule) to place drops, and computes the readout
    /// gauges.
    ///
    /// - `origin`: a station not on any line falls back to the entrance roots (#244), exactly as
    ///   an unresolvable origin slot did.
    /// - `waypoints` (#298): ordered stations the dive MUST visit; a `None` entry (a way point
    ///   that no longer resolves) or a disconnected one STALLS the walk: the field goes
    ///   all-unreachable rather than planning a route that skips it (the unsafe direction, §6).
    /// - `segments` (#317): one speed / scooter flag per route span (`waypoints.len() + 1`),
    ///   reconciled by `resolve_segments`; an empty list prices every span at the config speed.
    /// - `stage_drop_overrides` (#246): stage `k`'s drop pinned at a station (`None` keeps the
    ///   strategy default; an off-route or unknown station falls through to it).
    ///
    /// Contract: `gas_config` is assumed normalized (`GasConfig::normalize` - finite/positive
    /// speeds); a non-positive in-speed defensively yields an all-unreachable field rather than
    /// dividing by zero.
    pub fn gas_reach(
        &self,
        origin: Option<StationId>,
        endpoint: Option<StationId>,
        waypoints: &[Option<StationId>],
        segments: &[PlanSegment],
        diver: &Diver,
        cylinders: &[Cylinder],
        gas_config: &GasConfig,
        stage_drop_overrides: &[Option<StationId>],
    ) -> GasReach {
        // Disabled tanks (#245) are excluded from EVERY gas computation - budget, gauges, stage
        // drops. Filter once here so no downstream helper has to know about `enabled`; rig (vec)
        // order is preserved, so "stage 1/2" still means the first/second enabled Stage cylinder.
        // The ORIGINAL `cylinders` (all rows, order intact) is kept only for the per-row
        // `cylinder_usable_bars` display column, which zeroes disabled rows itself.
        let active: Vec<Cylinder> = cylinders.iter().filter(|c| c.enabled).cloned().collect();
        let active = active.as_slice();

        let topology = self.topology();
        let stations = topology.stations().to_vec();
        let n = stations.len();
        // The MEASURED positions (#34): per dense station, z without any line's hand move.
        let coords = dense_coords(topology, &self.raw_positions());

        let entrance_roots = || -> Vec<usize> {
            self.entrance_stations()
                .into_iter()
                .filter_map(|s| topology.dense(s))
                .collect()
        };
        // A set-but-unresolvable origin (e.g. its station was deleted while a dive plan stayed
        // loaded across a Survey edit, #244) falls back to the entrance rather than an
        // all-unreachable field - matching the `None` (entrance) case and the endpoint's own
        // deepest-in-budget fallback.
        let roots: Vec<usize> = match origin.and_then(|s| topology.dense(s)) {
            Some(i) => vec![i],
            None => entrance_roots(),
        };

        // In-leg cost: survey RMV bump on the slower survey-in speed (falls back to the swim
        // speed when no survey speed is set - symmetric). Depth from the measured coords.
        let in_rmv = diver.rmv_l_per_min * gas_config.survey_rmv_multiplier;
        let in_speed = gas_config
            .survey_speed_m_per_min
            .unwrap_or(gas_config.swim_speed_m_per_min);

        // Per-segment pace and propulsion (#317). `in_speed` is the default, so an empty segment
        // list reproduces the single-speed field exactly - the whole feature is inert until a
        // segment is edited. The RMV bump stays uniform: arguably a scootered span is not a
        // surveying span, but leaving the multiplier on can only ever RAISE gas, and lowering it
        // would extend reach on a change nobody asked for (§6).
        let plan_segments =
            resolve_segments(segments, waypoints.len(), in_speed, gas_config.dpv.as_ref());
        let scooter_limit = scooter_range_limit(gas_config.dpv.as_ref());

        // Defensive: a non-positive / non-finite in-speed OR in-rmv (e.g. an un-normalized
        // 0 survey_rmv_multiplier) would zero every edge cost and make the whole cave falsely
        // reachable - the unsafe over-stating direction (§6) - so bail to all-unreachable.
        //
        // `adj` is kept alongside the walk: its edge SET is identical to the plain topology
        // graph (same shots/closures, just different edge weights), so it doubles as the
        // turn-frontier's adjacency below - no second, purely-topological graph build. In the
        // degenerate branch `gas_in` is all-INFINITY, so `reachable` is all-false and the
        // turn-frontier's `reachable[u] && adj[u]...` filter short-circuits on `reachable[u]`
        // without ever reading `adj` - an empty adjacency list is provably equivalent, so build one
        // instead of paying for a real (unused) graph build.
        let degenerate = n == 0
            || roots.is_empty()
            || in_speed <= 0.0
            || !in_speed.is_finite()
            || in_rmv <= 0.0
            || !in_rmv.is_finite();
        // TWO aligned graphs, built once and reused for every segment.
        //
        // `adj` holds the speed- and RMV-INDEPENDENT gas basis `ata(avg_depth) x length x mult`
        // (see `edge_gas_basis`), so pricing a span is `basis x (rmv / speed)` - one scalar per
        // segment, applied inside Dijkstra. Rebuilding the graph per segment would be the obvious
        // alternative and costs a full O(E) walk per way point for no gain.
        //
        // `lens` holds the same edges' plain lengths and rides along the chosen path to give the
        // scootered distance and the runtime. It comes from the SAME builder, so it is aligned
        // edge-for-edge with `adj` by construction - which is what `dijkstra_scaled_with_riders`
        // requires, and why it indexes by position rather than by node pair.
        //
        // ⚠ `adj` is NOT a gas field any more; its weights are a basis. Anything reading it as
        // litres (the turn frontier reads only its edge SET, which is unchanged) must scale first.
        let adj = if degenerate {
            vec![Vec::new(); n]
        } else {
            self.survey_graph_weighted(|edge| {
                let shot = &edge.line.shots[edge.index];
                // A reference tie is not swimmable (#27 q7), so it must not carry a dive route at
                // all. Give it an INFINITE edge cost rather than skipping the edge: the edge set
                // must stay identical across weightings (the `lens` riders align by position). An
                // infinite weight never relaxes in Dijkstra, so the leg is unusable while the
                // ordering contract holds. Excluding it can only shrink reach, which is the safe
                // direction (§6).
                if !shot.kind.is_swum() && !gas_config.include_reference_legs {
                    return f64::INFINITY;
                }
                // Virtual legs burn gas at a higher RMV: tying in a jump is extra task-loading.
                // `normalize` keeps the multiplier ≥1.0, so this can only ever *raise* the gas -
                // never under-state it (the safe direction, §6).
                //
                // Keyed on `is_virtual`, not `== Jump`. A Reference only reaches this line when
                // `include_reference_legs` put it back in the graph, and that opt-in is documented
                // as restoring the pre-#27 inclusive behaviour - where every virtual leg carried
                // the multiplier. Charging it base RMV instead made an opted-in route cost LESS gas
                // than the same route did before the split, which overstates the reachable
                // frontier: the unsafe direction.
                let mult = if shot.kind.is_virtual() {
                    gas_config.jump_rmv_multiplier
                } else {
                    1.0
                };
                // The two stations' depths as shown on THIS shot's line (slot z, plan 3.5): a
                // legacy hand move's z on one line of a junction prices only that line's shots,
                // exactly as the per-slot coordinates did.
                let z_from = slot_coords(&coords, edge.from, edge.line, edge.from_station)[2];
                let z_to = slot_coords(&coords, edge.to, edge.line, edge.to_station)[2];
                edge_gas_basis(shot.length.max(0.0), z_from, z_to) * mult
            })
        };
        // Plain leg lengths, aligned with `adj`. An unswimmable leg keeps its real length here -
        // its `adj` weight is INFINITE so it is never relaxed, and the rider is only ever read on
        // an edge Dijkstra actually took.
        let lens = if degenerate {
            vec![Vec::new(); n]
        } else {
            self.survey_graph_weighted(|edge| edge.line.shots[edge.index].length.max(0.0))
        };

        // Ordered way points (#298), resolved to node indices. An entry that no longer resolves
        // (its station was deleted under a plan that stayed loaded across a Survey edit) or that is
        // graph-disconnected STALLS the walk: the mandated detour cannot be flown, so the field goes
        // all-unreachable rather than quietly planning a route that skips it (the unsafe direction,
        // §6). Unlike the origin and the endpoint - which are hints and degrade to the entrance /
        // deepest point - a way point is a constraint, so dropping one silently would change the
        // plan under the diver.
        let resolved_waypoints: Vec<Option<usize>> = waypoints
            .iter()
            .map(|w| w.and_then(|s| topology.dense(s)))
            .collect();
        let walk = waypoint_walk(
            &adj,
            &lens,
            &roots,
            &resolved_waypoints,
            &plan_segments,
            in_rmv,
            degenerate,
        );

        // The strategy actually applied (a stage-short staged request falls back to All-thirds; Free
        // and All-thirds never fall back). Drives the drops AND the gauges so they agree.
        let conservative = gas_config.conservative_tank_gas;
        let effective = effective_preset(active, gas_config.default_reserve);

        // Per-stage drop plans (safe IN gas, physical cap, default drop pressure) for the effective
        // strategy - the building blocks the override recompute (#246) reads. Empty for All-thirds
        // and any fell-back staged rig, so those strategies have no drops to override.
        let plans = stage_drop_plans(
            active,
            effective,
            gas_config.reserve_profile.stage_reserve_bar,
            conservative,
        );
        // Resolve each per-drop override (the station the diver dragged the marker to) to a node
        // index; an off-graph station falls through to the strategy default in `apply`. Only the
        // first `plans.len()` entries can ever affect a drop, so cap the (frontend-sized) input
        // there.
        let resolved_overrides: Vec<Option<usize>> = stage_drop_overrides
            .iter()
            .take(plans.len())
            .map(|o| o.and_then(|s| topology.dense(s)))
            .collect();

        // Base penetration budget (litres), then fold in the override deltas: the cumulative stage
        // thresholds, per-drop readout bars, over-reserve flags, and the budget change all come out of
        // one walk over the plans. `apply` returns the STRATEGY defaults untouched for an empty
        // override list, so this stays byte-identical to the no-override field.
        let base_budget = penetration_budget(
            active,
            gas_config.default_reserve,
            &gas_config.reserve_profile,
            conservative,
        );
        // The route target is a plain input lookup, independent of the budget.
        let endpoint_idx = endpoint.and_then(|s| topology.dense(s));
        // The un-overridden route: what the STRATEGY alone would fly, on the base budget. It serves
        // two purposes for the override walk, and BOTH need a route rather than the global field:
        //   - its drop positions are the "defaults" a release-in-place must reproduce, so a jiggle
        //     stays a true no-op (#246);
        //   - it is the basis a pinned drop is scored against. A drop is a position ALONG the route,
        //     so the gas it has cost is the route's cumulative gas at that position - NOT
        //     `gas_in[station]`, which is the cheapest way to reach that station after the way-point
        //     chain and corresponds to no pass the diver actually makes once a route doubles back
        //     (#298). Scoring against `gas_in` reads a return-leg drop as if it were made on the way
        //     in, which inflates the budget: the unsafe direction (§6).
        // Using the BASE route keeps the ordering acyclic - the final route depends on the budget,
        // which depends on these very overrides - and the two coincide whenever the delta is 0.
        let default_thresholds = cumulative(plans.iter().map(|p| p.safe_in_litres));
        let base_view = budget_view(&walk, endpoint_idx, base_budget, scooter_limit);
        let default_drop_nodes = default_drop_stations(&base_view, &default_thresholds);

        let (thresholds, stage_drop_bars, stage_drop_over_reserve, budget_delta) =
            apply_stage_drop_overrides(
                &plans,
                &resolved_overrides,
                &base_view,
                &walk,
                &default_drop_nodes,
                conservative,
            );
        // A pathological-but-valid cylinder (huge finite fill × volume) can overflow deliverable_gas
        // to a non-finite budget; treat that as all-unreachable (0.0) rather than leaking NaN/Inf to
        // the gauge readout. The staged paths already `.max(0.0)`-guard; this mirrors them. The
        // override budget is always `≥ back-gas ≥ 0` (every contribution is `≥ 0`), so the `.max(0.0)`
        // is only a finite-guard floor - a shallow drag can never drive the budget negative.
        let budget_litres = base_budget + budget_delta;
        let budget_litres = if budget_litres.is_finite() {
            budget_litres.max(0.0)
        } else {
            0.0
        };

        // Everything the final budget decides: the reachable mask, the reported gas-to-reach, and
        // the planned route origin -> way points -> target (populated for ANY strategy when
        // something is reachable). Per-stage drop points are placed on that route only where a
        // threshold exists (none for All-thirds / a fell-back staged rig / a Free rig with no stage
        // usable).
        let view = budget_view(&walk, endpoint_idx, budget_litres, scooter_limit);
        let BudgetView {
            reachable,
            can_end_at,
            gas_to_reach,
            max_reachable_gas,
            target: route_target,
            target_toward_endpoint: turn_toward_endpoint,
            route: route_nodes,
            route_gas,
            route_time,
            scooter_bound: turn_on_scooter_range,
        } = view;

        // Turn frontier: stations the diver could END at that border one they could not (out of
        // budget or in another component). Topology only - `adj`'s edge SET (built above) is
        // enough; the gas weights on it are irrelevant to "who borders whom".
        //
        // Keyed on `can_end_at`, NOT the overlay's `reachable`. A turn marker is a clickable
        // candidate endpoint, so the question it answers is "could the dive finish here", which
        // is the after-chain price - never the cheaper pass-through price the overlay paints.
        // The two masks are identical without way points.
        //
        // Stations on the committed chain are excluded outright: the diver has already said they
        // are going to the way points, so a station on the way there is transit, not a
        // destination. Picking one would route out past it to the way point and back.
        //
        // ⚠ That exclusion has to be settled across zero-cost ties, exactly like `reach_gas`. A
        // loop closure holds ONE physical point as two graph nodes joined at weight 0, and `chain`
        // is a predecessor-tree walk, so it names only the node the walk happened to pick. Marking
        // that node alone leaves its twin unexcluded, sitting at the same place with the same gas -
        // and the twin is usually the one carrying the side passage, so it borders the
        // out-of-budget ground and lands on the frontier. The result is a turn dot drawn on top of
        // a committed route station, which is the bug this exclusion exists to stop. (A junction
        // is one node on the station graph, so a merge no longer makes such a twin.)
        let mut on_chain = vec![false; n];
        let mut chain_queue: VecDeque<usize> = VecDeque::new();
        for &node in &walk.chain {
            if !on_chain[node] {
                on_chain[node] = true;
                chain_queue.push_back(node);
            }
        }
        while let Some(u) = chain_queue.pop_front() {
            for &(v, w) in &adj[u] {
                if w <= 0.0 && !on_chain[v] {
                    on_chain[v] = true;
                    chain_queue.push_back(v);
                }
            }
        }
        let turn_frontier: Vec<StationId> = (0..n)
            .filter(|&u| {
                can_end_at[u] && !on_chain[u] && adj[u].iter().any(|&(v, _)| !can_end_at[v])
            })
            .map(|u| stations[u])
            .collect();

        let route: Vec<StationId> = route_nodes.iter().map(|&i| stations[i]).collect();
        // Drop indices along the route (not stations): a way-point route can revisit a station,
        // so an index is the only unambiguous handle. No target means no plan, hence no drops.
        let drop_indices: Vec<usize> = if route_target.is_some() {
            place_stage_drops(&route_gas, &thresholds)
        } else {
            Vec::new()
        };

        // The legs: a leg is a real **swum** leg iff a shot joins its two stations (a shot,
        // including a reference/jump shot the diver swims); otherwise it is a loop-closure tie,
        // a zero-distance graph edge whose endpoints are the *same* surveyed point.
        let shot_between = self.shot_between_map();
        let route_shots: Vec<Option<ShotLocation>> = route_nodes
            .windows(2)
            .map(|w| {
                shot_between
                    .get(&(w[0].min(w[1]), w[0].max(w[1])))
                    .map(|&(line, index)| ShotLocation { line, index })
            })
            .collect();
        let leg_kinds = self.route_leg_kinds(&route_shots);
        let route_lines = route_node_lines(topology, &route, &route_shots);

        // Physical route metrics from the route's measured coordinates: the one-way swum line length
        // (a 3-D slope distance summed over real shot legs only), the deepest point, and the
        // distance-weighted average depth. This is the *swum* distance, NOT the on-screen highlight
        // length (a 2-D plan-view projection, and closure-adjusted in Plan) - the two intentionally
        // differ. Closure ties carry no geometry, so a loop closure's misclosure gap (never swum)
        // doesn't inflate the distance or skew the depth.
        let legs: Vec<LegEnds> = route_shots
            .iter()
            .enumerate()
            .map(|(i, shot)| {
                let shot = (*shot)?;
                let line = self.line(shot.line)?;
                let ends = [route_nodes[i], route_nodes[i + 1]]
                    .map(|node| slot_coords(&coords, node, line, stations[node]));
                Some(ends)
            })
            .collect();
        let depths: Vec<f64> = route_nodes
            .iter()
            .zip(&route_lines)
            .map(|(&node, line)| match line.and_then(|l| self.line(l)) {
                Some(line) => slot_coords(&coords, node, line, stations[node])[2],
                None => coords[node][2],
            })
            .collect();
        let (route_line_length_m, route_max_depth_m, route_avg_depth_m) =
            route_metrics(&depths, &legs);
        // Per-stage drop DISTANCE (metres, swum from origin), aligned with the drops, for the Plan
        // readout - same swum-distance basis as `route_line_length_m`. Read at the drop's route
        // INDEX, so a route that doubles back over a station reports the distance of the pass the
        // drop actually sits on.
        let cum_route_distance = cumulative_distances(route.len(), &legs);
        let stage_drop_distances_m: Vec<f64> = drop_indices
            .iter()
            .map(|&j| cum_route_distance.get(j).copied().unwrap_or(0.0))
            .collect();
        // The anticipated dive profile (#299). Built last, from the route vectors above, so it is a
        // pure re-presentation of numbers the compute has already committed to - except the return
        // leg, which is priced here at the exit-leg rates the reach field never uses.
        //
        // Gated on a target, exactly as `drop_indices` is above, and NOT merely on a non-empty
        // route. When a mandated way point costs more than the budget, the committed chain is
        // deliberately kept so the map can show the diver which way point blew the gas - but there
        // is no turnaround, and a profile is the specific claim "out to the turnaround and back".
        // Publishing one from the chain alone would hand the export a complete round-trip dive, with
        // a turn naming a station the diver cannot afford to reach, for a plan the engine has just
        // declared unflyable.
        //
        // ⚠ A target is necessary but NOT sufficient: the dive must actually go somewhere. Two ways
        // it might not, and the test is a positive SWUM DISTANCE because it is the only one that
        // catches both:
        // - **One station.** When only the origin is in budget - disable every cylinder and
        //   `budget_litres` is 0, which `can_end_at` satisfies at the origin for free, since
        //   reaching it costs nothing - `deepest` resolves to the origin itself. `path_to` always
        //   returns at least the target, so nothing below would have caught it.
        // - **Two stations, no metres.** A loop-closure tie holds one physical point as two graph
        //   nodes, so an endpoint on the origin's own twin gives a route of two stations that spans
        //   nothing: the tie leg has no geometry, so `cum_route_distance` stays 0. A station count
        //   passes this; swum distance does not.
        //
        // Either one renders as zero distance over zero duration, and the exporter would write a
        // syntactically valid Subsurface file describing a dive that never happened.
        let swum_route_m = cum_route_distance.last().copied().unwrap_or(0.0);
        let route_profile = if route_target.is_some() && swum_route_m > 0.0 {
            // A scootered span rides OUT at its own pace too: the mandatory backup makes the
            // ride home a modelled certainty (`DpvRig`), and the one-way range cap already
            // assumes the backup covers the same line back. Leg `i` joins route nodes `i` and
            // `i + 1` and belongs to the segment counted by the way points placed at or before
            // node `i` - the chain indices double as route indices, so this is the same
            // leg->segment mapping the walk priced the way in with. Swum spans keep the flat
            // swum exit.
            let placed: Vec<usize> = walk
                .waypoint_chain_idx
                .iter()
                .map_while(|idx| *idx)
                .collect();
            let scooter_speed_by_leg = (0..route.len().saturating_sub(1))
                .map(|i| {
                    let seg_idx = placed.iter().filter(|&&p| p <= i).count();
                    plan_segments
                        .get(seg_idx)
                        .filter(|s| s.on_scooter)
                        .map(|s| s.speed_m_per_min)
                })
                .collect();
            build_route_profile(
                &route,
                &depths,
                &leg_kinds,
                &cum_route_distance,
                &route_gas,
                &route_time,
                &ExitLegRates {
                    speed_m_per_min: gas_config.swim_speed_m_per_min,
                    rmv_l_per_min: diver.rmv_l_per_min,
                    jump_multiplier: gas_config.jump_rmv_multiplier,
                    scooter_speed_by_leg,
                },
            )
        } else {
            Vec::new()
        };

        // Scooter readouts (#317), read off the walk's riders AT THE ROUTE TARGET rather than
        // re-derived from the route's coordinates - the two would diverge the moment a route
        // doubled back over a station.
        //
        // ⚠ Reading `scoot_in`/`time_in` here is right for every target, INCLUDING one that sits on
        // the committed chain, and the reason is worth spelling out because the gas twin of this
        // line is not interchangeable. `route_gas` has to mix `chain_gas` and `gas_in` because it
        // reports a value per route node. These report only the TOTAL at the end of the route, and
        // `scoot_in[t] = cum_scoot + last_leg_scoot[t]` is exactly that total in all three cases:
        // the target is the final way point (last-leg cost 0, so the total is the chain's), an
        // earlier chain station (the route really does fly back to it, and the last-leg cost is
        // that return), or off-chain (chain plus tail). So no `chain_scoot` vector is needed HERE.
        // The profile is the other case: it reports a value per route node, so it needs the
        // per-node `chain_time` exactly as `route_gas` needs `chain_gas`.
        let scooter_distance_m = route_target.map_or(0.0, |t| {
            walk.scoot_in
                .get(t)
                .copied()
                .filter(|s| s.is_finite())
                .unwrap_or(0.0)
        });
        let route_runtime_min = route_target
            .and_then(|t| walk.time_in.get(t).copied())
            .filter(|t| t.is_finite());
        // The park point is the boundary after the last scootered segment, so it is a way point by
        // construction; the wire names it by the caller's ORIGINAL way-point slot.
        //
        // Gated on a target, exactly as `drop_indices` and `route_profile` are. The park point is
        // otherwise derived from the segment list ALONE, which is pure input - so a plan whose
        // way-point chain stalls (an unresolvable or disconnected way point, field all-unreachable)
        // would still hand back "leave the scooters at W1" for a dive the engine has just declared
        // unflyable. No turnaround, no route, nowhere to park.
        let scooter_drop_waypoint = route_target
            .and_then(|_| scooter_drop_index(&plan_segments))
            .filter(|&k| k < waypoints.len());
        let scooter_range_limit_m = gas_config.dpv.as_ref().map(DpvRig::usable_range_m);

        // Back-gas turn gauge - computed here so the UI never re-derives the budget math; honours the
        // conservative cap + effective strategy. Free averages the diver's per-tank turn directly.
        let margin = gas_config.reserve_profile.all_thirds_safety_margin_bar;
        let backgas_turn_bar = match effective {
            ReservePreset::Free => free_backgas_turn(active, conservative),
            ReservePreset::AllThirds => all_thirds_backgas_turn(active, margin, conservative),
            _ => backgas_turn_pressure(active, backgas_fraction(effective), conservative),
        };

        // Per-row "usable gas" column (#245) - over the ORIGINAL cylinders (all rows), so disabled
        // and non-budget rows read 0 and the frontend maps it 1:1 to the table by index.
        let cylinder_usable_bars = cylinder_usable_bars(
            cylinders,
            effective,
            gas_config.reserve_profile.stage_reserve_bar,
            margin,
            conservative,
        );

        GasReach {
            stations,
            gas_to_reach,
            reachable,
            turn_frontier,
            max_reachable_gas,
            budget_litres,
            final_turn: route_target.map(|t| topology.stations()[t]),
            route,
            route_lines,
            route_shots,
            route_leg_kinds: leg_kinds,
            turn_toward_endpoint,
            backgas_turn_bar,
            stage_drop_route_indices: drop_indices,
            stage_drop_bars,
            stage_drop_over_reserve,
            stage_drop_distances_m,
            cylinder_usable_bars,
            route_line_length_m,
            route_max_depth_m,
            route_avg_depth_m,
            waypoint_gas_litres: walk.waypoint_gas,
            waypoint_route_indices: walk.waypoint_chain_idx,
            route_profile,
            scooter_distance_m,
            scooter_range_limit_m,
            scooter_drop_waypoint,
            route_runtime_min,
            turn_on_scooter_range,
        }
    }

    /// The connecting shot's kind for each ordered route leg; `None` for a loop-closure tie
    /// (coincident points, never swum) or a leg whose shot cannot be found.
    ///
    /// One scan, shared by two consumers that need different slices of it: the route highlight
    /// dashes only `Jump` legs, while the return-leg gas pricing charges the jump RMV bump to every
    /// *virtual* leg, exactly as the in-leg graph does (a `Reference` only reaches the route at all
    /// when `include_reference_legs` put it back). Deriving one from the other would silently
    /// mis-price an opted-in reference leg.
    fn route_leg_kinds(&self, route_shots: &[Option<ShotLocation>]) -> Vec<Option<ShotKind>> {
        route_shots
            .iter()
            .map(|shot| {
                let shot = (*shot)?;
                self.line(shot.line)
                    .and_then(|line| line.shots.get(shot.index))
                    .map(|s| s.kind)
            })
            .collect()
    }
}

/// The line that names each route node: the line of the shot the route ARRIVES by; for the first
/// node, and for a node reached across a closure tie, the line of the shot it LEAVES by; for a
/// node with no swum leg on either side, the station's lowest line id (an order-independent
/// choice, so reordering the line list changes no result). Its slot on that line is the node's
/// wire address and its slot z the node's depth.
fn route_node_lines(
    topology: &Topology,
    route: &[StationId],
    route_shots: &[Option<ShotLocation>],
) -> Vec<Option<LineId>> {
    route
        .iter()
        .enumerate()
        .map(|(i, &station)| {
            let arriving = i
                .checked_sub(1)
                .and_then(|leg| route_shots.get(leg).copied().flatten());
            let departing = route_shots.get(i).copied().flatten();
            arriving
                .or(departing)
                .map(|shot| shot.line)
                .or_else(|| topology.lines_of(station).into_iter().min())
        })
        .collect()
}
