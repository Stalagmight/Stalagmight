//! Backend rules the Playwright IPC mock used to re-implement (#576 E3).
//!
//! The browser tier's mock (`e2e-playwright/mocks/tauri-ipc.ts`) once carried its own
//! copies of these rules, and specs asserted the mock's arithmetic. The mock now
//! answers canned replies, so the rules are pinned here, against the real model,
//! where nothing else did: the reverse leaves a NEIGHBOUR's anchor alone, a second
//! reverse puts the anchor back, a loop closure's endpoints follow their physical
//! stations through a reverse, and the gas readout says "on the way to the endpoint"
//! only when the turnaround really is.

#![expect(
    clippy::unwrap_used,
    reason = "test module: a failed unwrap is the failure report"
)]

use chrono::Utc;

use crate::config::{DpvRig, DpvUnit, GasConfig, ReservePreset};
use crate::test_fixtures::straight_shot;
use crate::{
    ClosureStrategy, CylRole, Cylinder, Diver, LoopClosure, PlanSegment, Project, SectionData,
    SectionId, ShotData, StationMerge, StationRef, TankMaterial,
};

fn add(project: &mut Project, shots: Vec<ShotData>) -> SectionId {
    project
        .add_section(SectionData {
            shots,
            ..Default::default()
        })
        .id
}

/// Reversing one section moves that section's anchor and nothing else's: the
/// mock kept `section_offsets` in two shapes and once wrote the anchor into the
/// wrong one, clobbering a neighbour. The model keys offsets by section id.
#[test]
fn a_reverse_moves_only_the_reversed_sections_anchor() {
    let mut project = Project::new("CaveA".to_string(), None);
    let a = add(
        &mut project,
        vec![straight_shot(90.0, 10.0), straight_shot(0.0, 5.0)],
    );
    let b = add(&mut project, vec![straight_shot(45.0, 7.0)]);
    project.ensure_shot_uuids();
    project.set_section_offset(a, (1.0, 2.0));
    project.set_section_offset(b, (3.0, 4.0));

    assert!(project.reverse_section(a, false));

    let (ax, ay) = project.section_offset(a);
    // Station 0 is now the far end: the anchor gained the section's span (10 m east, 5 m north).
    assert!(
        (ax - 11.0).abs() < 1e-9 && (ay - 7.0).abs() < 1e-9,
        "the reversed section's anchor should have moved by its span: ({ax}, {ay})"
    );
    assert_eq!(
        project.section_offset(b),
        (3.0, 4.0),
        "a neighbouring section's anchor must not move"
    );
}

/// The reverse is its own inverse for the anchor too: reversing twice puts the
/// section offset back where it was (the shots' own round trip is pinned in
/// `reverse.rs`; this is the half that lives outside the shot vec).
#[test]
fn reversing_twice_restores_the_section_anchor() {
    let mut project = Project::new("CaveA".to_string(), None);
    let a = add(
        &mut project,
        vec![straight_shot(30.0, 10.0), straight_shot(120.0, 6.0)],
    );
    project.ensure_shot_uuids();
    project.set_section_offset(a, (5.0, -3.0));

    assert!(project.reverse_section(a, false));
    let moved = project.section_offset(a);
    assert!(
        (moved.0 - 5.0).abs() > 1.0 || (moved.1 + 3.0).abs() > 1.0,
        "premise: one reverse moves the anchor, got {moved:?}"
    );
    assert!(project.reverse_section(a, false));

    let (x, y) = project.section_offset(a);
    assert!(
        (x - 5.0).abs() < 1e-9 && (y + 3.0).abs() < 1e-9,
        "two reverses must restore the anchor, got ({x}, {y})"
    );
}

/// A loop closure ending on the reversed section must still end on the same
/// PHYSICAL station afterwards: station `i` becomes station `shot_count - i`.
/// (Comments, datums and merges are pinned in `reverse.rs` and
/// `state/reverse_tests.rs`; closures go through the same re-key and had no
/// assertion of their own.)
#[test]
fn a_reverse_carries_a_loop_closure_to_the_mirrored_station() {
    let mut project = Project::new("CaveA".to_string(), None);
    let a = add(
        &mut project,
        vec![straight_shot(0.0, 10.0), straight_shot(0.0, 10.0)],
    );
    let b = add(&mut project, vec![straight_shot(180.0, 20.5)]);
    project.ensure_shot_uuids();
    project.station_merges.push(StationMerge {
        station_a: project.key_for_ref(StationRef::new(a, 2)).unwrap(),
        station_b: project.key_for_ref(StationRef::new(b, 0)).unwrap(),
        merged_at: Utc::now(),
    });
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: project.key_for_ref(StationRef::new(b, 1)).unwrap(),
        station_b: project.key_for_ref(StationRef::new(a, 0)).unwrap(),
        path_length: 40.5,
        misclosure_distance: 0.5,
        error_percentage: 1.2,
        horizontal_misclosure: 0.5,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    });

    assert!(project.reverse_section(a, false));

    let closure = &project.loop_closures[0];
    assert_eq!(
        project.resolve_key(&closure.station_b),
        Some(StationRef::new(a, 2)),
        "the closure's end on station a:0 must follow it to a:2"
    );
    assert_eq!(
        project.resolve_key(&closure.station_a),
        Some(StationRef::new(b, 1)),
        "the end on the untouched section stays put"
    );
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

