//! Differential test: the station-graph gas engine against the legacy slot engine
//! (`crate::Project::compute_gas_reach_field_with_overrides`) on random legacy surveys
//! (`legacy_convert::random_legacy_survey`: merges, closures incl. aligned snaps and legacy
//! across-one-shot ones, datums and GPS fixes with entrance overrides, jumps and reference shots,
//! hand moves), converted with `legacy_convert::convert`. Plus a determinism test.
//!
//! What is compared, per survey x rig x argument set:
//! - per slot: `gas_to_reach` (relative 1e-9) and `reachable` (identical);
//! - `budget_litres`, `max_reachable_gas` and every gauge;
//! - the turn frontier as a SET OF STATIONS (old slots through `station_of`);
//! - the route as a station sequence after collapsing consecutive duplicates (the old merge
//!   hops), with every route index (drops, way points, profile) mapped through that collapse;
//! - `final_turn_ref` and `stage_drop_refs` by station;
//! - the route readouts, scooter readouts and the profile (relative 1e-9).
//!
//! Two documented classes are compared differently:
//! - **hand z at a junction on the route**: a legacy hand move's z on one line of a junction makes
//!   its slots sit at different depths. The old route listed every slot it hopped through and
//!   took its max depth and the exit-leg gas over all of them; the station graph has one node with
//!   one depth (its slot on the arriving leg's line). Then `route_max_depth_m` and the profile's
//!   return-leg gas are not compared (each profile depth is still checked against the old z of
//!   the slot the new profile names).
//! - **a drop override on another slot of a junction**: the station graph cannot tell a
//!   junction's slots apart, so such a pin means the station. The old engine compared slot
//!   indices: a pin on the default drop's twin slot was scored as a fresh pin, and a pin on a slot
//!   the route did not pass fell back to the default. The old engine is therefore called with each
//!   override normalized to the slot the station graph means (the default drop's slot when it is
//!   the same station, else the station's first slot on the old route). One case cannot be
//!   normalized: a pin OFF the route when the route's target is a junction. The old engine priced
//!   it only if the path to it passed the target's exact SLOT, so a path through another slot of
//!   the target junction kept the default; the station graph prices it (the path passes the target
//!   station, which is the rule's intent). Such calls are counted, not compared.
//!
//! Fully synthetic data only.

use std::collections::BTreeSet;

use crate::config::{DpvRig, DpvUnit, GasConfig, ReservePreset, ReserveProfile};
use crate::gas::{CylRole, Cylinder, Diver, GasReachField, PlanSegment, TankMaterial};
use crate::{StationId, StationRef};

use super::super::legacy_convert::{
    convert, old_raw, random_legacy_survey, Converted, Lcg, Mix, Old,
};
use super::super::Project;

const REL: f64 = 1e-9;

fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= REL * a.abs().max(b.abs()).max(1.0)
}

fn close_opt(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => close(a, b),
        _ => false,
    }
}

fn close_all(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| close(x, y))
}

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

/// One engine call's arguments, in slots.
struct Args {
    origin: Option<StationRef>,
    endpoint: Option<StationRef>,
    waypoints: Vec<StationRef>,
    segments: Vec<PlanSegment>,
}

/// A slot that resolves to nothing (a deleted line under a loaded plan).
const GHOST: StationRef = StationRef {
    line_id: 9999,
    station_index: 0,
};

fn pick(rng: &mut Lcg, slots: &[StationRef]) -> StationRef {
    slots[rng.below(slots.len())]
}

fn args_for(rng: &mut Lcg, slots: &[StationRef], rig: &Rig, case: usize) -> Args {
    let origin = match case % 4 {
        0 => None,
        3 if rng.below(3) == 0 => Some(GHOST),
        _ => Some(pick(rng, slots)),
    };
    let endpoint = match rng.below(3) {
        0 => None,
        _ => Some(pick(rng, slots)),
    };
    let waypoints: Vec<StationRef> = match case % 3 {
        0 => Vec::new(),
        1 => vec![pick(rng, slots)],
        _ => vec![pick(rng, slots), pick(rng, slots)],
    };
    let spans = waypoints.len() + 1;
    let segments: Vec<PlanSegment> = match (rig.scooter, rng.below(3)) {
        (_, 0) => Vec::new(),
        (true, _) => (0..spans)
            .map(|k| PlanSegment {
                speed_m_per_min: [None, Some(25.0), Some(12.0)][rng.below(3)],
                on_scooter: k + 1 < spans || rng.below(2) == 0,
            })
            .collect(),
        (false, _) => (0..spans)
            .map(|_| PlanSegment {
                speed_m_per_min: [None, Some(7.5), Some(14.0)][rng.below(3)],
                on_scooter: false,
            })
            .collect(),
    };
    Args {
        origin,
        endpoint,
        waypoints,
        segments,
    }
}

