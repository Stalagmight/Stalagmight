//! Reserve strategies, the penetration budget and the cylinder gauges (PRD §5.1) - pure
//! arithmetic over the rig, no graph.
//!
//! Copied from `models/src/project/gas.rs` (`required_stages` .. `usable_oc_gas`, the tail of that
//! file) unchanged for the station-graph engine (`survey::gas`).

use crate::config::{ReservePreset, ReserveProfile};
use crate::project::gas::{CylRole, Cylinder};

use super::physics::deliverable_gas;

/// Number of `Stage` cylinders a preset requires in the rig. The pushy presets are valid
/// only when the diver actually carries that many stages: the engine falls back to All-thirds
/// otherwise (never invents stage gas - the safe direction, §6) and the UI greys the preset
/// (the §5.1 "advanced gate").
pub(super) fn required_stages(preset: ReservePreset) -> usize {
    match preset {
        // Free needs no stages - it budgets the diver's per-cylinder usable gas directly, so
        // like All-thirds it never falls back (required_stages 0 ⇒ effective_preset keeps it).
        ReservePreset::AllThirds | ReservePreset::Free => 0,
        ReservePreset::OneStage => 1,
        ReservePreset::TwoStage => 2,
    }
}

/// Conservative-capped fill (bar) - the pressure basis every strategy computes drops/usable from.
/// The "call-it-an-80" cap (§5.3) treats an overfill as its rated capacity; precise mode uses the
/// real fill.
pub(super) fn cyl_basis(c: &Cylinder, conservative: bool) -> f64 {
    if conservative {
        c.fill_bar.min(c.rated_pressure_bar)
    } else {
        c.fill_bar
    }
}

/// The diver's Free usable allowance (bar), sanitized: a non-finite input (NaN / ±inf - serde_json
/// can't carry those today, but a future non-JSON deserializer might, and NaN would slip a bare
/// `u <= 0.0` skip since NaN comparisons are false) becomes 0, then clamps to `[0, basis]` (the
/// conservative-capped fill the budget credits). One helper so every Free path - budget, turn,
/// drops, column - treats a stray value identically, and none can propagate NaN into a gauge.
pub(super) fn free_usable_bar(c: &Cylinder, conservative: bool) -> f64 {
    if !c.usable_gas_bar.is_finite() {
        return 0.0;
    }
    c.usable_gas_bar.clamp(0.0, cyl_basis(c, conservative))
}

/// Free-strategy IN gas for one cylinder (litres): the Z-derated span from full down to the diver's
/// chosen turn pressure `fill - usable_gas_bar`. The usable bar is clamped to `[0, basis]` so a
/// stray/over-large input can never credit more than the tank holds (the unsafe direction, §6).
/// Mirrors `stage_in_gas`'s basis-scale deliverable arithmetic.
pub(super) fn free_in_gas(c: &Cylinder, conservative: bool) -> f64 {
    let basis = cyl_basis(c, conservative);
    let u = free_usable_bar(c, conservative);
    let full = deliverable_gas(basis, c.water_volume_l, c.rated_pressure_bar, conservative);
    let held = deliverable_gas(
        basis - u,
        c.water_volume_l,
        c.rated_pressure_bar,
        conservative,
    );
    (full - held).max(0.0)
}

/// Free-strategy penetration budget (litres): the sum of each enabled budget-role (back-gas /
/// stage) cylinder's user-chosen IN gas. Dil-out / bailout / deco are excluded as elsewhere.
pub(super) fn free_budget(cylinders: &[Cylinder], conservative: bool) -> f64 {
    cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::BackGas | CylRole::Stage))
        .map(|c| free_in_gas(c, conservative))
        .sum()
}

/// Gas (litres) the All-thirds safety margin (#245) holds back from one cylinder: the deliverable in
/// the top `margin_bar` of the conservative-capped tank. 0 when the margin is 0.
pub(super) fn all_thirds_margin_litres(c: &Cylinder, margin_bar: f64, conservative: bool) -> f64 {
    if margin_bar <= 0.0 {
        return 0.0;
    }
    let basis = cyl_basis(c, conservative);
    let full = deliverable_gas(basis, c.water_volume_l, c.rated_pressure_bar, conservative);
    let held = deliverable_gas(
        (basis - margin_bar).max(0.0),
        c.water_volume_l,
        c.rated_pressure_bar,
        conservative,
    );
    (full - held).max(0.0)
}

