//! [#576 RV8c-2, decided D1-D3] A pinned stage drop decides the route.
//!
//! - D1 "Pin decides the route": the route passes the pin; with no endpoint the dive goes on to
//!   the deepest station beyond it, not the deepest anywhere.
//! - D2 "Endpoint via the pin": a pin off the endpoint's route is visited first, and the backtrack
//!   from it is charged like any other passage.
//! - D3 "Turn early, stage kept": a dive that runs out of gas (or scooter range) before the pin
//!   turns where the gas says on the way there, carries the stage, and reports the drop as not
//!   reached; whatever the pin strands stays stranded.
//! - D4 "Only where the dive goes": the overlay paints only the stations the pinned dive can be
//!   at, each priced at its pass on the route through the pins or after passing them, like a way
//!   point; the turn dots are the stations it can END at, off the way to the last pin.
//!
//! The first two tests rebuild the independent review's repro scenes (RV8c: scenario 12178 of the
//! cross-build dump, and the HEAD-only two-branch probe) and print the numbers the before/after
//! evidence quotes.

#![expect(
    clippy::unwrap_used,
    reason = "tests construct from known-valid literals; a failed unwrap should fail the test"
)]

use super::model::{CylRole, Cylinder, Diver, GasReachField, PlanSegment, TankMaterial};
use super::physics::deliverable_gas;
use super::pin_monotone_tests::{range_per_metre_of, route_line_m};
use super::route_sweep_tests::{scooter, two_branch_cave};
use crate::config::{DpvRig, DpvUnit, GasConfig, ReservePreset};
use crate::{ClosureStrategy, Project, SectionData, ShotData, StationRef};

fn cyl(volume: f64, rated: f64, fill: f64, role: CylRole) -> Cylinder {
    Cylinder::new(volume, rated, fill, TankMaterial::Steel, role, None).unwrap()
}

fn shot(length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

/// The review's three-branch cave: branch A of 10 x 80 m descending 3 m a leg, branch B of
/// 5 x 170 m at 5 m, branch C of 7 x 130 m at 12 m, all joined at the origin A:0.
fn three_branch_cave() -> (Project, StationRef, [u32; 3]) {
    let mut p = Project::new("Gas".to_string(), None);
    let a = p
        .add_section(SectionData {
            shots: (0..10)
                .map(|i| shot(80.0, f64::from(i) * 3.0, f64::from(i + 1) * 3.0))
                .collect(),
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: (0..5).map(|_| shot(170.0, 5.0, 5.0)).collect(),
            ..Default::default()
        })
        .id;
    let c = p
        .add_section(SectionData {
            shots: (0..7).map(|_| shot(130.0, 12.0, 12.0)).collect(),
            ..Default::default()
        })
        .id;
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    p.merge_stations(StationRef::new(a, 0), StationRef::new(c, 0))
        .unwrap();
    (p, StationRef::new(a, 0), [a, b, c])
}

fn name(s: StationRef) -> String {
    format!("{}:{}", s.section_id, s.station_index)
}

/// The route's stations in order (first leg's start, then every leg's end).
fn route_of(f: &GasReachField) -> Vec<StationRef> {
    f.route
        .first()
        .map(|l| l.from)
        .into_iter()
        .chain(f.route.iter().map(|l| l.to))
        .collect()
}

fn reach_of(f: &GasReachField) -> Vec<StationRef> {
    f.station_refs
        .iter()
        .zip(&f.reachable)
        .filter(|(_, &r)| r)
        .map(|(s, _)| *s)
        .collect()
}

fn gas_of(f: &GasReachField, s: StationRef) -> f64 {
    let i = f.station_refs.iter().position(|r| *r == s).unwrap();
    f.gas_to_reach[i].unwrap()
}

fn is_reachable(f: &GasReachField, s: StationRef) -> bool {
    let i = f.station_refs.iter().position(|r| *r == s).unwrap();
    f.reachable[i]
}

/// Whether drop `k` is on the route (the dive gets there) rather than past the turnaround.
fn reached(f: &GasReachField, k: usize) -> bool {
    f.stage_drop_route_indices[k] as usize <= f.route.len()
}

fn describe(label: &str, f: &GasReachField) -> String {
    let names = |v: Vec<StationRef>| v.into_iter().map(name).collect::<Vec<_>>().join(" ");
    format!(
        "{label}: budget {:.2} L, turn {}, route [{}], drops [{}] reached {:?} over {:?}, reach [{}]",
        f.budget_litres,
        f.final_turn_ref.map_or("none".to_string(), name),
        names(route_of(f)),
        names(f.stage_drop_refs.clone()),
        (0..f.stage_drop_refs.len())
            .map(|k| reached(f, k))
            .collect::<Vec<_>>(),
        f.stage_drop_over_reserve,
        names(reach_of(f)),
    )
}

/// The reach of `pinned` stays inside the plan without pins: no station it cannot reach, no gas it
/// does not have (#576 NF-PGAS-1).
fn assert_within_pin_free(pinned: &GasReachField, pin_free: &GasReachField) {
    assert!(pinned.budget_litres <= pin_free.budget_litres + 1e-9);
    for (i, (&with, &without)) in pinned.reachable.iter().zip(&pin_free.reachable).enumerate() {
        assert!(
            !with || without,
            "the pin made {} reachable",
            name(pinned.station_refs[i])
        );
    }
}

#[test]
fn scenario_12178_a_pin_on_another_branch_takes_the_dive_there() {
    // RV8c scenario 12178: OneStage, no endpoint, a 3 L back tank at 46.18 bar (a disabled 5.7 L
    // one beside it), a 15 L stage rated 176 filled to 228.47, drop pinned at B:1. The pin-free
    // dive runs down branch A, so the old rule ignored the pin there and credited the whole stage:
    // 979.52 L, turning at A:3 - stations a diver who leaves the stage at B:1 cannot reach.
    let (project, origin, [a, b, _c]) = three_branch_cave();
    let diver = Diver::new(20.0, None).unwrap();
    let mut cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    cfg.reserve_profile.all_thirds_safety_margin_bar = 10.0;
    cfg.normalize();
    let rig = [
        Cylinder {
            enabled: false,
            ..cyl(5.7, 232.0, 232.0, CylRole::BackGas)
        },
        cyl(3.0, 207.0, 46.18, CylRole::BackGas),
        cyl(15.0, 176.0, 228.472, CylRole::Stage),
    ];
    let pin = StationRef::new(b, 1);
    let field = |pins: &[Option<StationRef>]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            None,
            &[],
            &[],
            &diver,
            &rig,
            &cfg,
            pins,
        )
    };
    let pin_free = field(&[]);
    let pinned = field(&[Some(pin)]);
    println!("RV8C-12178 {}", describe("pin-free", &pin_free));
    println!("RV8C-12178 {}", describe("pinned B:1", &pinned));

    // The dive goes to the pin, drops the stage there and turns: nothing beyond B:1 is affordable
    // on the back gas left (B:2 costs another 170 m).
    assert!(route_of(&pinned).contains(&pin), "the route skips the pin");
    assert_eq!(pinned.final_turn_ref, Some(pin));
    assert_eq!(pinned.stage_drop_refs, vec![pin]);
    assert!(reached(&pinned, 0));
    assert_eq!(pinned.stage_drop_over_reserve, vec![false]);
    // What a diver who drops at the pin has: the stage breathed to B:1 plus the back gas's quarter
    // (3 L x 46.18 / 4 bar, below the Z == 1 knee, so linear).
    let back_quarter = 3.0 * 46.18 / 4.0;
    let has = gas_of(&pinned, pin) + back_quarter;
    assert!(
        (pinned.budget_litres - has).abs() < 1e-6,
        "budget {:.3} L, a diver dropping at the pin has {has:.3} L",
        pinned.budget_litres
    );
    assert!(!is_reachable(&pinned, StationRef::new(b, 2)));
    assert!(
        !is_reachable(&pinned, StationRef::new(a, 3)),
        "A:3 is the station the ignored pin lit up"
    );
    for (s, g) in pinned.station_refs.iter().zip(&pinned.gas_to_reach) {
        if is_reachable(&pinned, *s) {
            assert!(
                g.unwrap() <= has + 1e-9,
                "{} costs more than the dive has",
                name(*s)
            );
        }
    }
    assert_within_pin_free(&pinned, &pin_free);
}

