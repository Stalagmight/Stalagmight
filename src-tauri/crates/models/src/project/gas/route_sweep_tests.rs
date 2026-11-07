//! [#576 RV2-R-NITS] The moved-stage-drop sweep with a route: an endpoint, a way
//! point that makes the route double back, and a scooter rig.
//!
//! `gas_reach_field_a_moved_drop_never_credits_more_than_the_plan_without_it`
//! (#576 NF-PGAS-1, decided "re-check pins, cap at default") sweeps pinned drops
//! with no endpoint, no way point and no DPV - so the routed and scootered plans
//! the rule also has to hold for were pinned only by a one-off probe. This module
//! commits that part: wherever a drop is pinned, a plan with an endpoint, a way
//! point or a scooter never has a bigger budget than the same plan without the
//! pins, and never reaches a station that plan cannot.
#![expect(
    clippy::unwrap_used,
    reason = "tests construct from known-valid literals; a failed unwrap should fail the test"
)]

use super::model::{CylRole, Cylinder, Diver, PlanSegment, TankMaterial};
use crate::config::{DpvRig, DpvUnit, GasConfig, ReservePreset};
use crate::test_fixtures::bare_shot;
use crate::{Project, SectionData, StationRef};

/// A trunk of 6 x 100 m and a side line of 2 x 205 m, joined at the origin.
pub(super) fn two_branch_cave() -> (Project, StationRef, StationRef, StationRef) {
    let mut project = Project::new("Gas".to_string(), None);
    let trunk = project
        .add_section(SectionData {
            shots: (0..6).map(|_| bare_shot(100.0)).collect(),
            ..Default::default()
        })
        .id;
    let side = project
        .add_section(SectionData {
            shots: vec![bare_shot(205.0), bare_shot(205.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(trunk, 0), StationRef::new(side, 0))
        .unwrap();
    (
        project,
        StationRef::new(trunk, 0),
        StationRef::new(trunk, 6),
        StationRef::new(side, 2),
    )
}

pub(super) fn scooter() -> DpvRig {
    DpvRig {
        primary: DpvUnit {
            speed_m_per_min: 50.0,
            burn_minutes: 12.0,
        },
        backup: DpvUnit {
            speed_m_per_min: 50.0,
            burn_minutes: 20.0,
        },
        range_reserve_fraction: 0.1,
    }
}

#[test]
fn a_moved_drop_never_credits_more_on_a_routed_or_scootered_plan() {
    let (project, origin, trunk_end, side_end) = two_branch_cave();
    let diver = Diver::new(20.0, None).unwrap();
    let endpoints = [None, Some(trunk_end), Some(side_end)];
    // A way point on the side line makes a trunk-bound route double back.
    let waypoint_sets: [&[StationRef]; 2] = [&[], &[side_end]];
    let mut runs = 0usize;
    let mut violations = Vec::new();
    for preset in [
        ReservePreset::OneStage,
        ReservePreset::TwoStage,
        ReservePreset::Free,
    ] {
        for fill in [207.0, 280.0] {
            for on_scooter in [false, true] {
                let cfg = GasConfig {
                    default_reserve: preset,
                    dpv: on_scooter.then(scooter),
                    ..GasConfig::default()
                };
                let segments: Vec<PlanSegment> = if on_scooter {
                    vec![PlanSegment {
                        speed_m_per_min: None,
                        on_scooter: true,
                    }]
                } else {
                    Vec::new()
                };
                let tank = |v: f64, role: CylRole, usable: f64| Cylinder {
                    usable_gas_bar: usable,
                    ..Cylinder::new(v, 207.0, fill, TankMaterial::Steel, role, None).unwrap()
                };
                let rig = [
                    tank(15.0, CylRole::BackGas, 110.0),
                    tank(11.0, CylRole::Stage, 70.0),
                    tank(7.0, CylRole::Stage, 60.0),
                ];
                for endpoint in &endpoints {
                    for waypoints in waypoint_sets {
                        if on_scooter && !waypoints.is_empty() {
                            // One scooter segment covers a route with no way point.
                            continue;
                        }
                        let plain = project.compute_gas_reach_field_with_overrides(
                            Some(&origin),
                            endpoint.as_ref(),
                            waypoints,
                            &segments,
                            &diver,
                            &rig,
                            &cfg,
                            &[],
                        );
                        let stations = plain.station_refs.clone();
                        let mut pin_sets: Vec<Vec<Option<StationRef>>> = Vec::new();
                        for (i, &s) in stations.iter().enumerate() {
                            pin_sets.push(vec![Some(s)]);
                            pin_sets.push(vec![None, Some(s)]);
                            pin_sets.push(vec![Some(s), Some(stations[(i + 3) % stations.len()])]);
                        }
                        for pins in &pin_sets {
                            runs += 1;
                            let f = project.compute_gas_reach_field_with_overrides(
                                Some(&origin),
                                endpoint.as_ref(),
                                waypoints,
                                &segments,
                                &diver,
                                &rig,
                                &cfg,
                                pins,
                            );
                            let gained = f
                                .reachable
                                .iter()
                                .zip(&plain.reachable)
                                .any(|(&with, &without)| with && !without);
                            if f.budget_litres > plain.budget_litres + 1e-9 || gained {
                                violations.push(format!(
                                    "{preset:?} fill {fill} scooter {on_scooter} end {endpoint:?} \
                                     way points {waypoints:?} pins {pins:?}: budget {:.2} L \
                                     against {:.2} L, new stations {gained}",
                                    f.budget_litres, plain.budget_litres
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "routed/scootered sweep: {runs} pinned plans, {} gained reach",
        violations.len()
    );
    for v in violations.iter().take(4) {
        println!("  {v}");
    }
    assert!(runs > 800, "sweep ran {runs} plans");
    assert!(
        violations.is_empty(),
        "{} of {runs} pinned plans gained reach",
        violations.len()
    );
}
