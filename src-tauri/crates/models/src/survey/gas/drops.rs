//! Stage drops (PRD §5.1, #246): the per-stage drop plans of the effective strategy, the diver's
//! drop overrides folded into the budget, and where each drop lands on the route.
//!
//! Copied from `models/src/project/gas.rs` (`cumulative`, `place_stage_drops`,
//! `default_drop_stations`, `StageDropPlan` .. `drop_pressure_for_contribution`) unchanged for the
//! station-graph engine (`survey::gas`): every index here is a node of the walk, which is now a
//! station.

use crate::config::ReservePreset;
use crate::gas::{CylRole, Cylinder};

use super::budget::{cyl_basis, free_in_gas, free_usable_bar, required_stages, stage_in_gas};
use super::physics::deliverable_gas;
use super::walk::{path_to, BudgetView, WaypointWalk};

/// Running sums of `values`, i.e. `[v0, v0+v1, …]`.
pub(super) fn cumulative(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut acc = 0.0_f64;
    values
        .map(|v| {
            acc += v;
            acc
        })
        .collect()
}

/// Place each stage's drop on the route: the first station whose cumulative route gas reaches the
/// stage's threshold. Route gas increases monotonically along the route, so that station is the
/// point where the stage runs out. A threshold past the turnaround - the cave is shorter than the
/// staged gas, so the stage isn't fully breathed before turning - clamps to the last route station
/// (you carry/drop it there), so EVERY threshold yields exactly one drop and the per-stage gauges
/// stay aligned 1:1. Returns route INDICES, not refs: a way-point route can double back over a
/// station, and only an index says which pass the drop is on.
pub(super) fn place_stage_drops(route_gas: &[f64], thresholds: &[f64]) -> Vec<usize> {
    let Some(last) = route_gas.len().checked_sub(1) else {
        return Vec::new();
    };
    thresholds
        .iter()
        .map(|&t| route_gas.iter().position(|&g| g >= t).unwrap_or(last))
        .collect()
}

/// The stations a strategy's un-overridden drops land on, for the pin-to-default no-op check.
pub(super) fn default_drop_stations(view: &BudgetView, thresholds: &[f64]) -> Vec<usize> {
    if view.target.is_none() {
        return Vec::new();
    }
    place_stage_drops(&view.route_gas, thresholds)
        .iter()
        .filter_map(|&j| view.route.get(j).copied())
        .collect()
}

/// One stage's drop, as the override recompute (#246) needs it. `safe_in_litres` is the strategy's
/// IN gas off this stage (Half-Minus half / Free usable span); `phys_cap_litres` is its FULL
/// deliverable (breathing it to empty - the hard clamp a deeper drag can't cross); `default_bar` is
/// the un-dragged drop-pressure gauge. The `fill/water/rated/basis` fields let a moved drop invert
/// its litres-consumed back to a pressure on the conservative-capped basis scale.
pub(super) struct StageDropPlan {
    pub(super) safe_in_litres: f64,
    pub(super) phys_cap_litres: f64,
    pub(super) default_bar: f64,
    pub(super) water_volume_l: f64,
    pub(super) rated_pressure_bar: f64,
    pub(super) basis: f64,
}