/// The ordered route slots of a field (its legs re-joined; a one-station route has no leg, and
/// is its turnaround alone).
fn route_slots(field: &GasReachField) -> Vec<StationRef> {
    match field.route.first() {
        Some(first) => std::iter::once(first.from)
            .chain(field.route.iter().map(|l| l.to))
            .collect(),
        None => field.final_turn_ref.into_iter().collect(),
    }
}

/// The old route as stations with consecutive duplicates (merge hops) collapsed, and the new
/// route index of every old route index.
fn collapse(conv: &Converted, old_route: &[StationRef]) -> (Vec<StationId>, Vec<usize>) {
    let mut stations: Vec<StationId> = Vec::new();
    let mut index = Vec::with_capacity(old_route.len());
    for &slot in old_route {
        let station = conv.station(slot);
        if stations.last() != Some(&station) {
            stations.push(station);
        }
        index.push(stations.len() - 1);
    }
    (stations, index)
}

/// The stations of a set of slots of the converted project.
fn station_set(conv: &Converted, slots: &[StationRef]) -> BTreeSet<StationId> {
    slots.iter().map(|&slot| conv.station(slot)).collect()
}

/// Whether a station's slots sit at different depths in the old model (a legacy hand move's z on
/// one line of a junction).
fn split_depth(
    old_z: &std::collections::BTreeMap<StationRef, [f64; 3]>,
    p: &Project,
    s: StationId,
) -> bool {
    let zs: Vec<f64> = p
        .slots_of(s)
        .iter()
        .filter_map(|slot| old_z.get(slot).map(|c| c[2]))
        .collect();
    zs.iter().any(|&z| !close(z, zs[0]))
}

/// Normalize drop overrides for the old engine to the slot the station graph means (module doc).
fn normalize_overrides(
    conv: &Converted,
    overrides: &[Option<StationRef>],
    defaults: &[StationRef],
    base_route: &[StationRef],
) -> Vec<Option<StationRef>> {
    overrides
        .iter()
        .enumerate()
        .map(|(k, o)| {
            let slot = (*o)?;
            if !conv.station_of.contains_key(&slot) {
                return Some(slot);
            }
            let station = conv.station(slot);
            if let Some(&default) = defaults.get(k) {
                if conv.station(default) == station {
                    return Some(default);
                }
            }
            Some(
                base_route
                    .iter()
                    .copied()
                    .find(|&r| conv.station(r) == station)
                    .unwrap_or(slot),
            )
        })
        .collect()
}

#[derive(Default)]
struct Tally {
    calls: usize,
    reachable_slots: usize,
    with_route: usize,
    with_waypoints_routed: usize,
    with_drops: usize,
    with_overrides: usize,
    with_profile: usize,
    with_frontier: usize,
    scooter_bound: usize,
    stalled: usize,
    merge_hops: usize,
    split_depth_routes: usize,
    junction_target_pins: usize,
}

