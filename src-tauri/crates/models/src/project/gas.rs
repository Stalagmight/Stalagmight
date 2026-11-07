//! Gas & route-planning engine (issue #38).
//!
//! Phase 0 shipped the pure foundations ([`physics`], [`model`]). Phase 1 adds the
//! gas-weighted reachability compute here as an `impl Project` method, reaching the
//! `pub(super)` graph primitives (`build_survey_graph_weighted` / `dijkstra` /
//! `zero_datum_roots`) exactly as the sibling `distance` module does. The engine consumes
//! the **instant measured** coords (`compute_station_coordinates`) - reachability is
//! coordinate-independent, so it never triggers the slow closure-adjusted solve. The engine is
//! called by the `commands::gas` IPC command via `AppState::gas_reach_field`.

mod model;
mod physics;
mod pinned;
mod range_route;
mod scooter;

#[cfg(test)]
mod half_minus_gauge_tests;
#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "the printed plans are the evidence a run captures"
)]
mod pin_decides_route_tests;
#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "the printed sweep counts are the evidence a run captures"
)]
mod pin_monotone_tests;
#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "the printed sweep counts are the evidence a run captures"
)]
mod pin_route_tests;
#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "the printed sweep counts are the evidence a run captures"
)]
mod route_sweep_tests;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;

use crate::config::{GasConfig, ReservePreset, ReserveProfile};
use crate::{Coordinates, ShotKind, StationKey, StationRef};

use super::Project;
pub use model::{
    CylRole, Cylinder, Diver, GasReachField, PlanMode, PlanSegment, RouteLeg, RouteProfilePoint,
    TankMaterial, Team,
};
use physics::{deliverable_gas, edge_gas, edge_gas_basis};
use range_route::{RangedTail, Step};
use scooter::{resolve_segments, scooter_drop_index, scooter_range_limit, ResolvedSegment};
/// The bounds a stored per-span speed must fall inside to be honoured (outside them the span
/// inherits the plan speed). Re-exported because the `.plnx` merge and the frontend's own segment
/// editor have to agree with the engine about what a usable speed is - a second copy of the numbers
/// is exactly how a stored plan starts resolving differently from the plan the diver saw.
pub use scooter::{MAX_SEGMENT_SPEED_M_PER_MIN, MIN_SEGMENT_SPEED_M_PER_MIN};

