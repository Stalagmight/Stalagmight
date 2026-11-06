//! [#576 RV8c-2, decided D1-D3] A pinned stage drop decides the route.
//!
//! A drop pinned by the diver is a place the dive goes, so the route passes it the way it passes
//! a way point: the way points first, then every pin the way-point chain does not already pass,
//! in drop order, then on to the endpoint ("D2": backtracking from a pin to the endpoint's passage
//! is charged like any other leg) or, with no endpoint, to the deepest station beyond the last pin
//! ("D1"). When the gas runs out before a pin, the dive turns where the gas says on the way there,
//! carries the stage, and the drop is reported as not reached ("D3").
//!
//! This replaces the earlier rule (#576 NF-PGAS3-1, "route computed without pins, then pins
//! applied"). That rule priced a pin only when the pin-free route happened to pass it, so a change
//! that moved the pin-free route - LESS gas, with no endpoint, sending the deepest station onto
//! another branch - dropped a stranding pin, handed its stranded gas back, and reached further.
//! Here the route through the pins does not depend on the budget at all: only where the dive turns
//! on it does, so less gas can only turn it earlier.
//!
//! The reach overlay and the turn frontier follow the pinned dive too ("D4", decided "only where
//! the dive goes"): a station is priced the way a way-point plan prices it - at its pass on the
//! route through the pins, or after passing every pin - so the map paints only the stations a
//! diver who drops at the pins can be at. Painting everything the pinned budget affords from the
//! entrance lit passages the dive never goes up (RV8c scenario 12178: A:1, A:2 and C:1 for a dive
//! that drops its stage at B:1). The cost, accepted with the decision: a pin on its own default
//! station is a way point too, so it can change the overlay of an otherwise identical plan.

use super::range_route::{self, RangedTail};
use super::{budget_view, waypoint_walk, BudgetView, ResolvedSegment, WaypointWalk};

/// The walk a pinned plan flies, and where each pin sits on it.
pub(super) struct PinnedWalk {
    /// The way points, then the appended pins. `None` when every pin is already on the way-point
    /// chain, so the plan's own walk is the pinned walk.
    walk: Option<WaypointWalk>,
    /// Stage `k`'s pin as an index into the pinned walk's `chain` - its first pass on the way-point
    /// chain, or the end of its own appended leg. `None` for a stage without a usable pin.
    pub(super) pin_chain_idx: Vec<Option<usize>>,
    /// Index in `chain` the appended pin legs start from (the last way point, else the origin),
    /// and whether there are any: with none, the walk's last leg starts at a way point, not a pin.
    legs_start: usize,
    ends_at_pin: bool,
}

impl PinnedWalk {
    pub(super) fn walk<'a>(&'a self, base: &'a WaypointWalk) -> &'a WaypointWalk {
        self.walk.as_ref().unwrap_or(base)
    }

    /// The pinned plan's open tail, from its last pin on, priced on the routes its scooter can fly
    /// (#576 NF-W12GAS-1-D2): the rule the plan without pins prices its own tail by. `None` when
    /// every pin is on the way-point chain (the plain view is the pinned plan's) or there is no
    /// range to search ([`range_route::ranged_tail`]).
    pub(super) fn ranged_tail(
        &self,
        adj: &[Vec<(usize, f64)>],
        lens: &[Vec<(usize, f64)>],
        roots: &[usize],
        base: &WaypointWalk,
        segments: &[ResolvedSegment],
        in_rmv: f64,
        scooter_limit: f64,
    ) -> Option<RangedTail> {
        if !self.ends_at_pin {
            return None;
        }
        range_route::ranged_tail(
            adj,
            lens,
            roots,
            self.walk(base),
            segments,
            in_rmv,
            scooter_limit,
        )
    }
}

