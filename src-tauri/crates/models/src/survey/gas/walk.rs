//! The gas-weighted walk (#298, #317) and what a budget makes of it: the way-point chain, the
//! pass-through settling across zero-cost ties, the reachable masks and the route target.
//!
//! Copied from `models/src/project/gas.rs` (`path_to` .. `budget_view`) for the station-graph
//! engine (`survey::gas`). The algorithms are unchanged; only the node space differs: a node is a
//! station, so a junction (what a merge was) is ONE node and the zero-cost merge edges and merge
//! hops of the slot graph are gone. Zero-cost edges are now loop closures (aligned or not) and
//! zero-length shots with no depth change.

use std::collections::VecDeque;

use super::super::graph::dijkstra_scaled_with_riders;
use super::scooter::ResolvedSegment;

/// Reconstruct the shortest path from a predecessor tree's root to `target`, root first. `pred` is
/// acyclic by construction; the node-count cap is a defensive stop so a corrupt tree can never hang
/// the compute.
pub(super) fn path_to(pred: &[usize], target: usize) -> Vec<usize> {
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
pub(super) struct WaypointWalk {
    /// Gas-to-reach every station AFTER visiting every way point in order: the last leg's Dijkstra
    /// distance offset by the gas already spent getting to the last way point. All-infinite when the
    /// chain stalls, so nothing reads as reachable on a route that cannot be flown.
    ///
    /// This is the cost of being at a station as the route's FINAL destination, which is what the
    /// route tail and the post-chain stage-drop basis need. It is NOT what the overlay shows - see
    /// [`WaypointWalk::reach_gas`].
    pub(super) gas_in: Vec<f64>,
    /// What the OVERLAY reports: `gas_in`, but lowered to the pass-through gas at any station the
    /// committed chain actually swims through (then settled across zero-cost closure ties).
    ///
    /// The two differ only with way points, and only for chain stations. `gas_in` prices a
    /// pass-through as an out-and-back detour from the last way point, so the entrance itself would
    /// cost 2 x d(origin, W1), read as out of budget, and put a spurious turn-frontier dot in the
    /// middle of the diver's own route. They must stay separate: on a route that doubles back, a
    /// revisited station is passed cheaply the first time and expensively the second, and the route's
    /// own cumulative gas has to keep the higher number or the stage drops slide.
    pub(super) reach_gas: Vec<f64>,
    /// Scooter RANGE (metres) spent reaching every station after the whole chain, accumulated along
    /// the SAME gas-optimal path as `gas_in` - the destination price, the analogue of `gas_in`.
    /// Capped against the rig's usable range to decide whether the dive can END here. Equal to the
    /// scootered line's length at or above cruise; larger below it, where a metre costs more than a
    /// metre of the allowance (`scooter::range_per_metre`).
    pub(super) scoot_in: Vec<f64>,
    /// The overlay's scootered distance: `scoot_in` lowered to the pass-through value on chain
    /// stations and settled across zero-cost ties, exactly as `reach_gas` is to `gas_in`. Keeping
    /// the two apart matters for the same reason it does for gas - pricing a swum-through station
    /// as an out-and-back detour would make the diver's own route read as out of range.
    pub(super) reach_scoot: Vec<f64>,
    /// Runtime (minutes) to every station after the whole chain, at each segment's own speed. Read
    /// only at the route target, where it is the one-way runtime of the planned route; on a chain
    /// station it carries the same after-chain meaning as `gas_in` and is NOT a pass-through time.
    pub(super) time_in: Vec<f64>,
    /// Predecessor tree of the LAST leg - rooted at the final way point, or at the origin roots when
    /// there are none. Walk it back from the route target to finish the route.
    pub(super) pred: Vec<usize>,
    /// Route stations the way-point chain has already committed (origin → … → last way point) with
    /// their cumulative gas and runtime. Empty when there are no way points, so the route is then
    /// purely the origin→target path and the field is byte-identical to the single-Dijkstra one.
    ///
    /// `chain_time` is the time twin of `chain_gas` and exists for the same reason: the profile
    /// reports a value per ROUTE NODE, and on a chain station that value is the pass-through one.
    /// Without it the profile would have to re-derive time from distance at a single speed, which is
    /// precisely what made the chart and the `route_runtime_min` readout disagree (#317).
    pub(super) chain: Vec<usize>,
    pub(super) chain_gas: Vec<f64>,
    pub(super) chain_time: Vec<f64>,
    /// Cumulative gas at each way point, in order; `None` from the first one that stalls.
    pub(super) waypoint_gas: Vec<Option<f64>>,
    /// Where each way point sits in `chain`, in order; `None` from the first one that stalls. The
    /// chain is the route's own prefix, so these double as route indices - which is what a profile
    /// marker needs, since a route that doubles back visits a station more than once.
    pub(super) waypoint_chain_idx: Vec<Option<usize>>,
}

/// Walk the ordered way points: one Dijkstra per leg, each seeded at the previous way point, with
/// the legs' costs accumulated. `waypoints` carries `None` for an entry that no longer resolves to a
/// station. A `None` entry, or one the graph cannot reach, **stalls** the walk - every later way
/// point reports `None` and the whole field goes unreachable, because a plan that silently skipped a
/// mandated point would over-state reach (the unsafe direction, §6).
pub(super) fn waypoint_walk(
    adj: &[Vec<(usize, f64)>],
    lens: &[Vec<(usize, f64)>],
    roots: &[usize],
    waypoints: &[Option<usize>],
    segments: &[ResolvedSegment],
    in_rmv: f64,
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
            chain_time: Vec::new(),
            waypoint_gas: vec![None; waypoints.len()],
            waypoint_chain_idx: vec![None; waypoints.len()],
        };
    }
    // Leg `k` is flown on segment `k`. `resolve_segments` guarantees one segment per leg with a
    // finite positive speed, so a missing entry is a bug rather than an input - but if one ever
    // appears, price the leg at INFINITY (nothing beyond it is reachable) rather than at a speed of
    // infinity, which would scale the leg's whole cost to ZERO and hand back free penetration.
    let leg_scales = |k: usize| {
        let Some(seg) = segments.get(k).copied() else {
            return (f64::INFINITY, [0.0, f64::INFINITY]);
        };
        let speed = seg.speed_m_per_min;
        // [scooter range metres, minutes] per metre of line on this segment. The range rider is NOT
        // a flat 1 per metre: below the rig's cruise it climbs, because the cap converts battery
        // minutes to metres at cruise and only there (see `scooter::range_per_metre`). It is 0 on a
        // swum span.
        let riders = [seg.range_per_metre, 1.0 / speed];
        (in_rmv / speed, riders)
    };

    let (mut cost_scale, mut rider_scales) = leg_scales(0);
    let mut leg = dijkstra_scaled_with_riders(adj, lens, roots, cost_scale, rider_scales);
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
        for &node in path_to(&leg.2, w).iter().skip(skip) {
            chain.push(node);
            chain_gas.push(cum + leg.0.get(node).copied().unwrap_or(0.0));
            chain_scoot.push(cum_scoot + leg.1[0].get(node).copied().unwrap_or(0.0));
            chain_time.push(cum_time + leg.1[1].get(node).copied().unwrap_or(0.0));
        }
        cum += leg.0.get(w).copied().unwrap_or(0.0);
        cum_scoot += leg.1[0].get(w).copied().unwrap_or(0.0);
        cum_time += leg.1[1].get(w).copied().unwrap_or(0.0);
        waypoint_gas.push(Some(cum));
        // The loop above just pushed this way point's own node last, so the chain now ends on it.
        waypoint_chain_idx.push(chain.len().checked_sub(1));
        (cost_scale, rider_scales) = leg_scales(k + 1);
        leg = dijkstra_scaled_with_riders(adj, lens, &[w], cost_scale, rider_scales);
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
        chain_time,
        waypoint_gas,
        waypoint_chain_idx,
    }
}

