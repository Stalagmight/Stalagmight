//! RS 6 (plan 2026-11-23): Plan mode's route, the ruler's path and the turn
//! and drop lists name each station once across a loop closure that shows
//! its two ends as one station (D18's "each turn station once").
//!
//! The survey: A runs east 4 x 10 m (A0..A4); B leaves A0 north 10 m, east
//! 40 m and south 10 m (B1..B3), ending on A4's spot. A closure B3 - A4
//! keeping A4 shows B3 as A4 (B3 is a loop end). The short way from A3 to B2
//! crosses the closure tie: A3, A4, (tie) B3, B2, 20 m.
//!
//! Injections (each fails on 333e24ac5, before the collapse):
//! - no collapse in the engine: `a_route_lists_a_loop_end_once`,
//!   `the_profile_has_no_zero_length_point_at_a_loop_end`,
//!   `a_way_point_on_a_loop_end_indexes_the_collapsed_route`,
//!   `the_wire_route_has_no_tie_leg_at_a_loop_end`;
//! - no dedupe of the frontier: `the_turn_frontier_lists_a_loop_end_once`;
//! - no canonical drop override: `a_drop_pinned_on_the_loop_end_of_its_default_station_is_a_no_op`;
//! - no collapse in the ruler: `the_ruler_path_lists_a_loop_end_once`.
//!
//! A closure that keeps both names collapses nothing (the control).
//! Fully synthetic data.

use crate::config::{GasConfig, ReservePreset};
use crate::gas::{CylRole, Cylinder, Diver, GasReachField, TankMaterial};
use crate::{LineId, StationId, StationRef};

use super::super::ops_test_helpers::{assert_valid, branch, free_line, ok, run};
use super::super::test_helpers::{at, flat, project};
use super::super::{ClosureStrategy, Project, RulerPoint};

/// The loop survey above; `keep` is the closure's kept end (`None`: keep
/// both names). Returns the project and the lines A and B.
fn loop_survey(keep_a4: bool) -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 4, 10.0));
    let a0 = at(&p, a, 0);
    let b = branch(
        &mut p,
        a0,
        vec![flat(0.0, 10.0), flat(90.0, 40.0), flat(180.0, 10.0)],
    );
    let (a4, b3) = (at(&p, a, 4), at(&p, b, 3));
    let keep = keep_a4.then_some(a4);
    ok(p.create_closure(
        b3,
        Some(b),
        a4,
        Some(a),
        ClosureStrategy::Batch,
        keep,
        &[],
    ));
    assert_valid(&p);
    assert_eq!(p.shown(b3) == a4, keep_a4, "B3 is a loop end only when A4 is kept");
    (p, a, b)
}

fn diver() -> Diver {
    Diver::new(20.0, None).unwrap()
}

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

/// The plain plan from `origin` to `endpoint` with way points `waypoints`.
fn plan(
    p: &Project,
    origin: StationId,
    endpoint: StationId,
    waypoints: &[Option<StationId>],
) -> super::GasReach {
    p.gas_reach(
        Some(origin),
        Some(endpoint),
        waypoints,
        &[],
        &diver(),
        &[unlimited_backgas()],
        &GasConfig::default(),
        &[],
    )
}

