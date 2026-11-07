//! Gas & route-planning config types (issue #38).
//!
//! Originally split out of `config/types.rs` to keep that file under the
//! file-size ratchet; now lives under `models::config` as pure domain config.
//! Re-exported from `crate::config` (via `models::config`'s `pub use gas::*`),
//! so `crate::config::GasConfig` etc. resolve unchanged.

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

fn default_true() -> bool {
    true
}

fn default_swim_speed() -> f64 {
    10.0
}

fn default_survey_rmv_multiplier() -> f64 {
    1.0
}

fn default_stressed_rmv_multiplier() -> f64 {
    1.5
}

fn default_jump_rmv_multiplier() -> f64 {
    1.5
}

fn default_stage_reserve_bar() -> f64 {
    30.0
}

fn default_all_thirds_safety_margin_bar() -> f64 {
    0.0
}

/// Upper bound on the All-thirds safety margin (bar). A margin only ever SHRINKS usable gas (the
/// safe direction), so the high side is merely capped for sanity; per-cylinder the engine clamps
/// the resulting usable at 0, so an over-large margin just zeroes a small tank rather than crediting
/// negative gas.
const MAX_ALL_THIRDS_SAFETY_MARGIN_BAR: f64 = 200.0;

/// Upper bound on the per-stage reserve. A LARGER reserve only shrinks a stage's usable IN gas
/// (`(fill - reserve)/2`) - the *safe* direction - so the high side is merely capped for sanity,
/// not rejected; the genuinely unsafe direction is a *negative* reserve (it would drop the stage
/// lower and over-state reach), which `normalize` resets. No real "half, less a reserve" rule
/// holds back more than ~100 bar of a stage.
const MAX_STAGE_RESERVE_BAR: f64 = 100.0;

/// Upper bound on the DPV range reserve. Held just below 1.0 so the cap can never be driven to
/// exactly zero, which would report every scootered station unreachable with no explanation.
const MAX_DPV_RANGE_RESERVE_FRACTION: f64 = 0.9;

fn default_recalc_hold_stage2_bar() -> f64 {
    100.0
}

fn default_recalc_hold_stage1_bar() -> f64 {
    70.0
}

pub fn default_dpv_range_reserve_fraction() -> f64 {
    0.1
}

/// Display units for gas: metric (bar / litres) or imperial (psi / cuft).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum GasUnits {
    #[default]
    Metric,
    Imperial,
}

/// Reserve preset (PRD §5.1). All-thirds is the safe default; the 1-/2-stage presets are
/// pushy and advanced-gated in the UI. `Free` (#245) hands the diver full manual control: the
/// engine ignores the reserve rules and budgets the per-cylinder usable gas the user types
/// (`Cylinder::usable_gas_bar`), clamped to the conservative-capped fill - for planned/tech divers
/// who compute their own reserves.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ReservePreset {
    #[default]
    AllThirds,
    OneStage,
    TwoStage,
    Free,
}

/// Editable reserve constants (PRD §5.1) - kept out of the engine as named, tunable values
/// rather than magic literals. `stage_reserve_bar` is the gas held back in each stage under the
/// **Half Minus** strategy (default 30 bar): a stage spends `(basis - reserve)/2` of its gauge on
/// the way in - dropped at `fill - (basis - reserve)/2`, i.e. `(fill + reserve)/2` at or below
/// rated - and the other half is breathed back down to `reserve` on the picked-up exit. The two
/// `recalc_hold_*` constants are the back-gas held pressures (100 / 70 bar) for the Phase-2
/// return-leg recalc - carried now so the config shape is stable across phases; that compute path
/// is not implemented yet.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
pub struct ReserveProfile {
    #[serde(default = "default_stage_reserve_bar")]
    pub stage_reserve_bar: f64,
    /// Extra pressure (bar) held back in EVERY budget-role cylinder under **All-thirds** (#245), on
    /// top of the thirds reserve: each tank's turn pressure rises by this many bar - a diver-set
    /// conservatism knob for teams of two or specific caves. Default 0 (plain thirds). Only ever
    /// shrinks usable gas (the safe direction).
    #[serde(default = "default_all_thirds_safety_margin_bar")]
    pub all_thirds_safety_margin_bar: f64,
    #[serde(default = "default_recalc_hold_stage2_bar")]
    pub recalc_hold_stage2_bar: f64,
    #[serde(default = "default_recalc_hold_stage1_bar")]
    pub recalc_hold_stage1_bar: f64,
}

