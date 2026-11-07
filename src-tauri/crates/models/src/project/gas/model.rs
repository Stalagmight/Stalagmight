//! Gas-planning domain model: divers, cylinders, teams. Canonical SI (L/min, L, bar, m).
//! Constructors guard non-finite / non-positive inputs at the ingest boundary (mirroring
//! `Coordinates::new_checked`), so a NaN RMV or zero cylinder can never poison the gas
//! field downstream. The persisted/per-plan `GasConfig` (units, reserve preset, speeds)
//! lives in `crate::config` - these are the per-dive, ephemeral parts.

// The types/fields are `pub` (re-exported to `models` for IPC + ts-rs), so they are NOT dead;
// but the guarding constructors (`new` / `from_sac` / `gas_volume_l`) are exercised only by
// tests until the Phase-1.3 command uses them for input validation - dead in the lib build
// until then, so the expectation is scoped to the non-test build (matches `models/shot.rs`;
// `#[allow]` is denied crate-wide).
#![cfg_attr(not(test), expect(dead_code))]

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::StationRef;

use super::physics::{cylinder_gas_volume, sac_to_rmv};

fn default_true() -> bool {
    true
}

/// Cylinder construction material. Hints role suitability - steel ≈ negative buoyancy →
/// back gas; aluminium ≈ neutral → fine as a stage - but it is guidance, never a hard
/// rule (PRD §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum TankMaterial {
    Aluminium,
    Steel,
}

/// What a cylinder is carried for. Drives the reserve math (§5.1) and the CCR "dil-out"
/// model (§5.5). `Deco` gas is reserved for a future deco phase and is **never**
/// travel/penetration gas, so it is excluded from the penetration budget (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CylRole {
    BackGas,
    Stage,
    DilOut,
    Bailout,
    Deco,
}

/// Open-circuit vs CCR bailout planning. CCR is not gas-limited on penetration; its
/// constraint is the bailout volume to exit (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum PlanMode {
    OpenCircuit,
    CcrBailout,
}

/// A diver identified by RMV - the canonical, persisted gas-rate. SAC is a *constructor*
/// concern only (see [`Diver::from_sac`]); it is never stored, so an inconsistent
/// SAC/reference pair can't linger in the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Diver {
    pub rmv_l_per_min: f64,
    pub label: Option<String>,
}

impl Diver {
    /// Construct from an already-volumetric RMV (L/min). Rejects non-finite / non-positive.
    pub fn new(rmv_l_per_min: f64, label: Option<String>) -> Result<Self, String> {
        if !rmv_l_per_min.is_finite() || rmv_l_per_min <= 0.0 {
            return Err(format!(
                "RMV must be finite and positive, got {rmv_l_per_min}"
            ));
        }
        Ok(Self {
            rmv_l_per_min,
            label,
        })
    }

    /// Construct from a SAC (bar/min) measured against a reference cylinder's water
    /// volume. The SAC is consumed here and discarded; only the derived RMV is kept.
    pub(crate) fn from_sac(
        sac_bar_per_min: f64,
        ref_cyl_water_volume_l: f64,
        label: Option<String>,
    ) -> Result<Self, String> {
        if !sac_bar_per_min.is_finite() || sac_bar_per_min <= 0.0 {
            return Err(format!(
                "SAC must be finite and positive, got {sac_bar_per_min}"
            ));
        }
        if !ref_cyl_water_volume_l.is_finite() || ref_cyl_water_volume_l <= 0.0 {
            return Err(format!(
                "Reference cylinder volume must be finite and positive, got {ref_cyl_water_volume_l}"
            ));
        }
        Self::new(sac_to_rmv(sac_bar_per_min, ref_cyl_water_volume_l), label)
    }
}

/// A single cylinder. `fill_bar` is an INPUT and may exceed `rated_pressure_bar` -
/// overfilling LP steels to MP or higher is routine (§5.3) - so rated is reference only
/// and is **never** used to clamp the fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Cylinder {
    pub water_volume_l: f64,
    pub rated_pressure_bar: f64,
    pub fill_bar: f64,
    pub material: TankMaterial,
    pub role: CylRole,
    pub label: Option<String>,
    /// Whether this cylinder is carried on THIS dive (#245). A disabled tank stays in the
    /// table (greyed) but is excluded from every gas computation - budget, gauges, drops -
    /// so a diver can toggle rig variants without deleting rows. `#[serde(default)]` = `true`,
    /// so a plan/config saved before this field loads with all tanks enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Under the **Free** strategy (#245) this is the diver's manually-chosen penetration
    /// allowance (bar) - the gas they'll breathe off this tank on the way IN before turning.
    /// The engine budgets exactly this (Z-derated span from `fill` down to `fill - usable_gas_bar`).
    /// Ignored by every other strategy, which COMPUTE the usable bar from the reserve rules and
    /// report it back on `GasReachField::cylinder_usable_bars`. `#[serde(default)]` = `0.0`.
    #[serde(default)]
    pub usable_gas_bar: f64,
}

