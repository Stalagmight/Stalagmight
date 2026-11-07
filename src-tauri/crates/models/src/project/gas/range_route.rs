//! [#576 NF-W11GAS-1, decided D1 "price along the route"; NF-W12GAS-1, decided D2 "extend to all
//! legs"] Every leg of a scooter plan takes a route its scooter can fly.
//!
//! The walk ([`super::waypoint_walk`]) picks each station's route by gas alone and carries the
//! scooter range along it. Round a loop that is not the only way there: a shallow line can be the
//! cheaper gas and run out of range, while the deep way round is in range on gas the diver has. So
//! every leg takes, to its end, the cheapest gas among the routes the scooter can fly, with gas and
//! range priced along that route. Where the cheapest gas is in range that is the walk's own route,
//! so a plan the range does not bind is unchanged.
//!
//! - The open tail - from the last way point (or the origin) on, where the engine chooses where the
//!   dive goes - is priced per station: each station on the cheapest gas in range to it
//!   ([`ranged_tail`]). A pinned plan's tail, from its last pin on, the same.
//! - A leg to a way point, or to a pin (the pinned walk appends its pins as way points), commits
//!   one route to its end point: the cheapest gas in range to it, given what the legs before it
//!   have spent ([`ranged_leg`]). The leg is chosen on its own, not together with the legs after
//!   it, so a cheap-gas way that spends most of the range can leave a later leg short where a
//!   dearer, shorter way would not: the plan then turns where the range runs out, never later.
//!
//! The search is a two-criteria Dijkstra: routes are taken cheapest gas first, and a route reaching
//! a station is kept only while it spends less range than every cheaper one kept there - each one
//! kept is a way on that a cheaper one cannot match. A route past the usable range is never
//! extended. The first route kept at a station is its cheapest gas in range; the others carry on
//! the search. Every route kept is a real one, priced edge by edge, so a station the search
//! reaches is in range and in budget on a route the diver can fly; one it does not reach falls
//! back to the walk's own price, past the range.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use super::{leg_scales, path_to, relax_zero_cost, ResolvedSegment, WaypointWalk};

/// Most routes the search keeps at one station. A cave survey is mostly a tree, where each station
/// has one route per way round the loops before it, so this is never reached in practice; in a
/// maze it bounds the work. A route not kept is only a way the plan does not consider: the price
/// of every station is still that of a real route, so the cap can turn a dive earlier, never later.
const ROUTES_PER_STATION: u32 = 16;

/// One route to `node`: its cumulative gas, scooter range and runtime from the origin, and the
/// route it extends (`usize::MAX` at the start).
struct Label {
    node: usize,
    gas: f64,
    scoot: f64,
    time: f64,
    pred: usize,
}

/// A station on a route with what the route has spent on arriving there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Step {
    pub(super) node: usize,
    pub(super) gas: f64,
    pub(super) scoot: f64,
    pub(super) time: f64,
}

/// The plan's tail priced on the routes its scooter can fly: per station, the cheapest gas in
/// range and the range that route spends (`INFINITY` where no route of the tail is in range), and
/// the routes themselves.
pub(super) struct RangedTail {
    /// Destination price: the route the dive would END on at each station, after the way points.
    pub(super) gas_in: Vec<f64>,
    pub(super) scoot_in: Vec<f64>,
    /// The overlay's price: `gas_in` / `scoot_in` lowered to the pass on the way-point chain, as the
    /// walk lowers its own (`WaypointWalk::reach_gas`).
    pub(super) reach_gas: Vec<f64>,
    pub(super) reach_scoot: Vec<f64>,
    /// Each station's route (its first label), `usize::MAX` for none.
    best: Vec<usize>,
    labels: Vec<Label>,
}

impl RangedTail {
    /// The in-range route to `t` from the tail's start (the last way point, or an origin root),
    /// with what it has spent at every station; `None` when no route to `t` is in range.
    pub(super) fn route_to(&self, t: usize) -> Option<Vec<Step>> {
        route_of(&self.labels, self.best.get(t).copied()?)
    }
}

/// The route that label `at` ends, from its search's start, with what it has spent at every
/// station; `None` for no label (`usize::MAX`).
fn route_of(labels: &[Label], at: usize) -> Option<Vec<Step>> {
    let mut at = Some(at).filter(|&l| l != usize::MAX)?;
    let mut steps = Vec::new();
    while let Some(l) = labels.get(at) {
        steps.push(Step {
            node: l.node,
            gas: l.gas,
            scoot: l.scoot,
            time: l.time,
        });
        // A label's predecessor is always an earlier label, so this ends; the length check is a
        // defensive stop against a corrupt chain.
        if l.pred >= at || steps.len() > labels.len() {
            break;
        }
        at = l.pred;
    }
    steps.reverse();
    Some(steps)
}