/// The pinned plan's walk, or `None` when no stage has a usable pin.
///
/// A pin is usable when it resolves to a station the plan's own walk reaches (same graph component
/// as the last way point; a stalled way-point chain has no plan to pin anything on) and the dive
/// can reach it from the previous point: the last way point (or the origin roots) for the first
/// pin leg, the previous usable pin for later ones. A pin the dive cannot reach from there is
/// ignored, and its stage keeps its default drop (#576 FZ-ORACLE-1-D1) - as a pin outside the
/// walk's component is with an explicit origin. Without way points and with no origin, the roots
/// are every zero datum, so two pins can each be reachable from SOME datum in parts the dive cannot
/// fly between: the second pin's leg would stall the walk and leave nothing painted. Each usable
/// pin gets its own leg after the last way point, in drop order, on the last segment's pace -
/// unless the route already passes it before that leg starts: on the way-point chain (the diver's
/// own constraint), or on the way to an earlier pin (a drop moved shallower than the one before
/// it). Such a pin stays at that first pass, where the stage is dropped full (#246), rather than
/// sending the dive back to it.
///
/// Each pin leg takes the way a way-point leg does (#576 NF-W12GAS-1-D2): the gas-cheapest way,
/// or - when that one is out of `scooter_limit` at the pin and another way is in range - the
/// cheapest gas among the ways in range. The way points' own legs are the plan's, so the pinned
/// walk and the plan without pins share the chain to the last way point.
pub(super) fn pinned_walk(
    adj: &[Vec<(usize, f64)>],
    lens: &[Vec<(usize, f64)>],
    roots: &[usize],
    waypoints: &[Option<usize>],
    base: &WaypointWalk,
    pins: &[Option<usize>],
    segments: &[ResolvedSegment],
    in_rmv: f64,
    scooter_limit: f64,
) -> Option<PinnedWalk> {
    let mut stage_pins: Vec<Option<usize>> = pins
        .iter()
        .map(|p| p.filter(|&si| base.gas_in.get(si).is_some_and(|g| g.is_finite())))
        .collect();
    if stage_pins.iter().all(Option::is_none) {
        return None;
    }
    let legs_start = base.chain.len().saturating_sub(1);
    let first_pass = |w: &WaypointWalk, si: usize| w.chain.iter().position(|&n| n == si);
    let mut own_leg: Vec<bool> = stage_pins
        .iter()
        .map(|p| p.is_some_and(|si| first_pass(base, si).is_none()))
        .collect();
    // Each round either settles, takes one pin off its own leg, or ignores one pin whose leg cannot
    // be flown; a pin changes state at most once, so this ends within the stage count.
    loop {
        let appended: Vec<usize> = stage_pins
            .iter()
            .zip(&own_leg)
            .filter_map(|(&p, &own)| p.filter(|_| own))
            .collect();
        let walk = (!appended.is_empty()).then(|| {
            let mut chain_points = waypoints.to_vec();
            chain_points.extend(appended.iter().map(|&si| Some(si)));
            waypoint_walk(
                adj,
                lens,
                roots,
                &chain_points,
                segments,
                in_rmv,
                scooter_limit,
                false,
            )
        });
        let w = walk.as_ref().unwrap_or(base);
        let leg_end = |leg: usize| {
            w.waypoint_chain_idx
                .get(waypoints.len() + leg)
                .copied()
                .flatten()
        };
        let mut leg = 0usize;
        let mut passed_before_its_leg = None;
        let mut unreachable = None;
        let mut pin_chain_idx = Vec::with_capacity(stage_pins.len());
        for (k, &pin) in stage_pins.iter().enumerate() {
            let Some(si) = pin else {
                pin_chain_idx.push(None);
                continue;
            };
            if !own_leg[k] {
                pin_chain_idx.push(first_pass(w, si));
                continue;
            }
            // Passed before this leg starts - or, after another pin, AT its start: a second pin on
            // the station the previous one sits on is the same place, and a zero-length leg of its
            // own would make "beyond the last pin" every direction.
            let start = leg.checked_sub(1).map_or(Some(legs_start), leg_end);
            let passed = |(f, s): (usize, usize)| f < s || (leg > 0 && f == s);
            if first_pass(w, si).zip(start).is_some_and(passed) {
                passed_before_its_leg.get_or_insert(k);
            }
            // The walk stalls at the first leg it cannot fly, so the first leg with no end is the
            // first pin the dive cannot reach from the previous point.
            if leg_end(leg).is_none() {
                unreachable.get_or_insert(k);
            }
            pin_chain_idx.push(leg_end(leg));
            leg += 1;
        }
        // FZ-ORACLE-1-D1: a pin the dive cannot reach from the previous point is ignored, and its
        // stage keeps its default drop. The next pin's leg then starts at the pin before it.
        if let Some(k) = unreachable {
            stage_pins[k] = None;
            if stage_pins.iter().all(Option::is_none) {
                return None;
            }
            continue;
        }
        if let Some(k) = passed_before_its_leg {
            own_leg[k] = false;
            continue;
        }
        return Some(PinnedWalk {
            walk,
            pin_chain_idx,
            legs_start,
            ends_at_pin: !appended.is_empty(),
        });
    }
}

