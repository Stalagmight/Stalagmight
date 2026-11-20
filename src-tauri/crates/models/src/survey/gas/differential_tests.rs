//! Determinism of the station-graph gas engine on random legacy surveys
//! (`legacy_convert::random_legacy_survey`, converted with
//! `legacy_convert::convert`): station-level results do not depend on the
//! line or closure list order.
//!
//! Retired when `models::project` was deleted: the differential test against
//! the legacy slot engine (`compute_gas_reach_field_with_overrides`), which
//! is not in the frozen 0.6.0 crate (it carries only what the open needs). The
//! engine keeps its ported golden tests (`super::tests`), whose expectations
//! were set by the legacy engine.
//!
//! Fully synthetic data only.

use crate::config::{DpvRig, DpvUnit, GasConfig, ReservePreset, ReserveProfile};
use crate::gas::{CylRole, Cylinder, Diver, PlanSegment, TankMaterial};
use crate::StationId;

use super::super::legacy_convert::{convert, random_legacy_survey, Lcg, Mix};
use super::super::Project;

fn cyl(water: f64, rated: f64, fill: f64, role: CylRole) -> Cylinder {
    Cylinder::new(water, rated, fill, TankMaterial::Steel, role, None).unwrap()
}

/// A rig: cylinders plus the gas config, under every reserve preset.
struct Rig {
    name: &'static str,
    cylinders: Vec<Cylinder>,
    config: GasConfig,
    /// Whether the plan's segments fly on the scooter (`config.dpv` is set).
    scooter: bool,
}

