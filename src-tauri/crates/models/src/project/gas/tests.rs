#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests construct from known-valid literals; a failed unwrap should fail the test"
)]

use super::model::{
    CylRole, Cylinder, Diver, GasReachField, PlanMode, PlanSegment, TankMaterial, Team,
};
use super::physics::{
    air_z_factor, ata, avg_depth, cylinder_gas_volume, deliverable_gas, edge_gas, edge_gas_basis,
    gas_rate, sac_to_rmv,
};
use super::{is_swim_leg, penetration_budget, required_stages, route_metrics, usable_oc_gas};
use crate::config::{DpvRig, DpvUnit, GasConfig, ReservePreset, ReserveProfile};
use crate::{
    Coordinates, GpsCoordinates, Project, SectionData, ShotData, ShotKind, StationRef, ZeroDatum,
};

const EPS: f64 = 1e-9;

#[test]
fn ata_surface_depth_and_above_surface_clamp() {
    assert!((ata(0.0) - 1.0).abs() < EPS); // surface = 1 ATA
    assert!((ata(10.0) - 2.0).abs() < EPS); // +1 bar per 10 msw
    assert!((ata(30.0) - 4.0).abs() < EPS);
    assert!((ata(-5.0) - 1.0).abs() < EPS); // above surface clamps to 1 ATA
}

#[test]
fn gas_rate_scales_with_ambient_pressure() {
    // 20 L/min at 20 m (3 ATA) = 60 L/min.
    assert!((gas_rate(20.0, 20.0) - 60.0).abs() < EPS);
}

#[test]
fn avg_depth_is_the_midpoint() {
    assert!((avg_depth(10.0, 30.0) - 20.0).abs() < EPS);
}

#[test]
fn edge_gas_hand_computed() {
    // rmv 20, swim 10 m/min, 100 m leg, depth 10→30 (avg 20 m = 3 ATA):
    // rate = 60 L/min, time = 10 min ⇒ 600 L.
    assert!((edge_gas(20.0, 10.0, 100.0, 10.0, 30.0) - 600.0).abs() < 1e-6);
}

#[test]
fn edge_gas_basis_hand_computed() {
    // Same leg as above without the rmv/speed half: 3 ATA × 100 m ⇒ 300.
    assert!((edge_gas_basis(100.0, 10.0, 30.0) - 300.0).abs() < 1e-6);
}

#[test]
fn edge_gas_matches_basis_times_scalar() {
    // The engine never calls `edge_gas`; it prices every edge as `basis × (rmv / speed)` so one
    // graph build serves every plan segment (#317). That refactor is only sound while the two
    // agree, and nothing else would catch them drifting - `edge_gas` has no production caller
    // left to fail. Swept across depths (incl. above-surface clamping), lengths, rates and the
    // scooter-vs-swim speed range.
    for &(len, zf, zt) in &[
        (100.0, 10.0, 30.0),
        (7.5, 0.0, 0.0),
        (250.0, 45.0, 5.0),
        (0.0, 12.0, 12.0),
        (60.0, -3.0, 8.0),
    ] {
        for &rmv in &[12.0, 20.0, 31.7] {
            for &speed in &[6.0, 10.0, 55.0, 82.5] {
                let direct = edge_gas(rmv, speed, len, zf, zt);
                let factored = edge_gas_basis(len, zf, zt) * rmv / speed;
                assert!(
                    (direct - factored).abs() < 1e-9,
                    "edge_gas({rmv},{speed},{len},{zf},{zt}) = {direct} but basis form = {factored}"
                );
            }
        }
    }
}

#[test]
fn sac_to_rmv_hand_computed() {
    // 0.5 bar/min against a 24 L reference cylinder ⇒ 12 L/min.
    assert!((sac_to_rmv(0.5, 24.0) - 12.0).abs() < EPS);
}

#[test]
fn cylinder_gas_volume_is_ideal() {
    // LP85 at 230 bar overfill: 230 × 12 = 2760 L ideal.
    assert!((cylinder_gas_volume(230.0, 12.0) - 2760.0).abs() < EPS);
}

#[test]
fn diver_new_rejects_nonpositive_and_nonfinite() {
    assert!(Diver::new(0.0, None).is_err());
    assert!(Diver::new(-1.0, None).is_err());
    assert!(Diver::new(f64::NAN, None).is_err());
    assert!(Diver::new(f64::INFINITY, None).is_err());
    assert!(Diver::new(14.0, Some("me".into())).is_ok());
}

#[test]
fn diver_from_sac_derives_rmv_and_guards_inputs() {
    let diver = Diver::from_sac(0.5, 24.0, None).unwrap();
    assert!((diver.rmv_l_per_min - 12.0).abs() < EPS);
    assert!(Diver::from_sac(0.0, 24.0, None).is_err());
    assert!(Diver::from_sac(0.5, 0.0, None).is_err());
}

#[test]
fn cylinder_guards_and_gas_volume() {
    // Fill (230) may exceed rated (179) - overfilling is routine, never clamped.
    let cyl = Cylinder::new(
        12.0,
        179.0,
        230.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    assert!((cyl.gas_volume_l() - 2760.0).abs() < EPS);
    assert!(Cylinder::new(
        0.0,
        179.0,
        230.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None
    )
    .is_err());
    assert!(Cylinder::new(
        12.0,
        179.0,
        f64::NAN,
        TankMaterial::Steel,
        CylRole::BackGas,
        None
    )
    .is_err());
}

#[test]
fn team_holds_divers_cylinders_and_mode() {
    let diver = Diver::new(14.0, None).unwrap();
    let cyl = Cylinder::new(
        12.0,
        179.0,
        230.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let team = Team {
        divers: vec![diver],
        cylinders_per_diver: vec![vec![cyl]],
        mode: PlanMode::OpenCircuit,
    };
    assert_eq!(team.divers.len(), 1);
    assert_eq!(team.cylinders_per_diver[0].len(), 1);
    assert_eq!(team.mode, PlanMode::OpenCircuit);
}

// ---- Phase 1: gas reachability field ------------------------------------------------

fn make_shot(length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

/// One straight section of 3 shots descending 0 → 10 → 20 → 30 m, 100 m each (4 stations).
fn linear_descent_project() -> (Project, StationRef) {
    let mut project = Project::new("Gas".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                make_shot(100.0, 0.0, 10.0),
                make_shot(100.0, 10.0, 20.0),
                make_shot(100.0, 20.0, 30.0),
            ],
            ..Default::default()
        })
        .id;
    (project, StationRef::new(sec, 0))
}

fn field_gas(field: &GasReachField, sr: StationRef) -> f64 {
    let idx = field.station_refs.iter().position(|r| *r == sr).unwrap();
    field.gas_to_reach[idx].unwrap()
}

fn field_reachable(field: &GasReachField, sr: StationRef) -> bool {
    let idx = field.station_refs.iter().position(|r| *r == sr).unwrap();
    field.reachable[idx]
}

#[test]
fn required_stages_matches_presets() {
    assert_eq!(required_stages(ReservePreset::AllThirds), 0);
    assert_eq!(required_stages(ReservePreset::OneStage), 1);
    assert_eq!(required_stages(ReservePreset::TwoStage), 2);
}

#[test]
fn air_z_factor_matches_tabulated_air_and_never_credits() {
    // Anchored to the conservative (upper) end of tabulated air Z at cool cave temps
    // (NIST / US-Navy air tables: ~1.04-1.06 @ 200-232 bar, ~1.10-1.13 @ 300 bar). The fit is
    // derived from the 207 and 300 anchors; 232 bar is an INDEPENDENT interpolation check
    // against the table (not a derivation point), so a wrong coefficient is caught, not hidden.
    assert!((air_z_factor(0.0) - 1.0).abs() < EPS); // surface ≈ ideal
    assert!((air_z_factor(207.0) - 1.045).abs() < 0.008); // ~4.5% derate at a 207-bar fill
    assert!((air_z_factor(232.0) - 1.06).abs() < 0.01); // independent check vs tabulated air
    assert!((air_z_factor(300.0) - 1.12).abs() < 0.01); // ~12% at 300 bar (HP-steel fills)
                                                        // Clamped ≥ 1.0 everywhere - the derate can only ever REDUCE gas, never credit more than
                                                        // ideal (the safe direction), even where real air dips slightly below 1 (~50 bar).
    let mut prev = 0.0;
    for p in [0.0, 30.0, 50.0, 100.0, 150.0, 200.0, 250.0, 300.0] {
        let z = air_z_factor(p);
        assert!(z >= 1.0 - EPS, "z >= 1 at {p} bar");
        assert!(z >= prev - EPS, "monotone non-decreasing at {p} bar");
        prev = z;
    }
}

#[test]
fn deliverable_gas_always_below_ideal_and_conservative_caps_overfill() {
    // At rated (no overfill) the only derate is the Z correction, so deliverable < ideal.
    assert!(deliverable_gas(207.0, 11.1, 207.0, false) < cylinder_gas_volume(207.0, 11.1));
    // Overfilled LP steel: conservative caps to rated THEN applies Z; non-conservative only Z.
    let cons = deliverable_gas(230.0, 12.0, 179.0, true);
    let prec = deliverable_gas(230.0, 12.0, 179.0, false);
    assert!(cons < prec, "conservative rated-cap bites on an overfill");
    assert!((cons - 179.0 * 12.0 / air_z_factor(179.0)).abs() < 1e-6);
    assert!((prec - 230.0 * 12.0 / air_z_factor(230.0)).abs() < 1e-6);
}

#[test]
fn gas_reach_field_excludes_disabled_cylinders_from_budget() {
    // A disabled tank (#245) contributes nothing: the field computed WITH a disabled second
    // back-gas tank must match the field from the single enabled tank alone.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let enabled_bg = Cylinder::new(
        11.1,
        207.0,
        207.0,
        TankMaterial::Aluminium,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let disabled_bg = Cylinder {
        enabled: false,
        ..enabled_bg.clone()
    };

    let with_disabled = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[enabled_bg.clone(), disabled_bg],
        &GasConfig::default(),
    );
    let only_enabled = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[enabled_bg],
        &GasConfig::default(),
    );
    assert!(
        with_disabled.budget_litres > 0.0,
        "the one enabled tank still yields a budget"
    );
    assert!(
        (with_disabled.budget_litres - only_enabled.budget_litres).abs() < 1e-9,
        "the disabled tank must not add to the budget"
    );
}

#[test]
fn gas_reach_field_disabled_stage_falls_back_off_staged_strategy() {
    // When the rig's only Stage cylinder is disabled, a staged request has no stages to count,
    // so the effective strategy falls back to All-thirds - no stage drops (§6, never invent gas).
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        24.0,
        232.0,
        200.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let disabled_stage = Cylinder {
        enabled: false,
        ..Cylinder::new(
            11.0,
            207.0,
            200.0,
            TankMaterial::Aluminium,
            CylRole::Stage,
            None,
        )
        .unwrap()
    };
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg, disabled_stage], &cfg);
    assert!(
        field.stage_drop_refs.is_empty() && field.stage_drop_bars.is_empty(),
        "a disabled-only stage rig falls back to All-thirds (no drops)"
    );
}

#[test]
fn gas_free_budget_sums_user_usable_gas_and_gauges_match() {
    // Free (#245): budget = Σ (deliverable(fill) - deliverable(fill - usable)); at fill 90 / usable
    // 50 the pressures are < 100 bar so Z == 1 and it reduces to usable × water-vol.
    // usable = 50 is deliberately ≠ fill/3 (= 30): were Free to fall through to the All-thirds
    // arm, budget would be 720 / column 30 / turn 60 - so this test genuinely separates Free.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder {
        usable_gas_bar: 50.0,
        ..Cylinder::new(
            24.0,
            232.0,
            90.0,
            TankMaterial::Steel,
            CylRole::BackGas,
            None,
        )
        .unwrap()
    };
    let cfg = GasConfig {
        default_reserve: ReservePreset::Free,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg], &cfg);
    assert!(
        (field.budget_litres - 50.0 * 24.0).abs() < 1e-6,
        "Free budget = usable × water-vol at Z == 1 (1200, not the All-thirds 720)"
    );
    // The column echoes the diver's chosen usable bar; the turn gauge is fill - usable.
    assert_eq!(field.cylinder_usable_bars, vec![50.0]);
    assert!((field.backgas_turn_bar.unwrap() - 40.0).abs() < 1e-6);
}

#[test]
fn gas_free_clamps_usable_over_fill() {
    // A usable allowance larger than the tank holds is clamped to the fill - it can never credit
    // more than the whole cylinder (the unsafe over-stating direction, §6).
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder {
        usable_gas_bar: 500.0, // absurd - clamped to fill (90)
        ..Cylinder::new(
            24.0,
            232.0,
            90.0,
            TankMaterial::Steel,
            CylRole::BackGas,
            None,
        )
        .unwrap()
    };
    let cfg = GasConfig {
        default_reserve: ReservePreset::Free,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg], &cfg);
    assert!(
        (field.budget_litres - 90.0 * 24.0).abs() < 1e-6,
        "clamped usable = whole tank (fill × water-vol at Z == 1)"
    );
    assert_eq!(field.cylinder_usable_bars, vec![90.0]);
}

#[test]
fn gas_free_display_clamps_to_basis_not_fill_on_overfill() {
    // Overfilled tank (fill 250 > rated 232) under the default conservative cap: the budget clamps
    // usable to basis = 232, and so MUST the display column + turn gauge, or the readout would
    // over-state relative to the reach (the drift bug). usable 240 → column shows 232 (basis), NOT
    // 240 (fill); turn = fill - 232 = 18.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder {
        usable_gas_bar: 240.0,
        ..Cylinder::new(
            24.0,
            232.0,
            250.0,
            TankMaterial::Steel,
            CylRole::BackGas,
            None,
        )
        .unwrap()
    };
    let cfg = GasConfig {
        default_reserve: ReservePreset::Free,
        ..GasConfig::default() // conservative_tank_gas = true
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg], &cfg);
    assert!(
        (field.cylinder_usable_bars[0] - 232.0).abs() < 1e-6,
        "column clamps to basis (232), not raw fill (240)"
    );
    assert!(
        (field.backgas_turn_bar.unwrap() - 18.0).abs() < 1e-6,
        "turn = fill - basis-clamped usable = 250 - 232"
    );
}

#[test]
fn gas_free_places_a_drop_for_every_stage_with_usable() {
    // Free has no 2-stage cap (unlike Half-Minus): a 3rd/Nth stage with a positive usable must get
    // its own drop point. Long descent so all three cumulative thresholds are reachable.
    let mut project = Project::new("Gas".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: (0..8)
                .map(|i| make_shot(50.0, f64::from(i), f64::from(i) + 1.0))
                .collect(),
            ..Default::default()
        })
        .id;
    let origin = StationRef::new(sec, 0);
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        24.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = || Cylinder {
        usable_gas_bar: 40.0,
        ..Cylinder::new(
            11.0,
            207.0,
            90.0,
            TankMaterial::Aluminium,
            CylRole::Stage,
            None,
        )
        .unwrap()
    };
    let cfg = GasConfig {
        default_reserve: ReservePreset::Free,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg, stage(), stage(), stage()],
        &cfg,
    );
    assert_eq!(
        field.stage_drop_bars.len(),
        3,
        "one drop gauge per stage with usable (no 2-stage cap in Free)"
    );
    assert_eq!(field.stage_drop_refs.len(), 3, "one drop marker per stage");
    assert_eq!(
        field.stage_drop_distances_m.len(),
        3,
        "one drop distance per stage"
    );
}

#[test]
fn gas_free_nonfinite_usable_sanitizes_to_zero() {
    // A non-finite usable (NaN / ±inf - serde_json can't carry it today, but a future non-JSON path
    // might) must sanitize to 0 in every Free path, never poisoning the budget / turn / column with
    // NaN. `free_usable_bar` guards it; without the guard `basis - NaN` would flow through.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    for bad in [f64::NAN, f64::INFINITY] {
        let bg = Cylinder {
            usable_gas_bar: bad,
            ..Cylinder::new(
                24.0,
                232.0,
                90.0,
                TankMaterial::Steel,
                CylRole::BackGas,
                None,
            )
            .unwrap()
        };
        let cfg = GasConfig {
            default_reserve: ReservePreset::Free,
            ..GasConfig::default()
        };
        let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg], &cfg);
        assert!(
            field.budget_litres.is_finite() && field.budget_litres.abs() < 1e-9,
            "non-finite usable → 0 budget (not NaN), got {}",
            field.budget_litres
        );
        assert_eq!(field.cylinder_usable_bars, vec![0.0]);
        // turn gauge = fill - 0 = fill (finite), not NaN.
        assert!((field.backgas_turn_bar.unwrap() - 90.0).abs() < 1e-6);
    }
}

#[test]
fn gas_free_negative_usable_clamps_to_zero() {
    // A negative usable (hand-edited / stale config; the UI blocks it) contributes nothing and
    // shows 0 - the lower clamp holds.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder {
        usable_gas_bar: -50.0,
        ..Cylinder::new(
            24.0,
            232.0,
            90.0,
            TankMaterial::Steel,
            CylRole::BackGas,
            None,
        )
        .unwrap()
    };
    let cfg = GasConfig {
        default_reserve: ReservePreset::Free,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg], &cfg);
    assert!(
        field.budget_litres.abs() < 1e-9,
        "negative usable → 0 budget"
    );
    assert_eq!(field.cylinder_usable_bars, vec![0.0]);
}

#[test]
fn gas_cylinder_usable_bars_uncounted_stage_is_zero() {
    // Under a 1-stage staged strategy only the FIRST enabled stage is counted; a second stage
    // contributes 0 to the column (it isn't in the budget). Pins the `enabled_stage_i < needed`
    // counting arm, which the all-thirds alignment test never exercises.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        24.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = || {
        Cylinder::new(
            11.0,
            207.0,
            90.0,
            TankMaterial::Aluminium,
            CylRole::Stage,
            None,
        )
        .unwrap()
    };
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg, stage(), stage()], &cfg);
    // counted stage: (basis - reserve)/2 = (90 - 30)/2 = 30; uncounted second stage: 0.
    assert!(
        (field.cylinder_usable_bars[1] - 30.0).abs() < 1e-6,
        "first (counted) stage = (basis - reserve)/2"
    );
    assert!(
        field.cylinder_usable_bars[2].abs() < 1e-12,
        "second (uncounted) stage = 0"
    );
}