/// Stations as sorted names, so a set compares independent of the field's station order.
fn names_of(v: &[StationRef]) -> Vec<String> {
    let mut names: Vec<String> = v.iter().copied().map(name).collect();
    names.sort();
    names
}

/// RV8c scenario 12178's rig and config (see the test above).
fn scenario_12178_plan(
    project: &Project,
    origin: StationRef,
    pins: &[Option<StationRef>],
) -> GasReachField {
    let diver = Diver::new(20.0, None).unwrap();
    let mut cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    cfg.reserve_profile.all_thirds_safety_margin_bar = 10.0;
    cfg.normalize();
    let rig = [
        Cylinder {
            enabled: false,
            ..cyl(5.7, 232.0, 232.0, CylRole::BackGas)
        },
        cyl(3.0, 207.0, 46.18, CylRole::BackGas),
        cyl(15.0, 176.0, 228.472, CylRole::Stage),
    ];
    project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &rig,
        &cfg,
        pins,
    )
}

#[test]
fn scenario_12178_the_overlay_paints_only_where_the_pinned_dive_goes() {
    // #576 RV8c-2-D4, decided "only where the dive goes". The same plan: the stage is dropped at
    // B:1, and the dive turns there with 34.6 L of back gas left. The old overlay painted every
    // station the 555.97 L budget affords FROM THE ENTRANCE - A:1, A:2 and C:1 up the two other
    // branches - although a diver who has swum out to B:1 would need that gas again to come back
    // past the entrance. Priced after passing the pin, as a way point prices it, nothing but the
    // way to B:1 is in reach, and there is no other station the dive can end at.
    let (project, origin, [a, b, c]) = three_branch_cave();
    let pin = StationRef::new(b, 1);
    let pinned = scenario_12178_plan(&project, origin, &[Some(pin)]);
    let pin_free = scenario_12178_plan(&project, origin, &[]);
    println!("RV8C-12178-D4 {}", describe("pinned B:1", &pinned));
    println!(
        "RV8C-12178-D4 turn dots [{}]",
        names_of(&pinned.turn_frontier).join(" ")
    );
    for s in [
        StationRef::new(a, 1),
        StationRef::new(a, 2),
        StationRef::new(c, 1),
    ] {
        assert!(
            !is_reachable(&pinned, s),
            "{} is painted for a dive that drops its stage at B:1",
            name(s)
        );
        // Its price is the way back from the pin: out to B:1, back to the entrance, on to it.
        let after_pin = 2.0 * gas_of(&pinned, pin) + gas_of(&pin_free, s);
        assert!(
            (gas_of(&pinned, s) - after_pin).abs() < 1e-6,
            "{} priced {:.3} L, after the pin it costs {after_pin:.3} L",
            name(s),
            gas_of(&pinned, s)
        );
    }
    let expected = [
        StationRef::new(a, 0),
        StationRef::new(b, 0),
        StationRef::new(c, 0),
        pin,
    ];
    assert_eq!(names_of(&reach_of(&pinned)), names_of(&expected));
    assert!(
        pinned.turn_frontier.is_empty(),
        "turn dots [{}] for a dive that can only end at its pin",
        names_of(&pinned.turn_frontier).join(" ")
    );
    // The way out is priced at its pass, exactly as the plain field prices it.
    assert!((gas_of(&pinned, pin) - gas_of(&pin_free, pin)).abs() < 1e-9);
    assert_within_pin_free(&pinned, &pin_free);
}

#[test]
fn every_turn_dot_of_a_pinned_plan_is_a_station_the_pinned_dive_can_end_at() {
    // D4 for the turn dots: a dot is a candidate endpoint, and clicking one re-plans with it as the
    // endpoint (D2: through the pins first, then on to it). So each dot the pinned plan offers must
    // be a station the plan with that endpoint really turns at - priced after passing the pins, not
    // from the entrance - and painted. Swept over both test caves, rigs with more and less gas, no
    // endpoint and each branch end, and every station as the pin.
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let (three, three_origin, _) = three_branch_cave();
    let (two, two_origin, trunk_end, side_end) = two_branch_cave();
    let caves = [
        (&three, three_origin, None),
        (&two, two_origin, None),
        (&two, two_origin, Some(trunk_end)),
        (&two, two_origin, Some(side_end)),
    ];
    let mut dots = 0usize;
    let mut violations: Vec<String> = Vec::new();
    for (project, origin, endpoint) in caves {
        for (back_size, back_fill) in [(12.0, 60.0), (12.0, 120.0), (12.0, 232.0), (24.0, 232.0)] {
            let rig = [
                cyl(back_size, 232.0, back_fill, CylRole::BackGas),
                cyl(11.0, 207.0, 207.0, CylRole::Stage),
            ];
            let plan = |end: Option<&StationRef>, pins: &[Option<StationRef>]| {
                project.compute_gas_reach_field_with_overrides(
                    Some(&origin),
                    end,
                    &[],
                    &[],
                    &diver,
                    &rig,
                    &cfg,
                    pins,
                )
            };
            let stations = plan(endpoint.as_ref(), &[]).station_refs;
            for &pin in &stations {
                let pins = [Some(pin)];
                let pinned = plan(endpoint.as_ref(), &pins);
                for &dot in &pinned.turn_frontier {
                    dots += 1;
                    let clicked = plan(Some(&dot), &pins);
                    let label = format!(
                        "back {back_size} L at {back_fill} end {:?} pin {}: dot {}",
                        endpoint.map(name),
                        name(pin),
                        name(dot)
                    );
                    if !is_reachable(&pinned, dot) {
                        violations.push(format!("{label} is not painted"));
                    }
                    if clicked.final_turn_ref != Some(dot) {
                        violations.push(format!(
                            "{label}: clicked, the dive turns at {:?}",
                            clicked.final_turn_ref.map(name)
                        ));
                    }
                }
            }
        }
    }
    println!(
        "D4 turn dots: {dots} dots over the pinned plans, {} not a place the pinned dive ends",
        violations.len()
    );
    for v in violations.iter().take(8) {
        println!("  {v}");
    }
    assert!(dots > 60, "the sweep must offer dots ({dots})");
    assert!(violations.is_empty(), "{} turn dots lie", violations.len());
}