/// Settle a per-node cost across zero-cost edges - gas-to-reach, and equally the scootered distance
/// (#317), since both are per-node quantities that must agree between co-located nodes. Loop
/// closures join nodes that are the SAME physical point (a junction is already one node), so they
/// must report the same value - otherwise lowering one chain node leaves its co-located twin on the old (higher)
/// number, which reads as an unreachable station sitting exactly on top of a reachable one and puts
/// a spurious dot on the frontier.
///
/// A zero-weight edge therefore only ever joins coincident points: a tie, or the degenerate case of
/// a zero-length shot with no depth change (nothing floors shot length project-wide, so that edge
/// really can weigh 0). Both mean "the same place", so the relaxation cannot leak gas somewhere the
/// diver has not paid to reach.
pub(super) fn relax_zero_cost(adj: &[Vec<(usize, f64)>], gas: &mut [f64]) {
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
pub(super) struct BudgetView {
    pub(super) reachable: Vec<bool>,
    pub(super) can_end_at: Vec<bool>,
    pub(super) gas_to_reach: Vec<Option<f64>>,
    pub(super) max_reachable_gas: f64,
    pub(super) target: Option<usize>,
    /// Whether `target` is genuinely on the way to the diver's chosen endpoint, as opposed to the
    /// deepest-in-budget station anywhere. False when no endpoint is set, and when one is set but
    /// unusable (unresolvable, or in another graph component).
    pub(super) target_toward_endpoint: bool,
    pub(super) route: Vec<usize>,
    pub(super) route_gas: Vec<f64>,
    /// Cumulative runtime (minutes) at each route station, aligned 1:1 with `route`, built from the
    /// same two vectors as `route_gas` (`chain_time` on the committed chain, `time_in` on the tail)
    /// so the profile's clock and its gas axis are two readings of ONE walk. The last entry is
    /// `route_runtime_min` by construction.
    pub(super) route_time: Vec<f64>,
    /// Whether the scooter range, not gas, is what limits the dive: whether relaxing the range cap
    /// would move `target` FARTHER. Derived by [`route_target`], branch by branch, because the
    /// answer depends on which rule picked the target. Surfaced as
    /// `GasReachField::turn_on_scooter_range`.
    pub(super) scooter_bound: bool,
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
/// Returns `(target, toward_endpoint, bound_by_range)`. The third is
/// `GasReachField::turn_on_scooter_range`, and it is derived HERE - per branch - because the only
/// definition that is not a false-positive machine is **"relaxing the DPV range cap would move the
/// target farther"**, and which station answers that depends on which rule picked the target. A
/// cave-wide "is any gas-affordable station out of range" scan fires whenever some unvisited pocket
/// is out of range, however comfortably the dive reaches its endpoint - and because gas scales with
/// depth while range scales with length alone, a shallow distant passage trips it routinely.
///
/// `range_bound[i]` is the per-node witness "the diver has the gas to end at `i`, but it is out of
/// scooter range" - built in [`budget_view`] off the same two vectors as `can_end_at`, so the two
/// can never disagree about which cap excluded a station.
pub(super) fn route_target(
    endpoint: Option<usize>,
    walk: &WaypointWalk,
    can_end_at: &[bool],
    range_bound: &[bool],
    deepest: Option<usize>,
    deepest_gas: f64,
) -> (Option<usize>, bool, bool) {
    // The deepest-anywhere fallback's own range question: with the cap lifted, could the arg-max
    // land FARTHER than it did? Only a gas-affordable, out-of-range station that costs strictly
    // more gas than the winner can move it. The `> deepest_gas` clause is the whole difference from
    // the old cave-wide scan, which counted every out-of-range station anywhere - including the
    // shallow ones the arg-max had already beaten.
    let deepest_is_range_bound = || {
        walk.gas_in
            .iter()
            .zip(range_bound)
            .any(|(&g, &bound)| bound && g > deepest_gas)
    };
    let Some(endpoint) = endpoint else {
        return (deepest, false, deepest_is_range_bound());
    };
    if can_end_at.get(endpoint).copied().unwrap_or(false) {
        // The dive reaches the diver's own pick. Neither cap is binding it, whatever else in the
        // cave sits out of range.
        return (Some(endpoint), true, false);
    }
    // Walk the path toward the endpoint and keep the last station still in budget. Gas along a
    // shortest path is monotone non-decreasing, so that is exactly where the gas runs out. An
    // endpoint in another component has no path (its `pred` is unset), leaving just itself - which
    // is unreachable - so this yields nothing and we fall through.
    //
    // The returned flag says whether the target is genuinely ON THE WAY to the endpoint, which is
    // the only case the UI may describe that way. It is false for both deepest-anywhere fallbacks -
    // no endpoint set, and an endpoint that is set but unusable (deleted under a loaded plan, or in
    // another component). Telling a diver "turning at X on the way there" when X sits in an
    // unrelated passage is the very failure the walk-toward-the-endpoint rule was added to kill; it
    // would just have survived in the branches that rule cannot reach.
    let path = path_to(&walk.pred, endpoint);
    // The node that STOPPED the walk is the one to interrogate: the dive turned where it did
    // because that node was excluded, so whichever cap excluded IT is the cap that turned the dive.
    // `None` means the walk ran the whole path (the endpoint was reached), and then nothing bound.
    let stopped = path
        .iter()
        .position(|&n| !can_end_at.get(n).copied().unwrap_or(false));
    let last_in_budget = match stopped {
        Some(p) => p.checked_sub(1).map(|i| path[i]),
        None => path.last().copied(),
    };
    match last_in_budget {
        Some(n) => (
            Some(n),
            true,
            stopped.is_some_and(|p| range_bound.get(path[p]).copied().unwrap_or(false)),
        ),
        None => (deepest, false, deepest_is_range_bound()),
    }
}
pub(super) fn budget_view(
    walk: &WaypointWalk,
    endpoint: Option<usize>,
    budget_litres: f64,
    scooter_limit: f64,
) -> BudgetView {
    // Reach is the INTERSECTION of two independent constraints: the gas budget and the scooter
    // rig's usable range (#317). Both masks are built here, and here only, because everything
    // downstream - the overlay, the turn frontier, the route target, the stage-drop basis - reads
    // one of these two vectors. ANDing at the source is what makes them agree by construction
    // rather than by four separate remembered edits.
    //
    // `scooter_limit` is infinite with no rig, so this collapses to the pre-#317 predicate.
    let in_range = |scoot: f64| scoot <= scooter_limit;

    // What the OVERLAY shows: can the diver be at this station, within budget, on this dive.
    let reachable: Vec<bool> = walk
        .reach_gas
        .iter()
        .zip(&walk.reach_scoot)
        .map(|(&g, &s)| g.is_finite() && g <= budget_litres && in_range(s))
        .collect();
    // Whether a station can be the route's DESTINATION, which is a different (never cheaper)
    // question: ending there costs the after-chain price, not the pass-through price. Only the
    // target choice may read this, so a station the route merely swims past can never be picked as
    // a turnaround it cannot actually afford. Identical to `reachable` without way points.
    let can_end_at: Vec<bool> = walk
        .gas_in
        .iter()
        .zip(&walk.scoot_in)
        .map(|(&g, &s)| g.is_finite() && g <= budget_litres && in_range(s))
        .collect();
    // Per-node witness for "the RANGE, not the gas, is why the dive cannot end here": affordable on
    // gas, out of scooter range. Built off the same two vectors as `can_end_at`, in the same shape,
    // so `can_end_at[i] == gas_ok && in_range` and `range_bound[i] == gas_ok && !in_range` are
    // complementary by construction and cannot drift into disagreeing about which cap bound.
    //
    // Note this reads `gas_in` / `scoot_in` - the DESTINATION prices, already settled across
    // zero-cost closure ties by the Dijkstra that produced them - and not the overlay's
    // pass-through `reach_gas` / `reach_scoot`. It has to: it is only ever consumed by
    // [`route_target`], which is asking where the dive can END.
    let range_bound: Vec<bool> = walk
        .gas_in
        .iter()
        .zip(&walk.scoot_in)
        .map(|(&g, &s)| g.is_finite() && g <= budget_litres && !in_range(s))
        .collect();

    // `max_reachable_gas` (reported) is the largest in-budget gas-to-reach, floored at 0.
    let mut max_reachable_gas = 0.0_f64;
    let gas_to_reach: Vec<Option<f64>> = walk
        .reach_gas
        .iter()
        .zip(&reachable)
        .map(|(&g, &ok)| {
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
    for (i, (&g, &ok)) in walk.gas_in.iter().zip(&can_end_at).enumerate() {
        if ok && g.is_finite() && g > deepest_gas {
            deepest_gas = g;
            deepest = Some(i);
        }
    }

    // The range flag comes out of the target choice itself, never from a scan beside it: "the
    // scooter turned this dive" is exactly "lifting the cap would move the target", and only the
    // branch that picked the target knows which station answers that.
    let (target, target_toward_endpoint, scooter_bound) = route_target(
        endpoint,
        walk,
        &can_end_at,
        &range_bound,
        deepest,
        deepest_gas,
    );
    // The committed way-point chain, then the last leg's path to the target. That path starts at
    // the final way point, which the chain already ends on, so it is skipped from the tail. With no
    // way points the chain is empty and the route is exactly the origin→target path.
    let mut route = walk.chain.clone();
    let mut route_gas = walk.chain_gas.clone();
    let mut route_time = walk.chain_time.clone();
    if let Some(t) = target {
        let skip = usize::from(!route.is_empty());
        for &node in path_to(&walk.pred, t).iter().skip(skip) {
            route.push(node);
            route_gas.push(walk.gas_in.get(node).copied().unwrap_or(0.0));
            route_time.push(walk.time_in.get(node).copied().unwrap_or(0.0));
        }
    }
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
    }
}