impl Default for ReserveProfile {
    fn default() -> Self {
        Self {
            stage_reserve_bar: default_stage_reserve_bar(),
            all_thirds_safety_margin_bar: default_all_thirds_safety_margin_bar(),
            recalc_hold_stage2_bar: default_recalc_hold_stage2_bar(),
            recalc_hold_stage1_bar: default_recalc_hold_stage1_bar(),
        }
    }
}

/// One DPV (scooter). Same RMV as swimming - scootering gets no reduced-RMV credit (PRD §10),
/// which is the conservative direction.
///
/// The planning quantity is neither the speed nor the burn time on its own but their product,
/// the **line the unit can actually cover** (see [`DpvUnit::range_m`]). A slower backup needs
/// more minutes to fly the same passage home, so comparing raw burn times would silently favour
/// the wrong unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct DpvUnit {
    /// Cruising speed (m/min). The speed a segment flown on this scooter is priced at.
    pub speed_m_per_min: f64,
    /// Burn time at cruising speed (minutes).
    pub burn_minutes: f64,
}

impl DpvUnit {
    /// Line this unit can cover on a full charge: `burn x cruising speed` (metres).
    pub fn range_m(&self) -> f64 {
        self.speed_m_per_min * self.burn_minutes
    }
}

/// A DPV rig: the primary plus the mandatory backup (#317).
///
/// **A scootered dive without a backup is not modelled at all.** That single decision removes the
/// whole dead-scooter contingency from the gas math: you fly in on the primary and out on the
/// backup, so the exit mirrors the entry segment for segment and `gas_out == gas_in` exactly. The
/// existing fraction reserves (thirds, Half-Minus, Free) therefore stay correct and untouched, and
/// the scooter only ever adds a SECOND, non-gas constraint - see [`DpvRig::usable_range_m`].
///
/// ⚠ That symmetry is EXACT only while both units cruise at the same speed, which the seeded
/// default does. Gas on a span is `rmv x ata(depth) x length / speed`, so a slower backup takes
/// proportionally longer to fly the same passage home and costs proportionally more gas to do it -
/// a 55/30 rig spends roughly 1.83x the entry gas on each scootered span of the exit, and nothing
/// in the engine accounts for that today. What absorbs it is the fraction reserve: thirds and
/// Half-Minus hold back more than the entry leg cost, and that margin is the buffer a mismatched
/// pair eats into. Pricing the exit on the backup's own speed is a domain call for the maintainers,
/// not something to slip in behind this comment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
pub struct DpvRig {
    pub primary: DpvUnit,
    pub backup: DpvUnit,
    /// Fraction of the weaker unit's range held back (0.0..1.0). Proportional rather than a flat
    /// subtraction because what it guards against scales with distance: an ageing battery delivers
    /// a percentage less, and flow costs a percentage of your speed.
    #[serde(default = "default_dpv_range_reserve_fraction")]
    pub range_reserve_fraction: f64,
}

impl DpvRig {
    /// Scootered penetration allowance (metres, one-way).
    ///
    /// `min(primary.range, backup.range) x (1 - reserve)`. The minimum because either unit failing
    /// must still leave a working one able to cover the scootered line; **not halved**, because the
    /// primary only ever flies in and the backup only ever flies out, so each is asked for one
    /// direction and halving on top would reserve for a double failure.
    ///
    /// The cap counts metres at cruise. A span ridden slower drains each unit's battery by its
    /// own clock, so it is charged more than a metre per metre - whichever unit it drains harder
    /// (`range_per_metre` in `project/gas/scooter.rs`).
    pub fn usable_range_m(&self) -> f64 {
        let weaker = self.primary.range_m().min(self.backup.range_m());
        (weaker * (1.0 - self.range_reserve_fraction)).max(0.0)
    }
}