impl Project {
    /// Single-diver open-circuit reachability field (PRD §4/§5.1, the Phase-1 headline).
    ///
    /// Seeds a gas-weighted Dijkstra from `origin` (or the zero datums when `None`) over the
    /// survey graph, weighting each shot by the **in-leg** gas it costs
    /// (`edge_gas(rmv·survey_mult, in_speed, length, avg_depth)`), then marks every station whose
    /// one-way gas-to-reach is within the **turn budget** ([`penetration_budget`] for the active
    /// strategy; the diver holds the rest for the exit). The turn frontier is the reachable
    /// stations bordering an unreachable one. Under a staged (Half-Minus) strategy it also walks
    /// the route to `endpoint` (see [`route_target`] for the out-of-budget rule) to place drops,
    /// and computes the readout gauges; All-thirds leaves those empty.
    ///
    /// Contract: `gas_config` is assumed normalized (`GasConfig::normalize` - finite/positive
    /// speeds); a non-positive in-speed defensively yields an all-unreachable field rather
    /// than dividing by zero.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no-override convenience entry; prod calls _with_overrides, tests call this"
        )
    )]
    pub(crate) fn compute_gas_reach_field(
        &self,
        origin: Option<&StationRef>,
        endpoint: Option<&StationRef>,
        diver: &Diver,
        cylinders: &[Cylinder],
        gas_config: &GasConfig,
    ) -> GasReachField {
        self.compute_gas_reach_field_with_overrides(
            origin,
            endpoint,
            &[],
            &[],
            diver,
            cylinders,
            gas_config,
            &[],
        )
    }

    /// As [`Self::compute_gas_reach_field`], but with per-stage **drop overrides** (#246): the diver
    /// drags a stage's ▼ drop marker to a route station, and `stage_drop_overrides[k]` pins stage
    /// `k`'s drop there (aligned 1:1 with `stage_drop_refs` / `stage_drop_bars`; `None` = keep the
    /// strategy default; over-long / unresolvable refs fall through to the default).
    ///
    /// A pinned drop redefines how much of that stage is breathed IN: the litres consumed to reach
    /// the pinned station (its route gas minus the cumulative at the prior drop), hard-clamped to
    /// `[0, tank-empty]`. The penetration budget becomes `base + Σ(min(breathed, safe) - safe)`, so
    /// a drop dragged **shallower** strands surplus stage gas (budget shrinks → reach shortens) and
    /// one dragged **deeper** is shown where it was put, flagged `stage_drop_over_reserve`, but buys
    /// no reach (#576): no plan reaches further than the same plan without its moved drops. The
    /// pin decides the route (#576 RV8c-2): the dive passes it, doubling back to the endpoint if
    /// the pin is off its passage, and a dive whose gas runs out first turns on the way there and
    /// reports the drop as not reached (`gas/pinned.rs`). The reach overlay of a pinned plan shows
    /// only where that dive goes: each station priced at its pass on the route through the pins, or
    /// after passing them, as a way point prices it (RV8c-2-D4). Non-overridden
    /// downstream drops keep their safe pressure but shift position as the cumulative shifts; the
    /// back-gas turn pressure is independent of the stage drops and is unchanged. The reach math stays
    /// exact-litres (from the graph); only the moved drop's readout pressure is inverted from the Z
    /// curve. With an empty `stage_drop_overrides` this is byte-identical to the no-override field.
    ///
    /// `waypoints` (#298) are ordered intermediate route points the dive MUST visit, in order,
    /// between the origin and the endpoint. They re-base the field: a station's gas is the gas to
    /// reach it AFTER flying the whole way-point chain, so the reachable overlay, the turn frontier
    /// and the stage drops all cost the mandated detour instead of quietly planning the shortest
    /// origin-to-endpoint line.
    ///
    /// That re-basing is not uniform, and the distinction is the load-bearing part of the design:
    /// a station the committed chain SWIMS THROUGH is reported at its pass-through cost, not at the
    /// after-chain cost of returning to it (see [`WaypointWalk::reach_gas`]). Only the destination
    /// question - can the dive END here, which drives the turn frontier and the route target - uses
    /// the after-chain price throughout.
    ///
    /// With an empty `waypoints` the chain is empty, the two prices coincide and the field is the
    /// plain single-Dijkstra one. ⚠ It is NOT byte-identical to the pre-#298 engine even so: the
    /// out-of-budget-endpoint rule changed for every plan, with or without way points (see
    /// [`route_target`]).
    ///
    /// `segments` (#317) gives each route span its own speed and says whether it is flown on a
    /// scooter, one entry per span (`waypoints.len() + 1` of them; a short or over-long list is
    /// reconciled by [`resolve_segments`]). An EMPTY list prices every span at the config speed with
    /// no scooter - the pre-#317 field, arithmetically equal within float re-association (the edge
    /// weight is now a speed-free basis scaled at relax time) - so the feature is inert until a
    /// segment is edited.
    ///
    /// A DPV rig on the config adds a SECOND constraint on top of the gas budget: the scootered
    /// line to a station must be within [`DpvRig::usable_range_m`]. It never touches the gas half.
    /// The exit mirrors the entry span for span - you collect the units where you parked them - so
    /// `gas_out == gas_in` and the fraction reserves stay correct as they are. What a metre of
    /// scootered line COSTS against that cap depends on the span's speed, since the cap counts
    /// metres at cruise while each unit's battery runs by its own clock (see
    /// [`scooter::range_per_metre`]).
    pub fn compute_gas_reach_field_with_overrides(
        &self,
        origin: Option<&StationRef>,
        endpoint: Option<&StationRef>,
        waypoints: &[StationRef],
        segments: &[PlanSegment],
        diver: &Diver,
        cylinders: &[Cylinder],
        gas_config: &GasConfig,
        stage_drop_overrides: &[Option<StationRef>],
    ) -> GasReachField {
        self.gas_reach_field_and_chain(
            origin,
            endpoint,
            waypoints,
            segments,
            diver,
            cylinders,
            gas_config,
            stage_drop_overrides,
        )
        .0
    }

    /// [`Self::compute_gas_reach_field_with_overrides`], with the route the plan commits before
    /// its open tail: the stations of the walk through the way points and, for a pinned plan, the
    /// pins after them. The field does not report it (a dive that turns before a pin shows only
    /// part of it), and it is what tells a change that sends a leg another way (#576
    /// NF-W12GAS-1-D2, a lower scooter range) from one that only moves the turn along it - the
    /// pin sweeps compare it.
    pub(crate) fn gas_reach_field_and_chain(
        &self,
        origin: Option<&StationRef>,
        endpoint: Option<&StationRef>,
        waypoints: &[StationRef],
        segments: &[PlanSegment],
        diver: &Diver,
        cylinders: &[Cylinder],
        gas_config: &GasConfig,
        stage_drop_overrides: &[Option<StationRef>],
    ) -> (GasReachField, Vec<StationRef>) {
        // Disabled tanks (#245) are excluded from EVERY gas computation - budget, gauges, stage
        // drops. Filter once here so no downstream helper has to know about `enabled`; rig (vec)
        // order is preserved, so "stage 1/2" still means the first/second enabled Stage cylinder.
        // The ORIGINAL `cylinders` (all rows, order intact) is kept only for the per-row
        // `cylinder_usable_bars` display column, which zeroes disabled rows itself.
        let active: Vec<Cylinder> = cylinders.iter().filter(|c| c.enabled).cloned().collect();
        let active = active.as_slice();

        let stations = self.compute_station_coordinates();
        let n = stations.len();
        let station_refs: Vec<StationRef> = stations.iter().map(|s| s.station_ref).collect();
        let station_index = Self::station_index_map(&stations);
        let coords: Vec<Coordinates> = stations.iter().map(|s| s.coordinates).collect();

        let roots: Vec<usize> = match origin {
            Some(r) => {
                let key: StationKey = (*r).into();
                self.resolve_key(&key)
                    .and_then(|sr| station_index.get(&sr).copied())
                    .map_or_else(
                        // A set-but-unresolvable origin (e.g. its station was deleted while a
                        // dive plan stayed loaded across a Survey edit, #244) falls back to the
                        // entrance rather than an all-unreachable field - matching the `None`
                        // (entrance) case and the endpoint's own deepest-in-budget fallback.
                        || self.zero_datum_roots(&station_index),
                        |i| vec![i],
                    )
            }
            None => self.zero_datum_roots(&station_index),
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

        let coords_ref = &coords;
        // Defensive: a non-positive / non-finite in-speed OR in-rmv (e.g. an un-normalized
        // 0 survey_rmv_multiplier) would zero every edge cost and make the whole cave falsely
        // reachable - the unsafe over-stating direction (§6) - so bail to all-unreachable.
        //
        // `adj` is kept alongside the walk: its edge SET is identical to the plain topology
        // graph `build_survey_graph` would return (same shots/merges/closures, just different
        // edge weights), so it doubles as the turn-frontier's adjacency below - no second,
        // purely-topological graph build. In the degenerate branch `gas_in` is all-INFINITY, so
        // `reachable` is all-false and the turn-frontier's `reachable[u] && adj[u]...` filter
        // short-circuits on `reachable[u]` without ever reading `adj` - an empty adjacency list
        // is provably equivalent, so build one instead of paying for a real (unused) graph build.
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
            self.build_survey_graph_weighted(n, &station_index, |shot, fi, ti| {
                // A reference tie is not swimmable (#27 q7), so it must not
                // carry a dive route at all. Give it an INFINITE edge cost
                // rather than skipping the edge: `along_line_between` keys a
                // parallel shot-edge map on this function's edge-emission
                // order, so the edge set must stay identical. An infinite
                // weight never relaxes in Dijkstra, so the leg is unusable
                // while the ordering contract holds. Excluding it can only
                // shrink reach, which is the safe direction (§6).
                if !shot.kind.is_swum() && !gas_config.include_reference_legs {
                    return f64::INFINITY;
                }
                // Virtual legs burn gas at a higher RMV: tying in a jump is
                // extra task-loading. `normalize` keeps the multiplier ≥1.0, so
                // this can only ever *raise* the gas - never under-state it (the
                // safe direction, §6).
                //
                // Keyed on `is_virtual`, not `== Jump`. A Reference only reaches
                // this line when `include_reference_legs` put it back in the
                // graph, and that opt-in is documented as restoring the
                // pre-#27 inclusive behaviour - where every virtual leg carried
                // the multiplier. Charging it base RMV instead made an opted-in
                // route cost LESS gas than the same route did before the split,
                // which overstates the reachable frontier: the unsafe direction.
                let mult = if shot.kind.is_virtual() {
                    gas_config.jump_rmv_multiplier
                } else {
                    1.0
                };
                edge_gas_basis(shot.length.max(0.0), coords_ref[fi].z, coords_ref[ti].z) * mult
            })
        };
        // Plain leg lengths, aligned with `adj`. An unswimmable leg keeps its real length here -
        // its `adj` weight is INFINITE so it is never relaxed, and the rider is only ever read on
        // an edge Dijkstra actually took.
        let lens = if degenerate {
            vec![Vec::new(); n]
        } else {
            self.build_survey_graph_weighted(n, &station_index, |shot, _fi, _ti| {
                shot.length.max(0.0)
            })
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
            .map(|r| {
                let key: StationKey = (*r).into();
                self.resolve_key(&key)
                    .and_then(|sr| station_index.get(&sr).copied())
            })
            .collect();
        let walk = waypoint_walk(
            &adj,
            &lens,
            &roots,
            &resolved_waypoints,
            &plan_segments,
            in_rmv,
            scooter_limit,
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
        // Resolve each per-drop override (a StationRef the diver dragged the marker to) to a station
        // index; an unresolvable / off-graph ref falls through to the strategy default in `apply`.
        // Only the first `plans.len()` entries can ever affect a drop, so cap the (frontend-sized)
        // input there - a stray-long list can't cost unbounded `resolve_key` scans.
        let resolved_overrides: Vec<Option<usize>> = stage_drop_overrides
            .iter()
            .take(plans.len())
            .map(|o| {
                o.and_then(|r| {
                    let key: StationKey = r.into();
                    self.resolve_key(&key)
                        .and_then(|sr| station_index.get(&sr).copied())
                })
            })
            .collect();

        // Base penetration budget (litres): what the strategy alone credits, before any pin.
        let base_budget = penetration_budget(
            active,
            gas_config.default_reserve,
            &gas_config.reserve_profile,
            conservative,
        );
        // The route target is a plain input lookup, independent of the budget.
        let endpoint_idx = endpoint.and_then(|r| {
            let key: StationKey = (*r).into();
            self.resolve_key(&key)
                .and_then(|sr| station_index.get(&sr).copied())
        });
        // [#576 RV8c-2-D1, decided "pin decides the route"] A pinned drop is a point the dive
        // passes, like a way point: the route flies the way points, then every pin they do not
        // already pass, in drop order, then on to the endpoint - or, with none, to the deepest
        // station beyond the last pin. `None` when no stage is pinned (or no pin resolves to a
        // station the walk can reach): the plan is then the strategy's own, byte for byte.
        //
        // The pinned route does not depend on the budget - only where the dive TURNS on it does -
        // so the gas each pin has cost is read off it before the budget exists, and the ordering
        // stays acyclic: route, then arrival gas, then budget, then the turn.
        let pinned = pinned::pinned_walk(
            &adj,
            &lens,
            &roots,
            &resolved_waypoints,
            &walk,
            &resolved_overrides,
            &plan_segments,
            in_rmv,
            scooter_limit,
        );
        let no_pins = vec![None; plans.len()];
        let (pin_chain_idx, pinned_chain_gas): (&[Option<usize>], &[f64]) = match &pinned {
            Some(p) => (&p.pin_chain_idx, &p.walk(&walk).chain_gas),
            None => (&no_pins, &[]),
        };
        // Fold the pins into the drops: the cumulative stage thresholds, per-drop readout bars,
        // over-reserve flags and the budget change all come out of one walk over the plans. With no
        // pin this returns the STRATEGY defaults untouched, so the field stays byte-identical to
        // the no-override one.
        let (thresholds, stage_drop_bars, over_at_pin, budget_delta) =
            price_stage_drops(&plans, pin_chain_idx, pinned_chain_gas);
        // A pathological-but-valid cylinder (huge finite fill × volume) can overflow deliverable_gas
        // to a non-finite budget; treat that as all-unreachable (0.0) rather than leaking NaN/Inf to
        // the gauge readout. The staged paths already `.max(0.0)`-guard; this mirrors them. The
        // pinned budget is always `≥ back-gas ≥ 0` (every contribution is `≥ 0`), so the `.max(0.0)`
        // is only a finite-guard floor - a shallow pin can never drive the budget negative - and
        // never above `base_budget` (each credit is capped at the stage's safe IN gas), so no pin
        // can reach further than the plan without it (#576).
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
        //
        // A pinned plan's masks, route, turnaround and drop markers all come from the pinned walk
        // ([`pinned::pinned_view`]): the overlay paints only where the dive through the pins can be,
        // each station priced at its pass or after the pins, as a way point prices it (#576
        // RV8c-2-D4) - a pin that strands gas shrinks it, and no pin can grow it on gas. That
        // includes the dive that runs out of gas before a pin (#576 RV8c-2-D3): it turns where the
        // gas says on the way there, and the drop it never reaches is reported past the turnaround
        // (`full_route` keeps the stations up to the pin for exactly that). The scooter range is
        // priced on the same walk, so every station the pinned dive passes or turns at is painted
        // (#576 RV9a-1).
        //
        // The plan without pins prices each station on a route its scooter can fly (#576
        // NF-W11GAS-1-D1, "price along the route"): round a loop the gas-cheapest way can run out of
        // range while another way is in range on gas the diver has, and the dive takes that way.
        // Any station the pinned walk paints is reached by a route that plan considers, on no more
        // gas and no more range, so no pin paints a station the plan without pins leaves dark.
        let ranged = range_route::ranged_tail(
            &adj,
            &lens,
            &roots,
            &walk,
            &plan_segments,
            in_rmv,
            scooter_limit,
        );
        let plain_view = budget_view(
            &walk,
            ranged.as_ref(),
            endpoint_idx,
            budget_litres,
            scooter_limit,
            None,
        );
        let (view, full_route) = match &pinned {
            Some(p) => {
                // The pinned dive's tail, from its last pin on, takes the ways its range covers
                // as the plan without pins' does (#576 NF-W12GAS-1-D2).
                let pinned_tail = p.ranged_tail(
                    &adj,
                    &lens,
                    &roots,
                    &walk,
                    &plan_segments,
                    in_rmv,
                    scooter_limit,
                );
                pinned::pinned_view(
                    p,
                    &walk,
                    pinned_tail.as_ref(),
                    plain_view,
                    endpoint_idx,
                    budget_litres,
                    scooter_limit,
                )
            }
            None => (plain_view, None),
        };
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
            turn_scoot,
            turn_time,
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
        // destination. Picking one would route out past it to the way point and back. A pinned
        // plan's chain runs on through its pins (#576 RV8c-2-D4), so the way to a pin is transit
        // too: picking a station on it routes out to the pin and back (D2).
        //
        // ⚠ That exclusion has to be settled across zero-cost ties, exactly like `reach_gas`. A
        // merge or an aligned closure holds ONE physical station as several graph nodes joined at
        // weight 0, and `chain` is a predecessor-tree walk, so it names only the node the walk
        // happened to pick. Marking that node alone leaves its twin unexcluded, sitting at the same
        // place with the same gas - and the twin is usually the one carrying the side passage, so
        // it borders the out-of-budget ground and lands on the frontier. The result is a turn dot
        // drawn on top of a committed route station, which is the bug this exclusion exists to stop.
        let flown = pinned.as_ref().map_or(&walk, |p| p.walk(&walk));
        let committed: Vec<StationRef> = flown.chain.iter().map(|&i| station_refs[i]).collect();
        let mut on_chain = vec![false; n];
        let mut chain_queue: VecDeque<usize> = VecDeque::new();
        for &node in &flown.chain {
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
        let turn_frontier: Vec<StationRef> = (0..n)
            .filter(|&u| {
                can_end_at[u] && !on_chain[u] && adj[u].iter().any(|&(v, _)| !can_end_at[v])
            })
            .map(|u| station_refs[u])
            .collect();

        let route_refs: Vec<StationRef> = route_nodes.iter().map(|&i| station_refs[i]).collect();
        let final_turn_ref = route_target.map(|i| station_refs[i]);
        // The stations a drop marker can stand on: the route, then - only when the dive turns
        // before a pin (#576 RV8c-2-D3) - the rest of the way to the last pin, so a drop the dive
        // never reaches is still named at the station it was pinned to, past the turnaround.
        let (beyond_nodes, beyond_gas) = full_route.unwrap_or_default();
        let marker_refs: Vec<StationRef> = route_refs
            .iter()
            .copied()
            .chain(beyond_nodes.iter().map(|&i| station_refs[i]))
            .collect();
        // Drop indices along those stations (not refs): a way-point route can revisit a station,
        // so an index is the only unambiguous handle. No target means no plan, hence no drops.
        let drop_indices: Vec<usize> = if route_target.is_some() {
            place_stage_drops(&route_gas, &beyond_gas, &thresholds, pin_chain_idx)
        } else {
            Vec::new()
        };
        let stage_drop_refs: Vec<StationRef> = drop_indices
            .iter()
            .filter_map(|&j| marker_refs.get(j).copied())
            .collect();
        // `place_stage_drops` only ever returns in-range indices, so the filter_map above cannot
        // drop one. Assert it loudly rather than let a future change silently shorten
        // `stage_drop_refs` and misalign it from the per-stage gauges and distances, which are
        // built from `drop_indices` unfiltered.
        debug_assert_eq!(
            stage_drop_refs.len(),
            drop_indices.len(),
            "a stage drop fell off the route - refs would misalign with the gauges"
        );
        // A drop past the turnaround is one the dive never reaches: the stage is carried the
        // whole dive, so nothing is breathed into its reserve at the pin - the marker reads "not
        // reached", never "over reserve".
        let stage_drop_over_reserve: Vec<bool> = over_at_pin
            .iter()
            .zip(&drop_indices)
            .map(|(&over, &j)| over && j < route_refs.len())
            .collect();
        let leg_kinds = self.route_leg_kinds(&route_refs);
        let route = route_legs(&route_refs, &leg_kinds);

        // Physical route metrics from the route's measured coordinates: the one-way swum line length
        // (a 3-D slope distance summed over real shot legs only - see `is_swim_leg`), the deepest
        // point, and the distance-weighted average depth. This is the *swum* distance, NOT the
        // on-screen highlight length (a 2-D plan-view projection, and closure-adjusted in Plan) -
        // the two intentionally differ. `route_swim` excludes zero-distance merge/closure ties so a
        // loop closure's misclosure gap (never swum) doesn't inflate the distance or skew the depth.
        let marker_pts: Vec<Coordinates> = marker_refs
            .iter()
            .filter_map(|r| station_index.get(r).map(|&i| coords[i]))
            .collect();
        let marker_swim: Vec<bool> = marker_refs
            .windows(2)
            .map(|w| is_swim_leg(&w[0], &w[1]))
            .collect();
        // marker_pts (one coord per ref) and marker_swim (one flag per leg) must stay aligned, or
        // route_metrics would apply a leg's flag to the wrong segment. The filter_map above can only
        // drop a ref absent from station_index - impossible today (the refs come from station_refs,
        // which are station_index's keys) - so assert it loudly rather than silently mis-weighting if
        // a future graph change ever introduces a ref that isn't in the index.
        debug_assert_eq!(
            marker_pts.len(),
            marker_refs.len(),
            "route_pts dropped a route ref - the swim mask would misalign"
        );
        let route_pts = &marker_pts[..route_refs.len().min(marker_pts.len())];
        let route_swim = &marker_swim[..route_refs.len().saturating_sub(1).min(marker_swim.len())];
        let (route_line_length_m, route_max_depth_m, route_avg_depth_m) =
            route_metrics(route_pts, route_swim);
        // Per-stage drop DISTANCE (metres, swum from origin), aligned with `stage_drop_refs`, for the
        // Plan readout - same swum-distance basis as `route_line_length_m`. Read at the drop's route
        // INDEX, so a route that doubles back over a station reports the distance of the pass the
        // drop actually sits on; a drop past the turnaround reads the distance along the way to it.
        let cum_marker_distance = cumulative_distances(&marker_pts, &marker_swim);
        let cum_route_distance = &cum_marker_distance[..route_pts.len()];
        let stage_drop_distances_m: Vec<f64> = drop_indices
            .iter()
            .map(|&j| cum_marker_distance.get(j).copied().unwrap_or(0.0))
            .collect();
        // The same drop indices the distances above are read at, published so a profile marker can
        // be drawn on the right PASS of a route that doubles back (#299). `stage_drop_refs` names
        // only the station, which is ambiguous there. An index past the route's last station is a
        // drop the dive does not reach (#576 RV8c-2-D3).
        let stage_drop_route_indices: Vec<u32> = drop_indices
            .iter()
            .map(|&j| u32::try_from(j).unwrap_or(u32::MAX))
            .collect();
        let waypoint_route_indices: Vec<Option<u32>> = walk
            .waypoint_chain_idx
            .iter()
            .map(|idx| idx.map(|j| u32::try_from(j).unwrap_or(u32::MAX)))
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
        // - **Two stations, no metres.** A merge / loop-closure tie holds one physical point as two
        //   graph nodes, so an endpoint on the origin's own twin gives a route of two stations that
        //   spans nothing: `route_swim` is false for the tie, so `cum_route_distance` stays 0. A
        //   station count passes this; swum distance does not.
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
            let scooter_speed_by_leg = (0..route_refs.len().saturating_sub(1))
                .map(|i| {
                    let seg_idx = placed.iter().filter(|&&p| p <= i).count();
                    plan_segments
                        .get(seg_idx)
                        .filter(|s| s.on_scooter)
                        .map(|s| s.speed_m_per_min)
                })
                .collect();
            build_route_profile(
                &route_refs,
                route_pts,
                &leg_kinds,
                cum_route_distance,
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
        // doubled back over a station. The view carries them ([`BudgetView::turn_scoot`]): the
        // walk's after-chain totals for every target the plain field picks, the chain's own totals
        // for a pinned dive that turns before its pin.
        let scooter_distance_m = route_target.map_or(0.0, |_| turn_scoot);
        let route_runtime_min = route_target.and(turn_time);
        // The park point is the boundary after the last scootered segment, so it is a way point by
        // construction. Resolving it through the ORIGINAL `waypoints` (not the graph) keeps it a
        // station the diver named.
        //
        // Gated on a target, exactly as `drop_indices` and `route_profile` are. The park point is
        // otherwise derived from the segment list ALONE, which is pure input - so a plan whose
        // way-point chain stalls (an unresolvable or disconnected way point, field all-unreachable)
        // would still hand back "leave the scooters at W1" for a dive the engine has just declared
        // unflyable. No turnaround, no route, nowhere to park.
        let scooter_drop_ref = route_target
            .and_then(|_| scooter_drop_index(&plan_segments))
            .and_then(|k| waypoints.get(k).copied());
        let scooter_range_limit_m = gas_config.dpv.as_ref().map(|rig| rig.usable_range_m());

        // Back-gas turn gauge - computed here so the UI never re-derives the budget math. It reads
        // the same per-cylinder gauge rule as the budget (`in_usable_bar`), so the turn the diver
        // obeys and the reach the map draws describe the same gas.
        let profile = &gas_config.reserve_profile;
        let backgas_turn_bar = backgas_turn_bar(
            active,
            &in_usable_bars(active, effective, profile, conservative),
        );

        // Per-row "usable gas" column (#245) - over the ORIGINAL cylinders (all rows), so disabled
        // and non-budget rows read 0 and the frontend maps it 1:1 to the table by index. Each entry
        // is `fill - turn/drop`, the bar the budget credits that tank.
        let cylinder_usable_bars = in_usable_bars(cylinders, effective, profile, conservative);

        let field = GasReachField {
            station_refs,
            gas_to_reach,
            reachable,
            turn_frontier,
            max_reachable_gas,
            budget_litres,
            route,
            stage_drop_refs,
            final_turn_ref,
            turn_toward_endpoint,
            backgas_turn_bar,
            stage_drop_bars,
            stage_drop_over_reserve,
            stage_drop_distances_m,
            cylinder_usable_bars,
            route_line_length_m,
            route_max_depth_m,
            route_avg_depth_m,
            waypoint_gas_litres: walk.waypoint_gas,
            route_profile,
            stage_drop_route_indices,
            waypoint_route_indices,
            scooter_distance_m,
            scooter_range_limit_m,
            scooter_drop_ref,
            route_runtime_min,
            turn_on_scooter_range,
        };
        (field, committed)
    }

    /// The connecting shot's kind for each ordered route leg; `None` for a cross-section tie (a
    /// merge / loop closure - coincident points, never swum) or a leg whose shot cannot be found.
    ///
    /// One scan, shared by two consumers that need different slices of it: the route highlight
    /// dashes only `Jump` legs, while the return-leg gas pricing charges the jump RMV bump to every
    /// *virtual* leg, exactly as the in-leg graph does (a `Reference` only reaches the route at all
    /// when `include_reference_legs` put it back). Deriving one from the other would silently
    /// mis-price an opted-in reference leg.
    fn route_leg_kinds(&self, route_refs: &[StationRef]) -> Vec<Option<ShotKind>> {
        route_refs
            .windows(2)
            .map(|w| {
                let (from, to) = (w[0], w[1]);
                if !is_swim_leg(&from, &to) {
                    return None;
                }
                self.sections
                    .iter()
                    .find(|s| s.id == from.section_id)
                    .and_then(|s| {
                        s.shots
                            .get(from.station_index.min(to.station_index) as usize)
                    })
                    .map(|shot| shot.kind)
            })
            .collect()
    }
}

/// Build the route-highlight legs from the ordered route stations, flagging each leg whose
/// connecting shot is a reference/jump (a normally-hidden swum shortcut) so the UI can dash it.
/// A cross-section tie (a merge / loop closure - coincident points) is not a jump.
fn route_legs(route_refs: &[StationRef], leg_kinds: &[Option<ShotKind>]) -> Vec<RouteLeg> {
    route_refs
        .windows(2)
        .enumerate()
        .map(|(i, w)| RouteLeg {
            from: w[0],
            to: w[1],
            is_jump: leg_kinds.get(i).copied().flatten() == Some(ShotKind::Jump),
        })
        .collect()
}

/// Reconstruct the shortest path from a predecessor tree's root to `target`, root first. `pred` is
/// acyclic by construction; the node-count cap is a defensive stop so a corrupt tree can never hang
/// the compute.
fn path_to(pred: &[usize], target: usize) -> Vec<usize> {
    let mut chain = vec![target];
    let mut cur = target;
    while pred.get(cur).copied().unwrap_or(usize::MAX) != usize::MAX {
        cur = pred[cur];
        chain.push(cur);
        if chain.len() > pred.len() {
            break;
        }
    }
    chain.reverse();
    chain
}

/// Result of walking the ordered way points (#298) - the gas field the rest of the engine reads,
/// plus the part of the route the way points have already fixed.
struct WaypointWalk {
    /// Gas-to-reach every station AFTER visiting every way point in order: the last leg's Dijkstra
    /// distance offset by the gas already spent getting to the last way point. All-infinite when the
    /// chain stalls, so nothing reads as reachable on a route that cannot be flown.
    ///
    /// This is the cost of being at a station as the route's FINAL destination, which is what the
    /// route tail and the post-chain stage-drop basis need. It is NOT what the overlay shows - see
    /// [`WaypointWalk::reach_gas`].
    gas_in: Vec<f64>,
    /// What the OVERLAY reports: `gas_in`, but lowered to the pass-through gas at any station the
    /// committed chain actually swims through (then settled across zero-cost merge/closure ties).
    ///
    /// The two differ only with way points, and only for chain stations. `gas_in` prices a
    /// pass-through as an out-and-back detour from the last way point, so the entrance itself would
    /// cost 2 x d(origin, W1), read as out of budget, and put a spurious turn-frontier dot in the
    /// middle of the diver's own route. They must stay separate: on a route that doubles back, a
    /// revisited station is passed cheaply the first time and expensively the second, and the route's
    /// own cumulative gas has to keep the higher number or the stage drops slide.
    reach_gas: Vec<f64>,
    /// Scooter RANGE (metres) spent reaching every station after the whole chain, accumulated along
    /// the SAME gas-optimal path as `gas_in` - the destination price, the analogue of `gas_in`.
    /// Capped against the rig's usable range to decide whether the dive can END here. Equal to the
    /// scootered line's length where the weaker unit is ridden at or above its cruise and binds;
    /// larger otherwise, where a metre drains one unit's clock faster than the cap's metres assume
    /// (`scooter::range_per_metre`). The plan without pins prices its open tail on the routes its
    /// scooter can fly instead ([`range_route`], #576 NF-W11GAS-1-D1); a pinned plan's walk keeps
    /// this one.
    scoot_in: Vec<f64>,
    /// The overlay's scootered distance: `scoot_in` lowered to the pass-through value on chain
    /// stations and settled across zero-cost ties, exactly as `reach_gas` is to `gas_in`. Keeping
    /// the two apart matters for the same reason it does for gas - pricing a swum-through station
    /// as an out-and-back detour would make the diver's own route read as out of range.
    reach_scoot: Vec<f64>,
    /// Runtime (minutes) to every station after the whole chain, at each segment's own speed. Read
    /// only at the route target, where it is the one-way runtime of the planned route; on a chain
    /// station it carries the same after-chain meaning as `gas_in` and is NOT a pass-through time.
    time_in: Vec<f64>,
    /// Predecessor tree of the LAST leg - rooted at the final way point, or at the origin roots when
    /// there are none. Walk it back from the route target to finish the route.
    pred: Vec<usize>,
    /// Route stations the way-point chain has already committed (origin → … → last way point) with
    /// their cumulative gas and runtime. Empty when there are no way points, so the route is then
    /// purely the origin→target path and the field is byte-identical to the single-Dijkstra one.
    ///
    /// `chain_time` is the time twin of `chain_gas` and exists for the same reason: the profile
    /// reports a value per ROUTE NODE, and on a chain station that value is the pass-through one.
    /// Without it the profile would have to re-derive time from distance at a single speed, which is
    /// precisely what made the chart and the `route_runtime_min` readout disagree (#317).
    chain: Vec<usize>,
    chain_gas: Vec<f64>,
    /// The scooter-range twin of `chain_gas`: what a dive that turns ON the chain has spent
    /// (#576 RV8c-2-D3, a pinned dive that turns before its pin).
    chain_scoot: Vec<f64>,
    chain_time: Vec<f64>,
    /// The Dijkstra gas of the leg that ENDED at the final way point, measured from that leg's
    /// own start. Empty without way points. A pinned plan reads it to tell the passage beyond its
    /// last pin from a backtrack ([`pinned::PinnedWalk`]).
    last_leg_in: Vec<f64>,
    /// Cumulative gas at each way point, in order; `None` from the first one that stalls.
    waypoint_gas: Vec<Option<f64>>,
    /// Where each way point sits in `chain`, in order; `None` from the first one that stalls. The
    /// chain is the route's own prefix, so these double as route indices - which is what a profile
    /// marker needs, since a route that doubles back visits a station more than once.
    waypoint_chain_idx: Vec<Option<usize>>,
}

/// The gas scale and the [scooter range metres, minutes] riders per metre of line for leg `k`.
///
/// Leg `k` is flown on segment `k`. `resolve_segments` guarantees one segment per user way-point
/// leg with a finite positive speed; the legs a pinned plan appends after the last way point
/// (#576 RV8c-2-D1) are past it, so they fly the last segment. An empty list is a bug rather than
/// an input - but if one ever appears, price the leg at INFINITY (nothing beyond it is reachable)
/// rather than at a speed of infinity, which would scale the leg's whole cost to ZERO and hand
/// back free penetration.
fn leg_scales(segments: &[ResolvedSegment], k: usize, in_rmv: f64) -> (f64, [f64; 2]) {
    let Some(seg) = segments
        .get(k.min(segments.len().saturating_sub(1)))
        .copied()
    else {
        return (f64::INFINITY, [0.0, f64::INFINITY]);
    };
    let speed = seg.speed_m_per_min;
    // [scooter range metres, minutes] per metre of line on this segment. The range rider is NOT
    // a flat 1 per metre: it charges whichever unit's battery clock the span drains harder,
    // which climbs below cruise (see `scooter::range_per_metre`). It is 0 on a swum span.
    let riders = [seg.range_per_metre, 1.0 / speed];
    (in_rmv / speed, riders)
}

/// Walk the ordered way points: one Dijkstra per leg, each seeded at the previous way point, with
/// the legs' costs accumulated. `waypoints` carries `None` for an entry that no longer resolves to a
/// station. A `None` entry, or one the graph cannot reach, **stalls** the walk - every later way
/// point reports `None` and the whole field goes unreachable, because a plan that silently skipped a
/// mandated point would over-state reach (the unsafe direction, §6).
///
/// Each leg commits the gas-cheapest way to its way point - unless that way spends more than
/// `scooter_limit` of range by the time it gets there and another way is in range: the leg then
/// takes the cheapest gas among the ways in range (#576 NF-W12GAS-1-D2, [`range_route::ranged_leg`]),
/// priced edge by edge along it, so the chain's gas, range and time are that way's. A leg no way
/// reaches in range keeps the gas-cheapest way, and the plan turns where the range runs out on it.
/// The legs' Dijkstras themselves (`pred`, `gas_in`, `last_leg_in`) are the gas-cheapest ones,
/// seeded at the way point, whichever way the chain took to it.
fn waypoint_walk(
    adj: &[Vec<(usize, f64)>],
    lens: &[Vec<(usize, f64)>],
    roots: &[usize],
    waypoints: &[Option<usize>],
    segments: &[ResolvedSegment],
    in_rmv: f64,
    scooter_limit: f64,
    degenerate: bool,
) -> WaypointWalk {
    let n = adj.len();
    if degenerate {
        return WaypointWalk {
            gas_in: vec![f64::INFINITY; n],
            reach_gas: vec![f64::INFINITY; n],
            scoot_in: vec![f64::INFINITY; n],
            reach_scoot: vec![f64::INFINITY; n],
            time_in: vec![f64::INFINITY; n],
            pred: vec![usize::MAX; n],
            chain: Vec::new(),
            chain_gas: Vec::new(),
            chain_scoot: Vec::new(),
            chain_time: Vec::new(),
            last_leg_in: Vec::new(),
            waypoint_gas: vec![None; waypoints.len()],
            waypoint_chain_idx: vec![None; waypoints.len()],
        };
    }
    let (mut cost_scale, mut rider_scales) = leg_scales(segments, 0, in_rmv);
    let mut leg = Project::dijkstra_scaled_with_riders(adj, lens, roots, cost_scale, rider_scales);
    let mut cum = 0.0_f64;
    let mut cum_scoot = 0.0_f64;
    let mut cum_time = 0.0_f64;
    let mut chain: Vec<usize> = Vec::new();
    let mut chain_gas: Vec<f64> = Vec::new();
    let mut chain_scoot: Vec<f64> = Vec::new();
    let mut chain_time: Vec<f64> = Vec::new();
    let mut waypoint_gas: Vec<Option<f64>> = Vec::with_capacity(waypoints.len());
    let mut waypoint_chain_idx: Vec<Option<usize>> = Vec::with_capacity(waypoints.len());
    let mut stalled = false;
    let mut last_leg_in: Vec<f64> = Vec::new();
    for (k, w) in waypoints.iter().enumerate() {
        let reached = (!stalled)
            .then_some(*w)
            .flatten()
            .filter(|&i| leg.0.get(i).is_some_and(|d| d.is_finite()));
        let Some(w) = reached else {
            stalled = true;
            waypoint_gas.push(None);
            waypoint_chain_idx.push(None);
            continue;
        };
        // Commit this leg's path. Its first node is the previous leg's way point, which already
        // closes the chain, so skip it rather than emitting a zero-length duplicate.
        let skip = usize::from(!chain.is_empty());
        // The gas-cheapest way, unless it is out of range at the way point and another way is in
        // range (NF-W12GAS-1-D2): that way's steps carry what the route has spent, from the chain
        // so far, so they are committed as they are.
        let out_of_range =
            cum_scoot + leg.1[0].get(w).copied().unwrap_or(f64::INFINITY) > scooter_limit;
        let in_range_way = out_of_range
            .then(|| {
                let starts: Vec<(usize, f64, f64, f64)> = match chain.last() {
                    Some(&from) => vec![(from, cum, cum_scoot, cum_time)],
                    None => roots.iter().map(|&r| (r, 0.0, 0.0, 0.0)).collect(),
                };
                let scales = range_route::LegScales::of(segments, k, in_rmv);
                range_route::ranged_leg(adj, lens, &starts, w, scales, scooter_limit)
            })
            .flatten();
        if let Some(steps) = in_range_way {
            for step in steps.iter().skip(skip) {
                chain.push(step.node);
                chain_gas.push(step.gas);
                chain_scoot.push(step.scoot);
                chain_time.push(step.time);
            }
            if let Some(end) = steps.last() {
                (cum, cum_scoot, cum_time) = (end.gas, end.scoot, end.time);
            }
        } else {
            for &node in path_to(&leg.2, w).iter().skip(skip) {
                chain.push(node);
                chain_gas.push(cum + leg.0.get(node).copied().unwrap_or(0.0));
                chain_scoot.push(cum_scoot + leg.1[0].get(node).copied().unwrap_or(0.0));
                chain_time.push(cum_time + leg.1[1].get(node).copied().unwrap_or(0.0));
            }
            cum += leg.0.get(w).copied().unwrap_or(0.0);
            cum_scoot += leg.1[0].get(w).copied().unwrap_or(0.0);
            cum_time += leg.1[1].get(w).copied().unwrap_or(0.0);
        }
        waypoint_gas.push(Some(cum));
        // The loop above just pushed this way point's own node last, so the chain now ends on it.
        waypoint_chain_idx.push(chain.len().checked_sub(1));
        (cost_scale, rider_scales) = leg_scales(segments, k + 1, in_rmv);
        let next = Project::dijkstra_scaled_with_riders(adj, lens, &[w], cost_scale, rider_scales);
        last_leg_in = std::mem::replace(&mut leg, next).0;
    }
    let (dist, riders, pred) = leg;
    let [leg_scoot, leg_time] = riders;
    let gas_in: Vec<f64> = if stalled {
        vec![f64::INFINITY; n]
    } else {
        dist.iter().map(|d| cum + *d).collect()
    };
    // The riders offset by the committed chain exactly as `gas_in` does. A stalled chain leaves
    // everything infinite so nothing reads as in range on a route that cannot be flown.
    let (scoot_in, time_in): (Vec<f64>, Vec<f64>) = if stalled {
        (vec![f64::INFINITY; n], vec![f64::INFINITY; n])
    } else {
        (
            leg_scoot.iter().map(|s| cum_scoot + *s).collect(),
            leg_time.iter().map(|t| cum_time + *t).collect(),
        )
    };
    // No way points means no chain, hence nothing to lower - the overlay reads the plain
    // single-Dijkstra field untouched (what
    // `gas_reach_field_no_waypoints_is_identical_to_the_plain_field` pins).
    let mut reach_gas = gas_in.clone();
    let mut reach_scoot = scoot_in.clone();
    if !stalled && !chain.is_empty() {
        // A station the plan SWIMS THROUGH is reached at the moment it is passed, not after the
        // whole chain. Lowering these is the SAFE direction; it only ever reports less gas for a
        // station the route provably visits. Everything off the chain keeps the conservative
        // after-chain cost, so an off-route station can never look cheaper than picking it would be.
        //
        // The scootered distance gets exactly the same treatment, for exactly the same reason: a
        // station passed 300 m into a scootered leg has cost 300 m of range, not the range of
        // flying back out to it after the last way point.
        for (pos, &node) in chain.iter().enumerate() {
            let passed = chain_gas[pos];
            if passed < reach_gas[node] {
                reach_gas[node] = passed;
            }
            let passed_scoot = chain_scoot[pos];
            if passed_scoot < reach_scoot[node] {
                reach_scoot[node] = passed_scoot;
            }
        }
        relax_zero_cost(adj, &mut reach_gas);
        relax_zero_cost(adj, &mut reach_scoot);
    }
    WaypointWalk {
        gas_in,
        reach_gas,
        scoot_in,
        reach_scoot,
        time_in,
        pred,
        chain,
        chain_gas,
        chain_scoot,
        chain_time,
        last_leg_in,
        waypoint_gas,
        waypoint_chain_idx,
    }
}

/// Settle a per-node cost across zero-cost edges - gas-to-reach, and equally the scootered distance
/// (#317), since both are per-node quantities that must agree between co-located nodes. Station
/// merges and loop closures join nodes that are the SAME physical point, so they must report the
/// same value - otherwise lowering one chain node leaves its co-located twin on the old (higher)
/// number, which reads as an unreachable station sitting exactly on top of a reachable one and puts
/// a spurious dot on the frontier.
///
/// A zero-weight edge therefore only ever joins coincident points: a tie, or the degenerate case of
/// a zero-length shot with no depth change (nothing floors shot length project-wide, so that edge
/// really can weigh 0). Both mean "the same place", so the relaxation cannot leak gas somewhere the
/// diver has not paid to reach.
fn relax_zero_cost(adj: &[Vec<(usize, f64)>], gas: &mut [f64]) {
    let mut queue: VecDeque<usize> = (0..gas.len()).filter(|&i| gas[i].is_finite()).collect();
    while let Some(u) = queue.pop_front() {
        let gu = gas[u];
        for &(v, w) in &adj[u] {
            if w <= 0.0 && gu < gas[v] {
                gas[v] = gu;
                queue.push_back(v);
            }
        }
    }
}

/// Everything the penetration budget decides: which stations are in reach, the reported
/// gas-to-reach, the largest in-budget value, and the planned route (with its cumulative gas)
/// origin → way points → target.
struct BudgetView {
    reachable: Vec<bool>,
    can_end_at: Vec<bool>,
    gas_to_reach: Vec<Option<f64>>,
    max_reachable_gas: f64,
    target: Option<usize>,
    /// Whether `target` is genuinely on the way to the diver's chosen endpoint, as opposed to the
    /// deepest-in-budget station anywhere. False when no endpoint is set, and when one is set but
    /// unusable (unresolvable, or in another graph component).
    target_toward_endpoint: bool,
    route: Vec<usize>,
    route_gas: Vec<f64>,
    /// Cumulative runtime (minutes) at each route station, aligned 1:1 with `route`, built from the
    /// same two vectors as `route_gas` (`chain_time` on the committed chain, `time_in` on the tail)
    /// so the profile's clock and its gas axis are two readings of ONE walk. The last entry is
    /// `route_runtime_min` by construction.
    route_time: Vec<f64>,
    /// Whether the scooter range, not gas, is what limits the dive: whether relaxing the range cap
    /// would move `target` FARTHER. Derived by [`route_target`], branch by branch, because the
    /// answer depends on which rule picked the target. Surfaced as
    /// `GasReachField::turn_on_scooter_range`.
    scooter_bound: bool,
    /// Scooter range spent and runtime at `target` (0 / `None` without one): the readouts the
    /// field reports for the dive's own turn (`scooter_distance_m`, `route_runtime_min`).
    turn_scoot: f64,
    turn_time: Option<f64>,
}

/// Apply a budget to the walked field. Computed twice per field - once on the base budget to locate
/// the strategy's own drop stations, once on the final budget - which is two O(n) passes plus one
/// path walk, negligible next to the Dijkstras.
///
/// The route target is the user-selected endpoint when it is in budget. When it is NOT, the plan
/// stays on the passage the diver chose and simply turns earlier: the target becomes the farthest
/// in-budget station ON THE WAY to that endpoint. Only with no endpoint set at all (or when nothing
/// on that path is in budget) does it fall back to the deepest in-budget station anywhere.
///
/// That matters because "deepest anywhere" is an arg-max over the whole cave, so it can sit in a
/// completely different passage: adding a way point that costs a little extra gas would move the
/// turnaround from the northern lead the diver picked to some unrelated eastern one, which reads as
/// the plan choosing a destination at random. Walking toward the pick instead keeps the answer to
/// the question actually being asked - "how far toward X can I get on this gas?"
///
/// Returns `(target, tail, toward_endpoint, bound_by_range)`: `tail` is the last leg's route to the
/// target with what it has spent at each station (empty without a target). The fourth is
/// `GasReachField::turn_on_scooter_range`, and it is derived HERE - per branch - because the only
/// definition that is not a false-positive machine is **"relaxing the DPV range cap would move the
/// target farther"**, and which station answers that depends on which rule picked the target. A
/// cave-wide "is any gas-affordable station out of range" scan fires whenever some unvisited pocket
/// is out of range, however comfortably the dive reaches its endpoint - and because gas scales with
/// depth while range scales with length alone, a shallow distant passage trips it routinely.
///
/// `range_bound[i]` is the per-node witness "the diver has the gas to end at `i`, but no route in
/// range ends there on it" - built in [`budget_view`] off `can_end_at`, so the two can never
/// disagree about which cap excluded a station.
///
/// A scooter plan takes each station's route from `ranged` - the cheapest gas among the routes its
/// range covers (#576 NF-W11GAS-1-D1, NF-W12GAS-1-D2) - and the walk's own route where that has
/// none. Without `ranged` every route is the walk's.
///
/// `candidates`, when given, are the only stations the deepest fallback may pick (a pinned plan's
/// stations beyond its last pin, D1); `deepest` is already the arg-max over them, and the range
/// question below asks about them alone.
fn route_target(
    endpoint: Option<usize>,
    walk: &WaypointWalk,
    ranged: Option<&RangedTail>,
    can_end_at: &[bool],
    range_bound: &[bool],
    deepest: (Option<usize>, f64),
    caps: (f64, f64),
    candidates: Option<&[bool]>,
) -> (Option<usize>, Vec<Step>, bool, bool) {
    let (deepest, deepest_gas) = deepest;
    let (budget_litres, scooter_limit) = caps;
    let tail = |t: usize| {
        ranged
            .and_then(|r| r.route_to(t))
            .unwrap_or_else(|| range_route::walk_route(walk, t))
    };
    // The deepest-anywhere fallback's own range question: with the cap lifted, could the arg-max
    // land FARTHER than it did? Only a gas-affordable, out-of-range station that costs strictly
    // more gas than the winner can move it. The `> deepest_gas` clause is the whole difference from
    // the old cave-wide scan, which counted every out-of-range station anywhere - including the
    // shallow ones the arg-max had already beaten.
    let deepest_is_range_bound = || {
        walk.gas_in
            .iter()
            .zip(range_bound)
            .enumerate()
            .any(|(i, (&g, &bound))| bound && g > deepest_gas && is_candidate(candidates, i))
    };
    let fallback = || {
        (
            deepest,
            deepest.map(tail).unwrap_or_default(),
            false,
            deepest_is_range_bound(),
        )
    };
    let Some(endpoint) = endpoint else {
        return fallback();
    };
    if can_end_at.get(endpoint).copied().unwrap_or(false) {
        // The dive reaches the diver's own pick. Neither cap is binding it, whatever else in the
        // cave sits out of range.
        return (Some(endpoint), tail(endpoint), true, false);
    }
    // Walk the route toward the endpoint and keep the last station still in budget and in range.
    // What a route has spent is monotone non-decreasing along it, so that is exactly where the gas
    // or the range runs out. An endpoint in another component has no path (its `pred` is unset),
    // leaving just itself - which is unreachable - so this yields nothing and we fall through.
    //
    // The returned flag says whether the target is genuinely ON THE WAY to the endpoint, which is
    // the only case the UI may describe that way. It is false for both deepest-anywhere fallbacks -
    // no endpoint set, and an endpoint that is set but unusable (deleted under a loaded plan, or in
    // another component). Telling a diver "turning at X on the way there" when X sits in an
    // unrelated passage is the very failure the walk-toward-the-endpoint rule was added to kill; it
    // would just have survived in the branches that rule cannot reach.
    let path = tail(endpoint);
    // The station that STOPPED the walk is the one to interrogate: the dive turned where it did
    // because the route could not reach it, and the range turned the dive when the gas alone would
    // have let it end there. `None` means the walk ran the whole route (the endpoint was reached),
    // and then nothing bound.
    let within = |s: &Step| s.gas.is_finite() && s.gas <= budget_litres && s.scoot <= scooter_limit;
    let stopped = path.iter().position(|s| !within(s));
    let turn_len = stopped.unwrap_or(path.len());
    let Some(turn) = turn_len.checked_sub(1).map(|i| path[i].node) else {
        return fallback();
    };
    let range_turned = stopped.is_some_and(|p| {
        walk.gas_in
            .get(path[p].node)
            .is_some_and(|&g| g.is_finite() && g <= budget_litres)
    });
    let mut path = path;
    path.truncate(turn_len);
    (Some(turn), path, true, range_turned)
}
/// Whether station `i` may be the deepest fallback target: any station without `candidates`.
fn is_candidate(candidates: Option<&[bool]>, i: usize) -> bool {
    candidates.is_none_or(|c| c.get(i).copied().unwrap_or(false))
}

/// `ranged` is the walk's tail priced on the routes its scooter can fly ([`range_route`], #576
/// NF-W11GAS-1-D1, NF-W12GAS-1-D2): when it is given, every station's destination and overlay
/// price, and the route to the target, are those of the cheapest-gas route in range, the walk's
/// own where there is none. The plain field passes its own; a pinned plan passes the one from its
/// last pin, with `candidates` its stations beyond that pin ([`route_target`]).
fn budget_view(
    walk: &WaypointWalk,
    ranged: Option<&RangedTail>,
    endpoint: Option<usize>,
    budget_litres: f64,
    scooter_limit: f64,
    candidates: Option<&[bool]>,
) -> BudgetView {
    // Reach is the INTERSECTION of two independent constraints: the gas budget and the scooter
    // rig's usable range (#317). Both masks are built here, and here only, because everything
    // downstream - the overlay, the turn frontier, the route target, the stage-drop basis - reads
    // one of these two vectors. ANDing at the source is what makes them agree by construction
    // rather than by four separate remembered edits.
    //
    // `scooter_limit` is infinite with no rig, so this collapses to the pre-#317 predicate.
    let in_range = |scoot: f64| scoot <= scooter_limit;
    // The prices the masks read: each station's route in range where the plan has `ranged`, the
    // walk's own (gas-cheapest) route otherwise. Gas and range are always read off ONE route.
    let (reach_gas, reach_scoot, end_gas, end_scoot) = match ranged {
        Some(r) => (&r.reach_gas, &r.reach_scoot, &r.gas_in, &r.scoot_in),
        None => (
            &walk.reach_gas,
            &walk.reach_scoot,
            &walk.gas_in,
            &walk.scoot_in,
        ),
    };

    // What the OVERLAY shows: can the diver be at this station, within budget, on this dive.
    let reachable: Vec<bool> = reach_gas
        .iter()
        .zip(reach_scoot)
        .map(|(&g, &s)| g.is_finite() && g <= budget_litres && in_range(s))
        .collect();
    // Whether a station can be the route's DESTINATION, which is a different (never cheaper)
    // question: ending there costs the after-chain price, not the pass-through price. Only the
    // target choice may read this, so a station the route merely swims past can never be picked as
    // a turnaround it cannot actually afford. Identical to `reachable` without way points.
    let can_end_at: Vec<bool> = end_gas
        .iter()
        .zip(end_scoot)
        .map(|(&g, &s)| g.is_finite() && g <= budget_litres && in_range(s))
        .collect();
    // Per-node witness for "the RANGE, not the gas, is why the dive cannot end here": the cheapest
    // gas (the walk's) affords it, yet no route in range ends there within budget. Built off
    // `can_end_at`, so the two cannot drift into disagreeing about which cap bound: without
    // `ranged` it is exactly `gas_ok && !in_range` on the walk's route.
    //
    // Note this reads `gas_in` - the DESTINATION price, already settled across zero-cost
    // merge/closure ties by the Dijkstra that produced it - and not the overlay's pass-through
    // `reach_gas`. It has to: it is only ever consumed by [`route_target`], which is asking where
    // the dive can END.
    let range_bound: Vec<bool> = walk
        .gas_in
        .iter()
        .zip(&can_end_at)
        .map(|(&g, &ok)| g.is_finite() && g <= budget_litres && !ok)
        .collect();

    // `max_reachable_gas` (reported) is the largest in-budget gas-to-reach, floored at 0. A station
    // no route in range reaches keeps the walk's price: it is dark either way.
    let mut max_reachable_gas = 0.0_f64;
    let gas_to_reach: Vec<Option<f64>> = reach_gas
        .iter()
        .zip(&walk.reach_gas)
        .zip(&reachable)
        .map(|((&ranged_g, &walk_g), &ok)| {
            let g = if ranged_g.is_finite() {
                ranged_g
            } else {
                walk_g
            };
            if g.is_finite() {
                if ok && g > max_reachable_gas {
                    max_reachable_gas = g;
                }
                Some(g)
            } else {
                None
            }
        })
        .collect();

    // `deepest` (the route fallback target) is the farthest station the diver could actually END at,
    // so it is an arg-max over `gas_in`/`can_end_at`. Seeded at -∞ so a degenerate all-at-0 set
    // (only a root in budget) still yields a target.
    let mut deepest: Option<usize> = None;
    let mut deepest_gas = f64::NEG_INFINITY;
    for (i, (&g, &ok)) in end_gas.iter().zip(&can_end_at).enumerate() {
        if ok && g.is_finite() && g > deepest_gas && is_candidate(candidates, i) {
            deepest_gas = g;
            deepest = Some(i);
        }
    }

    // The range flag comes out of the target choice itself, never from a scan beside it: "the
    // scooter turned this dive" is exactly "lifting the cap would move the target", and only the
    // branch that picked the target knows which station answers that.
    let (target, tail, target_toward_endpoint, scooter_bound) = route_target(
        endpoint,
        walk,
        ranged,
        &can_end_at,
        &range_bound,
        (deepest, deepest_gas),
        (budget_litres, scooter_limit),
        candidates,
    );
    // The committed way-point chain, then the last leg's route to the target. That route starts at
    // the final way point, which the chain already ends on, so it is skipped from the tail. With no
    // way points the chain is empty and the route is exactly the origin→target route.
    let mut route = walk.chain.clone();
    let mut route_gas = walk.chain_gas.clone();
    let mut route_time = walk.chain_time.clone();
    let skip = usize::from(!route.is_empty());
    for step in tail.iter().skip(skip) {
        route.push(step.node);
        route_gas.push(step.gas);
        route_time.push(step.time);
    }
    // ⚠ The range and runtime at the turn are the tail's LAST step: what the route has spent on
    // arriving at the target. That is right for every target this view picks, INCLUDING one that
    // sits on the committed chain (the tail then flies back to it from the final way point), and
    // the turn on the way to an endpoint whose route is not the target's own cheapest (#576
    // NF-W11GAS-1-D1: the dive turns on the endpoint's route, so it has spent that route's range).
    // The profile reports a value per route node, so it needs the per-node `chain_time` on the
    // chain exactly as `route_gas` needs `chain_gas`.
    let turn_scoot = tail
        .last()
        .map(|s| s.scoot)
        .filter(|s| s.is_finite())
        .unwrap_or(0.0);
    let turn_time = tail.last().map(|s| s.time).filter(|t| t.is_finite());
    BudgetView {
        reachable,
        can_end_at,
        gas_to_reach,
        max_reachable_gas,
        target,
        target_toward_endpoint,
        route,
        route_gas,
        route_time,
        scooter_bound,
        turn_scoot,
        turn_time,
    }
}

/// Place each stage's drop on the route: the first station whose cumulative route gas reaches the
/// stage's threshold. Route gas increases monotonically along the route, so that station is the
/// point where the stage runs out. A threshold past the turnaround - the cave is shorter than the
/// staged gas, so the stage isn't fully breathed before turning - clamps to the last route station
/// (you carry/drop it there), so EVERY threshold yields exactly one drop and the per-stage gauges
/// stay aligned 1:1. Returns route INDICES, not refs: a way-point route can double back over a
/// station, and only an index says which pass the drop is on.
///
/// A PINNED stage (`pinned[k]` set) is the exception (#576 RV8c-2-D3): when the dive turns before
/// its pin, `beyond_gas` carries the route on from the turnaround to the last pin, and the drop is
/// placed there - past the turnaround, the index the field reports as "not reached" - instead of
/// being clamped onto a station the diver never chose.
///
/// A pinned drop is reported at the station the user pinned (#576 NF-W12GAS2-1-D1). `pinned[k]`
/// is the pin's index on the pinned walk's chain, which is also its index here: the route starts
/// with that chain, and `beyond_gas` carries it on from the turnaround. A merge or an aligned
/// closure holds one physical station as several nodes joined at no cost, so the route can pass
/// the pin's twin just before the pin, at the same gas, and the first station reaching the
/// threshold is then the twin. When the station the threshold lands on costs what the pin costs
/// (it is the pin, or a zero-cost twin passed on the way to it), the drop is the pin's own pass.
fn place_stage_drops(
    route_gas: &[f64],
    beyond_gas: &[f64],
    thresholds: &[f64],
    pinned: &[Option<usize>],
) -> Vec<usize> {
    let Some(last) = route_gas.len().checked_sub(1) else {
        return Vec::new();
    };
    let gas_at = |i: usize| {
        route_gas
            .get(i)
            .or_else(|| beyond_gas.get(i.checked_sub(route_gas.len())?))
            .copied()
    };
    thresholds
        .iter()
        .enumerate()
        .map(|(k, &t)| {
            let pin = pinned.get(k).copied().flatten();
            let on_route = route_gas.iter().position(|&g| g >= t);
            let j = match (on_route, pin) {
                (Some(j), _) => j,
                (None, Some(_)) if !beyond_gas.is_empty() => {
                    let past = beyond_gas.iter().position(|&g| g >= t);
                    route_gas.len() + past.unwrap_or(beyond_gas.len() - 1)
                }
                (None, _) => last,
            };
            // Gas never falls along the route, so a station at or before the pin that costs at
            // least the pin's gas costs exactly that: the same point, reached for free.
            let same_point =
                |p: usize| p >= j && gas_at(j).zip(gas_at(p)).is_some_and(|(gj, gp)| gj >= gp);
            pin.filter(|&p| same_point(p)).unwrap_or(j)
        })
        .collect()
}

/// Cumulative swum distance (metres) at each route station: 3-D segment lengths accumulated over
/// swum legs only, with zero-distance merge/closure ties excluded exactly as [`route_metrics`] does.
fn cumulative_distances(pts: &[Coordinates], swim: &[bool]) -> Vec<f64> {
    let mut cum = vec![0.0_f64; pts.len()];
    for i in 1..pts.len() {
        let seg = if swim.get(i - 1).copied().unwrap_or(true) {
            let (a, b) = (pts[i - 1], pts[i]);
            ((b.x - a.x).powi(2) + (b.y - a.y).powi(2) + (b.z - a.z).powi(2)).sqrt()
        } else {
            0.0
        };
        cum[i] = cum[i - 1] + seg;
    }
    cum
}

/// A route leg is a real **swum** leg - a shot (including a reference/jump shot the diver swims) -
/// iff it joins two adjacent stations within one section. A station merge / loop-closure tie is a
/// zero-distance graph edge between non-adjacent or cross-section stations whose endpoints are the
/// *same* surveyed point; the measured-coord gap there (a closure's misclosure) was never swum, so
/// it must not count toward route distance or depth.
fn is_swim_leg(a: &StationRef, b: &StationRef) -> bool {
    a.section_id == b.section_id && a.station_index.abs_diff(b.station_index) == 1
}

/// One stage's drop, as the override recompute (#246) needs it. `safe_in_litres` is the strategy's
/// IN gas off this stage (Half-Minus half / Free usable span); `phys_cap_litres` is its FULL
/// conservative-capped deliverable (breathing it to empty - the hard clamp a deeper drag can't
/// cross); `default_bar` is the un-dragged drop-pressure gauge, `fill - usable` on the real gauge.
/// The `water/rated/fill` fields let a moved drop invert its litres-consumed back to a pressure on
/// that same real gauge.
struct StageDropPlan {
    safe_in_litres: f64,
    phys_cap_litres: f64,
    default_bar: f64,
    water_volume_l: f64,
    rated_pressure_bar: f64,
    fill_bar: f64,
}

/// Per-drop plans for the effective strategy, in stage order, aligned 1:1 with the drops the field
/// exposes. Staged (Half-Minus) counts the first `required_stages` stages; Free takes every enabled
/// stage with a positive usable allowance (skipping the rest, exactly as the drops did before);
/// All-thirds and a fell-back staged rig have none. This is the single producer of the strategy
/// defaults the override walk starts from, so drops/thresholds/bars can never diverge from it.
fn stage_drop_plans(
    cylinders: &[Cylinder],
    effective: ReservePreset,
    reserve_bar: f64,
    conservative: bool,
) -> Vec<StageDropPlan> {
    let stages: Vec<&Cylinder> = cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::Stage))
        .collect();
    // The same per-cylinder gauge rule as the budget and the column: the drop reads `fill - usable`
    // and credits the gas between the real fill and it, so a drag to those litres reads it back.
    let plan_for = |c: &Cylinder, usable_bar: f64| StageDropPlan {
        safe_in_litres: gauge_window_litres(c, usable_bar),
        phys_cap_litres: deliverable_gas(
            cyl_basis(c, conservative),
            c.water_volume_l,
            c.rated_pressure_bar,
            conservative,
        ),
        default_bar: (c.fill_bar - usable_bar).max(0.0),
        water_volume_l: c.water_volume_l,
        rated_pressure_bar: c.rated_pressure_bar,
        fill_bar: c.fill_bar,
    };
    match effective {
        ReservePreset::Free => stages
            .iter()
            .filter_map(|&c| {
                let u = free_usable_bar(c, conservative);
                (u > 0.0).then(|| plan_for(c, u))
            })
            .collect(),
        ReservePreset::OneStage | ReservePreset::TwoStage => {
            let needed = required_stages(effective);
            if stages.len() < needed {
                return Vec::new();
            }
            stages[..needed]
                .iter()
                .map(|&c| plan_for(c, half_minus_usable_bar(c, reserve_bar, conservative)))
                .collect()
        }
        ReservePreset::AllThirds => Vec::new(),
    }
}

/// A drop dragged over-reserve is one whose actual IN gas exceeds the safe half by more than this
/// (litres) - a slop floor against float noise once we've already excluded the default station.
const OVER_RESERVE_EPS_LITRES: f64 = 1e-6;

/// Fold the pinned drops (#246) into the drops, in ONE cumulative walk over the stages. Returns
/// `(cumulative_thresholds, drop_bars, over_reserve, budget_delta_litres)`:
/// - `cumulative_thresholds[k]` (litres of route gas) is where drop `k` lands
///   ([`place_stage_drops`]) - the pinned station for a pin, so the marker stays where the diver
///   put it.
/// - `drop_bars[k]` is the readout pressure - the strategy default, or (for a moved drop) the stage
///   gauge after breathing the litres consumed to the pinned station, inverted from the Z curve.
/// - `over_reserve[k]` is true only for a pin DEEPER than the safe point (breathing into reserve).
/// - `budget_delta_litres` is `Σ(credited - safe)`, added to the base budget. Never positive.
///
/// `pin_chain_idx[k]` is stage `k`'s pin as an index into `chain_gas`, the pinned route's
/// cumulative gas ([`pinned::PinnedWalk`]): the route passes every pin (#576 RV8c-2-D1), so the gas
/// a pin has cost is the route's own at that pass - never `gas_in[S]`, which with way points is
/// measured from the last one and, for a station passed on the way out, reports the cost of coming
/// back to it. `None` keeps the strategy default.
///
/// A pinned station's breathed gas is the route gas at the pin minus the cumulative so far,
/// hard-clamped to `[0, phys_cap]` - so dragging past tank-empty snaps the drop back to the empty
/// point, and dragging a drop shallower than the one before it breathes 0 (drops the stage full)
/// rather than going negative. What the plan CREDITS for the stage is that, capped at the
/// strategy's own safe IN gas (#576, decided "re-check pins, cap at default"): a shallower pin
/// strands gas and shrinks the reach, but a deeper one is shown - red, at its station, reading its
/// real gauge - without buying any reach. Moving a drop can therefore never make a plan reach
/// further than the same plan without it, whatever moved under a persisted pin since it was set.
/// Uncapped, a pin left on what USED to be the default station turned into a deeper drag the moment
/// an input change moved the default shallower, and credited the overshoot.
///
/// The next stage starts at the pin (#576 RV8c-2-D5, decided "next stage starts at pin"), also
/// when the pinned stage ran dry before it: the dive carries that stage to its pin, and the next
/// stage is credited only the route from there. Starting it at the empty point instead made a
/// SMALLER stage hand more of the route to the next one - it empties sooner - and a later pinned
/// stage was credited the difference: lowering a stage's fill raised the budget (W9-GAS sweep: 18
/// cases, one from 1302 to 1805 L). The dry stage's marker still sits at its empty point.
///
/// The price is the same whether or not the dive gets to the pin (#576 RV8c-2-D3, "no stranding
/// forgiven"): a dive that runs out of gas on the way there turns early and carries the stage, and
/// what the pin strands stays stranded.
///
/// ⚠ A pin ON its default station is the default. The default ▼ is rendered at the first route
/// station whose route gas reaches the stage's threshold, so that station's gas usually OVERSHOOTS
/// it; reading it as a fresh pin would fabricate an over-reserve flag out of a nudge back. So a pin
/// at exactly the station where the threshold lands on the pinned route - the station before it
/// short of `cum + safe`, the pin at or past it - reproduces the default. Only a REAL landing
/// counts: a pin on the station a threshold past the end of the route was clamped to is a genuine
/// pin, priced like any other (the stage is dropped there with gas left) - forgiving it as the
/// default handed the stranded gas back (#576 RV8c-2).
fn price_stage_drops(
    plans: &[StageDropPlan],
    pin_chain_idx: &[Option<usize>],
    chain_gas: &[f64],
) -> (Vec<f64>, Vec<f64>, Vec<bool>, f64) {
    let mut thresholds = Vec::with_capacity(plans.len());
    let mut bars = Vec::with_capacity(plans.len());
    let mut over = Vec::with_capacity(plans.len());
    let mut eff_cum = 0.0_f64;
    let mut delta = 0.0_f64;
    for (k, p) in plans.iter().enumerate() {
        // The pinned pass's route gas, and the route gas one station before it.
        let pass = pin_chain_idx.get(k).copied().flatten().and_then(|j| {
            let at = chain_gas.get(j).copied().filter(|g| g.is_finite())?;
            let before = j
                .checked_sub(1)
                .and_then(|i| chain_gas.get(i).copied())
                .unwrap_or(f64::NEG_INFINITY);
            Some((before, at))
        });
        let default_threshold = eff_cum + p.safe_in_litres;
        // `next` is where the following stage starts: the pin for a priced pin (D5) - the same as
        // `eff_cum + breathed` unless the stage ran dry short of it - else this drop.
        let (breathed, bar, is_over, next) = match pass {
            Some((before, at)) if !(before < default_threshold && default_threshold <= at) => {
                let breathed = (at - eff_cum).clamp(0.0, p.phys_cap_litres);
                (
                    breathed,
                    drop_pressure_for_contribution(p, breathed),
                    breathed > p.safe_in_litres + OVER_RESERVE_EPS_LITRES,
                    at.max(eff_cum),
                )
            }
            _ => (p.safe_in_litres, p.default_bar, false, default_threshold),
        };
        // Credit no more of the stage than its default drop would: a deeper pin buys no reach.
        delta += breathed.min(p.safe_in_litres) - p.safe_in_litres;
        thresholds.push(eff_cum + breathed);
        eff_cum = next;
        bars.push(bar.max(0.0));
        over.push(is_over);
    }
    (thresholds, bars, over, delta)
}

/// Invert the Z-derated deliverable curve to the tank pressure (bar) the stage's real gauge reads
/// after breathing `contribution` litres off it from its real fill, on the precise curve - the scale
/// every default drop is on (`fill - usable`). `deliverable_gas` is monotone non-decreasing over
/// `[0, fill]` for every realistic fill (the `p / Z(p)` curve only turns over above ~714 bar, ~2×
/// any real cylinder, where a fill could pick a wrong root; that mis-reports only this DISPLAY
/// gauge, never the litres-exact reach). ~50 bisection halvings converge to sub-microbar, and at
/// `contribution == safe` it inverts back to `default_bar`. Under the conservative cap an
/// overfill's contribution clamps at the rated capacity (`phys_cap_litres`), so even its empty
/// point reads the overfill still in the tank rather than 0.
fn drop_pressure_for_contribution(p: &StageDropPlan, contribution: f64) -> f64 {
    let held = |bar: f64| deliverable_gas(bar, p.water_volume_l, p.rated_pressure_bar, false);
    let target_held = (held(p.fill_bar) - contribution).max(0.0);
    let (mut lo, mut hi) = (0.0_f64, p.fill_bar.max(0.0));
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if held(mid) < target_held {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Physical metrics of the planned route from its ordered station coordinates and a per-leg swim
/// mask (`swim[i]` = the leg `pts[i]→pts[i+1]` is a real swum shot, not a tie): the one-way swum
/// line length (sum of 3-D segment lengths over swum legs only), the deepest point, and the
/// distance-weighted average depth over the swum legs. `z` is depth in metres (positive below
/// surface, `station.rs`), so the deepest point is `max z` over *all* route stations. Max-depth is
/// deliberately over all stations, not just swum legs: both ends of a tie are real surveyed stations
/// the route passes through, so the deeper one is a genuine depth the diver reaches - only the
/// un-swum tie *distance* (and its depth-weight) is excluded, from length/avg. An empty route →
/// `(0.0, None, None)`; a single station (or all-tie route) → zero length with the depth at the
/// first point. A missing mask entry conservatively counts the leg.
fn route_metrics(pts: &[Coordinates], swim: &[bool]) -> (f64, Option<f64>, Option<f64>) {
    let Some(first) = pts.first() else {
        return (0.0, None, None);
    };
    let max_depth = pts.iter().map(|c| c.z).fold(f64::NEG_INFINITY, f64::max);
    let mut length = 0.0;
    let mut depth_len_sum = 0.0;
    for (i, w) in pts.windows(2).enumerate() {
        // Skip zero-distance ties (merges / loop closures): their measured-coord gap was not swum.
        if !swim.get(i).copied().unwrap_or(true) {
            continue;
        }
        let (a, b) = (w[0], w[1]);
        let seg = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2) + (b.z - a.z).powi(2)).sqrt();
        length += seg;
        // Weight each segment's mean depth by its length → the depth-along-distance average.
        depth_len_sum += seg * 0.5 * (a.z + b.z);
    }
    let avg_depth = if length > 0.0 {
        depth_len_sum / length
    } else {
        first.z
    };
    (length, Some(max_depth), Some(avg_depth))
}

/// What the diver breathes and how fast they move on the way **out** - the leg the reach field
/// never prices. `GasConfig::normalize` guarantees `speed_m_per_min` is finite, positive and no
/// slower than the survey speed, and `jump_multiplier` is >= 1.0.
struct ExitLegRates {
    /// The swum exit pace - every leg the diver was not riding comes home at this.
    speed_m_per_min: f64,
    rmv_l_per_min: f64,
    jump_multiplier: f64,
    /// Per route leg, aligned with `leg_kinds`: `Some(pace)` where the span was scootered - the
    /// backup rides the same line home, so that leg exits at the span's own pace
    /// (`resolve_segments`-guaranteed finite and positive) - else `None` for the swum exit. A
    /// missing entry falls back to the swum exit, the over-stating (conservative) direction.
    scooter_speed_by_leg: Vec<Option<f64>>,
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
fn build_route_profile(
    refs: &[StationRef],
    pts: &[Coordinates],
    leg_kinds: &[Option<ShotKind>],
    cum_distance: &[f64],
    route_gas: &[f64],
    route_time: &[f64],
    exit: &ExitLegRates,
) -> Vec<RouteProfilePoint> {
    if refs.is_empty() {
        return Vec::new();
    }
    let outbound: Vec<RouteProfilePoint> = refs
        .iter()
        .enumerate()
        .map(|(i, &station_ref)| {
            let distance_m = cum_distance.get(i).copied().unwrap_or(0.0);
            RouteProfilePoint {
                route_index: u32::try_from(i).unwrap_or(u32::MAX),
                station_ref,
                distance_m,
                depth_m: pts.get(i).map_or(0.0, |c| c.z),
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
        profile.push(RouteProfilePoint {
            route_index: to.route_index,
            station_ref: to.station_ref,
            distance_m: total_m + (total_m - to.distance_m),
            depth_m: to.depth_m,
            gas_l,
            elapsed_min,
            outbound: false,
        });
    }
    profile
}

/// Number of `Stage` cylinders a preset requires in the rig. The pushy presets are valid
/// only when the diver actually carries that many stages: the engine falls back to All-thirds
/// otherwise (never invents stage gas - the safe direction, §6) and the UI greys the preset
/// (the §5.1 "advanced gate").
fn required_stages(preset: ReservePreset) -> usize {
    match preset {
        // Free needs no stages - it budgets the diver's per-cylinder usable gas directly, so
        // like All-thirds it never falls back (required_stages 0 ⇒ effective_preset keeps it).
        ReservePreset::AllThirds | ReservePreset::Free => 0,
        ReservePreset::OneStage => 1,
        ReservePreset::TwoStage => 2,
    }
}

/// Conservative-capped fill (bar) - the pressure every gauge rule takes its share of (the third /
/// quarter / sixth of back gas, a Half-Minus stage's half above the reserve), the ceiling on a Free
/// usable bar, and the basis of every stage's tank-empty clamp. The "call-it-an-80" cap (§5.3)
/// treats an overfill as its rated pressure, so an overfill never earns a larger share; precise
/// mode uses the real fill.
fn cyl_basis(c: &Cylinder, conservative: bool) -> f64 {
    if conservative {
        c.fill_bar.min(c.rated_pressure_bar)
    } else {
        c.fill_bar
    }
}

/// The diver's Free usable allowance (bar), sanitized: a non-finite input (NaN / ±inf - serde_json
/// can't carry those today, but a future non-JSON deserializer might, and NaN would slip a bare
/// `u <= 0.0` skip since NaN comparisons are false) becomes 0, then clamps to `[0, basis]` (the
/// conservative-capped fill, so under the cap no usable bar exceeds the rated pressure). One helper
/// so every Free path - budget, turn, drops, column - treats a stray value identically, and none
/// can propagate NaN into a gauge.
fn free_usable_bar(c: &Cylinder, conservative: bool) -> f64 {
    if !c.usable_gas_bar.is_finite() {
        return 0.0;
    }
    c.usable_gas_bar.clamp(0.0, cyl_basis(c, conservative))
}

/// Gas (litres) breathed off one cylinder by taking `usable_bar` off its REAL fill: the Z-derated
/// span from `fill` down to `fill - usable_bar`, on the precise curve. Every strategy credits exactly
/// this for the bar its rule spends - every rule is a share of the GAUGE (#576, "thirds of the
/// gauge") - so the budget the map enforces, the turn and drop gauges and the usable column describe
/// the same gas by construction.
///
/// The window always sits at the real fill: the conservative cap shrinks the usable bar (a share of
/// the capped basis), it never slides the window down to rated. Z rises with pressure, so the top of
/// a tank holds less gas per bar than the bottom: a share of the gauge never holds more than the
/// same share of the gas, and the same bar at an overfill never holds more than at rated (`p / Z(p)`
/// is concave up to ~1200 bar and peaks at ~714 for this fit). Below the Z == 1 knee (~96 bar) gas
/// is linear in pressure and the two coincide.
fn gauge_window_litres(c: &Cylinder, usable_bar: f64) -> f64 {
    let full = deliverable_gas(c.fill_bar, c.water_volume_l, c.rated_pressure_bar, false);
    let held = deliverable_gas(
        c.fill_bar - usable_bar,
        c.water_volume_l,
        c.rated_pressure_bar,
        false,
    );
    (full - held).max(0.0)
}

/// Bar a back-gas fraction rule spends off one cylinder's gauge: `frac` of its conservative-capped
/// basis, less `margin_bar` (the All-thirds safety margin, #245, raises the turn pressure by that
/// many bar), clamped to `[0, fill]`. Thirds of a 207-bar tank turn at 207 - 69 = 138 bar.
fn fraction_usable_bar(c: &Cylinder, frac: f64, margin_bar: f64, conservative: bool) -> f64 {
    (frac * cyl_basis(c, conservative) - margin_bar).clamp(0.0, c.fill_bar)
}

/// Bar a counted Half-Minus stage spends on the way in: half of what its basis holds above the
/// reserve, `(basis - reserve) / 2`, clamped to `[0, fill]`. It is dropped at
/// `fill - (basis - reserve) / 2` - which is `(fill + reserve) / 2` for a stage at or below rated -
/// and the other half is breathed back down to the reserve on the picked-up exit. An overfilled
/// stage under the cap spends the same bar as a rated one, read off its real gauge: 11 L rated 207
/// filled 230 with a 30-bar reserve is dropped at 230 - 88.5 = 141.5 bar.
fn half_minus_usable_bar(c: &Cylinder, reserve_bar: f64, conservative: bool) -> f64 {
    (0.5 * (cyl_basis(c, conservative) - reserve_bar)).clamp(0.0, c.fill_bar)
}

/// Bar one cylinder spends on the way IN under the effective strategy - the single source of its
/// credit ([`gauge_window_litres`]), its turn or drop gauge (`fill - usable`) and its usable-column
/// entry. `stage_counted` marks a stage among the first `required_stages` (Half Minus counts only
/// those; All-thirds and Free count every stage). `0` for roles the budget does not count (dil-out
/// / bailout / deco) and for uncounted stages.
fn in_usable_bar(
    c: &Cylinder,
    effective: ReservePreset,
    stage_counted: bool,
    profile: &ReserveProfile,
    conservative: bool,
) -> f64 {
    match (effective, c.role) {
        (ReservePreset::Free, CylRole::BackGas | CylRole::Stage) => {
            free_usable_bar(c, conservative)
        }
        // A fell-back staged rig is effectively All-thirds, so it inherits the margin too.
        (ReservePreset::AllThirds, CylRole::BackGas | CylRole::Stage) => fraction_usable_bar(
            c,
            backgas_fraction(effective),
            profile.all_thirds_safety_margin_bar,
            conservative,
        ),
        (ReservePreset::OneStage | ReservePreset::TwoStage, CylRole::BackGas) => {
            fraction_usable_bar(c, backgas_fraction(effective), 0.0, conservative)
        }
        (ReservePreset::OneStage | ReservePreset::TwoStage, CylRole::Stage) if stage_counted => {
            half_minus_usable_bar(c, profile.stage_reserve_bar, conservative)
        }
        _ => 0.0,
    }
}

/// [`in_usable_bar`] for every cylinder, in input order. Disabled tanks read `0` and are not
/// counted, so "stage 1" / "the first two stages" are the first ENABLED `Stage` cylinders in rig
/// order - v1 has no per-stage drop-order field; with ≥3 stages on `TwoStage` only the first two
/// count (the ≥3-stage case is deferred - PRD §5.1), and ignoring a spare stage only shrinks the
/// budget (safe).
fn in_usable_bars(
    cylinders: &[Cylinder],
    effective: ReservePreset,
    profile: &ReserveProfile,
    conservative: bool,
) -> Vec<f64> {
    let needed = required_stages(effective);
    let mut enabled_stage_i = 0usize;
    cylinders
        .iter()
        .map(|c| {
            if !c.enabled {
                return 0.0;
            }
            let stage_counted = if matches!(c.role, CylRole::Stage) {
                enabled_stage_i += 1;
                enabled_stage_i <= needed
            } else {
                false
            };
            in_usable_bar(c, effective, stage_counted, profile, conservative)
        })
        .collect()
}

/// Back-gas turn pressure gauge (bar): the volume-weighted mean of each enabled back-gas tank's
/// turn pressure `fill - usable` (`usable_bars` aligned with `cylinders`) - exactly the turn for
/// one tank and for matched doubles or sidemount. `None` when the rig carries no back gas.
fn backgas_turn_bar(cylinders: &[Cylinder], usable_bars: &[f64]) -> Option<f64> {
    let (water, water_x_turn) = cylinders
        .iter()
        .zip(usable_bars)
        .filter(|(c, _)| c.enabled && matches!(c.role, CylRole::BackGas))
        .fold((0.0_f64, 0.0_f64), |(w, wt), (c, &u)| {
            (
                w + c.water_volume_l,
                wt + c.water_volume_l * (c.fill_bar - u),
            )
        });
    (water > 0.0).then(|| (water_x_turn / water).max(0.0))
}

/// The strategy actually applied: the requested preset when the rig carries enough `Stage`
/// cylinders, else All-thirds (never invents stage gas - the safe direction, §6). The budget, the
/// gauges, the drops and the usable column all key off this, so a stage-short staged request shows
/// All-thirds numbers everywhere at once.
fn effective_preset(cylinders: &[Cylinder], preset: ReservePreset) -> ReservePreset {
    let stages = cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::Stage))
        .count();
    if stages < required_stages(preset) {
        ReservePreset::AllThirds
    } else {
        preset
    }
}

/// Share of the back-gas gauge spent on the way in, per strategy (PRD §5.1): thirds / quarters /
/// sixths as the stages carry more of the penetration.
fn backgas_fraction(preset: ReservePreset) -> f64 {
    match preset {
        // Free has no single back-gas fraction (the diver sets each tank's usable directly), so
        // `in_usable_bar` never asks for it; ⅓ is a harmless placeholder.
        ReservePreset::AllThirds | ReservePreset::Free => 1.0 / 3.0,
        ReservePreset::OneStage => 0.25,
        ReservePreset::TwoStage => 1.0 / 6.0,
    }
}

/// One-way IN penetration budget (litres), PRD §5.1 - the gas a diver may consume on the way in
/// before turning: over the budget cylinders, the gas between each one's real fill and the turn or
/// drop pressure its gauge rule gives ([`in_usable_bar`], [`gauge_window_litres`]).
/// - **All-thirds**: a third of every back-gas and stage cylinder's gauge, less the safety margin.
/// - **Half Minus, 1-stage**: a quarter of the back gas + stage 1's IN half.
/// - **Half Minus, 2-stage**: a sixth of the back gas + the first two stages' IN halves.
/// - **Free**: the diver's own usable bar per cylinder.
///
/// Each share is of the conservative-capped basis ([`cyl_basis`]). Under **Half Minus** a stage is
/// breathed from full down to `fill - (basis - reserve)/2` and the other half back down to the
/// reserve on the picked-up exit; with the stages carrying the penetration the diver dips less of
/// the back gas without breaking thirds. The reserve is the editable
/// [`ReserveProfile::stage_reserve_bar`] (default 30 bar). A rig short of the required stages falls
/// back to All-thirds ([`effective_preset`]).
fn penetration_budget(
    cylinders: &[Cylinder],
    preset: ReservePreset,
    profile: &ReserveProfile,
    conservative: bool,
) -> f64 {
    let effective = effective_preset(cylinders, preset);
    cylinders
        .iter()
        .zip(in_usable_bars(cylinders, effective, profile, conservative))
        .map(|(c, u)| gauge_window_litres(c, u))
        .sum()
}

/// Total usable **open-circuit** breathing gas (litres) across the diver's back-gas + stage
/// cylinders. Dil-out / bailout / deco roles are excluded (CCR / deco, not OC penetration gas,
/// §5.3/§5.5). Every cylinder is Z-derated via [`deliverable_gas`] (real-gas correction is
/// always applied); `conservative` additionally caps an overfilled tank to its rated capacity
/// (the "call-it-an-80" community convention, §5.3). Retained as a tested building block - every
/// budget now sums per-cylinder gauge windows (`gauge_window_litres`), so this is exercised only by
/// tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "tested building block; budget now sums per-cylinder"
    )
)]
fn usable_oc_gas(cylinders: &[Cylinder], conservative: bool) -> f64 {
    cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::BackGas | CylRole::Stage))
        .map(|c| {
            deliverable_gas(
                c.fill_bar,
                c.water_volume_l,
                c.rated_pressure_bar,
                conservative,
            )
        })
        .sum()
}