/// Per-drop plans for the effective strategy, in stage order, aligned 1:1 with the drops the field
/// exposes. Staged (Half-Minus) counts the first `required_stages` stages; Free takes every enabled
/// stage with a positive usable allowance (skipping the rest, exactly as the drops did before);
/// All-thirds and a fell-back staged rig have none. This is the single producer of the strategy
/// defaults the override walk starts from, so drops/thresholds/bars can never diverge from it.
pub(super) fn stage_drop_plans(
    cylinders: &[Cylinder],
    effective: ReservePreset,
    reserve_bar: f64,
    conservative: bool,
) -> Vec<StageDropPlan> {
    let stages: Vec<&Cylinder> = cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::Stage))
        .collect();
    let plan_for = |c: &Cylinder, safe_in_litres: f64, default_bar: f64| {
        let basis = cyl_basis(c, conservative);
        StageDropPlan {
            safe_in_litres,
            phys_cap_litres: deliverable_gas(
                basis,
                c.water_volume_l,
                c.rated_pressure_bar,
                conservative,
            ),
            default_bar,
            water_volume_l: c.water_volume_l,
            rated_pressure_bar: c.rated_pressure_bar,
            basis,
        }
    };
    match effective {
        ReservePreset::Free => stages
            .iter()
            .filter_map(|&c| {
                let u = free_usable_bar(c, conservative);
                if u <= 0.0 {
                    return None;
                }
                Some(plan_for(
                    c,
                    free_in_gas(c, conservative),
                    (c.fill_bar - u).max(0.0),
                ))
            })
            .collect(),
        ReservePreset::OneStage | ReservePreset::TwoStage => {
            let needed = required_stages(effective);
            if stages.len() < needed {
                return Vec::new();
            }
            stages[..needed]
                .iter()
                .map(|&c| {
                    let basis = cyl_basis(c, conservative);
                    plan_for(
                        c,
                        stage_in_gas(c, reserve_bar, conservative),
                        (0.5 * (basis + reserve_bar)).clamp(0.0, basis),
                    )
                })
                .collect()
        }
        ReservePreset::AllThirds => Vec::new(),
    }
}

/// A drop dragged over-reserve is one whose actual IN gas exceeds the safe half by more than this
/// (litres) - a slop floor against float noise once we've already excluded the default station.
const OVER_RESERVE_EPS_LITRES: f64 = 1e-6;