impl Cylinder {
    pub fn new(
        water_volume_l: f64,
        rated_pressure_bar: f64,
        fill_bar: f64,
        material: TankMaterial,
        role: CylRole,
        label: Option<String>,
    ) -> Result<Self, String> {
        for (name, value) in [
            ("water volume", water_volume_l),
            ("rated pressure", rated_pressure_bar),
            ("fill", fill_bar),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "Cylinder {name} must be finite and positive, got {value}"
                ));
            }
        }
        Ok(Self {
            water_volume_l,
            rated_pressure_bar,
            fill_bar,
            material,
            role,
            label,
            enabled: true,
            usable_gas_bar: 0.0,
        })
    }

    /// Ideal free-gas volume (litres) at the current fill. Ideal-gas (see the §5.3
    /// real-gas caveat - the conservative derate / Z correction is applied by the engine,
    /// not baked in here).
    pub(crate) fn gas_volume_l(&self) -> f64 {
        cylinder_gas_volume(self.fill_bar, self.water_volume_l)
    }
}

/// A team of divers, each with their own cylinders, planned in one [`PlanMode`]. Phase 1
/// collapses to one diver / one cylinder; the `Vec` shape exists from day one so CCR
/// (§5.5) and the team feature (Phase 5) add no migrations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Team {
    pub divers: Vec<Diver>,
    pub cylinders_per_diver: Vec<Vec<Cylinder>>,
    pub mode: PlanMode,
}

/// One span of the planned route, with the pace and propulsion the diver plans to use on it (#317).
///
/// Segments are bounded by the ordered way points (#298), so a plan has exactly
/// `waypoints.len() + 1` of them: segment `k` runs from way point `k-1` to way point `k`, and the
/// last covers the open reach field past the final way point. Reusing the way points as boundaries
/// is what keeps this free of any new anchor type to place, resolve, persist and degrade.
///
/// ⚠ **`on_scooter` covers both towing and parking, and that is deliberate.** Pushing a scooter
/// through a restriction and leaving it parked there are numerically identical - swim-speed gas,
/// zero scooter range consumed - and differ only in narrative. So the drop point is *derived* (the
/// boundary after the last scootered segment) rather than being a separate thing to place. A
/// consequence worth keeping: no unflyable plan is representable, because a swim segment between
/// two scooter segments is just a tow.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
pub struct PlanSegment {
    /// Travel speed on this span (m/min), or `None` to fly it at the plan's own speed
    /// (`survey_speed_m_per_min` falling back to `swim_speed_m_per_min`). Gas is
    /// `rmv x ata(avg_depth) x length / speed`, so this is the only per-segment quantity the reach
    /// field needs.
    ///
    /// ⚠ **`None` is load-bearing, not a convenience.** The frontend always sends a full-length
    /// list, so a plain `f64` here would mean every span always carries an explicit number and the
    /// diver's configured swim / survey speed could never reach the engine again - which is exactly
    /// what shipped and had to be fixed. Only a span the diver has actually edited holds a number.
    pub speed_m_per_min: Option<f64>,
    /// Whether this span is flown under scooter power, and so spends the rig's usable range.
    pub on_scooter: bool,
}

/// Result of the gas-weighted reachability compute - the gas analogue of `DistanceField`.
/// Per-station gas-to-reach from the origin (one-way, in-leg), the reachable mask under the
/// reserve budget, and the turn frontier (reachable stations with an unreachable neighbour).
/// `gas_to_reach` is `None` for graph-unreachable stations (JSON-safe ∞); `station_refs` echo
/// the input order so the field is self-describing.
///
/// One leg of the planned route - origin→target, station to station. `is_jump` marks a
/// reference/jump shot (a swum shortcut normally hidden in the survey view), so the UI can draw it
/// dashed and distinct from a regular swum leg. A merge / loop-closure tie (coincident points) is
/// `is_jump = false` (zero-length, drawn solid).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct RouteLeg {
    pub from: StationRef,
    pub to: StationRef,
    pub is_jump: bool,
}