/// The walk's own route to `t` (the last leg's gas-cheapest path), priced as the walk prices it.
pub(super) fn walk_route(walk: &WaypointWalk, t: usize) -> Vec<Step> {
    path_to(&walk.pred, t)
        .into_iter()
        .map(|node| Step {
            node,
            gas: walk.gas_in.get(node).copied().unwrap_or(f64::INFINITY),
            scoot: walk.scoot_in.get(node).copied().unwrap_or(f64::INFINITY),
            time: walk.time_in.get(node).copied().unwrap_or(f64::INFINITY),
        })
        .collect()
}

/// Heap key: cheapest gas first, then least range, then the label's own index (a total order, so
/// the search is deterministic).
#[derive(Clone, Copy, PartialEq)]
struct Key(f64, f64, usize);
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .total_cmp(&other.0)
            .then(self.1.total_cmp(&other.1))
            .then(self.2.cmp(&other.2))
    }
}

/// A leg's prices per metre of line: its gas scale, and the scooter range and the minutes a metre
/// costs ([`leg_scales`]).
#[derive(Clone, Copy)]
pub(super) struct LegScales {
    cost_scale: f64,
    per_metre: f64,
    per_minute: f64,
}

impl LegScales {
    /// Leg `k`'s prices, as the walk prices it.
    pub(super) fn of(segments: &[ResolvedSegment], k: usize, in_rmv: f64) -> Self {
        let (cost_scale, [per_metre, per_minute]) = leg_scales(segments, k, in_rmv);
        Self {
            cost_scale,
            per_metre,
            per_minute,
        }
    }

    /// Whether the routes of this leg can differ in range at all: one flown without the scooter
    /// spends the same range whichever way it goes, so the walk's own way is already the cheapest
    /// in range.
    fn scootered(self) -> bool {
        self.per_metre > 0.0 && self.cost_scale.is_finite() && self.cost_scale > 0.0
    }
}

/// The two-criteria search from `starts` (a station, with the gas, range and minutes spent on
/// arriving there) over the routes within `limit` of range. Returns each station's first label -
/// its cheapest gas in range, `usize::MAX` for none - and the labels. With `stop_at` it stops once
/// that station has its first label, which nothing popped after it can change.
fn search(
    adj: &[Vec<(usize, f64)>],
    lens: &[Vec<(usize, f64)>],
    starts: &[(usize, f64, f64, f64)],
    scales: LegScales,
    limit: f64,
    stop_at: Option<usize>,
) -> (Vec<usize>, Vec<Label>) {
    let n = adj.len();
    let mut labels: Vec<Label> = Vec::new();
    let mut heap: BinaryHeap<Reverse<Key>> = BinaryHeap::new();
    for &(node, gas, scoot, time) in starts {
        if node < n && gas.is_finite() && scoot <= limit {
            heap.push(Reverse(Key(gas, scoot, labels.len())));
            labels.push(Label {
                node,
                gas,
                scoot,
                time,
                pred: usize::MAX,
            });
        }
    }
    let mut best = vec![usize::MAX; n];
    let mut least_scoot = vec![f64::INFINITY; n];
    let mut kept = vec![0u32; n];
    while let Some(Reverse(Key(gas, scoot, at))) = heap.pop() {
        let (u, time) = (labels[at].node, labels[at].time);
        // Every route kept at `u` so far is no more gas (the heap pops cheapest first); one that
        // also spends no more range leaves this one nothing to add.
        if scoot >= least_scoot[u] || kept[u] >= ROUTES_PER_STATION {
            continue;
        }
        least_scoot[u] = scoot;
        kept[u] += 1;
        if best[u] == usize::MAX {
            best[u] = at;
            if stop_at == Some(u) {
                break;
            }
        }
        // `lens` is aligned edge for edge with `adj` (one builder), as the walk's riders read it.
        for (k, &(v, w)) in adj[u].iter().enumerate() {
            let next_gas = gas + w * scales.cost_scale;
            let len = lens[u].get(k).map_or(0.0, |&(_, l)| l);
            let next_scoot = scoot + len * scales.per_metre;
            // An unswimmable leg is priced INFINITY and never taken; a route past the range is
            // never extended; one no better on range than a route already kept at `v` is pruned.
            if !next_gas.is_finite() || next_scoot > limit || next_scoot >= least_scoot[v] {
                continue;
            }
            heap.push(Reverse(Key(next_gas, next_scoot, labels.len())));
            labels.push(Label {
                node: v,
                gas: next_gas,
                scoot: next_scoot,
                time: time + len * scales.per_minute,
                pred: at,
            });
        }
    }
    (best, labels)
}