fn descent(project: &mut Project) -> SectionId {
    add(
        project,
        (0..3)
            .map(|i| ShotData {
                length: 100.0,
                depth_in: f64::from(i) * 10.0,
                depth_out: f64::from(i + 1) * 10.0,
                ..Default::default()
            })
            .collect(),
    )
}

/// `turn_toward_endpoint` is what lets the readout say the dive turns "on the way
/// to" the diver's pick. It must be false for both deepest-anywhere fallbacks -
/// no endpoint at all, and an endpoint in another passage - and true when the
/// dive reaches the pick. Only the `true` side had an assertion.
#[test]
fn turn_toward_endpoint_is_false_for_both_deepest_fallbacks() {
    let mut project = Project::new("CaveA".to_string(), None);
    let sec = descent(&mut project);
    let island = add(&mut project, vec![straight_shot(0.0, 50.0)]);
    let entrance = StationRef::new(sec, 0);
    let diver = Diver::new(20.0, None).unwrap();
    let rig = [unlimited_backgas()];
    let cfg = GasConfig::default();

    let reached = project.compute_gas_reach_field(
        Some(&entrance),
        Some(&StationRef::new(sec, 2)),
        &diver,
        &rig,
        &cfg,
    );
    assert_eq!(reached.final_turn_ref, Some(StationRef::new(sec, 2)));
    assert!(
        reached.turn_toward_endpoint,
        "premise: the dive reaches the pick"
    );

    let no_endpoint = project.compute_gas_reach_field(Some(&entrance), None, &diver, &rig, &cfg);
    assert!(
        no_endpoint.final_turn_ref.is_some(),
        "the deepest station is the target"
    );
    assert!(
        !no_endpoint.turn_toward_endpoint,
        "no endpoint: the turnaround is the deepest station, not on the way to anything"
    );

    let marooned = project.compute_gas_reach_field(
        Some(&entrance),
        Some(&StationRef::new(island, 1)),
        &diver,
        &rig,
        &cfg,
    );
    assert!(marooned.final_turn_ref.is_some());
    assert!(
        !marooned.turn_toward_endpoint,
        "an endpoint in another passage: the fallback is not on the way there"
    );
}

fn plain_rig() -> DpvRig {
    let unit = DpvUnit {
        speed_m_per_min: 55.0,
        burn_minutes: 100.0,
    };
    DpvRig {
        primary: unit.clone(),
        backup: unit,
        range_reserve_fraction: 0.0,
    }
}

fn span(speed: f64, on_scooter: bool) -> PlanSegment {
    PlanSegment {
        speed_m_per_min: Some(speed),
        on_scooter,
    }
}

/// The scooter park point is derived from the segment list: with a rig on board but no span
/// ridden, nothing is parked anywhere, however many spans the plan has. (The existing tests
/// cover a one-span plan, where the "last scootered span is the last span" rule hides this.)
#[test]
fn a_plan_with_no_scootered_span_parks_no_scooters() {
    let mut project = Project::new("CaveA".to_string(), None);
    let sec = descent(&mut project);
    let entrance = StationRef::new(sec, 0);
    let first_leg_end = StationRef::new(sec, 1);
    let diver = Diver::new(20.0, None).unwrap();
    let cfg = GasConfig {
        dpv: Some(plain_rig()),
        ..GasConfig::default()
    };
    let park = |segments: &[PlanSegment]| {
        project
            .compute_gas_reach_field_with_overrides(
                Some(&entrance),
                Some(&StationRef::new(sec, 3)),
                &[first_leg_end],
                segments,
                &diver,
                &[unlimited_backgas()],
                &cfg,
                &[],
            )
            .scooter_drop_ref
    };

    assert_eq!(
        park(&[span(55.0, true), span(10.0, false)]),
        Some(first_leg_end),
        "premise: ride the first span, swim the second - the units wait at the way point"
    );
    assert_eq!(
        park(&[span(10.0, false), span(10.0, false)]),
        None,
        "nothing ridden, nothing parked"
    );
}