/// Fold the per-drop overrides (#246) into the drops, in ONE cumulative walk. Returns
/// `(cumulative_thresholds, drop_bars, over_reserve, budget_delta_litres)`:
/// - `cumulative_thresholds[k]` (litres) is where drop `k` lands on the route (fed to `staged_route`).
/// - `drop_bars[k]` is the readout pressure - the strategy default, or (when overridden) the tank
///   pressure left after breathing the litres consumed to the pinned station, inverted from the Z
///   curve so it matches the gas the reach actually spent.
/// - `over_reserve[k]` is true only for a drag DEEPER than the safe point (breathing into reserve).
/// - `budget_delta_litres` is `Σ(actual - safe)`, added to the base budget to move the reach.
///
/// A pinned station's contribution is the gas the diver has burned by the time they reach it, minus
/// the effective-cumulative-so-far, hard-clamped to `[0, phys_cap]` - so dragging past tank-empty
/// snaps the drop back to the empty point, and dragging a drop shallower than the one before it
/// contributes 0 (drops the stage full) rather than going negative. An unresolvable / off-graph /
/// non-finite override falls through to the strategy default, as does one that is not ON the route.
///
/// ⚠ "Gas burned by the time they reach it" is the ROUTE's cumulative gas at the drop, never
/// `gas_in[S]`. `gas_in` is measured from the LAST way point, so for a station the diver passes on
/// the way OUT to that way point it reports the cost of coming back to it afterwards - which on a
/// cave whose return leg does not retrace the outbound can be far larger than what the diver had
/// actually burned when they swam past. Scoring an early drop against that inflated figure reads it
/// as a deep one and GROWS the penetration budget: the unsafe direction (§6). Reading the route
/// instead is right by construction, and it is also what makes an off-route pin fall back to the
/// default rather than being priced from a passage the plan never enters.
///
/// The route passed in is the BASE-budget one, which keeps the ordering acyclic: the final route
/// depends on the budget, which depends on these very contributions. The two coincide whenever the
/// delta is 0, i.e. whenever nothing is overridden.
///
/// ⚠ Pin-to-the-default-station is a NO-OP. The default ▼ is rendered at the first route station
/// whose route gas ≥ the stage's threshold, so that station's gas usually OVERSHOOTS the sub-station
/// threshold; reading it as a fresh contribution would fabricate an over-reserve plan and extend the
/// reach on a mere jiggle / release-in-place. `default_drop_nodes[k]` - stage `k`'s drop station on
/// the UN-overridden route - recognises it exactly. (It replaces an earlier predecessor-tree
/// heuristic, which could not stay exact once way points let a route double back over a station.)
pub(super) fn apply_stage_drop_overrides(
    plans: &[StageDropPlan],
    overrides: &[Option<usize>],
    base: &BudgetView,
    walk: &WaypointWalk,
    default_drop_nodes: &[usize],
    conservative: bool,
) -> (Vec<f64>, Vec<f64>, Vec<bool>, f64) {
    // Gas burned on arrival at `si`, in the two cases where "along the route" is meaningful:
    //
    //   1. `si` IS on the route - take the route's own cumulative gas at the FIRST pass. First
    //      because a way-point route can double back over a station, and the earliest pass is the
    //      smallest figure, hence the smallest budget: the conservative direction.
    //   2. `si` is past the end of the route on the SAME line, i.e. the route's target lies on the
    //      path to it. The route here is the BASE-budget one, and a drop pinned deeper legitimately
    //      pushes the budget out past it, so the deeper station the diver dragged to is off the base
    //      route by construction. Its `gas_in` IS the gas along that continuation, so it is exact.
    //
    // Anything else - a station on a DIFFERENT branch - yields `None`, which the caller reads as
    // "keep the strategy default". That is the only honest answer: a drop is a position along the
    // route, so a station the plan never passes has no arrival gas at all, and pricing it from its
    // own shortest path folds a passage the dive never enters into the penetration budget, which
    // extends the reach: the unsafe direction (§6). Reachable without a hand-edited file, since
    // `.plnx` persists overrides and a plan reloaded after the survey, the endpoint or the way
    // points moved can carry a pin the new route no longer passes.
    let arrival_gas = |si: usize| -> Option<f64> {
        if let Some(p) = base.route.iter().position(|&n| n == si) {
            return base.route_gas.get(p).copied().filter(|g| g.is_finite());
        }
        let target = base.target?;
        if path_to(&walk.pred, si).contains(&target) {
            return walk.gas_in.get(si).copied().filter(|g| g.is_finite());
        }
        None
    };
    let mut thresholds = Vec::with_capacity(plans.len());
    let mut bars = Vec::with_capacity(plans.len());
    let mut over = Vec::with_capacity(plans.len());
    let mut eff_cum = 0.0_f64;
    let mut delta = 0.0_f64;
    for (k, p) in plans.iter().enumerate() {
        let pinned = overrides.get(k).copied().flatten();
        let (contribution, bar, is_over) = match pinned {
            // Pinned to the stage's OWN default drop station: reproduce the un-dragged default so a
            // jiggle / release-in-place can't fabricate over-reserve or extend the reach.
            Some(si) if default_drop_nodes.get(k) == Some(&si) => {
                (p.safe_in_litres, p.default_bar, false)
            }
            Some(si) => match arrival_gas(si) {
                Some(gas) => {
                    let contribution = (gas - eff_cum).clamp(0.0, p.phys_cap_litres);
                    (
                        contribution,
                        drop_pressure_for_contribution(p, contribution, conservative),
                        contribution > p.safe_in_litres + OVER_RESERVE_EPS_LITRES,
                    )
                }
                None => (p.safe_in_litres, p.default_bar, false),
            },
            None => (p.safe_in_litres, p.default_bar, false),
        };
        delta += contribution - p.safe_in_litres;
        eff_cum += contribution;
        thresholds.push(eff_cum);
        bars.push(bar.max(0.0));
        over.push(is_over);
    }
    (thresholds, bars, over, delta)
}

/// Invert the Z-derated deliverable curve to the tank pressure (bar) left after breathing
/// `contribution` litres off a stage from full (its conservative-capped `basis`). `deliverable_gas`
/// is monotone non-decreasing over `[0, basis]` for every realistic fill (the `p / Z(p)` curve only
/// turns over above ~714 bar - ~2× any real cylinder - where a non-conservative fill could pick a
/// wrong root; that mis-reports only this DISPLAY gauge, never the litres-exact reach). ~50 bisection
/// halvings converge to sub-microbar, and at `contribution == safe` it inverts back to `default_bar`.
pub(super) fn drop_pressure_for_contribution(
    p: &StageDropPlan,
    contribution: f64,
    conservative: bool,
) -> f64 {
    let target_held = (p.phys_cap_litres - contribution).max(0.0);
    let (mut lo, mut hi) = (0.0_f64, p.basis.max(0.0));
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if deliverable_gas(mid, p.water_volume_l, p.rated_pressure_bar, conservative) < target_held
        {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}