fn rigs() -> Vec<Rig> {
    let staged = |preset: ReservePreset| GasConfig {
        default_reserve: preset,
        reserve_profile: ReserveProfile {
            stage_reserve_bar: 40.0,
            ..ReserveProfile::default()
        },
        ..GasConfig::default()
    };
    let mut disabled_stage = cyl(5.7, 207.0, 150.0, CylRole::Stage);
    disabled_stage.enabled = false;
    let mut free_bg = cyl(7.0, 232.0, 200.0, CylRole::BackGas);
    free_bg.usable_gas_bar = 60.0;
    let mut free_stage = cyl(5.7, 207.0, 200.0, CylRole::Stage);
    free_stage.usable_gas_bar = 90.0;
    let mut free_empty_stage = cyl(5.7, 207.0, 200.0, CylRole::Stage);
    free_empty_stage.usable_gas_bar = 0.0;
    vec![
        Rig {
            name: "all-thirds",
            cylinders: vec![cyl(7.0, 232.0, 200.0, CylRole::BackGas)],
            config: GasConfig::default(),
            scooter: false,
        },
        Rig {
            name: "all-thirds-margin",
            cylinders: vec![
                cyl(5.0, 232.0, 210.0, CylRole::BackGas),
                cyl(5.0, 232.0, 240.0, CylRole::BackGas),
                cyl(5.7, 207.0, 180.0, CylRole::Stage),
                cyl(3.0, 207.0, 200.0, CylRole::Deco),
            ],
            config: GasConfig {
                reserve_profile: ReserveProfile {
                    all_thirds_safety_margin_bar: 15.0,
                    ..ReserveProfile::default()
                },
                conservative_tank_gas: false,
                ..GasConfig::default()
            },
            scooter: false,
        },
        Rig {
            name: "one-stage",
            cylinders: vec![
                cyl(7.0, 232.0, 180.0, CylRole::BackGas),
                cyl(5.7, 207.0, 200.0, CylRole::Stage),
            ],
            config: staged(ReservePreset::OneStage),
            scooter: false,
        },
        Rig {
            name: "two-stage",
            cylinders: vec![
                cyl(7.0, 232.0, 180.0, CylRole::BackGas),
                disabled_stage,
                cyl(5.7, 207.0, 200.0, CylRole::Stage),
                cyl(4.0, 207.0, 230.0, CylRole::Stage),
            ],
            config: staged(ReservePreset::TwoStage),
            scooter: false,
        },
        Rig {
            name: "two-stage-short",
            cylinders: vec![
                cyl(7.0, 232.0, 180.0, CylRole::BackGas),
                cyl(5.7, 207.0, 200.0, CylRole::Stage),
            ],
            config: staged(ReservePreset::TwoStage),
            scooter: false,
        },
        Rig {
            name: "free",
            cylinders: vec![free_bg, free_stage, free_empty_stage],
            config: GasConfig {
                default_reserve: ReservePreset::Free,
                ..GasConfig::default()
            },
            scooter: false,
        },
        Rig {
            name: "dpv",
            cylinders: vec![cyl(12.0, 232.0, 220.0, CylRole::BackGas)],
            config: GasConfig {
                dpv: Some(DpvRig {
                    primary: DpvUnit {
                        speed_m_per_min: 30.0,
                        burn_minutes: 4.0,
                    },
                    backup: DpvUnit {
                        speed_m_per_min: 20.0,
                        burn_minutes: 5.0,
                    },
                    range_reserve_fraction: 0.2,
                }),
                ..GasConfig::default()
            },
            scooter: true,
        },
        Rig {
            name: "reference-legs",
            cylinders: vec![cyl(7.0, 232.0, 200.0, CylRole::BackGas)],
            config: GasConfig {
                include_reference_legs: true,
                jump_rmv_multiplier: 1.6,
                survey_speed_m_per_min: Some(6.0),
                survey_rmv_multiplier: 1.3,
                ..GasConfig::default()
            },
            scooter: false,
        },
    ]
}
#[test]
fn gas_reach_is_bit_identical_when_lines_and_closures_are_reordered() {
    // Station-level results do not depend on the line list order or the closure list order: the
    // graph emits shots by line id and closures by id, nodes are dense by station id, and a node's
    // naming line falls back to the lowest line id.
    let diver = Diver::new(20.0, None).unwrap();
    let rigs = rigs();
    let mut reordered = 0;
    let mut compared = 0;
    for seed in 0..12_u64 {
        let old = random_legacy_survey(seed, Mix::default());
        let base = convert(&old).project;
        let mut shuffled = base.clone();
        let mut rng = Lcg(seed.wrapping_add(101));
        let lines = shuffled.lines_mut();
        for i in (1..lines.len()).rev() {
            let j = rng.below(i + 1);
            lines.swap(i, j);
        }
        shuffled.closures.reverse();
        assert!(shuffled.validate().is_empty(), "seed {seed}: still valid");
        let ids = |p: &Project| -> Vec<crate::LineId> { p.lines().iter().map(|l| l.id).collect() };
        reordered += usize::from(ids(&base) != ids(&shuffled));
        let stations = base.topology().stations().to_vec();
        let pick = |rng: &mut Lcg| stations[rng.below(stations.len())];
        for rig in &rigs {
            for case in 0..4 {
                let origin = (case % 2 == 1).then(|| pick(&mut rng));
                let endpoint = (case >= 2).then(|| pick(&mut rng));
                let waypoints: Vec<Option<StationId>> =
                    (0..case % 3).map(|_| Some(pick(&mut rng))).collect();
                let overrides = vec![Some(pick(&mut rng)), None];
                let segments: Vec<PlanSegment> = (0..=waypoints.len())
                    .map(|k| PlanSegment {
                        speed_m_per_min: Some(9.0 + k as f64),
                        on_scooter: rig.scooter,
                    })
                    .collect();
                let run = |p: &Project| {
                    p.gas_reach(
                        origin,
                        endpoint,
                        &waypoints,
                        &segments,
                        &diver,
                        &rig.cylinders,
                        &rig.config,
                        &overrides,
                    )
                };
                let (a, b) = (run(&base), run(&shuffled));
                assert_eq!(
                    format!("{a:?}"),
                    format!("{b:?}"),
                    "seed {seed} rig {} case {case}",
                    rig.name
                );
                compared += usize::from(!a.route.is_empty());
            }
        }
    }
    assert!(
        reordered >= 10,
        "the shuffle must reorder lines ({reordered})"
    );
    assert!(compared > 100, "routes compared ({compared})");
}