#[test]
fn head_probe_less_back_gas_never_reaches_further_on_a_pinned_plan() {
    // RV8c's HEAD-only probe: the two-branch cave, OneStage, an 11 L stage at 207/207 dropped at
    // trunk 1 (200 L in), no endpoint, back gas 12 L rated 232 at 20 or 50 bar - both below the
    // knee. With 50 bar the pin-free dive runs the trunk and the pin strands the stage there; with
    // 20 bar the pin-free dive moved to the side line, the old rule ignored the pin, and the plan
    // lit trunk 4 (800 L) for a diver who has 200 + 60 = 260 L after dropping at trunk 1.
    let (project, origin, trunk_end, side_end) = two_branch_cave();
    let (trunk, side) = (trunk_end.section_id, side_end.section_id);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let pin = StationRef::new(trunk, 1);
    let plan = |fill: f64, pins: &[Option<StationRef>]| {
        let rig = [
            cyl(12.0, 232.0, fill, CylRole::BackGas),
            cyl(11.0, 207.0, 207.0, CylRole::Stage),
        ];
        project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            None,
            &[],
            &[],
            &diver,
            &rig,
            &cfg,
            pins,
        )
    };
    let mut previous: Option<GasReachField> = None;
    for fill in [50.0, 20.0] {
        let pin_free = plan(fill, &[]);
        let pinned = plan(fill, &[Some(pin)]);
        println!(
            "RV8C-MONO back fill {fill}: {}",
            describe("pin-free", &pin_free)
        );
        println!(
            "RV8C-MONO back fill {fill}: {}",
            describe("pinned trunk:1", &pinned)
        );
        let has = 200.0 + 12.0 * fill / 4.0;
        assert!(
            (pinned.budget_litres - has).abs() < 1e-6,
            "back fill {fill}: budget {:.2} L, a diver dropping at trunk 1 has {has:.2} L",
            pinned.budget_litres
        );
        assert_eq!(pinned.final_turn_ref, Some(pin), "back fill {fill}");
        assert!(!is_reachable(&pinned, StationRef::new(trunk, 2)));
        assert!(!is_reachable(&pinned, StationRef::new(side, 1)));
        assert_within_pin_free(&pinned, &pin_free);
        if let Some(more_gas) = &previous {
            // Less back gas, same pins: never more budget, never a new station.
            assert!(pinned.budget_litres <= more_gas.budget_litres + 1e-9);
            for (i, (&now, &before)) in pinned.reachable.iter().zip(&more_gas.reachable).enumerate()
            {
                assert!(
                    !now || before,
                    "less back gas lit {}",
                    name(pinned.station_refs[i])
                );
            }
        }
        previous = Some(pinned);
    }
}

#[test]
fn an_endpoint_on_another_branch_is_reached_through_the_pin_with_the_backtrack_charged() {
    // D2. Endpoint at the end of the side line, drop pinned at trunk 1. The dive swims out to
    // trunk 1 (200 L), drops the stage, swims back to the junction (200 L) and on to the side end
    // (820 L): 1220 L of route for 610 m of line.
    let (project, origin, trunk_end, side_end) = two_branch_cave();
    let trunk = trunk_end.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let rig = [
        cyl(12.0, 232.0, 232.0, CylRole::BackGas),
        cyl(12.0, 232.0, 232.0, CylRole::BackGas),
        cyl(11.0, 207.0, 207.0, CylRole::Stage),
    ];
    let pin = StationRef::new(trunk, 1);
    let field = |pins: &[Option<StationRef>]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            Some(&side_end),
            &[],
            &[],
            &diver,
            &rig,
            &cfg,
            pins,
        )
    };
    let pin_free = field(&[]);
    let pinned = field(&[Some(pin)]);
    println!("D2 {}", describe("pin-free", &pin_free));
    println!("D2 {}", describe("pinned trunk:1", &pinned));
    let route = route_of(&pinned);
    let at_pin = route.iter().position(|s| *s == pin).unwrap();
    assert_eq!(route.last(), Some(&side_end));
    assert!(
        route[at_pin..].contains(&origin),
        "no backtrack to the junction"
    );
    assert_eq!(pinned.final_turn_ref, Some(side_end));
    assert!(pinned.turn_toward_endpoint);
    assert!(
        (pinned.route_line_length_m - 610.0).abs() < 1e-6,
        "swum {:.1} m",
        pinned.route_line_length_m
    );
    let out_and_back = pinned
        .route_profile
        .iter()
        .rev()
        .find(|p| p.outbound)
        .unwrap();
    assert!(
        (out_and_back.gas_l - 1220.0).abs() < 1e-6,
        "the backtrack is charged: {:.1} L to the endpoint",
        out_and_back.gas_l
    );
    assert_eq!(pinned.stage_drop_refs, vec![pin]);
    assert!(reached(&pinned, 0));
    // The stage is breathed only to trunk 1 and the rest of it stays there: the plan has the
    // 200 L to the pin plus a quarter of each back tank's gauge (232 -> 174 bar).
    let quarter =
        deliverable_gas(232.0, 12.0, 232.0, false) - deliverable_gas(174.0, 12.0, 232.0, false);
    let has = 2.0 * quarter + 200.0;
    assert!(
        (pinned.budget_litres - has).abs() < 1e-6,
        "budget {:.2} L, a diver dropping at trunk 1 has {has:.2} L",
        pinned.budget_litres
    );
    assert_within_pin_free(&pinned, &pin_free);

    // Not enough back gas for the backtrack: the dive turns on the way, after the pin, still
    // heading for the endpoint.
    let lean = [
        cyl(12.0, 232.0, 100.0, CylRole::BackGas),
        cyl(11.0, 207.0, 207.0, CylRole::Stage),
    ];
    let short = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        Some(&side_end),
        &[],
        &[],
        &diver,
        &lean,
        &cfg,
        &[Some(pin)],
    );
    println!(
        "D2 {}",
        describe("pinned trunk:1, back 12 L at 100 bar", &short)
    );
    let route = route_of(&short);
    let at_pin = route.iter().position(|s| *s == pin).unwrap();
    assert!(route.len() > at_pin + 1, "turned at the pin");
    assert_ne!(short.final_turn_ref, Some(side_end));
    assert!(short.turn_toward_endpoint);
}

#[test]
fn a_pin_the_gas_does_not_reach_turns_the_dive_on_the_way_and_is_not_reached() {
    // D3. OneStage, back gas 12 L at 50 bar (150 L), a 7 L stage at 207/207 (561 L safe): 711 L.
    // The pin-free dive runs the trunk to trunk 3 (600 L). With the drop pinned at the side end
    // (820 L, deeper than the stage's safe gas) the dive heads down the side line instead and turns
    // at side 1 (410 L) - the next station is past the budget. The stage never reaches its pin:
    // carried all dive, so no over-reserve flag, and the budget is the pin-free one (a deeper pin
    // strands nothing and buys nothing).
    let (project, origin, _trunk_end, side_end) = two_branch_cave();
    let side = side_end.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let rig = [
        cyl(12.0, 232.0, 50.0, CylRole::BackGas),
        cyl(7.0, 207.0, 207.0, CylRole::Stage),
    ];
    let field = |pins: &[Option<StationRef>]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            None,
            &[],
            &[],
            &diver,
            &rig,
            &cfg,
            pins,
        )
    };
    let pin_free = field(&[]);
    let pinned = field(&[Some(side_end)]);
    println!("D3 {}", describe("pin-free", &pin_free));
    println!("D3 {}", describe("pinned side:2", &pinned));
    assert_eq!(pinned.final_turn_ref, Some(StationRef::new(side, 1)));
    assert!(route_of(&pinned).contains(&StationRef::new(side, 1)));
    assert_eq!(pinned.stage_drop_refs, vec![side_end]);
    assert!(!reached(&pinned, 0), "the drop is past the turnaround");
    assert_eq!(pinned.stage_drop_over_reserve, vec![false]);
    assert!((pinned.budget_litres - pin_free.budget_litres).abs() < 1e-9);
    // D4: the dive never passes the pin, so every station but the way to the turn is priced after
    // a pin it cannot afford - the overlay is the route out to side 1 and nothing else (the
    // pin-free plan also lit the trunk to trunk 3), and there is no station left to end at.
    assert_eq!(
        reach_of(&pinned),
        vec![origin, StationRef::new(side, 0), StationRef::new(side, 1)],
        "the overlay is the way to the turn"
    );
    assert!(pinned.turn_frontier.is_empty());
    assert_within_pin_free(&pinned, &pin_free);
    // Nothing to profile past the turnaround: the drop gets no profile point.
    assert!(pinned
        .route_profile
        .iter()
        .all(|p| p.route_index != pinned.stage_drop_route_indices[0]));
}