/// One budget-role cylinder's All-thirds usable IN gas (litres): ⅓ of its deliverable gas, less the
/// safety margin, clamped ≥ 0. At `margin_bar == 0` this is exactly ⅓ · deliverable (plain thirds),
/// so the default path is unchanged.
pub(super) fn all_thirds_usable_litres(c: &Cylinder, margin_bar: f64, conservative: bool) -> f64 {
    let deliverable = deliverable_gas(
        c.fill_bar,
        c.water_volume_l,
        c.rated_pressure_bar,
        conservative,
    );
    (deliverable / 3.0 - all_thirds_margin_litres(c, margin_bar, conservative)).max(0.0)
}

/// All-thirds back-gas turn gauge (bar) with the safety margin folded in: the volume-weighted start
/// gauge less the back-gas usable IN gas expressed as a common-gauge drop. At `margin_bar == 0` it
/// equals `backgas_turn_pressure(.., 1/3, ..)`.
pub(super) fn all_thirds_backgas_turn(
    cylinders: &[Cylinder],
    margin_bar: f64,
    conservative: bool,
) -> Option<f64> {
    let back: Vec<&Cylinder> = cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::BackGas))
        .collect();
    let sum_wv: f64 = back.iter().map(|c| c.water_volume_l).sum();
    if sum_wv <= 0.0 {
        return None;
    }
    let avg_start = back
        .iter()
        .map(|c| c.fill_bar * c.water_volume_l)
        .sum::<f64>()
        / sum_wv;
    let usable: f64 = back
        .iter()
        .map(|c| all_thirds_usable_litres(c, margin_bar, conservative))
        .sum();
    Some((avg_start - usable / sum_wv).max(0.0))
}

/// The strategy actually applied: the requested preset when the rig carries enough `Stage`
/// cylinders, else All-thirds (the fallback `penetration_budget` already takes). The gauges key off
/// this so a stage-short staged request shows All-thirds numbers, matching the budget + map.
pub(super) fn effective_preset(cylinders: &[Cylinder], preset: ReservePreset) -> ReservePreset {
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

/// Fraction of usable back gas spent on the way in, per strategy (PRD §5.1): thirds / quarters /
/// sixths as the stages carry more of the penetration.
pub(super) fn backgas_fraction(preset: ReservePreset) -> f64 {
    match preset {
        // Free has no single back-gas fraction (the diver sets each tank's usable directly), so
        // its turn gauge is computed separately and never calls this; ⅓ is a harmless placeholder.
        ReservePreset::AllThirds | ReservePreset::Free => 1.0 / 3.0,
        ReservePreset::OneStage => 0.25,
        ReservePreset::TwoStage => 1.0 / 6.0,
    }
}

/// Back-gas turn pressure gauge (bar): the volume-weighted average back-gas start gauge minus the
/// `frac` of usable back gas expressed as a common-gauge drop (`frac · Σdeliverable ÷ Σwater-vol`).
/// `None` when the rig carries no back gas. Z-derated (and conservative-capped) via
/// [`deliverable_gas`], so it tracks the reachable overlay's budget rather than the raw fill.
pub(super) fn backgas_turn_pressure(
    cylinders: &[Cylinder],
    frac: f64,
    conservative: bool,
) -> Option<f64> {
    let back: Vec<&Cylinder> = cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::BackGas))
        .collect();
    let sum_wv: f64 = back.iter().map(|c| c.water_volume_l).sum();
    if sum_wv <= 0.0 {
        return None;
    }
    let avg_start = back
        .iter()
        .map(|c| c.fill_bar * c.water_volume_l)
        .sum::<f64>()
        / sum_wv;
    let usable: f64 = back
        .iter()
        .map(|c| {
            deliverable_gas(
                c.fill_bar,
                c.water_volume_l,
                c.rated_pressure_bar,
                conservative,
            )
        })
        .sum();
    Some((avg_start - frac * usable / sum_wv).max(0.0))
}