/// One point on the anticipated dive profile (#299): a route station priced at the moment the diver
/// is standing on it. The series runs origin→turnaround (`outbound`), then the return leg back to
/// the origin, so it describes the whole round trip the budget is sized for.
///
/// The two directions are **not** symmetric, and that is the point of shipping both. Distance
/// mirrors exactly - the diver swims the same line back - but the engine prices the way IN at each
/// route span's own speed (#317, defaulting to the survey speed) on the survey RMV - the basis of
/// every gas and time number in this struct - and the way OUT at `swim_speed_m_per_min` with the
/// diver's base RMV, which `GasConfig::normalize` guarantees is no slower than the survey speed. So
/// a swum return leg is shorter in time and cheaper in gas than the identical distance was on the
/// way in; a span the diver SCOOTERED in is charged the swim pace going out, which over-states the
/// exit rather than under-stating it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct RouteProfilePoint {
    /// Position along the route's station sequence, 0 = origin. A way-point route can revisit a
    /// station, so this - not `station_ref` - says which pass this point is on; it is the same index
    /// space as `stage_drop_route_indices` / `waypoint_route_indices`. Counts back down on the
    /// return leg, so every index except the turnaround's appears twice - the diver stands on the
    /// turnaround once, and finishes back on the origin.
    pub route_index: u32,
    pub station_ref: StationRef,
    /// Cumulative swum distance (metres) from the origin, zero-distance merge/closure ties excluded -
    /// the same swum basis as `route_line_length_m` and `stage_drop_distances_m`. Runs 0 → L
    /// outbound and L → 2L back, so it is monotonic across the whole series.
    pub distance_m: f64,
    /// Depth (metres, positive = below surface) from the measured coordinates.
    pub depth_m: f64,
    /// Cumulative gas (litres) consumed on arrival here. ⚠ Compare only the OUTBOUND values against
    /// `budget_litres`: that budget is the one-way penetration allowance (the reserve fraction is
    /// what pays for the return), so the round trip legitimately finishes above it.
    pub gas_l: f64,
    /// Cumulative elapsed time (minutes) on arrival here, at each span's own speed - so the last
    /// outbound point IS `route_runtime_min`, not a re-derivation of it. Not a deco schedule - no
    /// ascent, no stops, no decompression is modelled anywhere in this app.
    pub elapsed_min: f64,
    /// `false` for the mirrored return leg.
    pub outbound: bool,
}