/// Compare one old field with the new one (module doc). `what` names the case.
fn compare(
    what: &str,
    old: &Old,
    conv: &Converted,
    o: &GasReachField,
    n: &GasReachField,
    flat_hand: bool,
    tally: &mut Tally,
) {
    let p = &conv.project;
    tally.calls += 1;
    assert_eq!(n.station_refs, o.station_refs, "{what}: station_refs");
    for (i, slot) in o.station_refs.iter().enumerate() {
        assert!(
            close_opt(n.gas_to_reach[i], o.gas_to_reach[i]),
            "{what}: gas at {slot}: {:?} vs legacy {:?}",
            n.gas_to_reach[i],
            o.gas_to_reach[i]
        );
        assert_eq!(
            n.reachable[i], o.reachable[i],
            "{what}: reachable at {slot}"
        );
    }
    tally.reachable_slots += o.reachable.iter().filter(|r| **r).count();
    assert!(
        close(n.budget_litres, o.budget_litres),
        "{what}: budget {} vs legacy {}; route {:?} vs legacy {:?}; drops {:?} vs {:?} bars {:?} vs {:?}",
        n.budget_litres,
        o.budget_litres,
        route_slots(n),
        route_slots(o),
        n.stage_drop_refs,
        o.stage_drop_refs,
        n.stage_drop_bars,
        o.stage_drop_bars
    );
    assert!(
        close(n.max_reachable_gas, o.max_reachable_gas),
        "{what}: max_reachable_gas"
    );
    assert!(
        close_opt(n.backgas_turn_bar, o.backgas_turn_bar),
        "{what}: backgas_turn_bar"
    );
    assert!(
        close_all(&n.stage_drop_bars, &o.stage_drop_bars),
        "{what}: drop bars"
    );
    assert_eq!(
        n.stage_drop_over_reserve, o.stage_drop_over_reserve,
        "{what}: over reserve"
    );
    assert!(
        close_all(&n.cylinder_usable_bars, &o.cylinder_usable_bars),
        "{what}: usable bars"
    );
    assert_eq!(
        n.scooter_range_limit_m, o.scooter_range_limit_m,
        "{what}: range limit"
    );

    // Frontier: a set of stations; the wire lists each station once, by its first slot.
    let new_frontier = station_set(conv, &n.turn_frontier);
    assert_eq!(
        new_frontier,
        station_set(conv, &o.turn_frontier),
        "{what}: frontier"
    );
    assert_eq!(
        new_frontier.len(),
        n.turn_frontier.len(),
        "{what}: frontier once each"
    );
    for slot in &n.turn_frontier {
        assert_eq!(
            p.topology().first_slot(conv.station(*slot)),
            Some(*slot),
            "{what}: frontier slot"
        );
    }
    tally.with_frontier += usize::from(!new_frontier.is_empty());

    // Route: stations after collapsing the merge hops.
    let old_route = route_slots(o);
    let (old_stations, to_new) = collapse(conv, &old_route);
    tally.merge_hops += old_route.len() - old_stations.len();
    let new_route = route_slots(n);
    let new_stations: Vec<StationId> = new_route.iter().map(|&s| conv.station(s)).collect();
    assert_eq!(new_stations, old_stations, "{what}: route stations");
    assert_eq!(
        n.final_turn_ref.map(|s| conv.station(s)),
        o.final_turn_ref.map(|s| conv.station(s)),
        "{what}: final turn"
    );
    assert_eq!(
        n.turn_toward_endpoint, o.turn_toward_endpoint,
        "{what}: toward"
    );
    assert_eq!(
        n.turn_on_scooter_range, o.turn_on_scooter_range,
        "{what}: scooter bound"
    );
    tally.scooter_bound += usize::from(n.turn_on_scooter_range);
    tally.with_route += usize::from(!new_route.is_empty());
    // Legs: the old merge hops (same station both ends) dropped, the rest kept in order.
    let old_jumps: Vec<bool> = o
        .route
        .iter()
        .filter(|l| conv.station(l.from) != conv.station(l.to))
        .map(|l| l.is_jump)
        .collect();
    let new_jumps: Vec<bool> = n.route.iter().map(|l| l.is_jump).collect();
    assert_eq!(new_jumps, old_jumps, "{what}: jump legs");

    // Drops and way points by route index (mapped through the collapse) and by station.
    // A route with no leg is one node (or none) on both sides: index 0 maps to itself.
    let mapped = |j: u32| {
        if o.route.is_empty() {
            Some(j)
        } else {
            to_new.get(j as usize).map(|&i| i as u32)
        }
    };
    let old_drops: Vec<Option<u32>> = o
        .stage_drop_route_indices
        .iter()
        .map(|&j| mapped(j))
        .collect();
    let new_drops: Vec<Option<u32>> = n
        .stage_drop_route_indices
        .iter()
        .map(|&j| Some(j))
        .collect();
    assert_eq!(new_drops, old_drops, "{what}: drop indices");
    assert_eq!(
        station_list(conv, &n.stage_drop_refs),
        station_list(conv, &o.stage_drop_refs),
        "{what}: drop stations"
    );
    tally.with_drops += usize::from(!n.stage_drop_refs.is_empty());
    assert!(
        close_all(&n.stage_drop_distances_m, &o.stage_drop_distances_m),
        "{what}: drop distances"
    );
    let old_wp: Vec<Option<u32>> = o
        .waypoint_route_indices
        .iter()
        .map(|j| j.and_then(mapped))
        .collect();
    assert_eq!(
        n.waypoint_route_indices, old_wp,
        "{what}: way point indices; gas {:?} vs {:?}; reachable {} vs {}; entrances {:?} vs legacy {:?}",
        n.waypoint_gas_litres,
        o.waypoint_gas_litres,
        n.reachable.iter().filter(|r| **r).count(),
        o.reachable.iter().filter(|r| **r).count(),
        p.entrance_stations(),
        old.entrance_datum_refs()
    );
    assert_eq!(
        n.waypoint_gas_litres.len(),
        o.waypoint_gas_litres.len(),
        "{what}: way point gas"
    );
    for (a, b) in n.waypoint_gas_litres.iter().zip(&o.waypoint_gas_litres) {
        assert!(close_opt(*a, *b), "{what}: way point gas {a:?} vs {b:?}");
    }
    tally.stalled += usize::from(o.waypoint_gas_litres.iter().any(Option::is_none));
    tally.with_waypoints_routed += usize::from(
        !o.waypoint_gas_litres.is_empty() && o.waypoint_gas_litres.iter().all(Option::is_some),
    );

    // Readouts.
    assert!(
        close(n.route_line_length_m, o.route_line_length_m),
        "{what}: length {} vs legacy {}",
        n.route_line_length_m,
        o.route_line_length_m
    );
    assert!(
        close(n.scooter_distance_m, o.scooter_distance_m),
        "{what}: scooter distance"
    );
    assert!(
        close_opt(n.route_runtime_min, o.route_runtime_min),
        "{what}: runtime"
    );
    assert_eq!(
        n.scooter_drop_ref, o.scooter_drop_ref,
        "{what}: scooter drop"
    );
    let old_z = old_raw(old);
    let split = new_stations.iter().any(|&s| split_depth(&old_z, p, s));
    assert!(!(split && flat_hand), "{what}: split depth without hand z");
    tally.split_depth_routes += usize::from(split);
    if !split {
        assert!(
            close_opt(n.route_max_depth_m, o.route_max_depth_m),
            "{what}: max depth"
        );
    }
    // The average weighs each swum leg by its own line's slot depths, exactly as before; only a
    // route with no swum metre falls back to the first node's depth, which is a node depth.
    if n.route_line_length_m > 0.0 || !split {
        assert!(
            close_opt(n.route_avg_depth_m, o.route_avg_depth_m),
            "{what}: avg depth {:?} vs legacy {:?}",
            n.route_avg_depth_m,
            o.route_avg_depth_m
        );
    }

    // Profile: one new point per old run of hop points, out and back.
    assert_eq!(
        n.route_profile.is_empty(),
        o.route_profile.is_empty(),
        "{what}: profile"
    );
    tally.with_profile += usize::from(!n.route_profile.is_empty());
    for np in &n.route_profile {
        let i = np.route_index as usize;
        assert_eq!(
            conv.station(np.station_ref),
            new_stations[i],
            "{what}: profile station"
        );
        // The old point of the same pass: the run's first index (all of a run's points agree
        // on distance, runtime and gas; on the way back it is the last of the run emitted).
        let first_old = to_new.iter().position(|&k| k == i).unwrap();
        let op = o
            .route_profile
            .iter()
            .find(|q| q.outbound == np.outbound && q.route_index as usize == first_old)
            .unwrap_or_else(|| panic!("{what}: no legacy profile point for {np:?}"));
        assert!(
            close(np.distance_m, op.distance_m),
            "{what}: profile distance"
        );
        assert!(
            close(np.elapsed_min, op.elapsed_min),
            "{what}: profile runtime"
        );
        if np.outbound || !split {
            assert!(
                close(np.gas_l, op.gas_l),
                "{what}: profile gas {np:?} vs legacy {op:?}"
            );
        }
        assert!(
            close(np.depth_m, old_z[&np.station_ref][2]),
            "{what}: profile depth"
        );
    }
}