#[test]
fn gas_cylinder_usable_bars_match_gauges_and_align_with_input() {
    // The per-row usable-bar column equals `fill - turn/drop` (so it can't drift from the gauges)
    // and is aligned 1:1 with the INPUT rows: a disabled tank and a non-budget role read 0.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        24.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let disabled = Cylinder {
        enabled: false,
        ..bg.clone()
    };
    let deco = Cylinder::new(
        6.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Deco,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg, disabled, deco],
        &GasConfig::default(),
    );
    assert_eq!(
        field.cylinder_usable_bars.len(),
        3,
        "one entry per input row"
    );
    let turn = field.backgas_turn_bar.unwrap();
    // All-thirds back gas: usable = fill - turn = ⅓ fill (Z == 1 at 90 bar).
    assert!((field.cylinder_usable_bars[0] - (90.0 - turn)).abs() < 1e-6);
    assert!((field.cylinder_usable_bars[0] - 90.0 / 3.0).abs() < 1e-6);
    assert!(
        field.cylinder_usable_bars[1].abs() < 1e-12,
        "disabled row = 0"
    );
    assert!(
        field.cylinder_usable_bars[2].abs() < 1e-12,
        "deco (non-budget) row = 0"
    );
}

#[test]
fn gas_all_thirds_safety_margin_shrinks_budget_and_column() {
    // The All-thirds safety margin (#245) holds back `margin` bar more per cylinder (Z==1 at 90 bar).
    // back-gas 24 L @ 90: thirds usable = 90×24/3 = 720 L; margin 15 bar holds 15×24 = 360 L more →
    // budget 360, column 15 bar (usable/wv), turn = 90 - 360/24 = 75 (vs 60 / 720 at margin 0).
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        24.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let plain = GasConfig::default();
    let with_margin = GasConfig {
        reserve_profile: ReserveProfile {
            all_thirds_safety_margin_bar: 15.0,
            ..ReserveProfile::default()
        },
        ..GasConfig::default()
    };
    let base = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg.clone()], &plain);
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg], &with_margin);
    assert!(
        (base.budget_litres - 720.0).abs() < 1e-6,
        "margin 0 = plain thirds (unchanged)"
    );
    assert!(
        (field.budget_litres - 360.0).abs() < 1e-6,
        "margin 15 shrinks the budget"
    );
    assert!(
        (field.cylinder_usable_bars[0] - 15.0).abs() < 1e-6,
        "column = usable/water-vol"
    );
    assert!(
        (field.backgas_turn_bar.unwrap() - 75.0).abs() < 1e-6,
        "turn rises with the margin"
    );
    assert!((base.backgas_turn_bar.unwrap() - 60.0).abs() < 1e-6);
}

#[test]
fn penetration_budget_all_thirds_is_usable_over_three() {
    let bg = Cylinder::new(
        11.1,
        207.0,
        207.0,
        TankMaterial::Aluminium,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let cyls = [bg];
    let budget = penetration_budget(
        &cyls,
        ReservePreset::AllThirds,
        &ReserveProfile::default(),
        true,
    );
    assert!((budget - usable_oc_gas(&cyls, true) / 3.0).abs() < 1e-6);
}

#[test]
fn penetration_budget_half_minus_pins() {
    // Half-Minus, hand-computed at Z == 1 (all pressures < 100 bar ⇒ air_z_factor clamps to 1.0,
    // so deliverable == pressure × water-volume). Default stage reserve = 30 bar. A stage's IN gas
    // = full - deliverable((fill + reserve)/2) = (fill - reserve)/2 × w_s (drop @ (fill+reserve)/2).
    let prof = ReserveProfile::default(); // stage_reserve_bar = 30
    let bg = Cylinder::new(
        24.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = || {
        Cylinder::new(
            11.0,
            207.0,
            90.0,
            TankMaterial::Aluminium,
            CylRole::Stage,
            None,
        )
        .unwrap()
    };
    // back gas deliverable = 90 × 24 = 2160 L; per stage IN = (90 - 30)/2 × 11 = 330 L (drop @ 60).

    let one = [bg.clone(), stage()];
    let b1 = penetration_budget(&one, ReservePreset::OneStage, &prof, false);
    assert!(
        (b1 - (0.25 * 2160.0 + 330.0)).abs() < 1e-6, // ¼·backgas + 1 stage = 540 + 330 = 870
        "1-stage = ¼ back gas + one stage's Half-Minus in-gas"
    );

    let two = [bg, stage(), stage()];
    let b2 = penetration_budget(&two, ReservePreset::TwoStage, &prof, false);
    assert!(
        (b2 - (2160.0 / 6.0 + 2.0 * 330.0)).abs() < 1e-6, // ⅙·backgas + 2 stages = 360 + 660 = 1020
        "2-stage = ⅙ back gas + two stages' Half-Minus in-gas"
    );
}

#[test]
fn penetration_budget_stage_reserve_lowers_budget() {
    // A larger per-stage reserve holds back more of the stage, so the staged budget shrinks
    // monotonically. Hand-computed at Z == 1 (fill 90 bar).
    let bg = Cylinder::new(
        24.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        11.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let rig = [bg, stage];
    let small = ReserveProfile {
        stage_reserve_bar: 10.0,
        ..ReserveProfile::default()
    };
    let big = ReserveProfile {
        stage_reserve_bar: 50.0,
        ..ReserveProfile::default()
    };
    let b_small = penetration_budget(&rig, ReservePreset::OneStage, &small, false);
    let b_big = penetration_budget(&rig, ReservePreset::OneStage, &big, false);
    // in(reserve=10) = (90-10)/2 × 11 = 440 → 540 + 440 = 980; in(reserve=50) = 220 → 760.
    assert!((b_small - 980.0).abs() < 1e-6);
    assert!((b_big - 760.0).abs() < 1e-6);
    assert!(b_big < b_small, "more reserve → smaller penetration budget");
}

#[test]
fn penetration_budget_falls_back_to_all_thirds_without_stages() {
    // Default twin-AL80 rig carries ZERO stages → the pushy presets must fall back to the
    // AllThirds budget (never fabricate stage gas the diver isn't carrying - the safe gate).
    let prof = ReserveProfile::default();
    let bg = Cylinder::new(
        11.1,
        207.0,
        207.0,
        TankMaterial::Aluminium,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let rig = [bg.clone(), bg.clone()];
    let thirds = penetration_budget(&rig, ReservePreset::AllThirds, &prof, true);
    assert!((penetration_budget(&rig, ReservePreset::OneStage, &prof, true) - thirds).abs() < EPS);
    assert!((penetration_budget(&rig, ReservePreset::TwoStage, &prof, true) - thirds).abs() < EPS);

    // Adversarial boundary: TwoStage with EXACTLY ONE stage (1 < 2) must fall back BEFORE the
    // `stages[..2]` slice would panic - guards the count-gate's off-by-one. OneStage with that
    // same single stage does NOT fall back (1 >= 1), so its budget differs from all-thirds.
    let stage = Cylinder::new(
        11.1,
        207.0,
        207.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let one_stage_rig = [bg.clone(), bg, stage];
    let thirds_1 = penetration_budget(&one_stage_rig, ReservePreset::AllThirds, &prof, true);
    assert!(
        (penetration_budget(&one_stage_rig, ReservePreset::TwoStage, &prof, true) - thirds_1).abs()
            < EPS,
        "TwoStage with 1 stage falls back to all-thirds (no slice panic)"
    );
    assert!(
        penetration_budget(&one_stage_rig, ReservePreset::OneStage, &prof, true) != thirds_1,
        "OneStage with 1 stage uses the stage path, not the fallback"
    );
}

#[test]
fn usable_oc_gas_counts_backgas_stage_excludes_others_and_derates() {
    let bg = Cylinder::new(
        12.0,
        200.0,
        200.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        11.0,
        207.0,
        207.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let deco = Cylinder::new(
        5.7,
        207.0,
        207.0,
        TankMaterial::Aluminium,
        CylRole::Deco,
        None,
    )
    .unwrap();
    let dilout = Cylinder::new(
        3.0,
        237.0,
        200.0,
        TankMaterial::Steel,
        CylRole::DilOut,
        None,
    )
    .unwrap();
    let cyls = [bg, stage, deco, dilout];
    // Counts back gas + stage (deco/dilout excluded), each Z-derated via deliverable_gas.
    assert!(
        (usable_oc_gas(&cyls, false)
            - (deliverable_gas(200.0, 12.0, 200.0, false)
                + deliverable_gas(207.0, 11.0, 207.0, false)))
        .abs()
            < 1e-6
    );

    // Overfilled backgas: conservative caps to rated (the "call-it-an-80" convention) then Z;
    // non-conservative applies only Z to the actual fill.
    let over = Cylinder::new(
        12.0,
        179.0,
        230.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    assert!(
        (usable_oc_gas(std::slice::from_ref(&over), true)
            - deliverable_gas(230.0, 12.0, 179.0, true))
        .abs()
            < 1e-6
    );
    assert!(
        (usable_oc_gas(&[over], false) - deliverable_gas(230.0, 12.0, 179.0, false)).abs() < 1e-6
    );
}

#[test]
fn gas_reach_field_huge_budget_reaches_everything_no_frontier() {
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    // Absurdly large cylinder → budget dwarfs the cave → all reachable, no turn needed.
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &GasConfig::default());
    assert_eq!(field.station_refs.len(), 4); // 3 shots → 4 stations
    assert!(field.reachable.iter().all(|&r| r));
    assert!(field.gas_to_reach.iter().all(Option::is_some));
    assert!(field.turn_frontier.is_empty()); // nothing borders an unreachable station
}

#[test]
fn gas_reach_field_tiny_budget_only_origin_is_the_frontier() {
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    // Negligible cylinder → budget ≈ 0 → only the origin (gas_in 0) qualifies; it's the frontier.
    let tiny = Cylinder::new(
        0.01,
        1.0,
        1.0,
        TankMaterial::Aluminium,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[tiny],
        &GasConfig::default(),
    );
    let origin_idx = field
        .station_refs
        .iter()
        .position(|r| *r == origin)
        .unwrap();
    assert!(field.reachable[origin_idx]);
    assert_eq!(field.reachable.iter().filter(|&&r| r).count(), 1); // only the origin
    assert_eq!(field.turn_frontier, vec![origin]);
    assert_eq!(field.gas_to_reach[origin_idx], Some(0.0)); // gas to reach the origin is 0
}

#[test]
fn gas_reach_field_one_stage_route_drop_and_final_turn() {
    // Staged Half-Minus places a drop on the deepest-penetration route + a final turn.
    // gas-to-reach along the descent: station 0/1/2/3 = 0/300/800/1500 L (Z==1 at these depths).
    // OneStage budget = ¼·backgas + stage_in. Back gas 80 bar × 25 L = 2000 L ⇒ ¼ = 500; stage
    // 90 bar × 10 L water, reserve 30 ⇒ stage_in = (90-30)/2 × 10 = 300 L (drop @ 60 bar). Budget
    // = 800 → reaches the 800 L station (index 2); index 3 (1500 L) is out of budget.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        25.0,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        10.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg, stage], &cfg);

    let sec = origin.section_id;
    assert!((field.budget_litres - 800.0).abs() < 1e-6);
    // Stage drops where cumulative in-gas first reaches stage_in (300 L) = station 1; the route runs
    // origin → deepest in-budget station (index 2) = the final turn.
    assert_eq!(field.stage_drop_refs, vec![StationRef::new(sec, 1)]);
    assert_eq!(field.final_turn_ref, Some(StationRef::new(sec, 2)));
}

#[test]
fn gas_reach_field_two_stage_places_two_drops_in_order() {
    // TwoStage: two drops at the two cumulative stage thresholds, then the back-gas leg.
    // budget = ⅙·backgas + 2·stage_in. Back gas 80 bar × 82.5 L = 6600 L ⇒ ⅙ = 1100; each stage
    // 70 bar × 10 L water, reserve 30 ⇒ stage_in = (70-30)/2 × 10 = 200 L. Budget = 1100 + 400 =
    // 1500 → reaches station 3 (1500 L). Thresholds: 200 → first station ≥200 = station 1 (300 L);
    // 400 → first ≥400 = station 2 (800 L).
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        82.5,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = || {
        Cylinder::new(
            10.0,
            207.0,
            70.0,
            TankMaterial::Aluminium,
            CylRole::Stage,
            None,
        )
        .unwrap()
    };
    let cfg = GasConfig {
        default_reserve: ReservePreset::TwoStage,
        ..GasConfig::default()
    };
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg, stage(), stage()], &cfg);

    let sec = origin.section_id;
    assert!((field.budget_litres - 1500.0).abs() < 1e-6);
    assert_eq!(
        field.stage_drop_refs,
        vec![StationRef::new(sec, 1), StationRef::new(sec, 2)]
    );
    assert_eq!(field.final_turn_ref, Some(StationRef::new(sec, 3)));
}

// ── #246 stage-drop overrides ─────────────────────────────────────────────────────────────────
// Shared OneStage rig for the override tests: back gas ¼ = 900 L, stage_in = 600 L ⇒ base budget
// 1500 L. Along `linear_descent_project` gas-to-reach is 0/300/800/1500 L at stations 0/1/2/3, so
// the default drop sits at station 2 (first ≥ 600) and the turn at station 3 (1500 ≤ budget). Every
// fill is ≤ 90 bar ⇒ Z clamps to 1, so `deliverable == fill × water` and the numbers below are exact.
fn override_rig() -> (Cylinder, Cylinder, GasConfig) {
    let bg = Cylinder::new(
        40.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        20.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    (bg, stage, cfg)
}

#[test]
fn gas_reach_field_override_drags_drop_shallower_shrinks_reach() {
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let (bg, stage, cfg) = override_rig();
    let sec = origin.section_id;

    // Baseline (no override): drop @ station 2, turn @ station 3, budget 1500, not over-reserve.
    let base = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg.clone(), stage.clone()],
        &cfg,
    );
    assert_eq!(base.stage_drop_refs, vec![StationRef::new(sec, 2)]);
    assert_eq!(base.final_turn_ref, Some(StationRef::new(sec, 3)));
    assert!((base.budget_litres - 1500.0).abs() < 1e-6);
    assert_eq!(base.stage_drop_over_reserve, vec![false]);

    // Drag the drop SHALLOWER to station 1 (300 L). Contribution 300 < 600 safe ⇒ strand 300 ⇒
    // budget 1200 ⇒ the turn retreats to station 2 (800 ≤ 1200 < 1500). Drop bar rises: held =
    // 1800 - 300 = 1500 L ⇒ 75 bar (breathed less than the 60-bar default). Not over-reserve.
    let overrides = vec![Some(StationRef::new(sec, 1))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &overrides,
    );
    assert_eq!(f.stage_drop_refs, vec![StationRef::new(sec, 1)]);
    assert_eq!(f.final_turn_ref, Some(StationRef::new(sec, 2)));
    assert!((f.budget_litres - 1200.0).abs() < 1e-6);
    assert!((f.stage_drop_bars[0] - 75.0).abs() < 1e-3);
    assert_eq!(f.stage_drop_over_reserve, vec![false]);
}

#[test]
fn gas_reach_field_override_drags_drop_deeper_is_over_reserve_and_extends_budget() {
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let (bg, stage, cfg) = override_rig();
    let sec = origin.section_id;

    // Drag the drop DEEPER to station 3 (1500 L). Contribution 1500 > 600 safe (and < the 1800 L
    // tank cap) ⇒ over-reserve. Budget 1500 + (1500 - 600) = 2400. Drop bar: held = 1800 - 1500 =
    // 300 L ⇒ 15 bar - below the 30-bar reserve, which is exactly what the red flag warns about.
    let overrides = vec![Some(StationRef::new(sec, 3))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &overrides,
    );
    assert_eq!(f.stage_drop_refs, vec![StationRef::new(sec, 3)]);
    assert!((f.budget_litres - 2400.0).abs() < 1e-6);
    assert!((f.stage_drop_bars[0] - 15.0).abs() < 1e-3);
    assert_eq!(f.stage_drop_over_reserve, vec![true]);
}

#[test]
fn gas_reach_field_override_deeper_than_tank_empty_clamps_to_empty() {
    // A stage that only holds 1000 L (fill 50 × water 20) dragged to station 3 (1500 L): the
    // contribution HARD-clamps to the tank cap (1000 L), so the drop reads 0 bar (empty), is
    // over-reserve, and the budget grows by only 1000 - 200, never crediting gas the tank lacks.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        40.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        20.0,
        207.0,
        50.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let sec = origin.section_id;
    // base budget = 900 (¼ back gas) + 200 (stage_in) = 1100.
    let overrides = vec![Some(StationRef::new(sec, 3))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &overrides,
    );
    assert!(f.stage_drop_bars[0].abs() < 1e-3, "empty tank reads 0 bar");
    assert_eq!(f.stage_drop_over_reserve, vec![true]);
    assert!((f.budget_litres - 1900.0).abs() < 1e-6); // 1100 + (1000 - 200)
}

#[test]
fn gas_reach_field_override_none_and_overlong_fall_through_to_default() {
    // A None entry (that drop wasn't dragged) and extra entries past the drop count are ignored, so
    // the field is byte-identical to the un-overridden default.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let (bg, stage, cfg) = override_rig();
    let sec = origin.section_id;
    let base = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg.clone(), stage.clone()],
        &cfg,
    );
    let overrides = vec![None, Some(StationRef::new(sec, 2))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &overrides,
    );
    assert_eq!(f, base);
}

#[test]
fn gas_reach_field_override_ignored_for_all_thirds_no_drops() {
    // All-thirds has no stage drops, so an override can't do anything - the field is unchanged.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        25.0,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let sec = origin.section_id;
    let base = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg.clone()],
        &GasConfig::default(),
    );
    let overrides = vec![Some(StationRef::new(sec, 1))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg],
        &GasConfig::default(),
        &overrides,
    );
    assert!(f.stage_drop_over_reserve.is_empty());
    assert_eq!(f, base);
}

#[test]
fn gas_reach_field_override_recomputes_free_strategy_drop() {
    // Free: the stage's 40-bar usable (800 L IN) drops it at station 2 by default. Drag it to
    // station 1 (300 L) ⇒ strand 500 ⇒ budget 1600 → 1100 ⇒ turn retreats to station 2; bar 50 → 75.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let mut bg = Cylinder::new(
        40.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    bg.usable_gas_bar = 20.0; // free_in = 3600 - 2800 = 800 L (no drop - back gas)
    let mut stage = Cylinder::new(
        20.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    stage.usable_gas_bar = 40.0; // free_in = 1800 - 1000 = 800 L, drop @ 90 - 40 = 50 bar
    let cfg = GasConfig {
        default_reserve: ReservePreset::Free,
        ..GasConfig::default()
    };
    let sec = origin.section_id;
    let base = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg.clone(), stage.clone()],
        &cfg,
    );
    assert_eq!(base.stage_drop_refs, vec![StationRef::new(sec, 2)]);
    assert!((base.budget_litres - 1600.0).abs() < 1e-6);
    assert!((base.stage_drop_bars[0] - 50.0).abs() < 1e-3);

    let overrides = vec![Some(StationRef::new(sec, 1))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &overrides,
    );
    assert_eq!(f.stage_drop_refs, vec![StationRef::new(sec, 1)]);
    assert_eq!(f.final_turn_ref, Some(StationRef::new(sec, 2)));
    assert!((f.budget_litres - 1100.0).abs() < 1e-6);
    assert!((f.stage_drop_bars[0] - 75.0).abs() < 1e-3);
    assert_eq!(f.stage_drop_over_reserve, vec![false]);
}