/// The `route` / `stage_drop_refs` / `final_turn_ref` describe the **planned route** to the route
/// target (the user-selected endpoint, else the farthest in-budget point): the ordered legs from
/// the origin to the target (for the route highlight), the station where each stage's IN gas runs
/// out (its drop point - staged strategies only), and the final turn. `route` + `final_turn_ref`
/// are populated for **any** strategy when a target is reachable; `stage_drop_refs` is **empty for
/// All-thirds** (and any rig that falls back to it - no per-stage drops).
///
/// `backgas_turn_bar` and `stage_drop_bars` are the **readout gauges**, computed here (the single
/// source of truth) so the UI never re-derives the budget math and drifts from the reachable
/// overlay: the back-gas turn pressure under the effective strategy's share of the gauge, and each
/// counted stage's drop pressure. Every rule is a gauge rule (#576), and the budget credits exactly
/// the gas between each cylinder's real fill and its turn or drop pressure, so gauge and map agree.
/// Both honour the conservative cap + normalized reserve + stage-count fallback.
/// `backgas_turn_bar` is `None` when the rig has no back gas; `stage_drop_bars` is empty for
/// All-thirds / a fell-back staged strategy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct GasReachField {
    pub station_refs: Vec<StationRef>,
    pub gas_to_reach: Vec<Option<f64>>,
    pub reachable: Vec<bool>,
    pub turn_frontier: Vec<StationRef>,
    pub max_reachable_gas: f64,
    pub budget_litres: f64,
    /// Ordered legs origin→target (for the route highlight); empty when nothing is reachable.
    /// With a pinned stage drop the route passes the pin (#576 RV8c-2-D1): the way points, then
    /// every pin, in drop order, then on to the endpoint - doubling back where a pin is off its
    /// passage - or, with no endpoint, to the deepest station beyond the last pin.
    pub route: Vec<RouteLeg>,
    /// Station where each stage's IN gas is exhausted, in stage order (staged strategies only), or
    /// the station the diver pinned it to. A pinned drop the dive runs out of gas (or scooter
    /// range) before is still named here, at its pin past the turnaround - see
    /// `stage_drop_route_indices` for how to tell it is not reached.
    pub stage_drop_refs: Vec<StationRef>,
    /// The final turn - the route target station; `None` when nothing is reachable.
    pub final_turn_ref: Option<StationRef>,
    /// Whether `final_turn_ref` lies on the way to the endpoint the diver chose.
    ///
    /// True when the endpoint is itself the target, and when the gas ran out partway along the
    /// route to it (the plan turns early on that same passage). FALSE for the deepest-in-budget
    /// fallback, which is an arg-max over the whole cave and so can sit in a completely unrelated
    /// lead - reached when no endpoint is set at all, or when one is set but unusable (its station
    /// was deleted under a loaded plan, or it is in another graph component).
    ///
    /// The UI needs the distinction because those read identically otherwise: both report a turn
    /// station that is not the endpoint, but only one of them is honestly "on the way there".
    pub turn_toward_endpoint: bool,
    /// Back-gas turn pressure gauge (bar); `None` when the rig has no back gas.
    pub backgas_turn_bar: Option<f64>,
    /// Per-stage Half-Minus drop pressure gauge (bar), one per counted stage; empty for All-thirds.
    pub stage_drop_bars: Vec<f64>,
    /// Whether each stage drop is **over-reserve** (#246), aligned 1:1 with `stage_drop_refs` /
    /// `stage_drop_bars`. `true` only when the diver dragged that drop DEEPER than its
    /// strategy-computed (gas-safe) point, so the stage is breathed past its reserve to reach the
    /// pinned station - the UI flags it red. Every entry is `false` for the un-dragged default
    /// (and for All-thirds, which has no drops). The flag is a warning, not a block (the diver
    /// asked for it): the plan credits the stage no more than its default drop would, so an
    /// over-reserve drop never extends the reach (#576). `false` for a drop the dive does not
    /// reach: the stage is carried the whole dive, never breathed to the pin.
    pub stage_drop_over_reserve: Vec<bool>,
    /// Swum distance (metres) from the origin to each stage-drop station, aligned with
    /// `stage_drop_refs` / `stage_drop_bars`, for the Plan readout (#245). Same swum-distance basis
    /// as `route_line_length_m`. Empty when there are no stage drops. For a drop the dive does not
    /// reach, the distance along the planned route to its pin.
    pub stage_drop_distances_m: Vec<f64>,
    /// Per-cylinder penetration allowance (bar) the engine used, aligned 1:1 with the INPUT
    /// cylinder order (disabled tanks and non-budget roles are `0.0`) so the table can show a
    /// "usable gas" value per row (#245). Under the reserve presets these are engine-COMPUTED
    /// (`fill - turn/drop`): a back-gas tank shows its strategy share of the gauge, a counted
    /// stage its Half-Minus IN half; under **Free** they echo the diver's clamped input. The budget
    /// credits exactly the gas between each row's fill and `fill - usable`, so the column can never
    /// drift from the budget/gauges.
    pub cylinder_usable_bars: Vec<f64>,
    /// Swum line length of the planned route (metres): the one-way penetration distance, summed as
    /// 3-D slope segments over the route's real shot legs (zero-distance merge/closure ties
    /// excluded), origin→turnaround. This is the *swum* distance, not the on-screen highlight length
    /// (a 2-D, closure-adjusted projection). `0.0` when nothing is reachable. (Not straight-line;
    /// not round-trip.)
    pub route_line_length_m: f64,
    /// Deepest point on the route (metres, positive = below surface); `None` when no route.
    pub route_max_depth_m: Option<f64>,
    /// Distance-weighted average depth along the route (metres, positive = below surface); `None`
    /// when no route.
    pub route_avg_depth_m: Option<f64>,
    /// Cumulative gas-to-reach (litres) at each ordered way point (#298), aligned 1:1 with the
    /// `waypoints` input. `None` from the first way point that no longer resolves to a station or
    /// that the graph cannot reach: the mandated route can't be flown, so every station also reads
    /// unreachable and the UI shows the route only as far as the chain got. Compare an entry against
    /// `budget_litres` to tell the diver the detour alone exceeds their gas. Empty when the plan has
    /// no way points.
    pub waypoint_gas_litres: Vec<Option<f64>>,
    /// The anticipated dive profile (#299): depth / distance / runtime / gas at every route station,
    /// origin→turnaround and back. See [`RouteProfilePoint`] for why the return leg is not a mirror
    /// in time or gas.
    ///
    /// Empty unless the plan has BOTH a turnaround and at least one leg to swim to it. Never one
    /// point long: a series of length 1 is a dive of zero distance and zero duration, and every
    /// consumer may treat a non-empty profile as something the diver actually swims.
    ///
    /// Two distinct states are excluded, for the same underlying reason - a route drawn on the map
    /// is not the same claim as a dive:
    /// - **No turnaround** (`final_turn_ref` is `None`) even though `route` is NOT empty: an
    ///   unaffordable way point keeps its committed chain for the map, so the diver can see which one
    ///   blew the gas, while leaving the plan unflyable.
    /// - **A turnaround but no leg**: when only the origin is within budget the engine picks the
    ///   origin as its own turnaround, which is honest for the overlay ("you can end here") and
    ///   meaningless as a profile.
    pub route_profile: Vec<RouteProfilePoint>,
    /// Route index of each stage drop, aligned 1:1 with `stage_drop_refs` / `stage_drop_bars` /
    /// `stage_drop_distances_m`, in `route_profile`'s outbound `route_index` space.
    /// `stage_drop_refs` alone cannot place a drop on a route that doubles back over a station - the
    /// ref names the station, not the pass - so the index is what a profile marker must be drawn at.
    ///
    /// An index PAST the route's last station (`> route.len()`, the number of legs) is a pinned
    /// drop the dive does not reach (#576 RV8c-2-D3): the gas or the scooter range turns the dive
    /// on the way there, the stage is carried the whole dive, and the index counts along the
    /// planned route on toward the pin. Every default drop sits on the route.
    pub stage_drop_route_indices: Vec<u32>,
    /// Route index of each way point, aligned 1:1 with the `waypoints` input, in the same index space
    /// as `stage_drop_route_indices`. `None` from the first way point that stalls, matching
    /// `waypoint_gas_litres`.
    pub waypoint_route_indices: Vec<Option<u32>>,
    /// Scooter range (metres) spent from the origin to the turnaround - the quantity
    /// [`Self::scooter_range_limit_m`] caps. Counts only the route's spans whose segment is
    /// `on_scooter`, so a plan that parks the units partway reports what it spent up to the park and
    /// nothing beyond it. `0.0` when nothing is flown on a scooter.
    ///
    /// ⚠ This is the scootered LINE only for a span flown at the units' cruise speed, which is the
    /// normal case and every plan before #317. Below cruise it is larger than the line: the cap is
    /// `burn x cruise`, so it converts battery minutes into metres at cruise and only there, and a
    /// span ridden slower spends the allowance faster than it covers ground.
    pub scooter_distance_m: f64,
    /// The rig's usable one-way range (metres): the weaker unit's `burn x cruising speed`, less the
    /// conservatism margin. `None` when the plan has no DPV rig, in which case the cap is infinite
    /// and only gas binds - exactly the pre-scooter behaviour.
    pub scooter_range_limit_m: Option<f64>,
    /// Where the scooters end up: the boundary after the last scootered segment. `None` when the
    /// units are carried all the way to the turnaround, and when nothing is flown on a scooter at
    /// all. Derived, never placed by the diver - see [`PlanSegment`].
    pub scooter_drop_ref: Option<StationRef>,
    /// One-way runtime (minutes) along the planned route, at each segment's own speed. The engine's
    /// first time-valued output; `None` when nothing is reachable.
    pub route_runtime_min: Option<f64>,
    /// Whether the **scooter range**, rather than gas, is what limits the dive: true when lifting
    /// the DPV range cap would move the turnaround FARTHER than where this plan turns.
    ///
    /// Scoped to the dive's own turn, never to the cave. "Some station the diver has the gas for is
    /// out of range" sounds like the same statement and is a false-positive machine: it fires on any
    /// unvisited pocket beyond the rig's reach however comfortably the dive makes its endpoint, and
    /// since gas scales with depth while range scales with length alone, one shallow distant passage
    /// trips it on plans the scooter never constrained at all.
    ///
    /// The UI must not re-derive this. Comparing `scooter_distance_m` against
    /// `scooter_range_limit_m` looks equivalent and is not - the route already stops at the cap, so
    /// the two read as merely equal in exactly the case being asked about. Same class of mistake as
    /// re-deriving [`Self::turn_toward_endpoint`], and as the #233 gauge divergence.
    pub turn_on_scooter_range: bool,
}