fn slot(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

/// RS 6: the route lists the loop end and its kept end as one station; the
/// tie leg between them is gone, and the leg after it still starts on the
/// loop end's own node.
#[test]
fn a_route_lists_a_loop_end_once() {
    let (p, a, b) = loop_survey(true);
    let (a3, a4, b2, b3) = (at(&p, a, 3), at(&p, a, 4), at(&p, b, 2), at(&p, b, 3));
    let reach = plan(&p, a3, b2, &[]);
    assert_eq!(reach.route, vec![a3, a4, b2], "A4 and its loop end B3 are one station");
    assert_eq!(reach.route_shots.len(), 2, "one leg per pair of route stations");
    assert!(
        reach.route_shots.iter().all(Option::is_some),
        "no closure tie leg is left between one station's two nodes: {:?}",
        reach.route_shots
    );
    assert_eq!(reach.route_ends, vec![[a3, a3], [a4, b3], [b2, b2]]);
    assert_eq!(reach.final_turn, Some(b2));
    assert!((reach.route_line_length_m - 20.0).abs() < 1e-9);
}

/// The control: a closure keeping both names shows two stations, so the
/// route keeps both nodes and the tie between them.
#[test]
fn a_closure_keeping_both_names_lists_both_ends() {
    let (p, a, b) = loop_survey(false);
    let (a3, a4, b2, b3) = (at(&p, a, 3), at(&p, a, 4), at(&p, b, 2), at(&p, b, 3));
    let reach = plan(&p, a3, b2, &[]);
    assert_eq!(reach.route, vec![a3, a4, b3, b2]);
    assert_eq!(reach.route_shots.iter().filter(|s| s.is_none()).count(), 1);
    assert_eq!(reach.route_ends, vec![[a3, a3], [a4, a4], [b3, b3], [b2, b2]]);
}

/// RS 6 with X8: the dive profile has one point per station, so the
/// outbound distances strictly increase (no zero-length point at the loop
/// end), the series is `2n - 1` points long, and every number stays the
/// node route's (the tie cost nothing).
#[test]
fn the_profile_has_no_zero_length_point_at_a_loop_end() {
    let (p, a, b) = loop_survey(true);
    let (a3, b2) = (at(&p, a, 3), at(&p, b, 2));
    let field = plan(&p, a3, b2, &[]).to_wire(&p, &[]);
    let outbound: Vec<f64> = field
        .route_profile
        .iter()
        .filter(|q| q.outbound)
        .map(|q| q.distance_m)
        .collect();
    assert_eq!(outbound, vec![0.0, 10.0, 20.0], "{outbound:?}");
    assert_eq!(field.route_profile.len(), 5, "2n - 1 points for n = 3");
    let indices: Vec<u32> = field.route_profile.iter().map(|q| q.route_index).collect();
    assert_eq!(indices, vec![0, 1, 2, 1, 0]);
    let turns = field
        .route_profile
        .iter()
        .filter(|q| q.route_index == 2)
        .count();
    assert_eq!(turns, 1, "the turnaround once");
    let back = field.route_profile.last().unwrap();
    assert!((back.distance_m - 40.0).abs() < 1e-9);
}

/// RS 6: a way point on the loop end is placed on the collapsed route, so
/// the profile marker and the segment colours find it.
#[test]
fn a_way_point_on_a_loop_end_indexes_the_collapsed_route() {
    let (p, a, b) = loop_survey(true);
    let (a3, b2, b3) = (at(&p, a, 3), at(&p, b, 2), at(&p, b, 3));
    let reach = plan(&p, a3, b2, &[Some(b3)]);
    assert_eq!(reach.route.len(), 3, "{:?}", reach.route);
    assert_eq!(reach.waypoint_route_indices, vec![Some(1)]);
    let field = reach.to_wire(&p, &[slot(b, 3)]);
    assert!(field
        .route_profile
        .iter()
        .any(|q| q.outbound && q.route_index == 1));
    assert_eq!(field.waypoint_route_indices, vec![Some(1)]);
}

/// The wire's legs (the map's route highlight): one swum leg per pair of
/// stations, each drawn between its own shot's slots; no tie leg from A4's
/// slot to B3's.
#[test]
fn the_wire_route_has_no_tie_leg_at_a_loop_end() {
    let (p, a, b) = loop_survey(true);
    let (a3, b2) = (at(&p, a, 3), at(&p, b, 2));
    let field: GasReachField = plan(&p, a3, b2, &[]).to_wire(&p, &[]);
    let legs: Vec<(StationRef, StationRef)> = field.route.iter().map(|l| (l.from, l.to)).collect();
    assert_eq!(
        legs,
        vec![(slot(a, 3), slot(a, 4)), (slot(b, 3), slot(b, 2))],
        "{legs:?}"
    );
    assert_eq!(field.final_turn_ref, Some(slot(b, 2)));
}

/// D18 under RS 6: the turn frontier lists each station once. From A4 with
/// no gas, A4 and its loop end B3 are both places the dive could end, both
/// border ground it cannot reach, and they are one station: one entry, the
/// kept end.
#[test]
fn the_turn_frontier_lists_a_loop_end_once() {
    let (p, a, _b) = loop_survey(true);
    let a4 = at(&p, a, 4);
    let mut off = unlimited_backgas();
    off.enabled = false;
    let reach = p.gas_reach(
        Some(a4),
        None,
        &[],
        &[],
        &diver(),
        &[off.clone()],
        &GasConfig::default(),
        &[],
    );
    assert_eq!(reach.budget_litres, 0.0);
    assert_eq!(reach.turn_frontier, vec![a4], "one entry for A4 and its loop end B3");
    let field = reach.to_wire(&p, &[]);
    assert_eq!(field.turn_frontier, vec![slot(a, 4)]);

    // The control: two names, two turn stations.
    let (q, qa, qb) = loop_survey(false);
    let reach = q.gas_reach(
        Some(at(&q, qa, 4)),
        None,
        &[],
        &[],
        &diver(),
        &[off],
        &GasConfig::default(),
        &[],
    );
    assert_eq!(reach.turn_frontier, vec![at(&q, qa, 4), at(&q, qb, 3)]);
}

/// D18 under RS 6: a drop pinned on another node of its default station (the
/// loop end the default drop's station shows across the tie) is the same
/// place, so it is a no-op like a release in place.
#[test]
fn a_drop_pinned_on_the_loop_end_of_its_default_station_is_a_no_op() {
    let (p, a, b) = loop_survey(true);
    let (a0, a4, b3) = (at(&p, a, 0), at(&p, a, 4), at(&p, b, 3));
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
    let run = |overrides: &[Option<StationId>]| {
        p.gas_reach(
            Some(a0),
            Some(b3),
            &[],
            &[],
            &diver(),
            &[bg.clone(), stage.clone()],
            &cfg,
            overrides,
        )
    };
    let plain = run(&[]);
    // The stage is not breathed down before the turn, so its drop clamps to
    // the turnaround: the station A4 / B3.
    assert_eq!(plain.final_turn, Some(b3));
    let drop = plain.stage_drop_route_indices[0];
    assert_eq!(plain.route_ends[drop], [a4, b3]);
    for pin in [a4, b3] {
        let pinned = run(&[Some(pin)]);
        assert_eq!(pinned.budget_litres, plain.budget_litres, "pin on {pin}");
        assert_eq!(pinned.stage_drop_bars, plain.stage_drop_bars, "pin on {pin}");
        assert_eq!(pinned.stage_drop_over_reserve, plain.stage_drop_over_reserve);
        assert_eq!(pinned.route, plain.route);
    }
}

/// RS 6: the ruler's path lists the loop end and its kept end as one
/// station. Its wire slots stay D19's (the two ends of each traversed shot,
/// for the tint), so B3's own point is tinted with the shot it ends.
#[test]
fn the_ruler_path_lists_a_loop_end_once() {
    let (p, a, b) = loop_survey(true);
    let (a3, a4, b2) = (at(&p, a, 3), at(&p, a, 4), at(&p, b, 2));
    let along = p.along_line_between(
        &p.raw_positions(),
        RulerPoint::Station(a3),
        RulerPoint::Station(b2),
        false,
    );
    assert!((along.along_line.unwrap() - 20.0).abs() < 1e-9);
    assert_eq!(along.path_stations, vec![a3, a4, b2]);
    assert_eq!(
        along.path_slots,
        vec![slot(a, 3), slot(a, 4), slot(b, 3), slot(b, 2)]
    );

    // Measured from the loop end to its kept end: one station, no shot.
    let b3 = at(&p, b, 3);
    let along = p.along_line_between(
        &p.raw_positions(),
        RulerPoint::Station(b3),
        RulerPoint::Station(a4),
        false,
    );
    assert_eq!(along.along_line, Some(0.0));
    assert_eq!(along.path_stations, vec![b3]);

    // The control: two names, two stations.
    let (q, qa, qb) = loop_survey(false);
    let along = q.along_line_between(
        &q.raw_positions(),
        RulerPoint::Station(at(&q, qa, 3)),
        RulerPoint::Station(at(&q, qb, 2)),
        false,
    );
    assert_eq!(along.path_stations.len(), 4);
}