/// Per-cylinder penetration allowance (bar), one entry per INPUT cylinder (original order) for the
/// "usable gas" table column (#245). Disabled tanks and non-budget roles are `0.0`. Computed as
/// `fill - turn/drop` so the column can never drift from the gauges/budget: a back-gas tank shows
/// its strategy fraction of usable (`frac · deliverable ÷ water-vol`, matching `backgas_turn`), a
/// counted stage its Half-Minus IN half (`(basis - reserve)/2`, matching its drop), an uncounted
/// stage `0`. Under **Free** it echoes the diver's own clamped input. `enabled_stage_i` counts only
/// enabled Stage cylinders, so "counted" tracks the same first-N-enabled-stages the budget uses.
pub(super) fn cylinder_usable_bars(
    cylinders: &[Cylinder],
    effective: ReservePreset,
    reserve_bar: f64,
    margin_bar: f64,
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
            let deliverable_bar = deliverable_gas(
                c.fill_bar,
                c.water_volume_l,
                c.rated_pressure_bar,
                conservative,
            ) / c.water_volume_l;
            let basis = cyl_basis(c, conservative);
            match effective {
                ReservePreset::Free => match c.role {
                    // The same sanitized `[0, basis]` clamp the budget uses, so the displayed usable
                    // bar equals what `free_in_gas` credits (the "single source of truth" guarantee)
                    // even on an overfilled tank under conservative, and a NaN input reads 0.
                    CylRole::BackGas | CylRole::Stage => free_usable_bar(c, conservative),
                    _ => 0.0,
                },
                // Thirds counts back gas AND stages at ⅓ (both are in `usable_oc_gas`), less the
                // safety margin - expressed as bar so the column equals what the budget credits.
                ReservePreset::AllThirds => match c.role {
                    CylRole::BackGas | CylRole::Stage => {
                        all_thirds_usable_litres(c, margin_bar, conservative) / c.water_volume_l
                    }
                    _ => 0.0,
                },
                ReservePreset::OneStage | ReservePreset::TwoStage => match c.role {
                    CylRole::BackGas => backgas_fraction(effective) * deliverable_bar,
                    CylRole::Stage => {
                        let counted = enabled_stage_i < needed;
                        enabled_stage_i += 1;
                        if counted {
                            (0.5 * (basis - reserve_bar)).max(0.0)
                        } else {
                            0.0
                        }
                    }
                    _ => 0.0,
                },
            }
        })
        .collect()
}

/// Free-strategy back-gas turn gauge (bar): the volume-weighted mean of each enabled back-gas
/// tank's chosen turn pressure `fill - usable_gas_bar`. The usable bar is clamped to `[0, basis]`
/// (the conservative-capped fill, same ceiling the budget uses) so the gauge can't credit more than
/// the plan does. `None` when the rig carries no back gas - matching [`backgas_turn_pressure`].
pub(super) fn free_backgas_turn(cylinders: &[Cylinder], conservative: bool) -> Option<f64> {
    let back: Vec<&Cylinder> = cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::BackGas))
        .collect();
    let sum_wv: f64 = back.iter().map(|c| c.water_volume_l).sum();
    if sum_wv <= 0.0 {
        return None;
    }
    let turn = back
        .iter()
        .map(|c| {
            let u = free_usable_bar(c, conservative);
            (c.fill_bar - u) * c.water_volume_l
        })
        .sum::<f64>()
        / sum_wv;
    Some(turn.max(0.0))
}