#[test]
fn a_shallow_pin_the_scooter_cannot_reach_keeps_its_gas_stranded() {
    // D3, "no stranding forgiven". A scootered dive (540 m of usable range) with the drop pinned at
    // trunk 6 (600 m in, 240 L at 50 m/min - far shallower than the stage's safe gas). The range
    // turns the dive at trunk 5; the stage is never dropped, and the gas the pin would strand stays
    // out of the budget: a diver planning to leave the stage at trunk 6 does not breathe it past
    // there either.
    let (project, origin, trunk_end, _side_end) = two_branch_cave();
    let trunk = trunk_end.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        dpv: Some(scooter()),
        ..GasConfig::default()
    };
    let segments = [PlanSegment {
        speed_m_per_min: Some(50.0),
        on_scooter: true,
    }];
    let rig = [
        cyl(12.0, 232.0, 50.0, CylRole::BackGas),
        cyl(11.0, 207.0, 207.0, CylRole::Stage),
    ];
    let field = |pins: &[Option<StationRef>]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            Some(&trunk_end),
            &[],
            &segments,
            &diver,
            &rig,
            &cfg,
            pins,
        )
    };
    let pin_free = field(&[]);
    let pinned = field(&[Some(trunk_end)]);
    println!("D3-range {}", describe("pin-free", &pin_free));
    println!("D3-range {}", describe("pinned trunk:6", &pinned));
    assert_eq!(pinned.final_turn_ref, Some(StationRef::new(trunk, 5)));
    assert!(pinned.turn_on_scooter_range);
    assert!(!reached(&pinned, 0));
    let arrival = gas_of(&pin_free, trunk_end);
    assert!(
        (pinned.budget_litres - (12.0 * 50.0 / 4.0 + arrival)).abs() < 1e-6,
        "budget {:.2} L: the stage is credited only the {arrival:.1} L to its pin",
        pinned.budget_litres
    );
    assert!(pinned.budget_litres < pin_free.budget_litres);
    assert_within_pin_free(&pinned, &pin_free);
}

#[test]
fn two_pins_on_different_branches_are_visited_in_drop_order() {
    // TwoStage, drop 1 pinned at trunk 2, drop 2 at side 1: out to trunk 2, back to the junction,
    // out to side 1 (400 + 400 + 410 = 1210 L of route), then on beyond the second pin.
    let (project, origin, trunk_end, side_end) = two_branch_cave();
    let (trunk, side) = (trunk_end.section_id, side_end.section_id);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::TwoStage,
        ..GasConfig::default()
    };
    let rig = [
        cyl(24.0, 232.0, 232.0, CylRole::BackGas),
        cyl(11.0, 207.0, 207.0, CylRole::Stage),
        cyl(11.0, 207.0, 207.0, CylRole::Stage),
    ];
    let pins = [
        Some(StationRef::new(trunk, 2)),
        Some(StationRef::new(side, 1)),
    ];
    let field = |pins: &[Option<StationRef>]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            None,
            &[],
            &[],
            &diver,
            &rig,
            &cfg,
            pins,
        )
    };
    let pin_free = field(&[]);
    let pinned = field(&pins);
    println!("TWO-PINS {}", describe("pin-free", &pin_free));
    println!("TWO-PINS {}", describe("pinned trunk:2, side:1", &pinned));
    let route = route_of(&pinned);
    let first = route.iter().position(|s| Some(*s) == pins[0]).unwrap();
    let second = route.iter().position(|s| Some(*s) == pins[1]).unwrap();
    assert!(first < second, "drop order");
    assert!(
        route[first..second].contains(&origin),
        "no backtrack between the pins"
    );
    assert_eq!(
        pinned.stage_drop_refs,
        vec![pins[0].unwrap(), pins[1].unwrap()]
    );
    assert!(reached(&pinned, 0) && reached(&pinned, 1));
    // Drop 2 breathes stage 2 from trunk 2 to side 1 - the 810 L backtrack-and-out is charged to
    // it - and drop 1 strands stage 1's gas past trunk 2: both inside their safe halves.
    let half =
        deliverable_gas(207.0, 11.0, 207.0, false) - deliverable_gas(118.5, 11.0, 207.0, false);
    assert!(810.0 < half);
    assert_eq!(pinned.stage_drop_over_reserve, vec![false, false]);
    let sixth = deliverable_gas(232.0, 24.0, 232.0, false)
        - deliverable_gas(232.0 - 232.0 / 6.0, 24.0, 232.0, false);
    assert!(
        (pinned.budget_litres - (sixth + 400.0 + 810.0)).abs() < 1e-6,
        "budget {:.2} L",
        pinned.budget_litres
    );
    assert_eq!(
        pinned.final_turn_ref,
        Some(side_end),
        "on beyond the last pin"
    );
    assert_within_pin_free(&pinned, &pin_free);
}

#[test]
fn a_pin_on_a_default_clamped_to_the_turnaround_is_priced_not_forgiven() {
    // RV8c-2 (b), scenario 10172's mechanism. Endpoint at the trunk end, out of budget: the plan
    // turns at trunk 4 (800 L), and the stage's threshold lies past it, so its default drop is
    // clamped onto trunk 4. A pin there used to count as "the default" and kept the whole stage's
    // credit. The stage is dropped at trunk 4 with gas still in it, so the plan has the 800 L to
    // trunk 4 plus the back gas's quarter.
    let (project, origin, trunk_end, _side_end) = two_branch_cave();
    let trunk = trunk_end.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        ..GasConfig::default()
    };
    let rig = [
        cyl(12.0, 232.0, 20.0, CylRole::BackGas),
        cyl(11.0, 207.0, 207.0, CylRole::Stage),
    ];
    let field = |pins: &[Option<StationRef>]| {
        project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            Some(&trunk_end),
            &[],
            &[],
            &diver,
            &rig,
            &cfg,
            pins,
        )
    };
    let pin_free = field(&[]);
    let turn = StationRef::new(trunk, 4);
    assert_eq!(pin_free.final_turn_ref, Some(turn));
    assert_eq!(
        pin_free.stage_drop_refs,
        vec![turn],
        "the default clamped to the turn"
    );
    let pinned = field(&[Some(turn)]);
    println!("CLAMPED {}", describe("pin-free", &pin_free));
    println!("CLAMPED {}", describe("pinned trunk:4", &pinned));
    let has = 800.0 + 12.0 * 20.0 / 4.0;
    assert!(
        (pinned.budget_litres - has).abs() < 1e-6,
        "budget {:.2} L, a diver dropping at trunk 4 has {has:.2} L",
        pinned.budget_litres
    );
    assert_eq!(pinned.final_turn_ref, Some(turn));
    assert_within_pin_free(&pinned, &pin_free);
}