fn station_list(conv: &Converted, slots: &[StationRef]) -> Vec<StationId> {
    slots.iter().map(|&slot| conv.station(slot)).collect()
}

#[test]
fn gas_reach_matches_the_legacy_engine_on_random_surveys() {
    let diver = Diver::new(20.0, None).unwrap();
    let rigs = rigs();
    let mut tally = Tally::default();
    // Every survey twice: as generated, and with every hand move's z zeroed, where no junction's
    // slots sit at different depths and every depth quantity is compared exactly.
    for (seed, flat_hand) in (0..36_u64).flat_map(|seed| [(seed, false), (seed, true)]) {
        let mix = Mix {
            across_one_shot: usize::from(seed % 4 == 0),
            datums: if seed % 9 == 4 { 0 } else { 4 },
            ..Mix::default()
        };
        let mut old = random_legacy_survey(seed, mix);
        if flat_hand {
            for offset in old.station_offsets.values_mut() {
                offset.2 = 0.0;
            }
        }
        let conv = convert(&old);
        let slots: Vec<StationRef> = old
            .compute_station_coordinates()
            .into_iter()
            .map(|s| s.station_ref)
            .collect();
        let mut rng = Lcg(seed.wrapping_mul(7919).wrapping_add(17));
        for rig in &rigs {
            for case in 0..5 {
                let a = args_for(&mut rng, &slots, rig, case);
                let what = format!(
                    "seed {seed} flat hand z {flat_hand} rig {} case {case} (origin {:?}, endpoint {:?}, \
                     way points {:?}, segments {:?})",
                    rig.name, a.origin, a.endpoint, a.waypoints, a.segments
                );
                let old_base = old.compute_gas_reach_field_with_overrides(
                    a.origin.as_ref(),
                    a.endpoint.as_ref(),
                    &a.waypoints,
                    &a.segments,
                    &diver,
                    &rig.cylinders,
                    &rig.config,
                    &[],
                );
                let new_base = conv.project.gas_reach_field_wire(
                    a.origin.as_ref(),
                    a.endpoint.as_ref(),
                    &a.waypoints,
                    &a.segments,
                    &diver,
                    &rig.cylinders,
                    &rig.config,
                    &[],
                );
                compare(
                    &what, &old, &conv, &old_base, &new_base, flat_hand, &mut tally,
                );

                // Drop overrides: on the base route, at the default drop, or anywhere.
                let drops = old_base.stage_drop_refs.len();
                if drops == 0 {
                    continue;
                }
                let base_route = route_slots(&old_base);
                let overrides: Vec<Option<StationRef>> = (0..=drops)
                    .map(|_| match rng.below(4) {
                        0 => None,
                        1 => Some(pick(&mut rng, &slots)),
                        _ if base_route.is_empty() => None,
                        _ => Some(pick(&mut rng, &base_route)),
                    })
                    .collect();
                // A pin off the route with a junction target: see the module doc.
                let base_stations: BTreeSet<StationId> = station_set(&conv, &base_route);
                let junction_target = old_base
                    .final_turn_ref
                    .is_some_and(|slot| conv.project.slots_of(conv.station(slot)).len() > 1);
                let off_route = overrides.iter().take(drops).flatten().any(|slot| {
                    conv.station_of
                        .get(slot)
                        .is_some_and(|s| !base_stations.contains(s))
                });
                if junction_target && off_route {
                    tally.junction_target_pins += 1;
                    continue;
                }
                let normalized =
                    normalize_overrides(&conv, &overrides, &old_base.stage_drop_refs, &base_route);
                let old_pinned = old.compute_gas_reach_field_with_overrides(
                    a.origin.as_ref(),
                    a.endpoint.as_ref(),
                    &a.waypoints,
                    &a.segments,
                    &diver,
                    &rig.cylinders,
                    &rig.config,
                    &normalized,
                );
                let new_pinned = conv.project.gas_reach_field_wire(
                    a.origin.as_ref(),
                    a.endpoint.as_ref(),
                    &a.waypoints,
                    &a.segments,
                    &diver,
                    &rig.cylinders,
                    &rig.config,
                    &overrides,
                );
                compare(
                    &format!(
                        "{what} pinned {overrides:?} normalized {normalized:?} base route {base_route:?} defaults {:?}",
                        old_base.stage_drop_refs
                    ),
                    &old,
                    &conv,
                    &old_pinned,
                    &new_pinned,
                    flat_hand,
                    &mut tally,
                );
                tally.with_overrides += 1;
            }
        }
    }
    let t = &tally;
    // The generator must exercise what the comparison claims to cover.
    // (One run: 3630 calls, 131572 reachable slots, 2942 routes, 1452 routed way-point plans,
    // 1548 with drops, 750 pinned, 2778 profiles, 1890 frontiers, 22 scooter-bound turns, 662
    // stalled chains, 9721 merge hops collapsed, 516 split-depth routes, 48 junction-target pins.)
    assert!(t.calls > 3000, "calls {}", t.calls);
    assert!(
        t.reachable_slots > 100_000,
        "reachable {}",
        t.reachable_slots
    );
    assert!(t.with_route > 2000, "routes {}", t.with_route);
    assert!(
        t.with_waypoints_routed > 1000,
        "way points {}",
        t.with_waypoints_routed
    );
    assert!(t.with_drops > 1000, "drops {}", t.with_drops);
    assert!(t.with_overrides > 500, "overrides {}", t.with_overrides);
    assert!(t.with_profile > 2000, "profiles {}", t.with_profile);
    assert!(t.with_frontier > 1000, "frontiers {}", t.with_frontier);
    assert!(t.scooter_bound > 10, "scooter bound {}", t.scooter_bound);
    assert!(t.stalled > 300, "stalled {}", t.stalled);
    assert!(
        t.split_depth_routes > 100,
        "split depth {}",
        t.split_depth_routes
    );
    assert!(t.merge_hops > 1000, "merge hops {}", t.merge_hops);
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