#[test]
fn gas_reach_field_override_shifts_downstream_drop_keeping_it_safe() {
    // TwoStage default drops @ stations 1, 2 (stage_in 300 + 200; ⅙ back gas 1100 ⇒ budget 1600).
    // Drag stage 0 DEEPER to station 2 (800 L, within its 900 L cap ⇒ over-reserve, not clamped).
    // Stage 1 wasn't dragged, so it keeps its safe 200 L contribution but its drop shifts deeper
    // (cumulative 800 + 200 = 1000 ⇒ station 3) and stays NOT over-reserve. Budget 1600 + 500 = 2100.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        82.5,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    // Stage 0 (fill 90 ⇒ 900 L cap, stage_in 300) then stage 1 (fill 70 ⇒ 700 L cap, stage_in 200).
    let stage0 = Cylinder::new(
        10.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let stage1 = Cylinder::new(
        10.0,
        207.0,
        70.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::TwoStage,
        ..GasConfig::default()
    };
    let sec = origin.section_id;
    let overrides = vec![Some(StationRef::new(sec, 2))]; // only stage 0 pinned
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage0, stage1],
        &cfg,
        &overrides,
    );
    assert_eq!(
        f.stage_drop_refs,
        vec![StationRef::new(sec, 2), StationRef::new(sec, 3)]
    );
    assert_eq!(f.stage_drop_over_reserve, vec![true, false]);
    assert!((f.budget_litres - 2100.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_override_to_default_station_is_a_no_op() {
    // Pinning the drop to its OWN default-rendered station reproduces the un-dragged field. The
    // default ▼ sits at station 2 (staged_route rounds the 600 L threshold up to the 800 L station),
    // so reading the override as gas_in[2]=800 would fabricate over-reserve + extend the reach on a
    // jiggle / release-in-place - the unsafe direction the review caught. `apply` must no-op it.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let (bg, stage, cfg) = override_rig();
    let sec = origin.section_id;
    let base = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg.clone(), stage.clone()],
        &cfg,
    );
    assert_eq!(base.stage_drop_refs, vec![StationRef::new(sec, 2)]);
    let overrides = vec![Some(StationRef::new(sec, 2))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &overrides,
    );
    assert_eq!(f, base);
}

#[test]
fn gas_reach_field_override_unresolvable_ref_falls_through_to_default() {
    // A Some(ref) that resolves to no station (index past the section) is dropped by the
    // `resolve_key`/`station_index` miss, so the field equals the un-overridden default.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let (bg, stage, cfg) = override_rig();
    let sec = origin.section_id;
    let base = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg.clone(), stage.clone()],
        &cfg,
    );
    let overrides = vec![Some(StationRef::new(sec, 9999))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &overrides,
    );
    assert_eq!(f, base);
}

#[test]
fn gas_reach_field_override_downstream_shallower_than_cumulative_drops_full() {
    // TwoStage: pin stage 0 DEEP (station 2, over-reserve, cumulative 800 L), then pin stage 1
    // SHALLOWER than that cumulative (station 1, gas 300 L < 800). Stage 1's contribution clamps to
    // 0 - it is dropped FULL (bar back at its 70-bar fill), never negative - and is not over-reserve.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        82.5,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage0 = Cylinder::new(
        10.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let stage1 = Cylinder::new(
        10.0,
        207.0,
        70.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::TwoStage,
        ..GasConfig::default()
    };
    let sec = origin.section_id;
    let overrides = vec![Some(StationRef::new(sec, 2)), Some(StationRef::new(sec, 1))];
    let f = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &[bg, stage0, stage1],
        &cfg,
        &overrides,
    );
    assert_eq!(f.stage_drop_over_reserve, vec![true, false]);
    assert!(
        (f.stage_drop_bars[1] - 70.0).abs() < 1e-3,
        "stage 1 dropped full at its 70-bar fill"
    );
}

#[test]
fn gas_reach_field_all_thirds_has_route_and_final_turn_but_no_drops() {
    // All-thirds has the route highlight + final turn (to the deepest reachable station) like any
    // strategy, but NO per-stage drops (those are staged-only).
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        25.0,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg], &GasConfig::default());
    assert!(field.stage_drop_refs.is_empty());
    assert!(field.stage_drop_bars.is_empty());
    // Route + final turn ARE populated (to the deepest reachable station).
    assert!(!field.route.is_empty());
    assert!(field.final_turn_ref.is_some());
    assert!(field.reachable.iter().any(|&r| r));
}

#[test]
fn gas_reach_field_gauges_use_effective_strategy_and_conservative_cap() {
    // The engine computes the readout gauges (single source of truth). They must honour the
    // conservative cap AND the stage-count fallback, so the UI can display them without drifting.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    // Matched twin back gas (fill 90, Z=1) + one OVERFILLED stage (fill 230 on a 207-rated AL).
    let bg = || {
        Cylinder::new(
            12.0,
            232.0,
            90.0,
            TankMaterial::Steel,
            CylRole::BackGas,
            None,
        )
        .unwrap()
    };
    let stage = Cylinder::new(
        11.0,
        207.0,
        230.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();

    // OneStage with the stage present → effective OneStage (back-gas frac ¼).
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let f = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[bg(), bg(), stage.clone()],
        &cfg,
    );
    // avg start 90 × (1 - ¼) = 67.5 (Z=1 so usable=90·24; drop = ¼·90·24/24 = 22.5 → 90-22.5).
    assert!((f.backgas_turn_bar.unwrap() - 67.5).abs() < 1e-6);
    // Stage drop uses the CONSERVATIVE-capped basis min(230,207)=207 → (207+30)/2 = 118.5 (NOT
    // (230+30)/2=130 from the raw overfill) - pins that the gauge matches the engine's drop.
    assert_eq!(f.stage_drop_bars.len(), 1);
    assert!((f.stage_drop_bars[0] - 118.5).abs() < 1e-6);

    // TwoStage REQUESTED with only ONE stage → effective falls back to AllThirds: back-gas frac ⅓,
    // and NO stage-drop gauges (mirrors the budget/route fallback).
    let cfg2 = GasConfig {
        default_reserve: ReservePreset::TwoStage,
        ..GasConfig::default()
    };
    let f2 =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg(), bg(), stage], &cfg2);
    assert!((f2.backgas_turn_bar.unwrap() - 90.0 * (2.0 / 3.0)).abs() < 1e-6);
    assert!(f2.stage_drop_bars.is_empty());
}

#[test]
fn gas_reach_field_backgas_turn_none_without_back_gas() {
    // A stage-only rig has no back-gas gauge (None), but the stage still has a drop gauge.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let stage = Cylinder::new(
        11.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[stage], &cfg);
    assert_eq!(field.backgas_turn_bar, None);
    assert_eq!(field.stage_drop_bars.len(), 1);
}

#[test]
fn gas_reach_field_staged_with_no_stages_falls_back_to_all_thirds() {
    // OneStage REQUESTED but the rig carries ZERO stages → effective AllThirds: NO per-stage drops,
    // empty stage-drop gauges, and the back-gas gauge uses the ⅓ fraction (but the route + final
    // turn are still populated, like any strategy).
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        25.0,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg], &cfg);
    assert!(field.stage_drop_refs.is_empty());
    assert!(field.stage_drop_bars.is_empty());
    assert!(field.final_turn_ref.is_some());
    assert!((field.backgas_turn_bar.unwrap() - 80.0 * (2.0 / 3.0)).abs() < 1e-6);
}