fn stage(fill_bar: f64, usable_gas_bar: f64) -> Cylinder {
    Cylinder {
        usable_gas_bar,
        ..Cylinder::new(
            11.0,
            207.0,
            fill_bar,
            TankMaterial::Aluminium,
            CylRole::Stage,
            None,
        )
        .unwrap()
    }
}

fn drop_bars(
    project: &Project,
    origin: StationRef,
    preset: ReservePreset,
    rig: &[Cylinder],
) -> Vec<f64> {
    let cfg = GasConfig {
        default_reserve: preset,
        ..GasConfig::default()
    };
    project
        .compute_gas_reach_field(
            Some(&origin),
            None,
            &Diver::new(20.0, None).unwrap(),
            rig,
            &cfg,
        )
        .stage_drop_bars
}

/// A staged preset drops the FIRST `required_stages` enabled stages in rig order; a spare stage
/// after them is carried, not dropped. Only rigs with exactly the required stages were asserted,
/// where first and last coincide.
#[test]
fn a_staged_preset_drops_its_first_stages_in_rig_order() {
    let mut project = Project::new("CaveA".to_string(), None);
    let sec = descent(&mut project);
    let origin = StationRef::new(sec, 0);
    let backgas = unlimited_backgas();
    let full = stage(200.0, 0.0);
    let low = stage(150.0, 0.0);
    let one = ReservePreset::OneStage;

    let full_only = drop_bars(&project, origin, one, &[backgas.clone(), full.clone()]);
    let low_only = drop_bars(&project, origin, one, &[backgas.clone(), low.clone()]);
    assert_eq!(full_only.len(), 1);
    assert_ne!(
        full_only, low_only,
        "premise: the two stages drop at different gauges"
    );

    assert_eq!(
        drop_bars(
            &project,
            origin,
            one,
            &[backgas.clone(), full.clone(), low.clone()]
        ),
        full_only,
        "the first stage in rig order is the one dropped"
    );
    assert_eq!(
        drop_bars(&project, origin, one, &[backgas, low, full]),
        low_only
    );
}

/// Free drops every enabled stage that has usable gas and skips one that has none.
#[test]
fn free_drops_only_the_stages_with_usable_gas() {
    let mut project = Project::new("CaveA".to_string(), None);
    let sec = descent(&mut project);
    let origin = StationRef::new(sec, 0);
    let cfg = GasConfig {
        default_reserve: ReservePreset::Free,
        ..GasConfig::default()
    };
    let field = project.compute_gas_reach_field(
        Some(&origin),
        None,
        &Diver::new(20.0, None).unwrap(),
        &[unlimited_backgas(), stage(207.0, 40.0), stage(207.0, 0.0)],
        &cfg,
    );
    assert_eq!(
        field.stage_drop_bars.len(),
        1,
        "only the stage with usable gas drops"
    );
    assert_eq!(field.stage_drop_refs.len(), 1);
    assert_eq!(field.stage_drop_distances_m.len(), 1);
}

/// `waypoint_gas_litres` is CUMULATIVE from the origin, not the cost of the leg into the way
/// point: the second way point of a straight run costs what reaching it directly costs. With a
/// single way point the two readings coincide, which is all the existing tests had.
#[test]
fn way_point_gas_is_cumulative_from_the_origin() {
    let mut project = Project::new("CaveA".to_string(), None);
    let sec = descent(&mut project);
    let entrance = StationRef::new(sec, 0);
    let (one, two) = (StationRef::new(sec, 1), StationRef::new(sec, 2));
    let diver = Diver::new(20.0, None).unwrap();
    let rig = [unlimited_backgas()];
    let cfg = GasConfig::default();

    let plain = project.compute_gas_reach_field(Some(&entrance), None, &diver, &rig, &cfg);
    let direct = |r: StationRef| {
        let i = plain.station_refs.iter().position(|s| *s == r).unwrap();
        plain.gas_to_reach[i].unwrap()
    };
    let via = project.compute_gas_reach_field_with_overrides(
        Some(&entrance),
        None,
        &[one, two],
        &[],
        &diver,
        &rig,
        &cfg,
        &[],
    );
    let at = |k: usize| via.waypoint_gas_litres[k].unwrap();
    assert!(
        (at(0) - direct(one)).abs() < 1e-6,
        "{} vs {}",
        at(0),
        direct(one)
    );
    assert!(
        (at(1) - direct(two)).abs() < 1e-6,
        "the second way point reads the whole way in: {} vs {}",
        at(1),
        direct(two)
    );
    assert!(at(1) > at(0));
}
