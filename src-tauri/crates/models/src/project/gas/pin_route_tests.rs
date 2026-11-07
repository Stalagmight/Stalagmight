//! [#576 RV8c-2-D1..D3, decided "pin decides the route"] A pinned drop is a place the dive
//! goes. (This sweep pinned the rule it replaces, NF-PGAS3-1 "route computed without pins": that
//! rule let LESS gas move the pin-free route away from a pin and hand its stranded gas back.)
//!
//! For every pinned plan of the sweep, the route either passes every pin the plan has, or turns
//! short of one because the gas (or the scooter range) runs out on the way - and then that drop is
//! reported past the turnaround, not reached, and never over-reserve. Either way the plan never
//! has more budget or reach than the plan without its pins, and its turnaround is in reach.
//!
//! Swept like the P-GAS3 probe: three strategies, two fills, three endpoints (none, each
//! branch end), with and without a way point that makes the route double back, with and
//! without a scooter, and every station pinned alone, as the second drop, and in pairs.

#![expect(
    clippy::unwrap_used,
    reason = "tests construct from known-valid literals; a failed unwrap should fail the test"
)]

use super::model::{CylRole, Cylinder, Diver, GasReachField, PlanSegment, TankMaterial};
use super::route_sweep_tests::{scooter, two_branch_cave};
use crate::config::{GasConfig, ReservePreset};
use crate::StationRef;

fn route_stations(field: &GasReachField) -> Vec<StationRef> {
    field
        .route
        .first()
        .map(|leg| leg.from)
        .into_iter()
        .chain(field.route.iter().map(|leg| leg.to))
        .collect()
}

/// What one pinned plan did that the decisions forbid, if anything.
fn verdict(
    plain: &GasReachField,
    pinned: &GasReachField,
    pins: &[Option<StationRef>],
) -> Option<String> {
    let gained = pinned
        .reachable
        .iter()
        .zip(&plain.reachable)
        .any(|(&with, &without)| with && !without);
    if pinned.budget_litres > plain.budget_litres + 1e-9 || gained {
        return Some("reaches further than the plan without pins".to_string());
    }
    let turn = pinned.final_turn_ref?;
    let i = pinned.station_refs.iter().position(|s| *s == turn)?;
    if !pinned.reachable[i] {
        return Some(format!("turnaround {turn:?} is out of the pinned budget"));
    }
    let mut route = route_stations(pinned);
    if route.is_empty() {
        route.push(turn);
    }
    let unreached: Vec<usize> = (0..pinned.stage_drop_refs.len())
        .filter(|&k| pinned.stage_drop_route_indices[k] as usize >= route.len())
        .collect();
    if let Some(&k) = unreached
        .iter()
        .find(|&&k| pinned.stage_drop_over_reserve[k])
    {
        return Some(format!("drop {k} is not reached but flagged over-reserve"));
    }
    // Every pin is on the route - unless the dive turned short of one (a drop past the turnaround),
    // or that stage ran dry before its pin and its drop snapped back to the empty point (#246).
    // Only the first `stage_drop_bars.len()` pins name a drop the strategy has.
    if unreached.is_empty() {
        let skipped = pins
            .iter()
            .take(pinned.stage_drop_bars.len())
            .enumerate()
            .find(|(k, p)| {
                p.is_some_and(|p| !route.contains(&p) && pinned.stage_drop_refs.get(*k) == Some(&p))
            });
        if let Some((k, _)) = skipped {
            return Some(format!("the route skips drop {k}'s pin, drawn on it"));
        }
    }
    None
}

#[test]
fn a_pinned_drop_is_passed_or_reported_past_the_turnaround() {
    let (project, origin, trunk_end, side_end) = two_branch_cave();
    let diver = Diver::new(20.0, None).unwrap();
    let endpoints = [None, Some(trunk_end), Some(side_end)];
    let waypoint_sets: [&[StationRef]; 2] = [&[], &[side_end]];
    let mut runs = 0usize;
    let mut rerouted = 0usize;
    let mut short_of_a_pin = 0usize;
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
                            continue;
                        }
                        let field = |pins: &[Option<StationRef>]| {
                            project.compute_gas_reach_field_with_overrides(
                                Some(&origin),
                                endpoint.as_ref(),
                                waypoints,
                                &segments,
                                &diver,
                                &rig,
                                &cfg,
                                pins,
                            )
                        };
                        let plain = field(&[]);
                        let stations = plain.station_refs.clone();
                        let mut pin_sets: Vec<Vec<Option<StationRef>>> = Vec::new();
                        for (i, &s) in stations.iter().enumerate() {
                            pin_sets.push(vec![Some(s)]);
                            pin_sets.push(vec![None, Some(s)]);
                            for &t in stations.iter().skip(i + 1) {
                                pin_sets.push(vec![Some(s), Some(t)]);
                            }
                        }
                        for pins in &pin_sets {
                            runs += 1;
                            let pinned = field(pins);
                            if pinned.route != plain.route {
                                rerouted += 1;
                            }
                            if (0..pinned.stage_drop_refs.len()).any(|k| {
                                pinned.stage_drop_route_indices[k] as usize > pinned.route.len()
                            }) {
                                short_of_a_pin += 1;
                            }
                            if let Some(why) = verdict(&plain, &pinned, pins) {
                                violations.push(format!(
                                    "{preset:?} fill {fill} scooter {on_scooter} end {endpoint:?} \
                                     way points {waypoints:?} pins {pins:?}: {why} (budget \
                                     {:.1} L against {:.1} L)",
                                    pinned.budget_litres, plain.budget_litres
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "pinned-route sweep: {runs} pinned plans, {rerouted} routed differently from the pin-free \
         plan, {short_of_a_pin} turned short of a pin, {} breaking the pin rules",
        violations.len()
    );
    for v in violations.iter().take(6) {
        println!("  {v}");
    }
    assert!(runs > 2000, "sweep ran {runs} plans");
    assert!(
        rerouted > 0 && short_of_a_pin > 0,
        "the sweep must include pins that move the dive and pins it runs out of gas before"
    );
    assert!(
        violations.is_empty(),
        "{} of {runs} pinned plans broke the pin rules",
        violations.len()
    );
}