#[test]
fn gas_reach_field_endpoint_selects_the_route_for_drops() {
    // Two branches from a shared origin (section A merged to B at station 0); flat shots ⇒ gas =
    // 2×length. A: a1 300, a2 500 (deepest). B: b1 200, b2 400.
    let mut project = Project::new("Gas".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![make_shot(150.0, 0.0, 0.0), make_shot(100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![make_shot(100.0, 0.0, 0.0), make_shot(100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    let origin = StationRef::new(a, 0);
    let diver = Diver::new(20.0, None).unwrap();
    // OneStage: stage_in 300 (fill 90, w 10, reserve 30); ¼ back gas 200 (fill 80, w 10 = 800) →
    // budget 500 reaches a2 (500) and b2 (400). deepest = a2.
    let bg = Cylinder::new(
        10.0,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        10.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cyls = [bg, stage];
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };

    // No endpoint → route to the deepest (a2); drop where gas ≥ 300 = a1.
    let d = project.compute_gas_reach_field(Some(&origin), None, &diver, &cyls, &cfg);
    assert_eq!(d.final_turn_ref, Some(StationRef::new(a, 2)));
    assert_eq!(d.stage_drop_refs, vec![StationRef::new(a, 1)]);

    // Endpoint = b2 → route follows B instead; drop where gas ≥ 300 = b2, final turn b2.
    let e = project.compute_gas_reach_field(
        Some(&origin),
        Some(&StationRef::new(b, 2)),
        &diver,
        &cyls,
        &cfg,
    );
    assert_eq!(e.final_turn_ref, Some(StationRef::new(b, 2)));
    assert_eq!(e.stage_drop_refs, vec![StationRef::new(b, 2)]);
}

#[test]
fn gas_reach_field_threshold_past_deepest_clamps_drop_to_turnaround() {
    // TwoStage whose budget only reaches the FIRST drop threshold: the 2nd threshold is past the
    // deepest reachable station, so its drop clamps to the turnaround (you turn still carrying that
    // stage). Every stage keeps a drop, so stage_drop_refs stays aligned 1:1 with the gauges.
    let (project, origin) = linear_descent_project(); // gas 0/300/800/1500
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        10.0,
        232.0,
        60.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = || {
        Cylinder::new(
            10.0,
            207.0,
            70.0,
            TankMaterial::Aluminium,
            CylRole::Stage,
            None,
        )
        .unwrap()
    };
    // stage_in = (70-30)/2 × 10 = 200; thresholds 200, 400. ⅙ back gas = 600/6 = 100. budget = 500
    // → reaches station 1 (300), not station 2 (800). drop1 at station 1 (≥200); drop2 (≥400) is
    // past the deepest reachable gas (300) → clamps to the turnaround (station 1). Two drops, both
    // aligned with the two stage gauges.
    let cfg = GasConfig {
        default_reserve: ReservePreset::TwoStage,
        ..GasConfig::default()
    };
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg, stage(), stage()], &cfg);
    let sec = origin.section_id;
    assert!((field.budget_litres - 500.0).abs() < 1e-6);
    assert_eq!(field.final_turn_ref, Some(StationRef::new(sec, 1)));
    assert_eq!(
        field.stage_drop_refs,
        vec![StationRef::new(sec, 1), StationRef::new(sec, 1)],
        "both drops present; the unreachable 2nd clamps to the turnaround"
    );
    assert_eq!(
        field.stage_drop_bars.len(),
        2,
        "gauges aligned with the drops"
    );
    assert_eq!(field.stage_drop_distances_m.len(), 2);
}

#[test]
fn is_swim_leg_distinguishes_shots_from_ties() {
    // Adjacent stations within a section = a swum shot (incl. a reference/jump).
    assert!(is_swim_leg(&StationRef::new(1, 0), &StationRef::new(1, 1)));
    assert!(is_swim_leg(&StationRef::new(1, 2), &StationRef::new(1, 1)));
    // Cross-section (a merge / closure tie) = NOT a swum leg.
    assert!(!is_swim_leg(&StationRef::new(1, 0), &StationRef::new(2, 0)));
    // Same section but non-adjacent (a closure within one section) = NOT a swum leg.
    assert!(!is_swim_leg(&StationRef::new(1, 0), &StationRef::new(1, 2)));
}

#[test]
fn route_metrics_pure_line_length_and_depths() {
    let pt = |x: f64, y: f64, z: f64| Coordinates { x, y, z };
    // Pure helper: 3-D polyline length + deepest point + distance-weighted average depth.
    // Degenerate cases first: empty route, single station.
    assert_eq!(route_metrics(&[], &[]), (0.0, None, None));
    assert_eq!(
        route_metrics(&[pt(5.0, 5.0, 12.0)], &[]),
        (0.0, Some(12.0), Some(12.0))
    );
    // Vertical descent 0→10→30 m (both legs swum): segments 10 + 20 = 30 m; deepest 30;
    // depth-weighted average = (10·mid5 + 20·mid20) / 30 = (50 + 400) / 30 = 15.
    let descent = [pt(0.0, 0.0, 0.0), pt(0.0, 0.0, 10.0), pt(0.0, 0.0, 30.0)];
    let (len, maxd, avgd) = route_metrics(&descent, &[true, true]);
    assert!((len - 30.0).abs() < EPS);
    assert_eq!(maxd, Some(30.0));
    assert!((avgd.unwrap() - 15.0).abs() < EPS);
    // True 3-D legs (east+north, then a pure depth drop): (0,0,0)→(3,4,0) = 5 m, →(3,4,12) = 12 m
    // ⇒ 17 m total, deepest 12.
    let three_d = [pt(0.0, 0.0, 0.0), pt(3.0, 4.0, 0.0), pt(3.0, 4.0, 12.0)];
    let (len2, maxd2, _) = route_metrics(&three_d, &[true, true]);
    assert!((len2 - 17.0).abs() < EPS);
    assert_eq!(maxd2, Some(12.0));
    // Deepest point at an INTERIOR station (descend then rise): max depth must be the interior 30,
    // not the 10 m terminal - a regression that returned the last station's depth would fail here.
    let dome = [pt(0.0, 0.0, 0.0), pt(0.0, 0.0, 30.0), pt(0.0, 0.0, 10.0)];
    let (_, maxd3, _) = route_metrics(&dome, &[true, true]);
    assert_eq!(maxd3, Some(30.0));
    // A TIE leg (swim=false) with a large coord gap - the loop-closure misclosure case - is
    // excluded from length AND the depth-weighted average: only the first (swum) 10 m leg counts.
    let with_tie = [pt(0.0, 0.0, 0.0), pt(0.0, 0.0, 10.0), pt(100.0, 0.0, 10.0)];
    let (len4, maxd4, avgd4) = route_metrics(&with_tie, &[true, false]);
    assert!((len4 - 10.0).abs() < EPS); // the 90 m tie gap is NOT counted
    assert_eq!(maxd4, Some(10.0));
    assert!((avgd4.unwrap() - 5.0).abs() < EPS); // only the 0→10 m leg, mid-depth 5
}

#[test]
fn gas_reach_field_reports_route_distance_and_depths() {
    // linear_descent (3×100 m shots, 0→10→20→30 m). A huge back-gas rig reaches the deepest
    // station, so the route is the whole descent: line length 3×100 = 300 m, deepest 30 m, and the
    // depth-weighted average over the three equal-length legs (mids 5/15/25 m) = 15 m.
    let (project, origin) = linear_descent_project();
    let sec = origin.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &GasConfig::default());
    assert_eq!(field.final_turn_ref, Some(StationRef::new(sec, 3)));
    // Placement preserves tape length for a constant-azimuth descent ⇒ 3×100 m.
    assert!((field.route_line_length_m - 300.0).abs() < 1e-3);
    assert_eq!(field.route_max_depth_m, Some(30.0));
    assert!((field.route_avg_depth_m.unwrap() - 15.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_no_route_has_no_distance_or_depth() {
    // An empty project has nothing reachable ⇒ no route ⇒ zero length and no depths (not 0-depth).
    let project = Project::new("Gas".to_string(), None);
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field(None, None, &diver, &[big], &GasConfig::default());
    assert!(field.final_turn_ref.is_none());
    assert!(field.route_line_length_m.abs() < EPS);
    assert_eq!(field.route_max_depth_m, None);
    assert_eq!(field.route_avg_depth_m, None);
}

#[test]
fn penetration_budget_reserve_at_or_above_fill_zeroes_stage() {
    // reserve == fill ⇒ drop = (fill+reserve)/2 = fill = full ⇒ stage contributes 0 IN gas; the
    // budget collapses to the bare back-gas fraction (no negative / NaN leak).
    let bg = Cylinder::new(
        24.0,
        232.0,
        90.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        10.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let rig = [bg, stage];
    let high = ReserveProfile {
        stage_reserve_bar: 90.0,
        ..ReserveProfile::default()
    };
    let b = penetration_budget(&rig, ReservePreset::OneStage, &high, false);
    assert!((b - 0.25 * 90.0 * 24.0).abs() < 1e-6); // ¼ back gas only
}

#[test]
fn gas_reach_field_monotonic_and_disconnected_is_none() {
    let mut project = Project::new("Gas".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![make_shot(100.0, 0.0, 10.0), make_shot(100.0, 10.0, 20.0)],
            ..Default::default()
        })
        .id;
    // A second, unconnected section (no merge/closure) → separate component.
    let other = project
        .add_section(SectionData {
            shots: vec![make_shot(50.0, 0.0, 5.0)],
            ..Default::default()
        })
        .id;
    let origin = StationRef::new(sec, 0);
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &GasConfig::default());

    // Same-section stations: gas_to_reach is monotonically non-decreasing along the line.
    let g0 = field_gas(&field, StationRef::new(sec, 0));
    let g1 = field_gas(&field, StationRef::new(sec, 1));
    let g2 = field_gas(&field, StationRef::new(sec, 2));
    assert!(g0 <= g1 && g1 <= g2);
    assert!(g0.abs() < EPS); // origin is 0

    // The disconnected section is unreachable: gas_to_reach None, reachable false.
    let oidx = field
        .station_refs
        .iter()
        .position(|r| *r == StationRef::new(other, 0))
        .unwrap();
    assert_eq!(field.gas_to_reach[oidx], None);
    assert!(!field.reachable[oidx]);
}

#[test]
fn gas_reach_field_reaches_across_a_station_merge() {
    // Cross-section reachability over a survey-tied StationMerge (0-cost edge in
    // build_survey_graph_weighted). The disconnected test above proves separate
    // components stay unreachable; this proves a merge bridges them - the multi-section
    // integrity case (a diver swimming A→B via the join). LoopClosures take the same
    // 0-cost path in the graph builder, so the merge exercises that edge class.
    let mut project = Project::new("Gas".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![make_shot(100.0, 0.0, 10.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![make_shot(100.0, 10.0, 20.0)],
            ..Default::default()
        })
        .id;
    // Join A's terminal station (index 1) to B's entry station (index 0).
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    let origin = StationRef::new(a, 0);
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &GasConfig::default());

    // B's far station is reachable across the merge (a separate component would be None).
    let bidx = field
        .station_refs
        .iter()
        .position(|r| *r == StationRef::new(b, 1))
        .unwrap();
    assert!(
        field.reachable[bidx],
        "station in section B must be reachable across the merge"
    );
    // Crossing the 0-cost merge adds no gas: A's terminal and B's entry are the same
    // physical point, so they share a gas_to_reach.
    let ga1 = field_gas(&field, StationRef::new(a, 1));
    let gb0 = field_gas(&field, StationRef::new(b, 0));
    assert!(
        (ga1 - gb0).abs() < EPS,
        "merged stations share gas_to_reach (0-cost join)"
    );

    // Route metrics across the merge TIE (#236): the route is A:0→A:1→[merge tie]→B:0→B:1. The two
    // SWUM shots are 100 m each; the 0-cost merge tie (A:1→B:0, cross-section) must NOT be counted in
    // the line length - is_swim_leg excludes it - so the distance is ~200 m end-to-end. This pins the
    // integration wiring (route_refs → is_swim_leg mask → route_metrics); a regression that dropped
    // the mask would count the tie's measured-coord gap and inflate the length. Deepest station is
    // B:1 (20 m); the swum-leg depth-weighted average is (100·5 + 100·15)/200 = 10 m.
    assert_eq!(field.final_turn_ref, Some(StationRef::new(b, 1)));
    assert!(
        (field.route_line_length_m - 200.0).abs() < 1.0,
        "route_line_length_m should be ~200 m (two swum shots; merge tie excluded), got {}",
        field.route_line_length_m
    );
    assert_eq!(field.route_max_depth_m, Some(20.0));
    assert!((field.route_avg_depth_m.unwrap() - 10.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_turn_frontier_crosses_a_station_merge() {
    // Pins the "weighted and topology graphs share one edge set" invariant a perf fix
    // relies on (#241): `turn_frontier` is filtered against the SAME `adj` the gas
    // Dijkstra ran on, not a separately-built topology graph, so a merge tie must show
    // up as an edge on both sides for the frontier to be correct across it.
    //
    // Section A: A:0 -> A:1, a 1 m leg (trivially cheap). Section B: B:0 -> B:1, a
    // 2000 m leg (expensive enough to fall outside the small cylinder's budget).
    // A:1 is merged onto B:0 (the 0-cost tie). So the only reachable/unreachable
    // boundary is exactly at the merge: B:0 is reachable (cheap: just the 1 m A leg
    // plus the 0-cost tie) and borders the unreachable B:1 (too far); A:0 borders only
    // the reachable A:1/B:0, so it is NOT on the frontier.
    let mut project = Project::new("Gas".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![make_shot(1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![make_shot(2000.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    let origin = StationRef::new(a, 0);
    let diver = Diver::new(20.0, None).unwrap();
    // Small single steel: 12 L x 200 bar = 2400 L of gas, All-Thirds budget ~800 L in-leg -
    // comfortably above the ~2 L A leg + tie, comfortably below the ~4000 L B leg.
    let small = Cylinder::new(
        12.0,
        200.0,
        200.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        &[small],
        &GasConfig::default(),
    );

    let a0 = StationRef::new(a, 0);
    let a1 = StationRef::new(a, 1);
    let b0 = StationRef::new(b, 0);
    let b1 = StationRef::new(b, 1);
    let idx = |sr: StationRef| field.station_refs.iter().position(|r| *r == sr).unwrap();

    assert!(field.reachable[idx(a0)], "A:0 (origin) must be reachable");
    assert!(field.reachable[idx(a1)], "A:1 must be reachable (1 m leg)");
    assert!(
        field.reachable[idx(b0)],
        "B:0 must be reachable across the 0-cost merge tie"
    );
    assert!(
        !field.reachable[idx(b1)],
        "B:1 must be out of budget (2000 m leg)"
    );

    // Hand-computed expected membership: only B:0 sits on the boundary.
    assert_eq!(
        field.turn_frontier,
        vec![b0],
        "turn_frontier must cross the merge tie and stop exactly at B:0"
    );
}

#[test]
fn gas_reach_field_pins_in_leg_gas_values() {
    // Default config: RMV 20, swim 10 m/min, multiplier 1.0, no survey speed. The three legs
    // (avg depths 5/15/25 m → 1.5/2.5/3.5 ATA) cost 20·1.5·10 / 20·2.5·10 / 20·3.5·10 =
    // 300/500/700 L; cumulative gas_in = 300/800/1500. Pins the engine's depth + ATA + speed +
    // RMV wiring end-to-end (a flattened depth, a swapped length↔speed, or a dropped multiplier
    // would all change these - the structural tests would not catch it).
    let (project, origin) = linear_descent_project();
    let sec = origin.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &GasConfig::default());
    assert!((field_gas(&field, StationRef::new(sec, 1)) - 300.0).abs() < 1e-6);
    assert!((field_gas(&field, StationRef::new(sec, 2)) - 800.0).abs() < 1e-6);
    assert!((field_gas(&field, StationRef::new(sec, 3)) - 1500.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_honors_survey_speed_and_multiplier() {
    // survey_speed 5 (½ the swim 10) and survey_rmv_multiplier 1.3 scale the in-leg gas by
    // (10/5)·1.3 = 2.6× - station-1 reach 300 → 780 L. Pins the §5.2 in-leg inputs.
    let (project, origin) = linear_descent_project();
    let sec = origin.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let config = GasConfig {
        survey_speed_m_per_min: Some(5.0),
        survey_rmv_multiplier: 1.3,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &config);
    assert!((field_gas(&field, StationRef::new(sec, 1)) - 780.0).abs() < 1e-6);
}

/// One permanent shot then a reference/jump shot (0 → 10 → 20 m, 100 m each): 3 stations.
fn project_with_jump_second_leg() -> (Project, StationRef) {
    let mut project = Project::new("Gas".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                make_shot(100.0, 0.0, 10.0),
                ShotData {
                    length: 100.0,
                    depth_in: 10.0,
                    depth_out: 20.0,
                    kind: ShotKind::Jump,
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
        .id;
    (project, StationRef::new(sec, 0))
}

#[test]
fn gas_reach_field_excludes_reference_legs() {
    // A Reference tie is not swimmable, so it must not carry a dive route at
    // all (#27 q7). Station 2 sits beyond the tie and becomes unreachable, while
    // station 1 (reached over a permanent shot) is untouched. This is the safe
    // direction: excluding an edge can only shrink reach, never overstate it.
    let (mut project, origin) = project_with_jump_second_leg();
    let sec = origin.section_id;
    project.sections[0].shots[1].kind = ShotKind::Reference;

    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();

    let field = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        std::slice::from_ref(&big),
        &GasConfig::default(),
    );
    let reachable_at = |field: &GasReachField, i: u32| {
        let target = StationRef::new(sec, i);
        field
            .station_refs
            .iter()
            .position(|r| *r == target)
            .map(|idx| field.reachable[idx])
    };
    assert_eq!(
        reachable_at(&field, 1),
        Some(true),
        "a permanent leg is unaffected by the reference exclusion"
    );
    assert_eq!(
        reachable_at(&field, 2),
        Some(false),
        "a station reachable only across a reference tie must be unreachable"
    );

    // The opt-in restores the pre-#27 behaviour exactly (#27 q13).
    let opted_in = GasConfig {
        include_reference_legs: true,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &opted_in);
    assert_eq!(
        reachable_at(&field, 2),
        Some(true),
        "include_reference_legs must make the leg divable again"
    );
}

/// #27: `include_reference_legs` is documented as restoring the pre-#27
/// inclusive behaviour, and before the split EVERY virtual leg carried
/// `jump_rmv_multiplier`. Charging an opted-in Reference at base RMV made a
/// route cost LESS gas than it did before the migration, which overstates the
/// reachable frontier - the unsafe direction the module's §6 rule forbids.
#[test]
fn opted_in_reference_legs_carry_the_jump_rmv_multiplier() {
    let (mut project, origin) = project_with_jump_second_leg();
    let sec = origin.section_id;
    // Leg 1 (station 1 -> 2) becomes a non-swimmable tie, opted back in below.
    project.sections[0].shots[1].kind = ShotKind::Reference;

    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();

    let gas_to_reach_station_2 = |multiplier: f64| -> f64 {
        let cfg = GasConfig {
            include_reference_legs: true,
            jump_rmv_multiplier: multiplier,
            ..GasConfig::default()
        };
        let field = project.compute_gas_reach_field(
            Some(&origin),
            None,
            &diver,
            std::slice::from_ref(&big),
            &cfg,
        );
        field_gas(&field, StationRef::new(sec, 2))
    };

    let plain = gas_to_reach_station_2(1.0);
    let bumped = gas_to_reach_station_2(2.0);
    assert!(
        bumped > plain,
        "the multiplier must raise an opted-in reference leg's gas: {bumped} vs {plain}"
    );
}

#[test]
fn gas_reach_field_route_legs_flag_jumps() {
    // The route's reference (jump) leg is flagged `is_jump` so the UI can dash it (jumps are
    // normally hidden). Route origin → station 2 (deepest): leg 0 (0→1) regular, leg 1 (1→2) jump.
    let (project, origin) = project_with_jump_second_leg();
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &GasConfig::default());
    let sec = origin.section_id;
    assert_eq!(field.route.len(), 2);
    assert!(!field.route[0].is_jump);
    assert!(field.route[1].is_jump);
    assert_eq!(field.route[1].from, StationRef::new(sec, 1));
    assert_eq!(field.route[1].to, StationRef::new(sec, 2));
}

#[test]
fn gas_reach_field_jump_multiplier_raises_only_jump_legs() {
    // Leg 0 is a permanent shot (avg 5 m → 1.5 ATA → 20·1.5·10 = 300 L); leg 1 is a
    // reference/jump (avg 15 m → 2.5 ATA → base 20·2.5·10 = 500 L). A 1.5× jump multiplier
    // scales ONLY the jump leg's RMV (20 → 30 → 750 L): station 1 is untouched at 300, while
    // station 2 rises from 300+500=800 to 300+750=1050. Pins that the bump targets jump
    // legs and nothing else (a misapplied multiplier would also move station 1).
    let (project, origin) = project_with_jump_second_leg();
    let sec = origin.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();

    // Explicit 1.0 base - the DEFAULT jump_rmv_multiplier is now 1.5, so a plain
    // `GasConfig::default()` base would already bump the jump leg and collapse the contrast.
    let unbumped = GasConfig {
        jump_rmv_multiplier: 1.0,
        ..GasConfig::default()
    };
    let base = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &diver,
        std::slice::from_ref(&big),
        &unbumped,
    );
    assert!((field_gas(&base, StationRef::new(sec, 1)) - 300.0).abs() < 1e-6);
    assert!((field_gas(&base, StationRef::new(sec, 2)) - 800.0).abs() < 1e-6);

    let bumped = GasConfig {
        jump_rmv_multiplier: 1.5,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[big], &bumped);
    assert!((field_gas(&field, StationRef::new(sec, 1)) - 300.0).abs() < 1e-6);
    assert!((field_gas(&field, StationRef::new(sec, 2)) - 1050.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_partial_frontier_between_interior_stations() {
    // AllThirds budget = deliverable(250 bar, 12 L)/3 = 3000 / Z(250)≈1.0753 / 3 ≈ 929.9 L
    // (Z-derated thirds), which sits between gas_in(station 2)=800 and gas_in(station 3)=1500 →
    // reach {0,1,2}, turn at station 2. The budget is pinned to an INDEPENDENT hand-computed
    // literal (catches a wrong /3, a dropped Z, or a bad fit), not re-derived from the function.
    let (project, origin) = linear_descent_project();
    let sec = origin.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let cyl = Cylinder::new(
        12.0,
        250.0,
        250.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[cyl], &GasConfig::default());
    assert!((field.budget_litres - 929.9).abs() < 1.0);
    assert_eq!(field.reachable.iter().filter(|&&r| r).count(), 3); // stations 0,1,2
    let idx = |sr| field.station_refs.iter().position(|r| *r == sr).unwrap();
    assert!(field.reachable[idx(StationRef::new(sec, 2))]);
    assert!(!field.reachable[idx(StationRef::new(sec, 3))]);
    assert_eq!(field.turn_frontier, vec![StationRef::new(sec, 2)]);
    // max_reachable_gas is the deepest WITHIN-budget cost (station 2 = 800 L), NOT the
    // out-of-budget station 3's finite 1500 - it feeds the "max penetration gas" readout,
    // which must never report gas the diver can't actually turn at.
    assert!((field.max_reachable_gas - 800.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_unnormalized_config_bails_to_all_unreachable() {
    // The engine's defensive guard: a non-positive / non-finite in-speed or in-rmv (a config
    // that skipped GasConfig::normalize) MUST yield an all-unreachable field - never the unsafe
    // over-stating direction (§6). compute_gas_reach_field is called DIRECTLY here, bypassing
    // normalize (covered separately in config/tests.rs), so a future refactor can't silently
    // invert the guard without failing this test.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let cyl = Cylinder::new(
        12.0,
        250.0,
        250.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();

    for cfg in [
        // Zero survey-in speed (present, so in_speed uses it rather than the swim fallback).
        GasConfig {
            survey_speed_m_per_min: Some(0.0),
            ..GasConfig::default()
        },
        // Zero / NaN survey-RMV multiplier → in_rmv = 0 / NaN.
        GasConfig {
            survey_rmv_multiplier: 0.0,
            ..GasConfig::default()
        },
        GasConfig {
            survey_rmv_multiplier: f64::NAN,
            ..GasConfig::default()
        },
    ] {
        let field = project.compute_gas_reach_field(
            Some(&origin),
            None,
            &diver,
            std::slice::from_ref(&cyl),
            &cfg,
        );
        assert!(
            field.reachable.iter().all(|&r| !r),
            "all stations unreachable"
        );
        assert!(
            field.gas_to_reach.iter().all(|g| g.is_none()),
            "all gas_to_reach None"
        );
        assert!(field.turn_frontier.is_empty(), "empty turn frontier");
        assert!(
            field.max_reachable_gas.abs() < EPS,
            "max_reachable_gas == 0"
        );
    }
}

#[test]
fn gas_reach_field_none_origin_seeds_from_zero_datums() {
    let (mut project, origin) = linear_descent_project();
    project.zero_datums.push(ZeroDatum {
        station_ref: origin.into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        accuracy_m: None,
    });
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    // None origin → seeds from the zero datum (the entrance station).
    let field = project.compute_gas_reach_field(None, None, &diver, &[big], &GasConfig::default());
    assert!(field_gas(&field, origin).abs() < EPS); // gas to reach the seed datum is 0
    assert!(field.reachable.iter().all(|&r| r)); // huge budget → all reachable from the datum
}

#[test]
fn gas_reach_field_unresolvable_origin_falls_back_to_entrance() {
    // A set-but-unresolvable origin (e.g. a dive plan stayed loaded across a Survey edit that
    // deleted its origin station, #244) must degrade to the entrance, NOT an all-unreachable
    // field. Without the fallback this returns empty roots → the whole map greys out.
    let (mut project, origin) = linear_descent_project();
    project.zero_datums.push(ZeroDatum {
        station_ref: origin.into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        accuracy_m: None,
    });
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let ghost = StationRef::new(origin.section_id + 999, 0); // section that does not exist
    let field =
        project.compute_gas_reach_field(Some(&ghost), None, &diver, &[big], &GasConfig::default());
    assert!(field.reachable.iter().all(|&r| r)); // fell back to the entrance, not all-grey
    assert!(field_gas(&field, origin).abs() < EPS);
}

// ---- #298: ordered way points ---------------------------------------------------------

/// A Y-shaped cave for the way-point tests: a trunk of 3 shots descending 0→30 m (100 m each)
/// and a 1-shot side branch tied to the trunk's entrance by a station merge. Returns
/// `(project, entrance, side_tip, trunk_tip)`.
///
/// Gas-to-reach from the entrance at 20 L/min and the default 10 m/min swim speed (Z ≈ 1 at
/// these depths): trunk stations 0/1/2/3 = 0/300/800/1500 L; the side tip = 300 L.
fn branching_project() -> (Project, StationRef, StationRef, StationRef) {
    let mut project = Project::new("Gas".to_string(), None);
    let trunk = project
        .add_section(SectionData {
            shots: vec![
                make_shot(100.0, 0.0, 10.0),
                make_shot(100.0, 10.0, 20.0),
                make_shot(100.0, 20.0, 30.0),
            ],
            ..Default::default()
        })
        .id;
    let side = project
        .add_section(SectionData {
            shots: vec![make_shot(100.0, 0.0, 10.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(trunk, 0), StationRef::new(side, 0))
        .unwrap();
    (
        project,
        StationRef::new(trunk, 0),
        StationRef::new(side, 1),
        StationRef::new(trunk, 3),
    )
}

/// A cylinder with effectively unlimited gas - for tests about ROUTE shape, not budget.
fn unlimited_backgas() -> Cylinder {
    Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap()
}

/// The ordered route stations of a field (its legs re-joined), for asserting the visit order.
fn route_stations(field: &GasReachField) -> Vec<StationRef> {
    match field.route.first() {
        Some(first) => std::iter::once(first.from)
            .chain(field.route.iter().map(|l| l.to))
            .collect(),
        None => Vec::new(),
    }
}

#[test]
fn gas_reach_field_no_waypoints_is_identical_to_the_plain_field() {
    // THE guard for the #298 generalisation: an empty way-point list must leave the single-Dijkstra
    // field exactly as it was. Compares every output of the way-point entry point against the
    // no-way-point convenience entry on a branching cave with a staged rig (so route, drops, gauges
    // and distances are all populated, not just the trivially-empty ones).
    let (project, entrance, _side, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        25.0,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        10.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let plain = project.compute_gas_reach_field(
        Some(&entrance),
        Some(&trunk_tip),
        &diver,
        &[bg.clone(), stage.clone()],
        &cfg,
    );
    let with_empty = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &[],
    );
    // ⚠ This equality is STRUCTURALLY VACUOUS on its own and is kept only as a wiring check:
    // `compute_gas_reach_field` now forwards to `compute_gas_reach_field_with_overrides` with an
    // empty slice, so both sides run the same code with the same arguments and the compiler
    // guarantees the assert. It cannot go red for any regression in the no-way-point path.
    assert_eq!(plain, with_empty);
    assert!(plain.waypoint_gas_litres.is_empty());
    assert!(!plain.route.is_empty());
    assert_eq!(plain.stage_drop_refs.len(), 1);

    // THE REAL GUARD: hand-computed gas, independent of the engine. At the default 1.0 survey
    // multiplier and 10 m/min, a 100 m leg costs `20 · ata(avg depth) · 10 min`, so the trunk's
    // 0→10 / 10→20 / 20→30 legs cost 300 / 500 / 700 L and the side branch's 0→10 leg costs 300 L.
    // These are the numbers the single-Dijkstra engine produced before way points existed; if the
    // chain machinery ever perturbs the empty-chain case, they move and this fails.
    let (_p, entrance2, side_tip, _tip) = branching_project();
    assert_eq!(entrance2, entrance);
    assert!(field_gas(&plain, entrance).abs() < 1e-6);
    assert!((field_gas(&plain, StationRef::new(trunk_tip.section_id, 1)) - 300.0).abs() < 1e-6);
    assert!((field_gas(&plain, StationRef::new(trunk_tip.section_id, 2)) - 800.0).abs() < 1e-6);
    assert!((field_gas(&plain, trunk_tip) - 1500.0).abs() < 1e-6);
    assert!((field_gas(&plain, side_tip) - 300.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_waypoint_forces_the_detour_and_rebases_the_field() {
    // A way point on the side branch must be VISITED: the route runs entrance → side tip → back →
    // trunk tip, and every station's gas-to-reach is re-based to include the mandated detour.
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig::default();

    let plain = project.compute_gas_reach_field(
        Some(&entrance),
        Some(&trunk_tip),
        &diver,
        &[unlimited_backgas()],
        &cfg,
    );
    let via_side = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[side_tip],
        &[],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );

    // The plain route never touches the side branch; the way-point route starts at the entrance,
    // takes in the side tip, and only then descends the trunk. (The entrance is a merged pair, so
    // the route legitimately lists both of its refs across the zero-distance tie.)
    assert!(!route_stations(&plain).contains(&side_tip));
    let route = route_stations(&via_side);
    assert_eq!(route.first(), Some(&entrance));
    assert_eq!(route.last(), Some(&trunk_tip));
    let side_at = route.iter().position(|r| *r == side_tip).unwrap();
    let tip_at = route.iter().position(|r| *r == trunk_tip).unwrap();
    assert!(side_at < tip_at, "the way point must be visited first");
    // The detour costs 300 L out + 300 L back, so every trunk station is 600 L dearer than before.
    assert!((field_gas(&via_side, trunk_tip) - field_gas(&plain, trunk_tip) - 600.0).abs() < 1e-6);
    // But a station the route merely SWIMS THROUGH is still reported at the gas burned when it is
    // passed - the entrance is where the dive starts, so it stays free. Pricing it as an out-and-back
    // from the way point (600 L here) would push it out of budget on a modest rig and manufacture a
    // turn-frontier dot - a red "candidate endpoint" - in the middle of the diver's own route.
    assert!(field_gas(&via_side, entrance).abs() < 1e-6);
    assert!((field_gas(&via_side, side_tip) - 300.0).abs() < 1e-6);
    // The cumulative gas AT the way point is reported for the readout.
    assert_eq!(via_side.waypoint_gas_litres.len(), 1);
    assert!((via_side.waypoint_gas_litres[0].unwrap() - 300.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_waypoints_are_visited_in_the_given_order() {
    // Two way points are an ORDERED constraint, not a set: swapping them swaps the visit order (and
    // here also the total cost, since the two detours are different depths).
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let trunk_mid = StationRef::new(entrance.section_id, 2);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig::default();
    let field_for = |waypoints: &[StationRef]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&entrance),
            Some(&trunk_tip),
            waypoints,
            &[],
            &diver,
            &[unlimited_backgas()],
            &cfg,
            &[],
        )
    };

    let side_first = field_for(&[side_tip, trunk_mid]);
    let mid_first = field_for(&[trunk_mid, side_tip]);

    let side_pos = |f: &GasReachField| {
        route_stations(f)
            .iter()
            .position(|r| *r == side_tip)
            .unwrap()
    };
    let mid_pos = |f: &GasReachField| {
        route_stations(f)
            .iter()
            .position(|r| *r == trunk_mid)
            .unwrap()
    };
    assert!(side_pos(&side_first) < mid_pos(&side_first));
    assert!(mid_pos(&mid_first) < side_pos(&mid_first));
    // Visiting the deep trunk station first and then backtracking to the shallow side branch costs
    // strictly more gas, so the order genuinely changes the plan rather than just the drawing.
    assert!(field_gas(&mid_first, trunk_tip) > field_gas(&side_first, trunk_tip));
}

#[test]
fn gas_reach_field_disconnected_waypoint_stalls_the_whole_plan() {
    // A way point in another component cannot be flown. Silently skipping it would plan a route the
    // diver never agreed to and OVER-state their reach, so the field goes all-unreachable and the
    // readout says which way point broke.
    let (mut project, entrance, side_tip, _trunk_tip) = branching_project();
    let island = project
        .add_section(SectionData {
            shots: vec![make_shot(100.0, 0.0, 10.0)],
            ..Default::default()
        })
        .id;
    let marooned = StationRef::new(island, 1);
    let diver = Diver::new(20.0, None).unwrap();

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[marooned, side_tip],
        &[],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );
    assert!(!field.reachable.iter().any(|&r| r));
    assert!(field.gas_to_reach.iter().all(Option::is_none));
    assert!(field.turn_frontier.is_empty());
    assert!(field.final_turn_ref.is_none());
    assert!(field.stage_drop_refs.is_empty());
    // Both entries report None: the first because it is unreachable, the second because the walk
    // stalled before it.
    assert_eq!(field.waypoint_gas_litres, vec![None, None]);
}

#[test]
fn gas_reach_field_unresolvable_waypoint_stalls_rather_than_being_skipped() {
    // A way point whose station was deleted under a loaded plan must NOT be dropped from the route:
    // that would quietly relax a constraint the diver set. Contrast the origin, which deliberately
    // degrades to the entrance.
    let (project, entrance, _side, trunk_tip) = branching_project();
    let ghost = StationRef::new(entrance.section_id + 999, 0);
    let diver = Diver::new(20.0, None).unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[ghost],
        &[],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );
    assert_eq!(field.waypoint_gas_litres, vec![None]);
    assert!(!field.reachable.iter().any(|&r| r));
}

#[test]
fn gas_reach_field_waypoint_over_budget_still_shows_the_committed_chain() {
    // When the mandated detour alone exceeds the budget there is no turnaround and the way point
    // itself is out of reach, but the route drawn so far is kept so the diver can SEE which way point
    // blew the gas (the panel compares waypoint_gas_litres against budget_litres). The near end of
    // the chain stays reachable - the diver really can swim that far, they just cannot finish - and
    // showing how far along the mandated route the gas lasts is the whole diagnostic.
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    // 20 bar x 10 L water, all-thirds ⇒ budget ≈ 66 L, far under the 300 L to the side tip.
    let tiny = Cylinder::new(
        10.0,
        207.0,
        20.0,
        TankMaterial::Aluminium,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[side_tip],
        &[],
        &diver,
        &[tiny],
        &GasConfig::default(),
        &[],
    );
    assert!(field.waypoint_gas_litres[0].unwrap() > field.budget_litres);
    // The way point is unaffordable, so there is no route target at all...
    assert!(!field_reachable(&field, side_tip));
    assert!(field.final_turn_ref.is_none());
    // ...but the diver can still swim off the entrance, which is where the plan starts.
    assert!(field_reachable(&field, entrance));
    // The chain the way point committed is still drawn - entrance → … → side tip - even though
    // nothing on it is in budget.
    let route = route_stations(&field);
    assert_eq!(route.first(), Some(&entrance));
    assert_eq!(route.last(), Some(&side_tip));
}

#[test]
fn gas_reach_field_default_drop_on_a_revisited_station_reports_the_right_pass_distance() {
    // A way-point route doubles back over the entrance, so that station appears TWICE on the route.
    // The drop's reported distance must come from the pass it actually sits on (the return leg),
    // which is why placement moved to route INDICES: a ref-based `position()` lookup would match the
    // first occurrence and report 0 m for a drop made 200 m into the dive.
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    // Sized so the plan actually exercises the revisit. Route gas runs
    // 0 (entrance) → 300 (side tip) → 600 (back at the entrance) → 2100 (trunk tip), so the stage's
    // IN gas must land strictly between 300 and 600 for its drop to fall on the RETURN pass through
    // the entrance rather than at the side tip; and the back gas must be big enough that the trunk
    // tip is still in budget, or the route would stop short of the doubling-back leg entirely.
    let bg = Cylinder::new(
        40.0,
        232.0,
        200.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        10.0,
        207.0,
        130.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    // No override: this is about where the STRATEGY's own drop lands and what distance it reports.
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[side_tip],
        &[],
        &diver,
        &[bg, stage],
        &cfg,
        &[],
    );
    let route = route_stations(&field);
    assert_eq!(
        route.iter().filter(|r| **r == entrance).count(),
        2,
        "the detour must revisit the entrance for this test to mean anything"
    );
    // The stage's IN gas is first cleared on the way BACK through the entrance point (its outbound
    // pass is at 0 L), so the drop sits on the return leg. (The entrance is a merged pair, so the
    // engine may name it by either section's ref - what matters is the distance.)
    assert_eq!(field.stage_drop_refs.len(), 1);
    assert_eq!(
        field.final_turn_ref,
        Some(trunk_tip),
        "the trunk tip must stay in budget or the route never doubles back"
    );
    // 100 m out to the side tip + 100 m back = 200 m swum at that pass. A ref-based lookup would
    // have matched the entrance's FIRST appearance on the route and reported 0 m.
    assert!(
        (field.stage_drop_distances_m[0] - 200.0).abs() < 1e-6,
        "expected 200 m, got {}",
        field.stage_drop_distances_m[0]
    );
    // The published route index (#299) must name the SAME pass - it is what a profile marker is
    // drawn at, and this is the one fixture where a ref-keyed lookup and an index-keyed one
    // disagree. Resolve it through the profile exactly as the chart does.
    assert_eq!(field.stage_drop_route_indices.len(), 1);
    let drop_at = field.stage_drop_route_indices[0];
    let point = field
        .route_profile
        .iter()
        .find(|p| p.outbound && p.route_index == drop_at)
        .expect("the drop index must resolve to a point on the profile");
    assert!(
        (point.distance_m - 200.0).abs() < 1e-6,
        "the profile marker landed on the wrong pass: {} m",
        point.distance_m
    );
    assert!(
        drop_at > 0,
        "index 0 is the entrance's FIRST pass - the one a ref lookup would have wrongly matched"
    );
}

#[test]
fn gas_reach_field_drop_pinned_on_the_outbound_chain_cannot_inflate_the_budget() {
    // THE #298 safety regression for the #246 override path. `gas_in` is measured from the LAST way
    // point, so for a station the diver passes on the way OUT it reports the cost of returning to it
    // - here 600 L for the entrance, versus the 0 L actually burned when they swam past it. Scoring
    // a pin there against `gas_in` would read a drop made at the entrance as a 600 L burn, flag it
    // over-reserve and ADD ~600 L to the penetration budget: an action that should leave the stage
    // FULL would instead extend the reach. Pinning must use the outbound (chain) gas.
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        25.0,
        232.0,
        200.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        10.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let rig = [bg, stage];
    let unpinned = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[side_tip],
        &[],
        &diver,
        &rig,
        &cfg,
        &[],
    );
    let pinned = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[side_tip],
        &[],
        &diver,
        &rig,
        &cfg,
        &[Some(entrance)],
    );

    // Dropping the stage at the entrance leaves it full, so the plan can only get SHORTER.
    assert!(
        pinned.budget_litres <= unpinned.budget_litres + 1e-9,
        "pinning a drop on the outbound chain must not grow the budget: {} > {}",
        pinned.budget_litres,
        unpinned.budget_litres
    );
    // And it is not a reserve breach - the diver breathed nothing off that stage.
    assert_eq!(pinned.stage_drop_over_reserve, vec![false]);
}

/// Two branches off one entrance, for the endpoint-fallback tests. `north` is the branch the diver
/// picks an endpoint on; `east` is a competing branch whose stations sit at different gas costs, so
/// "the deepest reachable station anywhere" can legitimately land on it. All shots are at the
/// surface, so gas is proportional to distance: a 100 m shot costs 200 L at the default 20 L/min and
/// 10 m/min.
///
/// Returns `(project, entrance, north_section, east_section, north_tip, east_mid)`.
fn two_branch_project() -> (Project, StationRef, u32, u32, StationRef, StationRef) {
    let mut project = Project::new("Gas".to_string(), None);
    let north = project
        .add_section(SectionData {
            shots: vec![
                make_shot(100.0, 0.0, 0.0),
                make_shot(100.0, 0.0, 0.0),
                make_shot(100.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let east = project
        .add_section(SectionData {
            shots: vec![
                make_shot(130.0, 0.0, 0.0),
                make_shot(130.0, 0.0, 0.0),
                make_shot(130.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(north, 0), StationRef::new(east, 0))
        .unwrap();
    (
        project,
        StationRef::new(north, 0),
        north,
        east,
        StationRef::new(north, 3),
        StationRef::new(east, 1),
    )
}

/// The reachable station with the largest gas-to-reach - i.e. what "deepest in budget ANYWHERE"
/// would have picked. Used to prove the endpoint-fallback test is not vacuous.
fn deepest_reachable(field: &GasReachField) -> StationRef {
    let mut best = field.station_refs[0];
    let mut best_gas = f64::NEG_INFINITY;
    for (i, r) in field.station_refs.iter().enumerate() {
        if field.reachable[i] {
            if let Some(g) = field.gas_to_reach[i] {
                if g > best_gas {
                    best_gas = g;
                    best = *r;
                }
            }
        }
    }
    best
}

#[test]
fn gas_reach_field_out_of_budget_endpoint_turns_early_on_its_own_branch() {
    // Reported from manual inspection: with an endpoint set on one branch, adding a way point that
    // costs a little extra gas moved the turnaround to a DIFFERENT branch, which reads as the plan
    // picking a destination at random. The turnaround should stay on the passage the diver chose and
    // simply turn earlier.
    let (project, entrance, north, east, north_tip, east_mid) = two_branch_project();
    let diver = Diver::new(20.0, None).unwrap();
    // ~800 L of All-thirds budget: enough to reach the north tip (600 L) directly, not enough once
    // the way-point detour is priced in.
    let bg = Cylinder::new(
        13.0,
        232.0,
        200.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let cfg = GasConfig::default();

    // Baseline: no way point, the endpoint is in budget and is the turnaround.
    let direct = project.compute_gas_reach_field(
        Some(&entrance),
        Some(&north_tip),
        &diver,
        std::slice::from_ref(&bg),
        &cfg,
    );
    assert_eq!(
        direct.final_turn_ref,
        Some(north_tip),
        "setup: the endpoint must be reachable without the detour (budget {})",
        direct.budget_litres
    );

    // Add a way point on the east branch. The detour now puts the north tip out of budget.
    let detoured = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&north_tip),
        &[east_mid],
        &[],
        &diver,
        std::slice::from_ref(&bg),
        &cfg,
        &[],
    );
    let turn = detoured
        .final_turn_ref
        .expect("something is still reachable");
    assert_ne!(
        turn, north_tip,
        "setup: the detour must push the endpoint out of budget for this test to mean anything"
    );

    // NON-VACUITY: the old rule (deepest in budget anywhere) really would have jumped branches here.
    assert_eq!(
        deepest_reachable(&detoured).section_id,
        east,
        "setup: the deepest reachable station must be on the OTHER branch, else this test would \
         pass under the old rule too"
    );

    // THE PROPERTY: the turnaround stays on the endpoint's own branch, just closer in.
    assert_eq!(
        turn.section_id, north,
        "the turnaround jumped to another passage instead of turning early on the chosen one"
    );
    assert!(
        turn.station_index < north_tip.station_index,
        "the turnaround must be closer in than the endpoint, got {turn:?}"
    );
    // And it is the FARTHEST such station - turning earlier than necessary would waste the dive.
    let turn_gas = field_gas(&detoured, turn);
    let next = StationRef::new(north, turn.station_index + 1);
    assert!(
        field_gas(&detoured, next) > detoured.budget_litres,
        "the next station out ({next:?}) was still in budget, so the turn was too early: {} vs \
         budget {}",
        field_gas(&detoured, next),
        detoured.budget_litres
    );
    assert!(turn_gas <= detoured.budget_litres);
}

#[test]
fn gas_reach_field_disconnected_endpoint_still_falls_back_to_deepest() {
    // The early-turn walk only applies when there IS a path toward the endpoint. An endpoint in
    // another component has none, so the plan must still fall back to the deepest reachable point
    // rather than returning no route at all.
    let (mut project, entrance, _north, _east, _north_tip, _east_mid) = two_branch_project();
    let island = project
        .add_section(SectionData {
            shots: vec![make_shot(100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let marooned = StationRef::new(island, 1);
    let diver = Diver::new(20.0, None).unwrap();
    let field = project.compute_gas_reach_field(
        Some(&entrance),
        Some(&marooned),
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
    );
    assert!(field.final_turn_ref.is_some());
    assert_eq!(field.final_turn_ref, Some(deepest_reachable(&field)));
}

#[test]
fn gas_reach_field_waypoint_does_not_put_a_turn_candidate_inside_the_endpoint() {
    // Reported from manual inspection: with a way point set, a red turn-frontier dot - which the map
    // offers as a CANDIDATE ENDPOINT - appeared on the route, nearer the exit than the endpoint the
    // diver had already chosen. Cause: `gas_in` prices a swum-through station as an out-and-back
    // detour from the last way point, so the entrance cost 2 x d(origin, W) (800 L here), fell out of
    // a 719 L budget, and the first station outboard of it became a frontier.
    let (project, entrance, north, _east, north_tip, _east_mid) = two_branch_project();
    let waypoint = StationRef::new(north, 2);
    let diver = Diver::new(20.0, None).unwrap();
    // 170 bar in a 13 L steel ⇒ ~719 L of All-thirds budget: the endpoint (600 L) is affordable,
    // while the entrance's old out-and-back price (800 L) was not. That gap is what produced the dot.
    let bg = Cylinder::new(
        13.0,
        232.0,
        170.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&north_tip),
        &[waypoint],
        &[],
        &diver,
        std::slice::from_ref(&bg),
        &GasConfig::default(),
        &[],
    );

    // SETUP: the endpoint really is affordable, so the turnaround must be the endpoint itself. If
    // this ever fails the test is measuring the out-of-budget path instead.
    assert!(field.budget_litres > 600.0 && field.budget_litres < 800.0);
    assert_eq!(field.final_turn_ref, Some(north_tip));

    // The dive STARTS at the entrance, so it is free and in reach. Both assertions failed before the
    // fix (0 L vs 800 L, reachable vs not), which is what makes this test non-vacuous.
    assert!(field_gas(&field, entrance).abs() < 1e-6);
    assert!(field_reachable(&field, entrance));

    // THE PROPERTY: nothing the route swims through on the way out is offered as a turn candidate.
    // Those dots mean "this is as far as you get" - on your own route, inside your own endpoint,
    // they read as the plan proposing a second, nearer endpoint.
    let route = route_stations(&field);
    let target_at = route.iter().position(|r| *r == north_tip).unwrap();
    for r in &route[..target_at] {
        assert!(
            !field.turn_frontier.contains(r),
            "{r:?} is on the route inside the endpoint but is offered as a turn candidate"
        );
    }
}

#[test]
fn gas_reach_field_waypoint_does_not_offer_a_transit_station_as_a_turn_candidate() {
    // The same manual inspection, one round later: red turn dots STILL sat on the route line, at
    // every junction where a side passage left it. Pricing a swum-through station correctly had made
    // it in-budget, and the frontier rule - "in budget, with an out-of-budget neighbour" - then fired
    // on it. Two things were wrong. A turn marker is a clickable candidate ENDPOINT, so it has to ask
    // whether the dive can finish there (the after-chain price), not whether the diver passes through
    // (the pass-through price the overlay paints). And a station the route commits to swimming
    // through is transit, never a destination.
    //
    // The entrance is the sharp case: it is a merge of the two sections, so the graph holds it as two
    // zero-cost-joined nodes. Only ONE of them lies on the route, which is why the route-walking
    // assertion in the test above did not catch this - the twin bordered the out-of-budget east
    // branch and became a frontier on its own.
    let (project, entrance, north, east, north_tip, _east_mid) = two_branch_project();
    let waypoint = StationRef::new(north, 2);
    let east_entrance = StationRef::new(east, 0);
    let east_next = StationRef::new(east, 1);
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        13.0,
        232.0,
        170.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&north_tip),
        &[waypoint],
        &[],
        &diver,
        std::slice::from_ref(&bg),
        &GasConfig::default(),
        &[],
    );

    // SETUP - the border this test is about has to exist: the entrance is painted in reach (the dive
    // starts there), while the east branch one shot along it is not, because reaching it AFTER the
    // way point is a 260 m round trip back down the north passage and out again.
    assert_eq!(field.final_turn_ref, Some(north_tip));
    assert!(field_reachable(&field, entrance));
    assert!(field_reachable(&field, east_entrance));
    assert!(!field_reachable(&field, east_next));

    // THE PROPERTY: neither node of that junction is offered as a place to turn the dive around.
    // Both were before this fix - the east twin via the frontier rule, the north one on the route.
    assert!(
        !field.turn_frontier.contains(&entrance),
        "the entrance is on the committed route but is offered as a turn candidate"
    );
    assert!(
        !field.turn_frontier.contains(&east_entrance),
        "the entrance's merge twin is offered as a turn candidate"
    );
}

#[test]
fn gas_reach_field_a_merge_twin_of_a_route_station_is_not_a_turn_candidate() {
    // The test above holds for the wrong reason at 170 bar: the twin is ALSO out of budget there, so
    // the destination check alone suppresses it. Raise the gas until the twin is affordable and the
    // real question is exposed - whether excluding the committed chain covers the twin at all.
    //
    // It did not. The chain is a predecessor-tree walk, so it names one node per physical station;
    // a merge holds the entrance as TWO zero-cost-joined nodes and only the north one is on that
    // walk. The east twin sits at the same place, at the same gas, carrying the east branch - so it
    // bordered the out-of-budget branch and put a turn dot back on the entrance, at a different gas
    // load than the one the first fix was written against.
    let (project, entrance, north, east, north_tip, _east_mid) = two_branch_project();
    let waypoint = StationRef::new(north, 2);
    let east_entrance = StationRef::new(east, 0);
    let east_next = StationRef::new(east, 1);
    let diver = Diver::new(20.0, None).unwrap();
    // 220 bar in the 13 L steel gives ~905 L of All-thirds budget, which straddles the entrance's
    // after-chain price (800 L) and the east branch's (1060 L). That window is what makes the twin
    // affordable as a destination while the ground past it is not - i.e. a genuine frontier, but on
    // a station the dive is committed to swimming through.
    let bg = Cylinder::new(
        13.0,
        232.0,
        220.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&north_tip),
        &[waypoint],
        &[],
        &diver,
        std::slice::from_ref(&bg),
        &GasConfig::default(),
        &[],
    );

    // SETUP - pin the window. If the budget ever drifts outside it the test stops measuring the
    // twin case and silently passes for the same wrong reason as its predecessor.
    assert!(
        field.budget_litres > 800.0 && field.budget_litres < 1060.0,
        "budget {} no longer straddles the entrance and the east branch",
        field.budget_litres
    );
    assert!(field_reachable(&field, east_entrance));
    assert!(!field_reachable(&field, east_next));

    // THE PROPERTY. Red before the zero-cost settling of `on_chain`, green after.
    assert!(
        !field.turn_frontier.contains(&east_entrance),
        "the entrance's merge twin is offered as a turn candidate at a higher gas load"
    );
    assert!(!field.turn_frontier.contains(&entrance));
}

#[test]
fn gas_reach_field_an_off_route_stage_drop_override_keeps_the_strategy_default() {
    // A stage drop is a position ALONG the route, and the UI only ever writes route stations (the
    // drag snaps, the nudge is index-clamped, the menu entry hides off-route). But `.plnx` PERSISTS
    // overrides, so a plan reloaded after the survey, the endpoint or the way points moved can carry
    // a pin the new route no longer passes - no hand-edited file required.
    //
    // Scoring such a pin from the station's own shortest-path gas prices a passage the plan never
    // enters and folds the difference into the penetration budget, which EXTENDS the reach: the
    // unsafe direction. An off-route pin has no route gas, so the only honest answer is the
    // strategy default.
    let (project, entrance, north, _east, north_tip, east_mid) = two_branch_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        13.0,
        232.0,
        200.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        11.1,
        207.0,
        207.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let rig = [bg, stage];
    let plan_north = |overrides: &[Option<StationRef>]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&entrance),
            Some(&north_tip),
            &[],
            &[],
            &diver,
            &rig,
            &cfg,
            overrides,
        )
    };

    let baseline = plan_north(&[]);
    // SETUP - the route runs down the north passage, so an east station is genuinely off it, and
    // the rig really does have a stage slot to override.
    assert_eq!(baseline.stage_drop_refs.len(), 1);
    let route = route_stations(&baseline);
    assert!(route.contains(&north_tip));
    assert!(!route.contains(&east_mid));
    // ...and it is not off-GRAPH: it resolves fine, which is what let it reach the scoring code.
    assert!(baseline.station_refs.contains(&east_mid));
    // Non-vacuity: the east station's own gas-to-reach is a real, different number, so the old
    // `gas_in` basis had something to score and would not have coincided with the default.
    let east_gas = field_gas(&baseline, east_mid);
    assert!(east_gas > 0.0);

    let pinned_off_route = plan_north(&[Some(east_mid)]);
    assert!(
        (pinned_off_route.budget_litres - baseline.budget_litres).abs() < 1e-6,
        "an off-route pin moved the penetration budget by {} L",
        pinned_off_route.budget_litres - baseline.budget_litres
    );
    assert_eq!(pinned_off_route.stage_drop_refs, baseline.stage_drop_refs);
    assert_eq!(pinned_off_route.stage_drop_bars, baseline.stage_drop_bars);
    assert!(!pinned_off_route.stage_drop_over_reserve[0]);

    // The guard must not have been bought by disabling overrides altogether: an ON-route pin, one
    // station deeper than the default drop, still moves the budget.
    let default_drop = baseline.stage_drop_refs[0];
    let deeper = route
        .iter()
        .skip_while(|r| **r != default_drop)
        .find(|r| **r != default_drop && r.section_id == north)
        .copied();
    if let Some(deeper) = deeper {
        let pinned_on_route = plan_north(&[Some(deeper)]);
        assert!(
            (pinned_on_route.budget_litres - baseline.budget_litres).abs() > 1e-6,
            "an on-route pin at {deeper:?} left the budget unchanged - the override path is dead"
        );
    }
}

#[test]
fn gas_reach_field_the_way_point_itself_is_not_a_turn_candidate() {
    // The other half of the same rule, on the far end of the chain. Park the way point at the limit
    // of the gas and the station one shot beyond it falls out of budget, so the way point borders
    // unreachable ground and the frontier rule wants to mark it - putting a red "turn here" dot on
    // top of the diver's own green way point marker. It is already a committed route point; offering
    // it as a turnaround says nothing and reads as a competing suggestion.
    let (project, entrance, north, _east, _north_tip, _east_mid) = two_branch_project();
    let waypoint = StationRef::new(north, 2);
    let beyond = StationRef::new(north, 3);
    let diver = Diver::new(20.0, None).unwrap();
    // ~115 bar in the 13 L steel lands the All-thirds budget between the way point (400 L) and the
    // station past it (600 L), which is the whole point of the fixture.
    let bg = Cylinder::new(
        13.0,
        232.0,
        115.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[waypoint],
        &[],
        &diver,
        std::slice::from_ref(&bg),
        &GasConfig::default(),
        &[],
    );

    // SETUP - the way point is the end of the affordable route and the ground past it is not.
    assert!(
        field.budget_litres > 400.0 && field.budget_litres < 600.0,
        "budget {} no longer straddles the way point",
        field.budget_litres
    );
    assert_eq!(field.final_turn_ref, Some(waypoint));
    assert!(!field_reachable(&field, beyond));

    // THE PROPERTY. Before the fix this was a frontier station.
    assert!(
        !field.turn_frontier.contains(&waypoint),
        "the way point is offered as a turn candidate on top of itself"
    );
}

// ---- #299: the anticipated dive profile ----------------------------------------------

/// The outbound half of the profile, in route order.
fn outbound(field: &GasReachField) -> Vec<&super::RouteProfilePoint> {
    field.route_profile.iter().filter(|p| p.outbound).collect()
}

/// The return half, in the order the diver swims it (turnaround first, origin last).
fn inbound(field: &GasReachField) -> Vec<&super::RouteProfilePoint> {
    field.route_profile.iter().filter(|p| !p.outbound).collect()
}

fn plain_field(project: &Project, origin: StationRef, cfg: &GasConfig) -> GasReachField {
    let diver = Diver::new(20.0, None).unwrap();
    project.compute_gas_reach_field(Some(&origin), None, &diver, &[unlimited_backgas()], cfg)
}

#[test]
fn route_profile_outbound_reports_the_same_numbers_the_readout_does() {
    // The whole point of building the profile inside the compute: it can only ever re-present
    // figures the field has already committed to. If these ever disagree, the chart and the Plan
    // readout are telling the diver two different stories about the same dive.
    let (project, origin) = linear_descent_project();
    let field = plain_field(&project, origin, &GasConfig::default());

    let out = outbound(&field);
    assert_eq!(out.len(), route_stations(&field).len());
    assert_eq!(
        out.iter().map(|p| p.station_ref).collect::<Vec<_>>(),
        route_stations(&field)
    );
    // Distance: the last outbound point IS the one-way swum penetration.
    assert!((out.last().unwrap().distance_m - field.route_line_length_m).abs() < 1e-9);
    assert!(out[0].distance_m.abs() < 1e-9);
    // Depth: the deepest profile point is the reported max depth.
    let deepest = out
        .iter()
        .map(|p| p.depth_m)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!((deepest - field.route_max_depth_m.unwrap()).abs() < 1e-9);
    // Gas: the turnaround's cumulative gas is the gas-to-reach the overlay paints there.
    let turn = field.final_turn_ref.unwrap();
    assert!((out.last().unwrap().gas_l - field_gas(&field, turn)).abs() < 1e-6);
    // Monotonic in every accumulating axis.
    for w in out.windows(2) {
        assert!(w[1].distance_m >= w[0].distance_m);
        assert!(w[1].elapsed_min >= w[0].elapsed_min);
        assert!(w[1].gas_l >= w[0].gas_l);
        assert_eq!(w[1].route_index, w[0].route_index + 1);
    }
}

#[test]
fn route_profile_runtime_is_the_runtime_the_gas_was_priced_with() {
    // Time is not a second, independently-derived axis - it is the runtime the engine already
    // divided by to get the gas beside it.
    //
    // ⚠ Pinned to hand-computed constants, NOT to `distance_m / 8.0`. Re-deriving the expected
    // value with production's own expression proves only that the expression evaluates the same way
    // twice; it cannot fail for the reason this test is named after. The gas column below is worked
    // out here from the physics independently (rate = rmv x (1 + avg_depth / 10), gas = rate x
    // time), so runtime and gas can no longer drift apart without one of them breaking.
    let (project, origin) = linear_descent_project();
    let cfg = GasConfig {
        swim_speed_m_per_min: 20.0,
        survey_speed_m_per_min: Some(8.0),
        ..GasConfig::default()
    };
    let field = plain_field(&project, origin, &cfg);

    // 3 legs of 100 m at 8 m/min = 12.5 min each, descending 0 -> 10 -> 20 -> 30 m, on a 20 L/min
    // diver with the default 1.0 survey RMV multiplier and an unlimited back-gas budget.
    // Leg gas: 20 x 1.5 x 12.5 = 375, then 20 x 2.5 x 12.5 = 625, then 20 x 3.5 x 12.5 = 875.
    let expected = [
        // distance_m, depth_m, elapsed_min, cumulative gas_l
        (0.0, 0.0, 0.0, 0.0),
        (100.0, 10.0, 12.5, 375.0),
        (200.0, 20.0, 25.0, 1000.0),
        (300.0, 30.0, 37.5, 1875.0),
    ];
    let out = outbound(&field);
    assert_eq!(out.len(), expected.len());
    for (p, (distance, depth, elapsed, gas)) in out.iter().zip(expected) {
        assert!((p.distance_m - distance).abs() < 1e-9, "distance at {p:?}");
        assert!((p.depth_m - depth).abs() < 1e-9, "depth at {p:?}");
        assert!((p.elapsed_min - elapsed).abs() < 1e9, "runtime at {p:?}");
        assert!((p.gas_l - gas).abs() < 1e-6, "gas at {p:?}");
    }
}

#[test]
fn route_profile_is_empty_when_the_whole_route_is_a_zero_distance_tie() {
    // Two stations is not the same as a dive. A merge / loop-closure tie holds one physical point as
    // two graph nodes, so an endpoint set on the origin's OWN TWIN gives a route of two stations
    // that spans no swum distance whatsoever: a three-point round trip at zero metres, zero minutes
    // and zero gas, which the chart would draw and the export would write as a real dive.
    //
    // This is why the gate is a positive swum distance rather than a station count - counting
    // stations passes here, and `branching_project`'s entrance is exactly such a merged pair.
    let (project, entrance, side_tip, _trunk_tip) = branching_project();
    let twin = StationRef::new(side_tip.section_id, 0);
    let diver = Diver::new(20.0, None).unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&twin),
        &[],
        &[],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );

    // The state really is reached: a target, two stations, and not one swum metre between them.
    assert_eq!(field.final_turn_ref, Some(twin));
    assert!(field.route_line_length_m.abs() < 1e-9);
    assert!(
        field.route_profile.is_empty(),
        "a zero-distance route published {} profile point(s)",
        field.route_profile.len()
    );
}

#[test]
fn route_profile_is_empty_when_the_route_has_no_leg_to_swim() {
    // A turnaround is necessary but not sufficient. With nothing to breathe the budget is zero,
    // which the ORIGIN satisfies for free (reaching it costs nothing), so the engine picks the
    // origin as its own turnaround and the route is one station long. Published, that one point
    // renders as a dive of zero distance over zero duration - and the exporter would happily write
    // a valid Subsurface file for it.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let field =
        project.compute_gas_reach_field(Some(&origin), None, &diver, &[], &GasConfig::default());

    // The degenerate state really is reached - otherwise this test would pass for the wrong reason.
    // A turnaround IS published (which is what `route_profile` used to gate on, and why it emitted a
    // one-point series), while `route` - a list of LEGS, `windows(2)` over the stations - is empty
    // because one station spans none. That is also why `GasPlanPanel`'s `route.length > 0` readout
    // guard was already correct for this case and needed no change.
    assert!(field.budget_litres.abs() < 1e-9);
    assert_eq!(field.final_turn_ref, Some(origin));
    assert!(field.route.is_empty());
    // ...and the profile refuses it anyway.
    assert!(
        field.route_profile.is_empty(),
        "a legless route published {} profile point(s)",
        field.route_profile.len()
    );
}

#[test]
fn route_profile_return_leg_mirrors_distance_but_is_faster_and_cheaper() {
    // The finding this whole field exists to express: a round trip is a mirror in DISTANCE only.
    // The engine prices the way in at the survey speed and survey RMV; the way out is swum at the
    // (never slower) exit speed on the diver's base RMV, so the identical metres cost less time and
    // less gas going home.
    let (project, origin) = linear_descent_project();
    let cfg = GasConfig {
        swim_speed_m_per_min: 20.0,
        survey_speed_m_per_min: Some(5.0),
        survey_rmv_multiplier: 1.4,
        ..GasConfig::default()
    };
    let field = plain_field(&project, origin, &cfg);

    let out = outbound(&field);
    let back = inbound(&field);
    // The turnaround is swum once, the origin twice: 2n - 1 points, n - 1 of them inbound.
    assert_eq!(back.len(), out.len() - 1);
    assert_eq!(field.route_profile.len(), out.len() * 2 - 1);

    let total = out.last().unwrap().distance_m;
    for (k, p) in back.iter().enumerate() {
        let twin = out[out.len() - 2 - k];
        assert_eq!(p.station_ref, twin.station_ref);
        assert_eq!(p.route_index, twin.route_index);
        assert!(!p.outbound);
        // Distance mirrors exactly about the turnaround.
        assert!((p.distance_m - (2.0 * total - twin.distance_m)).abs() < 1e-9);
        assert!((p.depth_m - twin.depth_m).abs() < 1e-9);
    }
    // Home is the far end of a 2L axis, and the whole return took less time than the way in.
    let home = back.last().unwrap();
    assert!((home.distance_m - 2.0 * total).abs() < 1e-9);
    let out_min = out.last().unwrap().elapsed_min;
    let back_min = home.elapsed_min - out_min;
    assert!(
        back_min < out_min - 1e-9,
        "exit at 20 m/min must beat the 5 m/min survey-in: {back_min} vs {out_min}"
    );
    assert!((back_min - total / 20.0).abs() < 1e-9);
    // Same for gas: identical depths, but a cheaper rate over a shorter time.
    let out_gas = out.last().unwrap().gas_l;
    assert!(home.gas_l - out_gas < out_gas - 1e-9);
    // And the round trip legitimately finishes ABOVE the one-way penetration budget - the reserve
    // fraction is what pays for the exit, so a UI must not compare the return against it.
    assert!(out_gas <= field.budget_litres);
}

#[test]
fn route_profile_symmetric_speeds_make_the_return_a_time_mirror_too() {
    // No survey speed set = symmetric (`in_speed` falls back to the swim speed), and with the
    // default 1.0 survey multiplier the RMVs match as well - so here, and only here, the return
    // really is a mirror in all three axes. Guards the fallback branch of `in_speed`.
    let (project, origin) = linear_descent_project();
    let cfg = GasConfig {
        swim_speed_m_per_min: 12.0,
        survey_speed_m_per_min: None,
        ..GasConfig::default()
    };
    let field = plain_field(&project, origin, &cfg);

    let out = outbound(&field);
    let back = inbound(&field);
    let turn_min = out.last().unwrap().elapsed_min;
    let turn_gas = out.last().unwrap().gas_l;
    for (k, p) in back.iter().enumerate() {
        let twin = out[out.len() - 2 - k];
        assert!((p.elapsed_min - (2.0 * turn_min - twin.elapsed_min)).abs() < 1e-9);
        assert!((p.gas_l - (2.0 * turn_gas - twin.gas_l)).abs() < 1e-6);
    }
}

#[test]
fn route_profile_stage_drop_indices_agree_with_the_refs_and_the_distances() {
    // The drop markers on the chart are drawn at these indices, so they have to resolve to exactly
    // the station and distance the existing per-stage readouts report.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let bg = Cylinder::new(
        25.0,
        232.0,
        80.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let stage = Cylinder::new(
        10.0,
        207.0,
        90.0,
        TankMaterial::Aluminium,
        CylRole::Stage,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(Some(&origin), None, &diver, &[bg, stage], &cfg);

    assert!(!field.stage_drop_refs.is_empty());
    assert_eq!(
        field.stage_drop_route_indices.len(),
        field.stage_drop_refs.len()
    );
    let out = outbound(&field);
    for (i, &idx) in field.stage_drop_route_indices.iter().enumerate() {
        let point = out
            .iter()
            .find(|p| p.route_index == idx)
            .expect("a stage drop indexed a station that is not on the profile");
        assert_eq!(point.station_ref, field.stage_drop_refs[i]);
        assert!((point.distance_m - field.stage_drop_distances_m[i]).abs() < 1e-9);
    }
}

#[test]
fn route_profile_indexes_each_pass_of_a_route_that_doubles_back() {
    // The reason the profile is keyed on a route INDEX and not a station ref: a way-point route
    // revisits stations, so a ref alone cannot say which pass a marker belongs on. The way-point
    // index must land on the pass the diver actually tags it.
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[side_tip],
        &[],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );

    let out = outbound(&field);
    // The detour doubles back, so at least one station appears on the outbound leg twice - the case
    // `stage_drop_refs` / a ref-keyed marker cannot express.
    let repeated = out.iter().any(|p| {
        out.iter()
            .filter(|q| q.station_ref == p.station_ref)
            .count()
            > 1
    });
    assert!(repeated, "the branching detour should revisit a station");
    // Indices stay unique and strictly increasing even where the refs repeat.
    for w in out.windows(2) {
        assert_eq!(w[1].route_index, w[0].route_index + 1);
    }
    assert_eq!(field.waypoint_route_indices.len(), 1);
    let idx = field.waypoint_route_indices[0].unwrap();
    let at = out.iter().find(|p| p.route_index == idx).unwrap();
    assert_eq!(at.station_ref, side_tip);
    // ...and it is the FIRST time the route reaches it, not the pass back through.
    assert_eq!(
        out.iter().position(|p| p.station_ref == side_tip),
        Some(idx as usize)
    );
    // The way point's profile gas is the cumulative gas the readout reports for it.
    assert!((at.gas_l - field.waypoint_gas_litres[0].unwrap()).abs() < 1e-6);
}

#[test]
fn route_profile_charges_a_zero_distance_tie_no_distance_time_or_gas() {
    // A merge / loop-closure tie holds one physical point as two graph nodes. The route lists both,
    // but the gap between them was never swum: counting it would inflate the axis and, worse, bill
    // the diver for gas on the way home that they never breathe. `branching_project`'s entrance is
    // a merged pair, so its route starts across exactly such a tie.
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[side_tip],
        &[],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );

    let out = outbound(&field);
    let ties: Vec<_> = out
        .windows(2)
        .filter(|w| !is_swim_leg(&w[0].station_ref, &w[1].station_ref))
        .collect();
    assert!(
        !ties.is_empty(),
        "the merged entrance should produce a tie leg"
    );
    for w in &ties {
        assert!((w[1].distance_m - w[0].distance_m).abs() < 1e-9);
        assert!((w[1].elapsed_min - w[0].elapsed_min).abs() < 1e-9);
        // "or gas" is in this test's name, so assert it on the way IN too rather than only on the
        // way home: the two directions are priced by different code.
        assert!((w[1].gas_l - w[0].gas_l).abs() < 1e-9);
    }
    // Same on the way home - the tie must not cost exit gas either. Counted, not just asserted
    // inside an `if`: a version of this loop that never enters its tie branch would pass while
    // proving nothing about the return-leg pricing, which is the code path under test.
    let back = inbound(&field);
    let mut back_ties = 0;
    for w in back.windows(2) {
        if !is_swim_leg(&w[0].station_ref, &w[1].station_ref) {
            back_ties += 1;
            assert!((w[1].gas_l - w[0].gas_l).abs() < 1e-9);
            assert!((w[1].elapsed_min - w[0].elapsed_min).abs() < 1e-9);
        }
    }
    assert_eq!(
        back_ties,
        ties.len(),
        "the return leg must re-swim exactly the ties the way in did"
    );
}

#[test]
fn route_profile_is_empty_when_no_route_exists() {
    // A stalled way-point chain makes the whole field unreachable, so there is nothing to plot -
    // and an empty series is what the panel must be handed rather than a one-point stub.
    let (project, origin) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let orphan = StationRef::new(origin.section_id + 999, 0);
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[orphan],
        &[],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );

    assert!(field.final_turn_ref.is_none());
    assert!(field.route_profile.is_empty());
    assert!(field.stage_drop_route_indices.is_empty());
    assert_eq!(field.waypoint_route_indices, vec![None]);
}

#[test]
fn route_profile_is_empty_when_the_chain_has_no_affordable_turnaround() {
    // The committed way-point chain is deliberately KEPT for the map when the detour alone blows the
    // budget (see `gas_reach_field_waypoint_over_budget_still_shows_the_committed_chain`) - the diver
    // needs to see which way point cost them the dive. A dive PROFILE is a different claim: it says
    // "out to the turnaround and back", and there is no turnaround here. Publishing one anyway hands
    // the export a complete round-trip dive - with a `Turn:` line naming a station the diver cannot
    // afford to reach - for a plan the engine has just declared unflyable.
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    let tiny = Cylinder::new(
        10.0,
        207.0,
        20.0,
        TankMaterial::Aluminium,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[side_tip],
        &[],
        &diver,
        &[tiny],
        &GasConfig::default(),
        &[],
    );

    // SETUP - the state this guards: a drawn route, but no turnaround.
    assert!(field.final_turn_ref.is_none());
    assert_eq!(route_stations(&field).last(), Some(&side_tip));

    // THE PROPERTY.
    assert!(
        field.route_profile.is_empty(),
        "published a {}-point profile for a plan with no turnaround",
        field.route_profile.len()
    );
    assert!(field.stage_drop_route_indices.is_empty());
}

#[test]
fn route_profile_exit_leg_charges_a_jump_the_jump_rmv_bump() {
    // The exit leg re-prices every virtual (jump/reference) leg at the higher RMV, exactly as the
    // in-leg graph does - that parity is the whole reason `route_leg_kinds` exists as a shared scan
    // rather than being derived from `RouteLeg::is_jump`. Nothing else covers it: both other profile
    // fixtures use only default-kind shots, so dropping the multiplier from the return leg would
    // not fail a single assertion.
    let (project, origin) = project_with_jump_second_leg();
    let cfg = GasConfig {
        swim_speed_m_per_min: 10.0,
        survey_speed_m_per_min: None,
        jump_rmv_multiplier: 3.0,
        ..GasConfig::default()
    };
    let field = plain_field(&project, origin, &cfg);

    let out = outbound(&field);
    let back = inbound(&field);
    assert_eq!(out.len(), 3, "origin, jump start, jump end");

    // Leg 0 (stations 0→1) is a Normal shot; leg 1 (1→2) is the Jump. On the way home they are
    // re-swum in reverse, so the FIRST return step is the jump and the second is the normal leg.
    let jump_back = back[0].gas_l - out[2].gas_l;
    let normal_back = back[1].gas_l - back[0].gas_l;

    // Same length, same depths on the way back? No - the two legs sit at different depths, so
    // compare each against its own hand-computed edge_gas rather than against each other.
    let expect_jump = edge_gas(20.0 * 3.0, 10.0, 100.0, out[2].depth_m, out[1].depth_m);
    let expect_normal = edge_gas(20.0, 10.0, 100.0, out[1].depth_m, out[0].depth_m);
    assert!(
        (jump_back - expect_jump).abs() < 1e-6,
        "jump leg not charged the {}x bump on the way out: {jump_back} vs {expect_jump}",
        cfg.jump_rmv_multiplier
    );
    assert!(
        (normal_back - expect_normal).abs() < 1e-6,
        "normal leg mis-priced on the way out: {normal_back} vs {expect_normal}"
    );
    // And the bump is real, not a coincidence of equal depths.
    assert!(jump_back > normal_back);
}
// ---- #317: plan segments and the DPV range cap -----------------------------------------

/// A rig whose weaker unit covers `range_m` of line, with no conservatism held back. Built from a
/// speed and a burn so the tests exercise the real `burn x speed` derivation rather than smuggling
/// a range in directly.
fn rig_covering(range_m: f64) -> DpvRig {
    DpvRig {
        // 55 m/min is a typical cruising speed; the burn is whatever covers the wanted line.
        primary: DpvUnit {
            speed_m_per_min: 55.0,
            burn_minutes: range_m / 55.0,
        },
        // Deliberately the STRONGER unit, so every one of these tests is also asserting that the
        // weaker one is the one that counts.
        backup: DpvUnit {
            speed_m_per_min: 50.0,
            burn_minutes: (range_m * 2.0) / 50.0,
        },
        range_reserve_fraction: 0.0,
    }
}

fn seg(speed_m_per_min: f64, on_scooter: bool) -> PlanSegment {
    PlanSegment {
        speed_m_per_min: Some(speed_m_per_min),
        on_scooter,
    }
}

/// A span the diver has not given a speed of its own - it flies at the plan's speed. This is what
/// the frontend sends for every unedited span, so it is the shape most engine tests should use.
fn inherit_seg(on_scooter: bool) -> PlanSegment {
    PlanSegment {
        speed_m_per_min: None,
        on_scooter,
    }
}

#[test]
fn the_plan_speed_still_reaches_the_engine_through_a_full_length_segment_list() {
    // ⚠ THE #317 REGRESSION GUARD. Segments shipped with `speed_m_per_min: f64`, and the panel
    // sends one entry per span from the moment Plan mode opens - so every span always carried an
    // explicit number and the diver's configured survey / swim speed stopped reaching the engine
    // entirely. Settings > Gas and the panel's own Swim-speed field went dead while still looking
    // live, because changing them nulls `reachField` and the refetch comes back IDENTICAL.
    //
    // Every engine test at the time passed `&[]`, which selects the "no segments" branch that
    // production never sends - so the whole suite stayed green over a disabled feature. This test
    // exists to be the one that sends what the frontend actually sends.
    //
    // survey_speed 5 against the swim default 10 doubles the in-leg gas: station 1 costs 300 L at
    // 10 m/min and 600 L at 5. Any regression that ignores the config speed reads 300.
    let (project, origin) = linear_descent_project();
    let sec = origin.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let big = Cylinder::new(
        1000.0,
        300.0,
        300.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let config = GasConfig {
        survey_speed_m_per_min: Some(5.0),
        ..GasConfig::default()
    };

    // What the frontend sends for a plan with one way point and nothing edited: TWO spans, both
    // inheriting. (The way point itself is what makes the list longer than one entry.)
    let waypoint = StationRef::new(sec, 1);
    let sent = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[waypoint],
        &[inherit_seg(false), inherit_seg(false)],
        &diver,
        &[big.clone()],
        &config,
        &[],
    );
    assert!(
        (field_gas(&sent, StationRef::new(sec, 1)) - 600.0).abs() < 1e-6,
        "the configured survey speed did not reach the engine: station 1 cost {} L, expected 600",
        field_gas(&sent, StationRef::new(sec, 1))
    );

    // And an all-inheriting list is byte-for-byte the empty-list field, which is what makes every
    // `&[]` test in this file meaningful for the shape production sends.
    let empty = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[waypoint],
        &[],
        &diver,
        &[big],
        &config,
        &[],
    );
    assert_eq!(sent.gas_to_reach, empty.gas_to_reach);
    assert_eq!(sent.reachable, empty.reachable);
    assert_eq!(sent.route_runtime_min, empty.route_runtime_min);
}

/// A trunk of 3 shots (100 m each, descending 0 -> 30 m) with a 1-shot side branch whose station 0
/// is MERGED onto the trunk's station 1 - so the branch hangs off the middle of the trunk rather
/// than off its entrance. Returns `(project, trunk_entrance, trunk_mid, trunk_tip, branch_root,
/// branch_tip)`, where `branch_root` is the merged twin of `trunk_mid`.
fn merged_mid_branch_project() -> (
    Project,
    StationRef,
    StationRef,
    StationRef,
    StationRef,
    StationRef,
) {
    let mut project = Project::new("Gas".to_string(), None);
    let trunk = project
        .add_section(SectionData {
            shots: vec![
                make_shot(100.0, 0.0, 10.0),
                make_shot(100.0, 10.0, 20.0),
                make_shot(100.0, 20.0, 30.0),
            ],
            ..Default::default()
        })
        .id;
    let branch = project
        .add_section(SectionData {
            shots: vec![make_shot(100.0, 10.0, 20.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(trunk, 1), StationRef::new(branch, 0))
        .unwrap();
    (
        project,
        StationRef::new(trunk, 0),
        StationRef::new(trunk, 1),
        StationRef::new(trunk, 3),
        StationRef::new(branch, 0),
        StationRef::new(branch, 1),
    )
}

#[test]
fn gas_reach_field_prices_each_segment_at_its_own_speed() {
    // THE headline arithmetic. A way point at the trunk's first station splits the route into two
    // spans; the first is flown at 50 m/min and the second at the default 10, so the same passage
    // costs five times less gas on the fast span.
    //
    // Hand-computed at rmv 20 (gas = rmv x ata(avg depth) x length / speed):
    //   span 0, entrance -> trunk 1 @ 50:  20 x 1.5 x 100 / 50 =  60 L
    //   span 1, trunk 1 -> trunk 2 @ 10:   20 x 2.5 x 100 / 10 = 500 L  (cumulative  560)
    //   span 1, trunk 2 -> trunk 3 @ 10:   20 x 3.5 x 100 / 10 = 700 L  (cumulative 1260)
    let (project, entrance, _side, trunk_tip) = branching_project();
    let mid = StationRef::new(trunk_tip.section_id, 1);
    let two_thirds = StationRef::new(trunk_tip.section_id, 2);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig::default();

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[mid],
        &[seg(50.0, false), seg(10.0, false)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert!((field_gas(&field, mid) - 60.0).abs() < 1e-6);
    assert!((field_gas(&field, two_thirds) - 560.0).abs() < 1e-6);
    assert!((field_gas(&field, trunk_tip) - 1260.0).abs() < 1e-6);

    // NON-VACUOUS: the same plan with no segment list must produce the pre-#317 numbers, so the
    // assertions above can only pass because the fast span was actually applied.
    let flat = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[mid],
        &[],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert!((field_gas(&flat, mid) - 300.0).abs() < 1e-6);
    assert!((field_gas(&flat, trunk_tip) - 1500.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_scooter_range_binds_before_gas() {
    // Unlimited gas, a rig that covers only 220 m of line, and the whole dive on the scooter. The
    // trunk's stations sit at 100 / 200 / 300 m of line, so the range - not the budget - is what
    // turns the dive, and it turns at 200 m.
    let (project, entrance, _side, trunk_tip) = branching_project();
    let mid = StationRef::new(trunk_tip.section_id, 1);
    let two_thirds = StationRef::new(trunk_tip.section_id, 2);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        dpv: Some(rig_covering(220.0)),
        ..GasConfig::default()
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[],
        &[seg(55.0, true)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert!(field_reachable(&field, mid));
    assert!(field_reachable(&field, two_thirds));
    assert!(
        !field_reachable(&field, trunk_tip),
        "300 m of line is past the 220 m the rig covers"
    );
    assert_eq!(field.final_turn_ref, Some(two_thirds));
    assert!(
        field.turn_on_scooter_range,
        "the scooter, not the gas, is what stopped this dive"
    );
    assert!((field.scooter_distance_m - 200.0).abs() < 1e-6);
    assert_eq!(field.scooter_range_limit_m, Some(220.0));

    // NON-VACUOUS: drop the rig and the identical plan reaches the tip. Whatever else could make
    // `trunk_tip` unreachable above, it was not the gas.
    let no_rig = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[],
        &[seg(55.0, true)],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );
    assert!(field_reachable(&no_rig, trunk_tip));
    assert!(!no_rig.turn_on_scooter_range);
}

#[test]
fn gas_reach_field_gas_still_binds_when_the_scooter_range_is_ample() {
    // The mirror of the test above, and the reason `turn_on_scooter_range` exists at all: the same
    // frontier for a different reason must report a different flag. A rig covering 1100 m never
    // binds over a 300 m trunk, so the small back-gas tank is what turns the dive.
    //
    // At 55 m/min the trunk costs 54.5 / 145.5 / 272.7 L cumulative, and a 3 L tank at 200 bar
    // yields an All-thirds budget near 192 L - between the last two.
    let (project, entrance, _side, trunk_tip) = branching_project();
    let two_thirds = StationRef::new(trunk_tip.section_id, 2);
    let diver = Diver::new(20.0, None).unwrap();
    let small = Cylinder::new(
        3.0,
        200.0,
        200.0,
        TankMaterial::Steel,
        CylRole::BackGas,
        None,
    )
    .unwrap();
    let cfg = GasConfig {
        dpv: Some(rig_covering(1100.0)),
        ..GasConfig::default()
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[],
        &[seg(55.0, true)],
        &diver,
        &[small],
        &cfg,
        &[],
    );
    assert!(field_reachable(&field, two_thirds));
    assert!(!field_reachable(&field, trunk_tip));
    assert!(
        !field.turn_on_scooter_range,
        "the rig covers 1100 m and the route is 200 m; gas is what turned this dive"
    );
}

#[test]
fn gas_reach_field_a_swim_segment_spends_no_scooter_range() {
    // The tow case: scooter, swim a restriction, scooter again. The swum span costs gas at swim
    // rates but no range at all, so the 300 m route spends only 200 m of the rig.
    let (project, entrance, _side, trunk_tip) = branching_project();
    let mid = StationRef::new(trunk_tip.section_id, 1);
    let two_thirds = StationRef::new(trunk_tip.section_id, 2);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        dpv: Some(rig_covering(550.0)),
        ..GasConfig::default()
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[mid, two_thirds],
        &[seg(55.0, true), seg(10.0, false), seg(55.0, true)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert_eq!(field.final_turn_ref, Some(trunk_tip));
    assert!(
        (field.scooter_distance_m - 200.0).abs() < 1e-6,
        "the swum middle span must not spend range; got {}",
        field.scooter_distance_m
    );
    // The units are still in hand at the turnaround, so nothing was parked.
    assert_eq!(field.scooter_drop_ref, None);
}

#[test]
fn gas_reach_field_derives_the_scooter_drop_at_the_park_way_point() {
    // Scooter, scooter, then swim on: the units are left at the boundary after the last scootered
    // span, which is the second way point. Nothing places that marker - it is derived.
    let (project, entrance, _side, trunk_tip) = branching_project();
    let mid = StationRef::new(trunk_tip.section_id, 1);
    let two_thirds = StationRef::new(trunk_tip.section_id, 2);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        dpv: Some(rig_covering(550.0)),
        ..GasConfig::default()
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[mid, two_thirds],
        &[seg(55.0, true), seg(55.0, true), seg(10.0, false)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert_eq!(field.scooter_drop_ref, Some(two_thirds));
    assert!((field.scooter_distance_m - 200.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_scooter_range_settles_across_a_station_merge() {
    // ⚠ The zero-cost twin trap, in scooter currency. One physical station is several graph nodes
    // when a merge joins them, and the committed chain is a predecessor walk that names only the
    // node it happened to route through. Here the way point is the trunk tip, so the chain runs
    // along the trunk and `branch_root` - the merged twin of `trunk_mid`, carrying the side
    // passage - is never named by it.
    //
    // Without settling the scootered distance across the zero-weight merge edge, `branch_root`
    // keeps the after-chain price: 300 m out to the tip plus 200 m back = 500 m, over the 250 m
    // cap, so a station sitting exactly on top of a reachable one reads unreachable. Settled, it
    // takes its twin's pass-through 100 m.
    let (project, entrance, trunk_mid, trunk_tip, branch_root, _branch_tip) =
        merged_mid_branch_project();
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        dpv: Some(rig_covering(250.0)),
        ..GasConfig::default()
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[trunk_tip],
        &[seg(55.0, true), seg(55.0, true)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert!(
        field_reachable(&field, trunk_mid),
        "the chain station itself must be in range at its pass-through distance"
    );
    assert!(
        field_reachable(&field, branch_root),
        "the merged twin sits at the same place and must report the same scootered distance"
    );
}

#[test]
fn gas_reach_field_reports_the_route_runtime() {
    // The engine's first time-valued output. 300 m of trunk at 10 m/min is 30 minutes; the same
    // line on a scooter at 50 m/min is 6.
    let (project, entrance) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig::default();

    let swum = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[],
        &[seg(10.0, false)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert!((swum.route_runtime_min.unwrap() - 30.0).abs() < 1e-6);

    let scootered = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[],
        &[seg(50.0, false)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert!((scootered.route_runtime_min.unwrap() - 6.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_without_a_rig_ignores_the_scooter_flag() {
    // A plan can carry `on_scooter` spans with no rig configured - the diver removed the rig, or
    // the config never had one. The speed still applies (variable pace is useful either way) but
    // nothing is reported as scootered, because there is no unit to hold the plan to.
    let (project, entrance) = linear_descent_project();
    let diver = Diver::new(20.0, None).unwrap();

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[],
        &[seg(55.0, true)],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );
    assert!((field.scooter_distance_m - 0.0).abs() < f64::EPSILON);
    assert_eq!(field.scooter_range_limit_m, None);
    assert_eq!(field.scooter_drop_ref, None);
    assert!(!field.turn_on_scooter_range);
    // The speed was still honoured: 300 m at 55 m/min, not at the config's 10.
    assert!((field.route_runtime_min.unwrap() - 300.0 / 55.0).abs() < 1e-6);
}

#[test]
fn gas_reach_field_an_out_of_range_branch_elsewhere_does_not_claim_the_scooter_turned_the_dive() {
    // ⚠ THE FALSE POSITIVE. `turn_on_scooter_range` was a cave-wide scan - "is ANY station the
    // diver has the gas for out of scooter range" - which says yes on a plan that makes its
    // endpoint with range to spare, as long as anything at all beyond the rig's reach exists
    // somewhere. That is most caves: gas scales with depth while range scales with length alone, so
    // a shallow distant passage is affordable long after it is out of range.
    //
    // Here the dive is a 100 m hop to the endpoint on a rig covering 150 m. The side branch (200 m
    // of line) and the rest of the trunk are affordable and out of range - and irrelevant, because
    // lifting the cap would not move this turnaround one metre.
    let (project, entrance, trunk_mid, _trunk_tip, _branch_root, branch_tip) =
        merged_mid_branch_project();
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        dpv: Some(rig_covering(150.0)),
        ..GasConfig::default()
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_mid),
        &[],
        &[seg(55.0, true)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    // The premise: the dive reaches the diver's own pick, and there IS an out-of-range,
    // gas-affordable station in another passage. Without both, the assertion below proves nothing.
    assert_eq!(field.final_turn_ref, Some(trunk_mid));
    assert!(field.turn_toward_endpoint);
    assert!(
        !field_reachable(&field, branch_tip),
        "the side branch must be out of range for this test to be about anything"
    );
    assert!(
        !field.turn_on_scooter_range,
        "the dive made its endpoint; the scooter did not turn it"
    );
}

#[test]
fn gas_reach_field_turns_on_scooter_range_when_the_next_station_toward_the_pick_is_out_of_range() {
    // The other half of the pair: the same rig, the same cave, an endpoint the range genuinely
    // cannot reach. The walk toward `trunk_tip` stops at the 200 m station, which the diver can
    // afford on gas and cannot get to on the rig - so lifting the cap WOULD move the turnaround,
    // and the flag says so.
    let (project, entrance, trunk_mid, trunk_tip, _branch_root, _branch_tip) =
        merged_mid_branch_project();
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        dpv: Some(rig_covering(150.0)),
        ..GasConfig::default()
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[],
        &[seg(55.0, true)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert_eq!(field.final_turn_ref, Some(trunk_mid));
    assert!(field.turn_toward_endpoint);
    assert!(
        field.turn_on_scooter_range,
        "the station just past the turn is affordable on gas and out of range"
    );

    // NON-VACUOUS: the identical plan on a rig that covers the whole trunk reaches the endpoint and
    // reports false, so the `true` above is not simply what this geometry always says.
    let ample = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[],
        &[seg(55.0, true)],
        &diver,
        &[unlimited_backgas()],
        &GasConfig {
            dpv: Some(rig_covering(1000.0)),
            ..GasConfig::default()
        },
        &[],
    );
    assert_eq!(ample.final_turn_ref, Some(trunk_tip));
    assert!(!ample.turn_on_scooter_range);
}

#[test]
fn gas_reach_field_a_stalled_plan_parks_no_scooters() {
    // The park point is derived from the segment list alone, which is pure input - so it survived a
    // stalled way-point chain, and the engine handed back "leave the units at W1" naming a station
    // that no longer exists, for a dive it had just declared unflyable. Gated on a target now, like
    // the stage drops and the profile.
    let (project, entrance, side_tip, _trunk_tip) = branching_project();
    let ghost = StationRef::new(entrance.section_id + 999, 0);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        dpv: Some(rig_covering(1000.0)),
        ..GasConfig::default()
    };
    // Scooter in, park, swim on: a two-span plan whose drop is way point 0.
    let park_then_swim = [seg(55.0, true), seg(10.0, false)];

    let stalled = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[ghost],
        &park_then_swim,
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert_eq!(
        stalled.waypoint_gas_litres,
        vec![None],
        "the plan must stall"
    );
    assert!(stalled.final_turn_ref.is_none());
    assert_eq!(stalled.scooter_drop_ref, None);

    // NON-VACUOUS: the same segments over a way point that DOES resolve still derive the drop, so
    // the gate above is the stall and not a blanket "never report a park point".
    let flyable = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[side_tip],
        &park_then_swim,
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );
    assert_eq!(flyable.scooter_drop_ref, Some(side_tip));
}

#[test]
fn gas_reach_field_a_denormal_segment_speed_inherits_instead_of_poisoning_the_field() {
    // A positive, finite subnormal passes a `> 0.0` filter and then overflows both of the engine's
    // reciprocals (`rmv / speed` and `1 / speed`) to INFINITY. Every real edge is then priced at
    // INFINITY and never relaxes, and every zero-weight merge tie at `0.0 * INFINITY = NaN`, which
    // fails the same comparison - so the field collapses to the origin with no error anywhere.
    // Bounded now, so the span inherits the plan speed like any other unusable number.
    let (project, entrance, side_tip, trunk_tip) = branching_project();
    let diver = Diver::new(20.0, None).unwrap();
    let denormal = PlanSegment {
        speed_m_per_min: Some(1e-320),
        on_scooter: false,
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[],
        &[denormal],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );
    let inherited = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        Some(&trunk_tip),
        &[],
        &[inherit_seg(false)],
        &diver,
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    );
    assert_eq!(field.gas_to_reach, inherited.gas_to_reach);
    assert!(
        field.gas_to_reach.iter().flatten().all(|g| g.is_finite()),
        "a reported gas-to-reach must never be NaN or infinite"
    );
    // The merge tie is what NaN kills first: the side branch hangs off the entrance by a
    // zero-weight edge and nothing else.
    assert!((field_gas(&field, side_tip) - 300.0).abs() < 1e-6);
    assert!(field.route_runtime_min.is_some_and(f64::is_finite));
}

#[test]
fn route_profile_runtime_follows_the_per_segment_speeds() {
    // ⚠ THE PROFILE'S CLOCK RAN AT ONE SPEED. `build_route_profile` timed every outbound point as
    // `cumulative distance / in_speed` while `route_runtime_min` - the panel's "One-way runtime",
    // out of the same compute - was already per-segment. So the chart and BOTH dive-profile exports
    // (the Subsurface duration and its samples, the plain-text mm:ss column) contradicted the number
    // on screen: a 55 m/min scootered span was written up as if it had been swum, in the document
    // the diver carries underwater and plans against later.
    //
    // Two spans over the branching trunk, whose stations sit at 0 / 100 / 200 / 300 m of line:
    //   span 0, entrance -> trunk 1 @ 55:            100 / 55 = 1.8181... min
    //   span 1, trunk 1 -> trunk 3 @ the plan's 10:  200 / 10 = 20 min   (cumulative 21.8181...)
    // The old single-speed form reports 300 / 10 = 30: the whole dive at the slow span's pace.
    let (project, entrance, _side, trunk_tip) = branching_project();
    let mid = StationRef::new(trunk_tip.section_id, 1);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        swim_speed_m_per_min: 20.0,
        survey_speed_m_per_min: Some(10.0),
        ..GasConfig::default()
    };

    let field = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[mid],
        &[seg(55.0, false), inherit_seg(false)],
        &diver,
        &[unlimited_backgas()],
        &cfg,
        &[],
    );

    let out = outbound(&field);
    let fast = 100.0 / 55.0;
    let expected = [
        (0.0, 0.0),
        (100.0, fast),
        (200.0, fast + 10.0),
        (300.0, fast + 20.0),
    ];
    assert_eq!(
        out.len(),
        expected.len(),
        "route was not the trunk: {out:?}"
    );
    for (p, (distance, elapsed)) in out.iter().zip(expected) {
        assert!((p.distance_m - distance).abs() < 1e-9, "distance at {p:?}");
        assert!((p.elapsed_min - elapsed).abs() < 1e-9, "runtime at {p:?}");
    }

    // THE PROPERTY: the chart's clock IS the readout beside it, not a second derivation of it.
    let turn_min = out.last().unwrap().elapsed_min;
    assert!(
        (turn_min - field.route_runtime_min.unwrap()).abs() < 1e-12,
        "the profile says {turn_min} min and the runtime readout says {:?}",
        field.route_runtime_min
    );
    // NON-VACUOUS: and it is emphatically not the single-speed figure, which the tolerances above
    // are far too tight to have admitted by accident.
    assert!(
        (turn_min - 30.0).abs() > 1.0,
        "the whole route was priced at the slow span's speed"
    );

    // The return half still leaves from the corrected total, at the one swum exit speed (20 m/min
    // over 300 m = 15 min): neither span is scootered, and a custom pace is an ENTRY property -
    // the way home is a plain swim. A RIDDEN span is the exception; see
    // `route_profile_scootered_span_rides_out_at_scooter_pace`.
    let home = inbound(&field).last().copied().unwrap();
    assert!((home.elapsed_min - (turn_min + 15.0)).abs() < 1e-9);
}

#[test]
fn route_profile_scootered_span_rides_out_at_scooter_pace() {
    // ⚠ THE EXIT WAS SWUM. The return half priced every leg at `swim_speed_m_per_min`, scootered
    // spans included - but there is no dead-scooter swim-out in this model: the mandatory backup
    // rides the diver home over the same line (`DpvRig`), and the range cap charges its allowance
    // one way for exactly that reason. Billing a 55 m/min span at the 20 m/min swim pace wrote
    // runtime and exit gas the plan does not spend into the export the diver carries.
    //
    // Trunk stations at 0 / 100 / 200 / 300 m of line, way point at 100 m:
    //   span 0 (entrance -> WP 1): scootered at 55
    //   span 1 (WP 1 -> endpoint): inherited survey pace, 10
    // Way out: 200 m swum home at 20, then the ridden 100 m at 55.
    let (project, entrance, _side, trunk_tip) = branching_project();
    let mid = StationRef::new(trunk_tip.section_id, 1);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        swim_speed_m_per_min: 20.0,
        survey_speed_m_per_min: Some(10.0),
        dpv: Some(rig_covering(1000.0)),
        ..GasConfig::default()
    };
    let plan = |ridden: bool| {
        project.compute_gas_reach_field_with_overrides(
            Some(&entrance),
            None,
            &[mid],
            &[seg(55.0, ridden), inherit_seg(false)],
            &diver,
            &[unlimited_backgas()],
            &cfg,
            &[],
        )
    };
    let field = plan(true);

    let out_ridden = outbound(&field);
    let turn = out_ridden.last().unwrap();
    assert!(
        (turn.distance_m - 300.0).abs() < 1e-9,
        "route was not the trunk: {out_ridden:?}"
    );
    let turn_min = turn.elapsed_min;
    let home = inbound(&field).last().copied().unwrap();
    let ridden_home = 200.0 / 20.0 + 100.0 / 55.0;
    assert!(
        (home.elapsed_min - (turn_min + ridden_home)).abs() < 1e-9,
        "the scootered span must ride home at 55: {} vs {}",
        home.elapsed_min,
        turn_min + ridden_home
    );
    // NON-VACUOUS: the all-swum exit is 300 / 20 = 15 min home, more than 3 minutes slower - far
    // outside the tolerance above.
    assert!((home.elapsed_min - (turn_min + 15.0)).abs() > 1.0);

    // The gas follows the clock. Same speeds ridden or swum, so the way IN is identical either
    // way - any round-trip difference is the exit pricing alone - and riding the span home means
    // less time at depth, hence less exit gas, than swimming the same line out.
    let swum = plan(false);
    let out_swum = outbound(&swum);
    assert_eq!(out_ridden.len(), out_swum.len());
    for (r, s) in out_ridden.iter().zip(&out_swum) {
        assert!(
            (r.gas_l - s.gas_l).abs() < 1e-9 && (r.elapsed_min - s.elapsed_min).abs() < 1e-9,
            "the way in must not depend on the propulsion flag: {r:?} vs {s:?}"
        );
    }
    let home_swum = inbound(&swum).last().copied().unwrap();
    assert!(
        home.gas_l < home_swum.gas_l - 1e-9,
        "riding out must cost less gas than swimming out: {} vs {}",
        home.gas_l,
        home_swum.gas_l
    );
}

#[test]
fn gas_reach_field_riding_below_cruise_turns_the_dive_earlier() {
    // ⚠ THE REACH-EXTENDING BUG. The range cap is `burn x cruise`, i.e. battery MINUTES expressed as
    // metres at cruise - but every scootered metre was charged a flat metre of it, whatever speed
    // the span was ridden at. A span flown at a third of cruise takes three times the minutes the
    // cap assumed for those metres, so the slower the diver rode, the FARTHER the overlay said they
    // could get. Reach that the battery cannot deliver, in the unsafe direction.
    //
    // One rig, one cap, one route; only the span speed differs. The trunk's stations sit at
    // 0 / 100 / 200 / 300 m, and the rig covers 450 m of line at its 45 m/min cruise:
    //   at cruise 45:  300 m of line spends 300 m of the allowance -> the tip is in reach.
    //   at 15 m/min:   each metre spends three, so 200 m of line spends 600 -> past the 450 m cap.
    // Before the fix BOTH reached the tip.
    let (project, entrance, _side, trunk_tip) = branching_project();
    let mid = StationRef::new(trunk_tip.section_id, 1);
    let two_thirds = StationRef::new(trunk_tip.section_id, 2);
    let diver = Diver::new(20.0, None).unwrap();
    // Both units cruise at 45, so the "slower cruise" is unambiguous; the weaker covers 450 m.
    let cfg = GasConfig {
        dpv: Some(DpvRig {
            primary: DpvUnit {
                speed_m_per_min: 45.0,
                burn_minutes: 10.0,
            },
            backup: DpvUnit {
                speed_m_per_min: 45.0,
                burn_minutes: 20.0,
            },
            range_reserve_fraction: 0.0,
        }),
        ..GasConfig::default()
    };
    let plan = |speed: f64| {
        project.compute_gas_reach_field_with_overrides(
            Some(&entrance),
            None,
            &[],
            &[seg(speed, true)],
            &diver,
            &[unlimited_backgas()],
            &cfg,
            &[],
        )
    };

    // The premise: at cruise the whole trunk is in reach, on gas the tank never binds.
    let cruise = plan(45.0);
    assert_eq!(cruise.scooter_range_limit_m, Some(450.0));
    assert!(field_reachable(&cruise, trunk_tip));
    assert_eq!(cruise.final_turn_ref, Some(trunk_tip));
    assert!(
        (cruise.scooter_distance_m - 300.0).abs() < 1e-6,
        "at cruise a metre of line must still cost exactly a metre of range: {}",
        cruise.scooter_distance_m
    );

    // The same 450 m rig, ridden at a third of cruise, turns two stations earlier.
    let slow = plan(15.0);
    assert!(
        field_reachable(&slow, mid),
        "100 m of line spends 300 of the 450 m allowance - that much is affordable"
    );
    assert!(
        !field_reachable(&slow, two_thirds),
        "200 m of line spends 600 m of a 450 m allowance"
    );
    assert!(!field_reachable(&slow, trunk_tip));
    assert!(
        slow.turn_on_scooter_range,
        "the battery, not the gas, is what turned this dive"
    );
    // And the readout says what was actually spent: 100 m of line, 300 m of the allowance.
    assert!(
        (slow.scooter_distance_m - 300.0).abs() < 1e-6,
        "spent range should be 3x the line ridden; got {}",
        slow.scooter_distance_m
    );
}