/// [#576 NF-W12GAS-1-D2] The route a scootered leg takes to its end point `to` - a way point, or a
/// pin of a pinned walk - when its gas-cheapest way is out of range: the cheapest gas among the
/// routes from `starts` (the previous point with what the route has spent on arriving there, or
/// the origin roots) that reach `to` within `limit`, with what it has spent at every station.
/// `None` when no route to `to` is in range, or the leg is flown without the scooter (every way
/// then spends the same range): the walk keeps the gas-cheapest way, and the plan turns where the
/// range runs out on it, as before.
pub(super) fn ranged_leg(
    adj: &[Vec<(usize, f64)>],
    lens: &[Vec<(usize, f64)>],
    starts: &[(usize, f64, f64, f64)],
    to: usize,
    scales: LegScales,
    limit: f64,
) -> Option<Vec<Step>> {
    if !limit.is_finite() || !scales.scootered() {
        return None;
    }
    let (best, labels) = search(adj, lens, starts, scales, limit, Some(to));
    route_of(&labels, best.get(to).copied()?)
}

/// Search the plan's tail for routes within `limit` of scooter range. `None` when there is nothing
/// to search: no range cap, a tail flown without the scooter (every route then spends the same
/// range, so the walk's own route is already the cheapest in range), or a way-point chain that
/// stalls (nothing is reachable). The walk can be a pinned plan's: its tail starts at the last
/// pin and is flown on the last segment, as the pin legs are.
pub(super) fn ranged_tail(
    adj: &[Vec<(usize, f64)>],
    lens: &[Vec<(usize, f64)>],
    roots: &[usize],
    walk: &WaypointWalk,
    segments: &[ResolvedSegment],
    in_rmv: f64,
    limit: f64,
) -> Option<RangedTail> {
    if !limit.is_finite() || walk.waypoint_gas.iter().any(Option::is_none) {
        return None;
    }
    let scales = LegScales::of(segments, walk.waypoint_gas.len(), in_rmv);
    if !scales.scootered() {
        return None;
    }
    // The tail starts where the walk's last leg does: at the last way point with what the chain
    // has spent, or at the origin roots. Only a start the walk itself reaches is seeded, so a field
    // the walk leaves all-unreachable stays so.
    let starts: Vec<(usize, f64, f64, f64)> = match walk.chain.last() {
        Some(&w) => vec![(
            w,
            walk.chain_gas.last().copied().unwrap_or(0.0),
            walk.chain_scoot.last().copied().unwrap_or(0.0),
            walk.chain_time.last().copied().unwrap_or(0.0),
        )],
        None => roots.iter().map(|&r| (r, 0.0, 0.0, 0.0)).collect(),
    }
    .into_iter()
    .filter(|&(node, ..)| walk.gas_in.get(node).is_some_and(|g| g.is_finite()))
    .collect();
    let (best, labels) = search(adj, lens, &starts, scales, limit, None);
    let price = |pick: fn(&Label) -> f64| -> Vec<f64> {
        best.iter()
            .map(|&l| labels.get(l).map_or(f64::INFINITY, pick))
            .collect()
    };
    let gas_in = price(|l| l.gas);
    let scoot_in = price(|l| l.scoot);
    // The overlay lowers a station the way-point chain swims through to its pass, exactly as the
    // walk lowers its own: the pass comes before the tail starts, so it is no more gas and no more
    // range than any route of the tail.
    let mut reach_gas = gas_in.clone();
    let mut reach_scoot = scoot_in.clone();
    if !walk.chain.is_empty() {
        for (pos, &node) in walk.chain.iter().enumerate() {
            reach_gas[node] = reach_gas[node].min(walk.chain_gas[pos]);
            reach_scoot[node] = reach_scoot[node].min(walk.chain_scoot[pos]);
        }
        relax_zero_cost(adj, &mut reach_gas);
        relax_zero_cost(adj, &mut reach_scoot);
    }
    Some(RangedTail {
        gas_in,
        scoot_in,
        reach_gas,
        reach_scoot,
        best,
        labels,
    })
}