/// A north line of 3 x 100 m and an east line of 3 x 130 m joined at the entrance: the project,
/// the entrance, both section ids, the north tip and east 1.
fn north_east_short() -> (Project, StationRef, u32, u32, StationRef, StationRef) {
    let mut project = Project::new("Gas".to_string(), None);
    let north = project
        .add_section(SectionData {
            shots: (0..3).map(|_| shot(100.0, 0.0, 0.0)).collect(),
            ..Default::default()
        })
        .id;
    let east = project
        .add_section(SectionData {
            shots: (0..3).map(|_| shot(130.0, 0.0, 0.0)).collect(),
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

#[test]
fn an_off_route_pin_takes_the_dive_through_it_first() {
    // #576 RV8c-2-D2. A stage drop is a position along the route, and the UI only ever writes route
    // stations - but `.plnx` PERSISTS overrides, so a plan reloaded after the survey, the endpoint
    // or the way points moved can carry a pin the new route does not pass. The pin decides the
    // route: out to it first, then on to the endpoint, the backtrack priced like any other passage,
    // and the stage credited only the gas breathed to the pin. Before #576 such a pin was scored
    // from the station's own gas and EXTENDED the reach; NF-PGAS3-1 then ignored it.
    let (project, entrance, north, _east, north_tip, east_mid) = north_east_short();
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
    let route = route_of(&baseline);
    assert!(route.contains(&north_tip) && !route.contains(&east_mid));
    assert_eq!(baseline.stage_drop_refs.len(), 1);

    let pinned = plan_north(&[Some(east_mid)]);
    let route = route_of(&pinned);
    let at_pin = route.iter().position(|r| *r == east_mid).unwrap();
    assert!(
        route[at_pin..].contains(&entrance),
        "no backtrack to the junction"
    );
    let turn = pinned.final_turn_ref.unwrap();
    assert_eq!(
        turn.section_id, north,
        "the dive turns heading for the north tip"
    );
    assert!(pinned.turn_toward_endpoint);
    assert_eq!(pinned.stage_drop_refs, vec![east_mid]);
    assert!(!pinned.stage_drop_over_reserve[0]);
    // The stage is credited only the gas to east 1: the rest of its safe half is stranded there.
    let to_pin = gas_of(&baseline, east_mid);
    let stranded = baseline.budget_litres - pinned.budget_litres;
    assert!(stranded > 0.0 && stranded + to_pin < baseline.budget_litres);
    for (i, (&with, &without)) in pinned.reachable.iter().zip(&baseline.reachable).enumerate() {
        assert!(
            !with || without,
            "the pin made {:?} reachable",
            pinned.station_refs[i]
        );
    }
}

#[test]
fn a_pin_past_the_end_of_the_route_turns_the_dive_on_the_way_there() {
    // #576 NF-PGAS-1's independent-review case, under RV8c-2-D3. Free, back gas 24 L at 90 bar with
    // 2 bar usable (48 L), an 11 L stage rated 207 filled to 300 with 80 bar usable, no endpoint:
    // the plan turns at north 3 (697.64 L). A drop left pinned at north 4/5/6, past that turn, was
    // once priced from the passage beyond it (848 / 1048 / 1248 L credited). Now the dive heads for
    // the pin, turns where the gas says (north 3 again), carries the stage, and reports the drop at
    // its pin as not reached - same budget, same reach, no over-reserve warning.
    let (project, origin, trunk_end, _) = two_branch_cave();
    let north = trunk_end.section_id;
    let diver = Diver::new(20.0, None).unwrap();
    let rig = [
        Cylinder {
            usable_gas_bar: 2.0,
            ..Cylinder::new(
                24.0,
                232.0,
                90.0,
                TankMaterial::Steel,
                CylRole::BackGas,
                None,
            )
            .unwrap()
        },
        Cylinder {
            usable_gas_bar: 80.0,
            ..Cylinder::new(
                11.0,
                207.0,
                300.0,
                TankMaterial::Aluminium,
                CylRole::Stage,
                None,
            )
            .unwrap()
        },
    ];
    let cfg = GasConfig {
        default_reserve: ReservePreset::Free,
        ..GasConfig::default()
    };
    let plain = project.compute_gas_reach_field(Some(&origin), None, &diver, &rig, &cfg);
    assert_eq!(plain.final_turn_ref, Some(StationRef::new(north, 3)));
    assert!((plain.budget_litres - 697.642).abs() < 1e-3);
    for station in 4..=6 {
        let pin = StationRef::new(north, station);
        let f = project.compute_gas_reach_field_with_overrides(
            Some(&origin),
            None,
            &[],
            &[],
            &diver,
            &rig,
            &cfg,
            &[Some(pin)],
        );
        let why = format!("pin at north {station}: budget {:.2} L", f.budget_litres);
        assert!(
            (f.budget_litres - plain.budget_litres).abs() < 1e-9,
            "{why}"
        );
        assert_eq!(f.final_turn_ref, plain.final_turn_ref, "{why}");
        assert_eq!(f.route, plain.route, "{why}");
        // D4: only the way to the turn is painted - the side line the plain plan also lit is
        // priced after a pin the dive never reaches.
        let way_to_turn: Vec<StationRef> = (0..=3).map(|i| StationRef::new(north, i)).collect();
        assert_eq!(
            reach_of(&f)
                .into_iter()
                .filter(|s| s.section_id == north)
                .collect::<Vec<_>>(),
            way_to_turn,
            "{why}"
        );
        assert!(
            reach_of(&f)
                .iter()
                .all(|s| s.section_id == north || s.station_index == 0),
            "{why}: a side station is painted"
        );
        assert_within_pin_free(&f, &plain);
        assert_eq!(f.stage_drop_refs, vec![pin], "{why}");
        assert!(
            f.stage_drop_route_indices[0] as usize > f.route.len(),
            "{why}: reached"
        );
        assert_eq!(f.stage_drop_over_reserve, vec![false], "{why}");
    }
}

#[test]
fn two_drops_pinned_on_one_station_still_carry_on_beyond_it() {
    // TwoStage, both drops pinned at the trunk end (a dead end, 1200 L in). Drop 2 is passed on
    // the way to drop 1 - the same station - so it is dropped there full, and the dive goes no
    // further than the passage beyond that pin: it turns at the trunk end. Given a zero-length leg
    // of its own, "beyond" the second pin would be every direction, and the dive would double back
    // down the trunk as the deepest station "through" it.
    let (project, origin, trunk_end, _side_end) = two_branch_cave();
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        default_reserve: ReservePreset::TwoStage,
        ..GasConfig::default()
    };
    let rig = [
        cyl(12.0, 232.0, 232.0, CylRole::BackGas),
        cyl(12.0, 232.0, 232.0, CylRole::BackGas),
        cyl(11.0, 207.0, 207.0, CylRole::Stage),
        cyl(11.0, 207.0, 207.0, CylRole::Stage),
    ];
    let pinned = project.compute_gas_reach_field_with_overrides(
        Some(&origin),
        None,
        &[],
        &[],
        &diver,
        &rig,
        &cfg,
        &[Some(trunk_end), Some(trunk_end)],
    );
    println!(
        "SAME-STATION {}",
        describe("both pinned at trunk:6", &pinned)
    );
    assert!(pinned.budget_litres > 1200.0, "the trunk end is affordable");
    assert_eq!(pinned.final_turn_ref, Some(trunk_end));
    assert_eq!(route_of(&pinned).last(), Some(&trunk_end));
    assert_eq!(pinned.stage_drop_refs, vec![trunk_end, trunk_end]);
}

/// The independent review's seed 0x9a0058be (RV9a-1), rebuilt with lengths and depths as its
/// probe drew them, to the millimetre: a shallow seven-leg line 0:0..0:7, and a deep two-leg
/// passage that leaves it at 0:5 (merged with 1:0) and comes back to the entrance (1:2 tied to
/// 0:0 by a loop closure). So 0:5 is two swims away: 684 m along the line, shallow and the
/// cheaper gas, or 312 m round the deep loop. Returns the project, the line's id and the loop's.
fn rv9a_loop_cave() -> (Project, u32, u32) {
    let mut p = Project::new("Gas".to_string(), None);
    let line = p
        .add_section(SectionData {
            shots: vec![
                shot(65.363, 0.907, 8.441),
                shot(54.387, 8.441, 11.557),
                shot(186.697, 11.557, 4.145),
                shot(147.776, 4.145, 1.837),
                shot(230.089, 1.837, 0.509),
                shot(114.586, 0.509, 8.687),
                shot(124.955, 8.687, 7.812),
            ],
            ..Default::default()
        })
        .id;
    let deep = p
        .add_section(SectionData {
            shots: vec![shot(172.062, 25.740, 34.368), shot(140.259, 34.368, 43.000)],
            ..Default::default()
        })
        .id;
    p.merge_stations(StationRef::new(line, 5), StationRef::new(deep, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(deep, 2),
        StationRef::new(line, 0),
        ClosureStrategy::Batch,
    )
    .unwrap();
    (p, line, deep)
}

/// The seed's plan: OneStage under the conservative cap, the endpoint at 0:6, one scootered
/// segment at 16.787 m/min - below both units' cruise, so a metre of line costs 2.55 m of range -
/// and the review's rig. `range_reserve` is the DPV's reserve fraction (0.267 in the seed).
fn rv9a_plan(
    project: &Project,
    line: u32,
    range_reserve: f64,
    pins: &[Option<StationRef>],
) -> GasReachField {
    rv9a_plan_via(project, line, range_reserve, &[], pins)
}

/// [`rv9a_plan`] through `waypoints`, every span scootered at the seed's speed.
fn rv9a_plan_via(
    project: &Project,
    line: u32,
    range_reserve: f64,
    waypoints: &[StationRef],
    pins: &[Option<StationRef>],
) -> GasReachField {
    let mut cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        conservative_tank_gas: true,
        dpv: Some(DpvRig {
            primary: DpvUnit {
                speed_m_per_min: 26.712,
                burn_minutes: 75.836,
            },
            backup: DpvUnit {
                speed_m_per_min: 48.313,
                burn_minutes: 47.324,
            },
            range_reserve_fraction: range_reserve,
        }),
        ..GasConfig::default()
    };
    cfg.reserve_profile.stage_reserve_bar = 1.753;
    cfg.normalize();
    let rig = [
        cyl(16.373, 176.0, 91.946, CylRole::BackGas),
        cyl(16.565, 300.0, 272.541, CylRole::BackGas),
        cyl(21.398, 207.0, 121.556, CylRole::Stage),
        cyl(7.383, 176.0, 166.773, CylRole::Stage),
    ];
    let segments = vec![
        PlanSegment {
            speed_m_per_min: Some(16.787),
            on_scooter: true,
        };
        waypoints.len() + 1
    ];
    project.compute_gas_reach_field_with_overrides(
        Some(&StationRef::new(line, 0)),
        Some(&StationRef::new(line, 6)),
        waypoints,
        &segments,
        &Diver::new(28.044, None).unwrap(),
        &rig,
        &cfg,
        pins,
    )
}

/// The stations of `f`'s route up to and including its turnaround that the overlay leaves dark.
fn dark_on_route(f: &GasReachField) -> Vec<String> {
    route_of(f)
        .into_iter()
        .chain(f.final_turn_ref)
        .filter(|&s| !is_reachable(f, s))
        .map(name)
        .collect()
}

#[test]
fn rv9a_1_a_scootered_dive_pinned_round_a_loop_is_painted_to_its_turn() {
    // #576 RV9a-1, D4 with a scooter. The stage pinned at 1:1 sends the dive round the deep loop:
    // more gas than the line, far less range. Before D4 the pinned plan kept the masks of the plan
    // without pins, whose range then rode the gas-cheapest path - along the line, 1158 m of range
    // to 0:4 and out of range at 0:5 - so the dive turned at 0:5 (796 m of range on its own route)
    // on a station the map left dark, and passed 1:0 dark too. The overlay is now priced on the
    // pinned walk for the range as well as the gas: the route to the turn and the turn are
    // painted, and with less range the dive turns sooner and nothing past its range is painted.
    let (project, line, deep) = rv9a_loop_cave();
    let line_at = |i: u32| StationRef::new(line, i);
    let deep_at = |i: u32| StationRef::new(deep, i);
    let pins = [Some(deep_at(1))];
    let pinned = rv9a_plan(&project, line, 0.267, &pins);
    let pin_free = rv9a_plan(&project, line, 0.267, &[]);
    println!("RV9a-1 {}", describe("pinned 1:1", &pinned));
    println!(
        "RV9a-1   scooter {:.2} of {:.2} m, gas at 1:0 {:.1} L",
        pinned.scooter_distance_m,
        pinned.scooter_range_limit_m.unwrap(),
        gas_of(&pinned, deep_at(0))
    );
    println!("RV9a-1 {}", describe("pin-free", &pin_free));
    assert_eq!(
        route_of(&pinned),
        vec![line_at(0), deep_at(2), deep_at(1), deep_at(0), line_at(5)],
        "round the loop through the pin, on toward 0:6"
    );
    assert_eq!(pinned.final_turn_ref, Some(line_at(5)));
    assert!(pinned.scooter_distance_m <= pinned.scooter_range_limit_m.unwrap());
    assert!(
        dark_on_route(&pinned).is_empty(),
        "the pinned dive passes or turns on stations the map leaves dark: {:?}",
        dark_on_route(&pinned)
    );
    // The plan without pins used to price 0:5 along the line - on gas it could (1810 L against
    // 2524 L), on range it could not - so it turned at 0:4 and left 0:5 dark while the pinned plan
    // painted it. It now takes the way round the loop itself (#576 NF-W11GAS-1-D1, see
    // `nf_w11gas_1_...`), so the pinned overlay stays inside it, and on gas the pin never pays less.
    assert_eq!(pin_free.final_turn_ref, Some(line_at(6)));
    assert!(is_reachable(&pin_free, line_at(5)));
    assert_within_pin_free(&pinned, &pin_free);
    for (i, (with, without)) in pinned
        .gas_to_reach
        .iter()
        .zip(&pin_free.gas_to_reach)
        .enumerate()
    {
        if let (Some(with), Some(without)) = (with, without) {
            assert!(
                with + 1e-9 >= *without,
                "the pin priced {} below the plan without it",
                name(pinned.station_refs[i])
            );
        }
    }

    // Less range (a 70% reserve, 608 m): 1:1 is 358 m of range in, 1:0 796 m. The dive reaches its
    // pin and turns there on the range; 1:0 and 0:5 are affordable on gas but out of range along
    // the pinned walk, so they stay dark.
    let short = rv9a_plan(&project, line, 0.7, &pins);
    println!(
        "RV9a-1 {}",
        describe("pinned 1:1, 70% range reserve", &short)
    );
    assert_eq!(short.final_turn_ref, Some(deep_at(1)));
    assert!(short.turn_on_scooter_range);
    assert!(dark_on_route(&short).is_empty());
    for s in [deep_at(0), line_at(5)] {
        assert!(
            gas_of(&short, s) <= short.budget_litres,
            "{} on gas",
            name(s)
        );
        assert!(!is_reachable(&short, s), "{} painted out of range", name(s));
    }
}

/// Metres of range one metre of line costs on the seed's rig at 16.787 m/min, from the rig's
/// numbers.
fn rv9a_range_per_metre(f: &GasReachField) -> f64 {
    assert!(f.scooter_range_limit_m.is_some());
    let rig = DpvRig {
        primary: DpvUnit {
            speed_m_per_min: 26.712,
            burn_minutes: 75.836,
        },
        backup: DpvUnit {
            speed_m_per_min: 48.313,
            burn_minutes: 47.324,
        },
        range_reserve_fraction: 0.0,
    };
    let per_metre = range_per_metre_of(&rig, 16.787);
    assert!((per_metre - 2.55).abs() < 0.01, "the seed's 2.55 m a metre");
    per_metre
}

/// Route gas at the turnaround: the last outbound point of the profile.
fn turn_gas_of(f: &GasReachField) -> f64 {
    f.route_profile
        .iter()
        .rev()
        .find(|p| p.outbound)
        .map(|p| p.gas_l)
        .unwrap()
}

#[test]
fn nf_w11gas_1_a_scootered_dive_without_pins_takes_the_way_round_the_loop_its_range_covers() {
    // #576 NF-W11GAS-1, decided D1 "price along the route": the plan without pins takes a route
    // its scooter can fly, and prices the range (and the gas) along it, as the pinned plan does
    // along its walk. The seed's endpoint 0:6 is two ways from the entrance: along the shallow
    // line, the cheaper gas but 684 m of line to 0:5 (1744 m of range, out of range), or round the
    // deep loop, 312 m to 0:5 and 427 m to 0:6 - more gas, within both the gas and the range.
    // Before, the plan priced the range along the cheaper gas only and turned at 0:4 on range.
    let (project, line, deep) = rv9a_loop_cave();
    let line_at = |i: u32| StationRef::new(line, i);
    let deep_at = |i: u32| StationRef::new(deep, i);
    let pin_free = rv9a_plan(&project, line, 0.267, &[]);
    let limit = pin_free.scooter_range_limit_m.unwrap();
    let per_metre = rv9a_range_per_metre(&pin_free);
    let line_m = route_line_m(&project, &pin_free);
    let along_route = line_m * per_metre;
    println!("NF-W11GAS-1 {}", describe("pin-free", &pin_free));
    println!(
        "NF-W11GAS-1   turn gas {:.1} of {:.2} L; range at the turn {:.2} of {limit:.2} m \
         (route line {line_m:.3} m x {per_metre:.4} = {along_route:.2} m); range-bound {}",
        pin_free
            .route_profile
            .iter()
            .rev()
            .find(|p| p.outbound)
            .map_or(f64::NAN, |p| p.gas_l),
        pin_free.budget_litres,
        pin_free.scooter_distance_m,
        pin_free.turn_on_scooter_range,
    );
    assert_eq!(
        route_of(&pin_free),
        vec![
            line_at(0),
            deep_at(2),
            deep_at(1),
            deep_at(0),
            line_at(5),
            line_at(6)
        ],
        "round the loop to the endpoint"
    );
    assert_eq!(pin_free.final_turn_ref, Some(line_at(6)));
    assert!(pin_free.turn_toward_endpoint);
    assert!(!pin_free.turn_on_scooter_range, "the endpoint is reached");
    // The range used along the route, as the engine reports it and as worked out from the route's
    // own shots, is within the rig's usable range (its reserve taken off).
    assert!((pin_free.scooter_distance_m - along_route).abs() < 1e-6 * along_route);
    assert!(pin_free.scooter_distance_m <= limit);
    assert!(turn_gas_of(&pin_free) <= pin_free.budget_litres);
    assert!(dark_on_route(&pin_free).is_empty());
    // Every station round the loop is painted and priced at what the loop costs to it; the same
    // plan with no rig (no range cap) prices 0:5 and 0:6 along the line, on less gas.
    let mut no_rig_cfg = GasConfig {
        default_reserve: ReservePreset::OneStage,
        conservative_tank_gas: true,
        ..GasConfig::default()
    };
    no_rig_cfg.reserve_profile.stage_reserve_bar = 1.753;
    no_rig_cfg.normalize();
    let gas_only = project.compute_gas_reach_field_with_overrides(
        Some(&line_at(0)),
        Some(&line_at(6)),
        &[],
        &[PlanSegment {
            speed_m_per_min: Some(16.787),
            on_scooter: true,
        }],
        &Diver::new(28.044, None).unwrap(),
        &[
            cyl(16.373, 176.0, 91.946, CylRole::BackGas),
            cyl(16.565, 300.0, 272.541, CylRole::BackGas),
            cyl(21.398, 207.0, 121.556, CylRole::Stage),
            cyl(7.383, 176.0, 166.773, CylRole::Stage),
        ],
        &no_rig_cfg,
        &[],
    );
    assert!((gas_only.budget_litres - pin_free.budget_litres).abs() < 1e-9);
    for s in [deep_at(1), deep_at(0), line_at(5), line_at(6)] {
        assert!(is_reachable(&pin_free, s), "{} dark", name(s));
        assert!(gas_of(&pin_free, s) <= pin_free.budget_litres);
    }
    for s in [line_at(5), line_at(6)] {
        assert!(
            gas_of(&pin_free, s) > gas_of(&gas_only, s) + 1.0,
            "{} priced along the loop, not the line",
            name(s)
        );
    }
    assert!((gas_of(&pin_free, line_at(6)) - turn_gas_of(&pin_free)).abs() < 1e-9);

    // With less range (a 70% reserve, 608 m) no way to 0:6 is in range. The plan heads for it
    // along the line, as before, and turns where the range runs out there, at 0:2 (306 m); the
    // loop's 1:1 (358 m) is painted, 1:0 (796 m) is not.
    let short = rv9a_plan(&project, line, 0.7, &[]);
    println!(
        "NF-W11GAS-1 {}",
        describe("pin-free, 70% range reserve", &short)
    );
    assert_eq!(short.final_turn_ref, Some(line_at(2)));
    assert!(short.turn_on_scooter_range);
    assert!(short.scooter_distance_m <= short.scooter_range_limit_m.unwrap());
    assert!(is_reachable(&short, deep_at(1)));
    assert!(!is_reachable(&short, deep_at(0)));
    assert!(dark_on_route(&short).is_empty());
}

/// What `f` prints for the NF-W12GAS-1 evidence: its route and turn, the range at the turn as the
/// field reports it and as worked out from the route's shots, and the gas at the turn.
fn describe_range(label: &str, project: &Project, f: &GasReachField) -> String {
    let line_m = route_line_m(project, f);
    let per_metre = rv9a_range_per_metre(f);
    format!(
        "{}; turn gas {:.1} of {:.2} L, range at the turn {:.2} of {:.2} m (route line {line_m:.3} \
         m x {per_metre:.4} = {:.2} m), range-bound {}, way point gas {:?}",
        describe(label, f),
        f.route_profile
            .iter()
            .rev()
            .find(|p| p.outbound)
            .map_or(f64::NAN, |p| p.gas_l),
        f.budget_litres,
        f.scooter_distance_m,
        f.scooter_range_limit_m.unwrap(),
        line_m * per_metre,
        f.turn_on_scooter_range,
        f.waypoint_gas_litres
    )
}

#[test]
fn nf_w12gas_1_a_way_point_leg_round_a_loop_takes_the_way_its_range_covers() {
    // #576 NF-W12GAS-1, decided D2 "extend to all legs": the seed's dive with a way point at 0:5,
    // both spans scootered. The leg to it is two ways from the entrance: along the shallow line,
    // the cheaper gas but 684 m of line (1744 m of range, out of range), or round the deep loop,
    // 312 m. The open tail already took the way round (NF-W11GAS-1); the leg to a way point kept
    // the cheaper gas, so the dive ran out of range on its way to 0:5 and had no turnaround at all.
    // The leg now takes the cheapest gas its range covers: round the loop, to the way point and on
    // to 0:6 - the same dive as without the way point.
    let (project, line, deep) = rv9a_loop_cave();
    let line_at = |i: u32| StationRef::new(line, i);
    let deep_at = |i: u32| StationRef::new(deep, i);
    let via = rv9a_plan_via(&project, line, 0.267, &[line_at(5)], &[]);
    let direct = rv9a_plan(&project, line, 0.267, &[]);
    println!(
        "NF-W12GAS-1 {}",
        describe_range("way point 0:5", &project, &via)
    );
    println!(
        "NF-W12GAS-1 {}",
        describe_range("no way point", &project, &direct)
    );
    assert_eq!(
        route_of(&via),
        vec![
            line_at(0),
            deep_at(2),
            deep_at(1),
            deep_at(0),
            line_at(5),
            line_at(6)
        ],
        "round the loop to the way point, then on to the endpoint"
    );
    assert_eq!(via.final_turn_ref, Some(line_at(6)));
    assert!(via.turn_toward_endpoint && !via.turn_on_scooter_range);
    // Priced along the way round: the way point costs what the loop costs to it, and the dive is
    // the plan's without the way point, gas and range.
    assert!((via.waypoint_gas_litres[0].unwrap() - gas_of(&direct, line_at(5))).abs() < 1e-9);
    assert!((turn_gas_of(&via) - turn_gas_of(&direct)).abs() < 1e-9);
    assert!((via.scooter_distance_m - direct.scooter_distance_m).abs() < 1e-9);
    let along_route = route_line_m(&project, &via) * rv9a_range_per_metre(&via);
    assert!((via.scooter_distance_m - along_route).abs() < 1e-6 * along_route);
    assert!(via.scooter_distance_m <= via.scooter_range_limit_m.unwrap());
    assert!(turn_gas_of(&via) <= via.budget_litres);
    assert!(dark_on_route(&via).is_empty());

    // With less range (a 70% reserve, 608 m) no way to 0:5 is in range - the loop is 796 m of
    // range to it - so the leg keeps the cheaper gas and the dive cannot fly the way point: no
    // turnaround, as before, and nothing past the range painted.
    let short = rv9a_plan_via(&project, line, 0.7, &[line_at(5)], &[]);
    println!(
        "NF-W12GAS-1 {}",
        describe("way point 0:5, 70% range reserve", &short)
    );
    assert_eq!(short.final_turn_ref, None);
    for s in [deep_at(0), line_at(5), line_at(6)] {
        assert!(!is_reachable(&short, s), "{} painted out of range", name(s));
    }
}

#[test]
fn nf_w12gas_1_a_pin_leg_round_a_loop_takes_the_way_its_range_covers() {
    // #576 NF-W12GAS-1, decided D2: the seed's dive with its stage pinned at 0:5. The pinned walk
    // flies a leg to the pin like a way point's, and kept the cheaper gas too - along the line -
    // so the dive turned at 0:4 where the range ran out (D3) and never reached its pin, while the
    // plan without pins went round the loop to 0:6. The pin leg now takes the cheapest gas its
    // range covers, round the loop: the dive reaches the pin and goes on to the endpoint.
    let (project, line, deep) = rv9a_loop_cave();
    let line_at = |i: u32| StationRef::new(line, i);
    let deep_at = |i: u32| StationRef::new(deep, i);
    let pins = [Some(line_at(5))];
    let pinned = rv9a_plan(&project, line, 0.267, &pins);
    let pin_free = rv9a_plan(&project, line, 0.267, &[]);
    println!(
        "NF-W12GAS-1 {}",
        describe_range("pinned 0:5", &project, &pinned)
    );
    println!(
        "NF-W12GAS-1 {}",
        describe_range("pin-free", &project, &pin_free)
    );
    println!(
        "NF-W12GAS-1   drop bars {:?}, route indices {:?}, over {:?}",
        pinned.stage_drop_bars, pinned.stage_drop_route_indices, pinned.stage_drop_over_reserve
    );
    assert_eq!(
        route_of(&pinned),
        vec![
            line_at(0),
            deep_at(2),
            deep_at(1),
            deep_at(0),
            line_at(5),
            line_at(6)
        ],
        "round the loop through the pin, then on to the endpoint"
    );
    assert_eq!(pinned.final_turn_ref, Some(line_at(6)));
    assert!(reached(&pinned, 0), "the dive reaches its pin");
    // The pin is deeper than the stage's default drop: shown over reserve, no reach bought - the
    // budget, and here the whole dive, are the plan's without the pin.
    assert_eq!(pinned.stage_drop_over_reserve, vec![true]);
    assert!((pinned.budget_litres - pin_free.budget_litres).abs() < 1e-9);
    assert!((turn_gas_of(&pinned) - turn_gas_of(&pin_free)).abs() < 1e-9);
    let along_route = route_line_m(&project, &pinned) * rv9a_range_per_metre(&pinned);
    assert!((pinned.scooter_distance_m - along_route).abs() < 1e-6 * along_route);
    assert!(pinned.scooter_distance_m <= pinned.scooter_range_limit_m.unwrap());
    assert!(dark_on_route(&pinned).is_empty());
    assert_within_pin_free(&pinned, &pin_free);

    // With less range (a 70% reserve, 608 m) no way to the pin is in range: the leg keeps the
    // cheaper gas, along the line, and the dive turns where the range runs out on it, short of the
    // pin, which it reports not reached (D3) - as before.
    let short = rv9a_plan(&project, line, 0.7, &pins);
    println!(
        "NF-W12GAS-1 {}",
        describe("pinned 0:5, 70% range reserve", &short)
    );
    assert!(!reached(&short, 0), "no way to the pin is in range");
    assert!(short.turn_on_scooter_range);
    assert!(route_of(&short).iter().all(|s| s.section_id == line));
    assert!(short.scooter_distance_m <= short.scooter_range_limit_m.unwrap());
    assert!(dark_on_route(&short).is_empty());
}

#[test]
fn nf_w12gas2_1_a_drop_pinned_on_a_merged_station_is_reported_at_the_pin() {
    // #576 NF-W12GAS2-1, decided D1 "the station the user pinned": 0:5 is merged with 1:0, and
    // the route round the loop passes 1:0 and then 0:5 at the same gas (the merge costs nothing).
    // The drop was placed at the first route station whose gas reaches the pin's threshold - the
    // twin 1:0 - so the panel, the map marker and the profile/slate named a station the diver did
    // not pick. It is now reported at 0:5, the pinned station, on the pass the route makes there.
    let (project, line, deep) = rv9a_loop_cave();
    let line_at = |i: u32| StationRef::new(line, i);
    let deep_at = |i: u32| StationRef::new(deep, i);
    let pinned = rv9a_plan(&project, line, 0.267, &[Some(line_at(5))]);
    let route = route_of(&pinned);
    let profile_names: Vec<String> = pinned
        .stage_drop_route_indices
        .iter()
        .filter_map(|&j| {
            pinned
                .route_profile
                .iter()
                .find(|p| p.outbound && p.route_index == j)
        })
        .map(|p| name(p.station_ref))
        .collect();
    println!("NF-W12GAS2-1 {}", describe("pinned 0:5", &pinned));
    println!(
        "NF-W12GAS2-1   drop route indices {:?}, profile station at the drop {:?}",
        pinned.stage_drop_route_indices, profile_names
    );
    let twin = route.iter().position(|&s| s == deep_at(0)).unwrap();
    let pin = route.iter().position(|&s| s == line_at(5)).unwrap();
    assert_eq!(
        twin + 1,
        pin,
        "precondition: the route passes the twin, then the pin"
    );
    let gas_at = |i: usize| {
        pinned
            .route_profile
            .iter()
            .find(|p| p.outbound && p.route_index as usize == i)
            .map(|p| p.gas_l)
            .unwrap()
    };
    assert!(
        (gas_at(twin) - gas_at(pin)).abs() < 1e-12,
        "precondition: the twin and the pin cost the same gas"
    );
    assert_eq!(
        pinned.stage_drop_refs,
        vec![line_at(5)],
        "the drop names the pinned station"
    );
    assert_eq!(
        pinned.stage_drop_route_indices,
        vec![u32::try_from(pin).unwrap()],
        "the drop sits on the pinned station's pass"
    );
    assert_eq!(
        profile_names,
        vec![name(line_at(5))],
        "the profile marker names the pin"
    );
    // Same physical point as the twin: still reached, still over reserve.
    assert!(reached(&pinned, 0));
    assert_eq!(pinned.stage_drop_over_reserve, vec![true]);
}
