//! Pure gas-planning physics - no graph, no config, no I/O. Golden-tested in
//! `super::tests`. Canonical SI throughout: depth in metres (positive-down), RMV in
//! L/min, pressure in bar, gas volume in litres. These are plain `f64` functions; the
//! finite/positive guards live in the `model` constructors (RMV, cylinders) and in
//! `GasConfig::normalize` (speeds/multipliers), so callers pass validated inputs (mirrors the
//! `Coordinates::new` / `new_checked` split).

/// Ambient pressure in atmospheres-absolute at depth `depth_m` metres (positive-down).
/// 10 msw ≈ 1 bar, so `ata = 1 + max(depth, 0)/10`; above the surface clamps to 1 ATA.
pub(crate) fn ata(depth_m: f64) -> f64 {
    1.0 + depth_m.max(0.0) / 10.0
}

/// Gas-consumption rate (L/min) at depth: the surface RMV scaled by ambient pressure.
///
/// Reference physics: since #317 the engine prices edges through [`edge_gas_basis`], so the only
/// remaining caller is [`edge_gas`].
pub(crate) fn gas_rate(rmv_l_per_min: f64, depth_m: f64) -> f64 {
    rmv_l_per_min * ata(depth_m)
}

/// Mean depth (m) of a straight leg between two stations. Trapezoidal mean of the
/// endpoints - exact for a leg whose depth varies linearly along its length.
pub(crate) fn avg_depth(z_from: f64, z_to: f64) -> f64 {
    0.5 * (z_from + z_to)
}

/// Gas (litres) consumed traversing one straight leg of `length_m` at `speed_m_per_min`,
/// breathing `rmv_l_per_min`, the leg running from depth `z_from` to `z_to` metres.
/// `gas = rate(avg_depth) × time = gas_rate(rmv, avg_depth) × (length / speed)`.
///
/// Precondition: `speed_m_per_min > 0` - guaranteed for config-sourced speeds by
/// `GasConfig::normalize`; any other caller must validate before calling.
///
/// Since #317 the reach field does NOT price its graph edges through this: it uses
/// `edge_gas_basis x (rmv / speed)` so one graph build serves every plan segment. The #299 exit-leg
/// profile still calls it directly, and it remains the canonical statement of the physics -
/// `edge_gas_matches_basis_times_scalar` pins the two forms against each other so they cannot
/// drift.
pub(crate) fn edge_gas(
    rmv_l_per_min: f64,
    speed_m_per_min: f64,
    length_m: f64,
    z_from: f64,
    z_to: f64,
) -> f64 {
    let rate = gas_rate(rmv_l_per_min, avg_depth(z_from, z_to));
    rate * (length_m / speed_m_per_min)
}

/// The **speed- and RMV-independent** half of [`edge_gas`]: `ata(avg_depth) x length`.
///
/// `edge_gas = rmv x ata(avg_depth) x length / speed`, so everything that varies per plan *segment*
/// (#317 - the diver's pace, and whether they are on a scooter) collapses into the single scalar
/// `rmv / speed`. Pricing an edge is then `basis x scalar`, which is what lets the gas graph be
/// built **once** and re-weighted per segment by one multiplication instead of being rebuilt.
///
/// Keep this factorisation in mind before "simplifying" either function into the other: they are
/// deliberately two evaluation orders of the same product, and `edge_gas` is the one whose exact
/// arithmetic the golden tests pin.
pub(crate) fn edge_gas_basis(length_m: f64, z_from: f64, z_to: f64) -> f64 {
    ata(avg_depth(z_from, z_to)) * length_m
}

/// Convert a SAC (surface air consumption, bar/min measured against a reference
/// cylinder) to a volumetric RMV (L/min): `rmv = sac × reference_water_volume`.
/// SAC is a constructor concern only - never stored (see `model::Diver::from_sac`).
pub(crate) fn sac_to_rmv(sac_bar_per_min: f64, ref_cyl_water_volume_l: f64) -> f64 {
    sac_bar_per_min * ref_cyl_water_volume_l
}

/// Ideal free-gas volume (litres) a cylinder holds at `fill_bar`: `fill × water_volume`.
/// Ideal-gas relation - at typical 200-232 bar fills real compressibility (Z) over-states this
/// by ~4-6%, rising to ~12% at 300 bar (the unsafe direction). This is the raw ideal primitive;
/// the usable-gas paths use [`deliverable_gas`] instead, which divides by [`air_z_factor`].
pub(crate) fn cylinder_gas_volume(fill_bar: f64, water_volume_l: f64) -> f64 {
    fill_bar * water_volume_l
}

/// Real-gas compressibility factor Z for air/nitrox at `fill_bar` (bar). A quadratic fit
/// anchored to the **conservative (upper) end** of tabulated air Z at cool cave temps -
/// `Z(207)≈1.045`, `Z(300)≈1.12` (cf. NIST/US-Navy air tables, ~1.04-1.06 @ 200-232 bar,
/// ~1.10-1.13 @ 300 bar) - so `deliverable = fill·vol / Z` never over-states real gas. **Clamped
/// to ≥ 1.0**: real air dips ~0.5% below 1 near 50 bar, but crediting *more* than ideal there is
/// the unsafe direction, so the derate can never become a credit. Monotone non-decreasing after
/// the clamp over the working range (the parabola turns down only well past any real fill).
pub(crate) fn air_z_factor(fill_bar: f64) -> f64 {
    let p = fill_bar.max(0.0);
    (1.0 - 1.89e-4 * p + 1.96e-6 * p * p).max(1.0)
}

/// Real deliverable free-gas volume (litres) - **always** Z-corrected (never ideal):
/// `p × water_volume / air_z_factor(p)`. When `conservative` (the "call-it-an-80"
/// community derate, PRD §5.3) the pressure is first capped to `rated_bar`
/// (`p = min(fill, rated)`), so an overfilled tank counts only its rated capacity; the Z
/// correction then applies on top. `conservative = false` is the precise `fill × vol / Z`.
/// Either way Z ensures the default at-rated rig (fill ≤ rated) no longer over-states usable
/// gas. Negative inputs clamp to 0 via `air_z_factor`/`max`.
pub(crate) fn deliverable_gas(
    fill_bar: f64,
    water_volume_l: f64,
    rated_pressure_bar: f64,
    conservative: bool,
) -> f64 {
    let p = if conservative {
        fill_bar.min(rated_pressure_bar)
    } else {
        fill_bar
    }
    .max(0.0);
    p * water_volume_l / air_z_factor(p)
}