/// Gas & route-planning defaults (issue #38). Persisted *personal defaults* a plan is
/// seeded from - never the per-dive plan itself (that stays ephemeral, never in the
/// project). The master metric/imperial toggle drives `units` alongside `UnitsConfig`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GasConfig {
    /// Display units for gas pressure/volume (kept in step with `UnitsConfig.length` by the
    /// master toggle, PRD §7.2 - not an independent control).
    #[serde(default)]
    pub units: GasUnits,
    /// Default reserve preset for a new plan (PRD §5.1).
    #[serde(default)]
    pub default_reserve: ReservePreset,
    /// Editable reserve constants (the Half-Minus per-stage reserve + the Phase-2 recalc holds), §5.1.
    #[serde(default)]
    pub reserve_profile: ReserveProfile,
    /// Exit-leg swim speed (m/min). Default 10 (no DPV).
    #[serde(default = "default_swim_speed")]
    pub swim_speed_m_per_min: f64,
    /// Survey-in speed (m/min); often 2-3× slower than the swim-out. `None` = symmetric
    /// (swim speed both ways). PRD §5.2.
    #[serde(default)]
    pub survey_speed_m_per_min: Option<f64>,
    /// Opt-in task-loading RMV bump on the survey-in leg only. Default 1.0 (no bump).
    #[serde(default = "default_survey_rmv_multiplier")]
    pub survey_rmv_multiplier: f64,
    /// Stressed-RMV multiplier for CCR bailout / contingency. Default 1.5×, user-settable.
    #[serde(default = "default_stressed_rmv_multiplier")]
    pub stressed_rmv_multiplier: f64,
    /// Per-jump RMV multiplier applied to *reference/jump* legs only (not permanent shots).
    /// Tying in a jump is extra task-loading that raises the breathing rate, so its gas burns
    /// at a higher RMV. **Default 1.5** ("putting in a jump is work"). Always ≥1.0 after
    /// `normalize`, so it can only ever *raise* gas (the conservative direction, §6).
    /// ⚠ **Stacks MULTIPLICATIVELY** with `survey_rmv_multiplier` on jump legs:
    /// the engine uses `edge_rmv = rmv · survey_rmv_multiplier · jump_rmv_multiplier`. With the
    /// default `survey_rmv_multiplier = 1.0` there is no stacking (jumps burn `rmv · 1.5`);
    /// raising both compounds (e.g. 1.5 × 1.5 = 2.25×).
    /// Since #27 this applies to [`ShotKind::Jump`] only. A
    /// [`ShotKind::Reference`] is not swimmable at all, so it leaves the gas
    /// graph entirely rather than burning gas at a higher rate.
    #[serde(default = "default_jump_rmv_multiplier")]
    pub jump_rmv_multiplier: f64,
    /// Opt-in escape hatch (#27 q13): when `true`, reference legs are treated as
    /// divable again and rejoin the gas / penetration / distance graphs exactly
    /// as they did before #27.
    ///
    /// Default `false`, which is the **safe** direction: a reference tie is a
    /// tape measurement or backsight, not a swimmable passage, and routing a
    /// dive through one invents a shortcut that does not exist. That is the
    /// precise failure #77 left open. Excluding an edge can only ever shrink
    /// reachability or lengthen a route, never overstate either (§6).
    ///
    /// Turn it on if your survey uses reference shots to record legs you really
    /// do swim and you would rather have the old, inclusive numbers.
    #[serde(default)]
    pub include_reference_legs: bool,
    /// `true` (default) = the conservative "call-it-an-80" convention: every strategy's share of a
    /// cylinder's gauge is taken of `min(fill, rated)`, so an overfill never earns a larger share;
    /// `false` = of the fill, for the few who want it (PRD §5.3). Either way the credited gas is
    /// the Z-corrected gas from the real fill down to the resulting turn or drop pressure.
    #[serde(default = "default_true")]
    pub conservative_tank_gas: bool,
    /// Optional DPV rig (#317). `None` = no scooter, and the engine behaves exactly as it did
    /// before scooters existed: the range cap is infinite and never binds.
    ///
    /// Deserialized via [`lenient_dpv`] rather than the plain derive: this field replaced the
    /// old `DpvProfile` shape, and a config written by a previous build carries a `[gas.dpv]`
    /// table that no longer fits `DpvRig`. Erroring here would fail the WHOLE config parse -
    /// `Config::load().unwrap_or_default()` then silently resets every OTHER setting because one
    /// optional block changed shape. An incompatible `dpv` value degrades to `None` instead,
    /// exactly as if the key were absent.
    #[serde(default, deserialize_with = "lenient_dpv")]
    pub dpv: Option<DpvRig>,
}