/// One-way IN penetration budget (litres), PRD §5.1 - the gas a diver may consume on the way
/// in before turning. The strategies:
/// - **All-thirds**: ⅓ of every back-gas + stage cylinder's deliverable gas (= ⅓ of total).
/// - **Half Minus, 1-stage**: ¼ of back gas + stage 1's IN gas.
/// - **Half Minus, 2-stage**: ⅙ of back gas + that IN gas for the first two stages.
///
/// Under **Half Minus** a stage is breathed from full down to its drop pressure `(fill + reserve)/2`,
/// contributing `(fill - reserve)/2` of IN gas; the other half is breathed back down to `reserve` on
/// the picked-up exit. With the stages carrying the penetration the diver dips less of the back gas
/// (¼ / ⅙ instead of ⅓) without breaking thirds. If the rig lacks the required stages it falls back
/// to the All-thirds budget (never invents stage gas - the safe direction, §6). Z-derated throughout
/// (back-gas AND stage term) via [`deliverable_gas`]. The reserve is the editable
/// [`ReserveProfile::stage_reserve_bar`] (default 30 bar).
///
/// "Stage 1"/"the first two stages" = the first `Stage`-role cylinder(s) by rig (vec) order -
/// v1 has no per-stage drop-order field. With ≥3 stages on `TwoStage` only the first two count
/// (the ≥3-stage case is deferred - PRD §5.1); excluding the extra only shrinks the budget (safe).
pub(super) fn penetration_budget(
    cylinders: &[Cylinder],
    preset: ReservePreset,
    profile: &ReserveProfile,
    conservative: bool,
) -> f64 {
    // All-thirds = ⅓ of each budget-role cylinder's deliverable, less the safety margin (0 by
    // default, so this is plain thirds). The margin is All-thirds-only - a fell-back staged rig is
    // now effectively All-thirds, so it inherits the margin too.
    let all_thirds = || {
        cylinders
            .iter()
            .filter(|c| matches!(c.role, CylRole::BackGas | CylRole::Stage))
            .map(|c| {
                all_thirds_usable_litres(c, profile.all_thirds_safety_margin_bar, conservative)
            })
            .sum::<f64>()
    };

    let stages: Vec<&Cylinder> = cylinders
        .iter()
        .filter(|c| matches!(c.role, CylRole::Stage))
        .collect();
    if stages.len() < required_stages(preset) {
        return all_thirds();
    }

    let back_gas = |frac: f64| -> f64 {
        cylinders
            .iter()
            .filter(|c| matches!(c.role, CylRole::BackGas))
            .map(|c| {
                deliverable_gas(
                    c.fill_bar,
                    c.water_volume_l,
                    c.rated_pressure_bar,
                    conservative,
                )
            })
            .sum::<f64>()
            * frac
    };
    let stage_in = |c: &Cylinder| stage_in_gas(c, profile.stage_reserve_bar, conservative);

    match preset {
        ReservePreset::AllThirds => all_thirds(),
        ReservePreset::Free => free_budget(cylinders, conservative),
        ReservePreset::OneStage => back_gas(0.25) + stage_in(stages[0]),
        ReservePreset::TwoStage => {
            back_gas(1.0 / 6.0) + stages[..2].iter().map(|&c| stage_in(c)).sum::<f64>()
        }
    }
}

/// Gas breathed off one stage on the way IN under the **Half-Minus** strategy (litres): from
/// `full` down to its drop pressure `(fill + reserve)/2`, i.e. `(fill - reserve)/2` of pressure.
/// The drop keys off the SAME (conservative-capped) basis as `full`, so an overfilled stage's
/// start and drop pressures stay on one consistent scale. The other half is breathed back down to
/// `reserve` on the picked-up exit; only this IN half is penetration gas. Z-derated via
/// [`deliverable_gas`].
pub(super) fn stage_in_gas(c: &Cylinder, reserve_bar: f64, conservative: bool) -> f64 {
    let basis = if conservative {
        c.fill_bar.min(c.rated_pressure_bar)
    } else {
        c.fill_bar
    };
    let full = deliverable_gas(basis, c.water_volume_l, c.rated_pressure_bar, conservative);
    let drop = (0.5 * (basis + reserve_bar)).clamp(0.0, basis);
    let held = deliverable_gas(drop, c.water_volume_l, c.rated_pressure_bar, conservative);
    (full - held).max(0.0)
}

/// Total usable **open-circuit** breathing gas (litres) across the diver's back-gas + stage
/// cylinders. Dil-out / bailout / deco roles are excluded (CCR / deco, not OC penetration gas,
/// §5.3/§5.5). Every cylinder is Z-derated via [`deliverable_gas`] (real-gas correction is
/// always applied); `conservative` additionally caps an overfilled tank to its rated capacity
/// (the "call-it-an-80" community convention, §5.3). Retained as a tested building block - the
/// All-thirds budget now sums per-cylinder `all_thirds_usable_litres` (to fold in the safety margin),
/// so this is exercised only by tests.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "tested building block; budget now sums per-cylinder"
    )
)]
pub(super) fn usable_oc_gas(cylinders: &[Cylinder], conservative: bool) -> f64 {
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