/// The pinned plan's view at the final budget: the reach masks priced on the pinned walk (D4),
/// with the turnaround and the route to it chosen on that walk. Also returns the stations past the
/// turnaround up to the last pin (and their route gas) when the dive turns before reaching it, so
/// a drop it never reaches can still be named at its pin.
///
/// `plain` is the view on the plan's own walk, returned as it is when every pin is on the
/// way-point chain (that walk already passes the pins). `ranged` is the pinned walk's tail priced
/// on the routes its scooter can fly ([`PinnedWalk::ranged_tail`]).
pub(super) fn pinned_view(
    pinned: &PinnedWalk,
    base: &WaypointWalk,
    ranged: Option<&RangedTail>,
    plain: BudgetView,
    endpoint: Option<usize>,
    budget_litres: f64,
    scooter_limit: f64,
) -> (BudgetView, Option<(Vec<usize>, Vec<f64>)>) {
    // Every pin on the way-point chain: the chain passes them, and the plan's own rule picks the
    // turn after it - the plain view already is the pinned plan.
    if !pinned.ends_at_pin {
        return (plain, None);
    }
    let walk = pinned.walk(base);
    // D4: the masks a way-point plan through the pins would show - pass-through gas on the route
    // to the last pin, the after-pins price everywhere else - at the same final budget. The pinned
    // walk does not depend on the budget, so less gas can only take stations off the map. Its gas
    // price for every station is at least the plain field's - the walk through the pins is one the
    // plain field could fly too, and the pinned budget is never larger - so a pin cannot paint a
    // station on gas that the plan without pins leaves dark.
    //
    // The same view also picks the turn for a dive that reaches every pin: on from the last pin,
    // exactly as the plain field goes on from its last way point - its tail on the routes its
    // scooter can fly, as that one's is (NF-W12GAS-1-D2) - but with no endpoint the target is the
    // deepest station BEYOND that pin (D1), the passage the dive continues into, never a station
    // only a backtrack reaches. A dive that runs out first (D3) keeps only its masks.
    let beyond_pin = beyond_last_pin(walk);
    let masks = budget_view(
        walk,
        ranged,
        endpoint,
        budget_litres,
        scooter_limit,
        Some(&beyond_pin),
    );
    let affordable =
        |i: usize| walk.chain_gas[i] <= budget_litres && walk.chain_scoot[i] <= scooter_limit;
    let last_pin = walk.chain.len() - 1;
    // Usable endpoint: resolves, and the walk past the last pin reaches its component.
    let endpoint = endpoint.filter(|&e| walk.gas_in.get(e).is_some_and(|g| g.is_finite()));

    if !affordable(last_pin) {
        // D3: the gas (or the scooter range) runs out on the way to a pin. A way point the plan
        // cannot afford stalls it as it always has; otherwise the dive turns at the last station
        // of the route toward the pins that it can afford.
        if !affordable(pinned.legs_start) {
            return (no_target(masks, walk), None);
        }
        let mut turn = pinned.legs_start;
        while turn < last_pin && affordable(turn + 1) {
            turn += 1;
        }
        let next = turn + 1;
        let view = BudgetView {
            target: Some(walk.chain[turn]),
            target_toward_endpoint: endpoint.is_some(),
            route: walk.chain[..=turn].to_vec(),
            route_gas: walk.chain_gas[..=turn].to_vec(),
            route_time: walk.chain_time[..=turn].to_vec(),
            // The range, not the gas, stopped the walk when the next station is affordable on gas.
            scooter_bound: walk.chain_gas[next] <= budget_litres,
            turn_scoot: walk.chain_scoot[turn],
            turn_time: Some(walk.chain_time[turn]).filter(|t| t.is_finite()),
            ..masks
        };
        let beyond = (walk.chain[next..].to_vec(), walk.chain_gas[next..].to_vec());
        return (view, Some(beyond));
    }

    // Every pin reached: the view above already turned on from the last pin (D1/D2 - the endpoint
    // on the way the range covers, else the deepest station beyond the pin).
    (masks, None)
}

/// No turnaround: the way-point chain itself is unaffordable. The committed chain is kept for the
/// map, exactly as the plain field does, so the diver can see which way point blew the gas.
fn no_target(masks: BudgetView, walk: &WaypointWalk) -> BudgetView {
    BudgetView {
        target: None,
        target_toward_endpoint: false,
        route: walk.chain.clone(),
        route_gas: walk.chain_gas.clone(),
        route_time: walk.chain_time.clone(),
        scooter_bound: false,
        turn_scoot: 0.0,
        turn_time: None,
        ..masks
    }
}

/// The stations the dive reaches by carrying on PAST the last pin rather than doubling back: those
/// a shortest path from the previous chain point runs through the pin to, i.e. where going via the
/// pin costs nothing extra. The pin itself (and its zero-cost merge twins) is among them.
///
/// Read off the two Dijkstras the walk already ran, compared with a relative tolerance: the two
/// sums add the same edges in a different order, so a station exactly on a shortest path through
/// the pin can come out a rounding error either side.
fn beyond_last_pin(walk: &WaypointWalk) -> Vec<bool> {
    let last_pin = walk.chain.last().copied();
    let cum = walk.chain_gas.last().copied().unwrap_or(0.0);
    let to_pin = last_pin
        .and_then(|p| walk.last_leg_in.get(p).copied())
        .unwrap_or(f64::INFINITY);
    walk.gas_in
        .iter()
        .zip(&walk.last_leg_in)
        .map(|(&g, &direct)| {
            let via_pin = to_pin + (g - cum);
            via_pin.is_finite() && via_pin <= direct + 1e-9 * direct.abs().max(1.0)
        })
        .collect()
}