/// Tolerant deserializer for [`GasConfig::dpv`] (#317). Captures the raw value as a
/// `serde_json::Value` first - this works through any self-describing `Deserializer`
/// (TOML's included), since `Value`'s own `Deserialize` impl only relies on `deserialize_any`,
/// not on the JSON wire format - and only then attempts `DpvRig`. A value that does not fit
/// (an old-shape `DpvProfile` table, a stray scalar, anything) degrades to `None` rather than
/// failing the field, so one shape change can never take down the rest of the config.
fn lenient_dpv<'de, D>(deserializer: D) -> Result<Option<DpvRig>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value::<DpvRig>(value).ok())
}

impl Default for GasConfig {
    fn default() -> Self {
        Self {
            units: GasUnits::default(),
            default_reserve: ReservePreset::default(),
            reserve_profile: ReserveProfile::default(),
            swim_speed_m_per_min: default_swim_speed(),
            survey_speed_m_per_min: None,
            survey_rmv_multiplier: default_survey_rmv_multiplier(),
            stressed_rmv_multiplier: default_stressed_rmv_multiplier(),
            jump_rmv_multiplier: default_jump_rmv_multiplier(),
            include_reference_legs: false,
            conservative_tank_gas: true,
            dpv: None,
        }
    }
}

impl GasConfig {
    /// Clamp non-finite / non-positive speeds and multipliers back to their defaults, and drop a
    /// DPV profile whose speed is invalid. A foreign or hand-edited config can carry a 0 / NaN /
    /// negative `swim_speed_m_per_min` that would later divide into `edge_gas` (→ +Inf or NaN gas
    /// and a poisoned reachability field), so guard it at the config boundary - mirrors the other
    /// sub-configs' `normalize()`. Called from `Config::normalize`.
    pub fn normalize(&mut self) {
        if !self.swim_speed_m_per_min.is_finite() || self.swim_speed_m_per_min <= 0.0 {
            self.swim_speed_m_per_min = default_swim_speed();
        }
        if let Some(survey) = self.survey_speed_m_per_min {
            // Survey-in is slower than (or equal to) the swim-out; a survey speed FASTER than the
            // swim speed is backwards and would make the in-leg-only reach field OVER-state reach
            // (the unsafe direction, §6), so treat it as symmetric (None). Runs after the swim
            // clamp above, so `self.swim_speed_m_per_min` is already valid.
            if !survey.is_finite() || survey <= 0.0 || survey > self.swim_speed_m_per_min {
                self.survey_speed_m_per_min = None;
            }
        }
        // survey / stressed / jump RMV multipliers are all work-factors RELATIVE to the
        // baseline swim RMV, so none is ever LESS work than baseline: a value below 1.0 would
        // under-state a leg's gas and over-state reach (the unsafe direction, #241 / §6). Clamp
        // anything non-finite or < 1.0 back to its default (defaults: survey 1.0, stressed 1.5,
        // jump 1.5), so a hand-edited config can only ever raise gas, never lower it.
        if !self.survey_rmv_multiplier.is_finite() || self.survey_rmv_multiplier < 1.0 {
            self.survey_rmv_multiplier = default_survey_rmv_multiplier();
        }
        if !self.stressed_rmv_multiplier.is_finite() || self.stressed_rmv_multiplier < 1.0 {
            self.stressed_rmv_multiplier = default_stressed_rmv_multiplier();
        }
        if !self.jump_rmv_multiplier.is_finite() || self.jump_rmv_multiplier < 1.0 {
            self.jump_rmv_multiplier = default_jump_rmv_multiplier();
        }
        // Reserve-profile constants. The per-stage reserve feeds the Half-Minus stage drop
        // `(fill + reserve)/2` and IN gas `(fill - reserve)/2`: a NEGATIVE reserve drops the
        // stage lower and credits more IN gas → the budget OVER-states reach (the unsafe
        // direction, §6), so reset it to the default. A LARGE reserve only shrinks the IN gas
        // (the safe side), so the high side is merely capped at MAX rather than reset (resetting
        // it to the smaller default would be *less* conservative). The recalc holds are carried
        // for Phase 2 but guarded now.
        let p = &mut self.reserve_profile;
        if !p.stage_reserve_bar.is_finite() || p.stage_reserve_bar < 0.0 {
            p.stage_reserve_bar = default_stage_reserve_bar();
        } else if p.stage_reserve_bar > MAX_STAGE_RESERVE_BAR {
            p.stage_reserve_bar = MAX_STAGE_RESERVE_BAR;
        }
        // All-thirds safety margin: a NEGATIVE margin would ADD usable gas (over-state reach, the
        // unsafe direction) so reset it to 0; a large one only shrinks usable (safe) so just cap it.
        if !p.all_thirds_safety_margin_bar.is_finite() || p.all_thirds_safety_margin_bar < 0.0 {
            p.all_thirds_safety_margin_bar = default_all_thirds_safety_margin_bar();
        } else if p.all_thirds_safety_margin_bar > MAX_ALL_THIRDS_SAFETY_MARGIN_BAR {
            p.all_thirds_safety_margin_bar = MAX_ALL_THIRDS_SAFETY_MARGIN_BAR;
        }
        if !p.recalc_hold_stage2_bar.is_finite() || p.recalc_hold_stage2_bar < 0.0 {
            p.recalc_hold_stage2_bar = default_recalc_hold_stage2_bar();
        }
        if !p.recalc_hold_stage1_bar.is_finite() || p.recalc_hold_stage1_bar < 0.0 {
            p.recalc_hold_stage1_bar = default_recalc_hold_stage1_bar();
        }
        // DPV rig (#317). A unit whose speed or burn is 0 / NaN / negative has no meaningful range,
        // and the engine would divide the speed into `edge_gas` (→ +Inf or NaN gas and a poisoned
        // field), so drop the WHOLE rig rather than repairing one unit: an invented backup would
        // silently set the penetration cap, and inventing a limit is worse than having none.
        // Without a rig the cap is infinite and only the gas budget binds - the pre-scooter
        // behaviour, which is safe because nothing is then flown on a scooter either.
        if self
            .dpv
            .as_ref()
            .is_some_and(|rig| !dpv_unit_is_valid(&rig.primary) || !dpv_unit_is_valid(&rig.backup))
        {
            self.dpv = None;
        }
        if let Some(rig) = self.dpv.as_mut() {
            // A NEGATIVE reserve fraction would CREDIT more range than the units have - over-stating
            // reach, the unsafe direction (§6) - so reset it to the default. At/above 1.0 it would
            // zero the cap and make every scootered station unreachable; that is merely useless
            // rather than dangerous, so clamp it just below instead of resetting.
            if !rig.range_reserve_fraction.is_finite() || rig.range_reserve_fraction < 0.0 {
                rig.range_reserve_fraction = default_dpv_range_reserve_fraction();
            } else if rig.range_reserve_fraction > MAX_DPV_RANGE_RESERVE_FRACTION {
                rig.range_reserve_fraction = MAX_DPV_RANGE_RESERVE_FRACTION;
            }
        }
    }
}

/// A DPV unit is usable only if both its speed and its burn time are finite and positive - the
/// range is their product, so a zero in either makes the unit cover no line at all.
fn dpv_unit_is_valid(unit: &DpvUnit) -> bool {
    unit.speed_m_per_min.is_finite()
        && unit.speed_m_per_min > 0.0
        && unit.burn_minutes.is_finite()
        && unit.burn_minutes > 0.0
}
