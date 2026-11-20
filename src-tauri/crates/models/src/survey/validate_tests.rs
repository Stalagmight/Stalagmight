//! Every invariant of plan section 2 is detected, with a precise violation.

use crate::{Section, StationRef, MAX_STATION_ID};

use super::test_helpers::{add, at, flat, gps, three_line_junction};
use super::{
    ClosureEnd, ClosureStrategy, CounterKind, NumberField, Project, StartAt, Station, StationAlias,
    Violation, ZeroDatum,
};

/// The base survey validates, and `mutate` makes it report `expected` (among
/// possibly others).
fn expect_violation(mutate: impl FnOnce(&mut Project), expected: &Violation) {
    let (mut p, _) = three_line_junction();
    assert_eq!(p.validate(), Vec::new(), "the base survey must be valid");
    mutate(&mut p);
    let violations = p.validate();
    assert!(
        violations.contains(expected),
        "expected {expected:?} in {violations:?}"
    );
    assert!(!expected.to_string().is_empty());
}

#[test]
fn the_base_survey_and_a_lone_station_line_are_valid() {
    let (mut p, _) = three_line_junction();
    assert!(p.is_valid());
    let lone = add(&mut p, StartAt::New, Vec::new());
    let station = at(&p, lone, 0);
    p.set_placement(station, -5.0, 0.0).unwrap();
    assert_eq!(p.validate(), Vec::new());
}

#[test]
fn a_station_stored_under_another_id_is_reported() {
    expect_violation(
        |p| {
            p.stations.insert(77, Station::new(78, "z"));
        },
        &Violation::StationKeyMismatch { key: 77, id: 78 },
    );
}

#[test]
fn an_out_of_range_station_id_is_reported() {
    expect_violation(
        |p| {
            p.stations
                .insert(MAX_STATION_ID, Station::new(MAX_STATION_ID, "big"));
        },
        &Violation::IdOutOfRange {
            kind: CounterKind::Station,
            id: u64::from(MAX_STATION_ID),
        },
    );
}

#[test]
fn an_alias_equal_to_a_live_id_is_reported() {
    expect_violation(
        |p| {
            p.stations
                .get_mut(&0)
                .unwrap()
                .aliases
                .push(StationAlias { id: 1, name: None });
        },
        &Violation::AliasCollision {
            station: 0,
            alias: 1,
        },
    );
}

#[test]
fn a_repeated_line_id_is_reported() {
    expect_violation(
        |p| {
            let copy = p.lines()[1].clone();
            p.lines_mut().push(copy);
        },
        &Violation::DuplicateLineId { line: 1 },
    );
}

#[test]
fn repeated_closure_and_section_ids_are_reported() {
    expect_violation(
        |p| {
            let (a, b) = (at(p, 0, 0), at(p, 0, 3));
            p.push_closure(a, None, b, None, ClosureStrategy::Batch);
            let first = p.closures[0];
            p.closures.push(first);
        },
        &Violation::DuplicateClosureId { closure: 1 },
    );
    expect_violation(
        |p| {
            let section = Section {
                id: 3,
                name: "S".to_string(),
                color: "#fff".to_string(),
            };
            p.sections.push(section.clone());
            p.sections.push(section);
            p.counters.next_section_id = 4;
        },
        &Violation::DuplicateSectionId { section: 3 },
    );
}

#[test]
fn empty_and_repeated_shot_uuids_are_reported() {
    expect_violation(
        |p| p.line_mut(0).unwrap().shots[1].uuid = String::new(),
        &Violation::InvalidShotUuid {
            line: 0,
            index: 1,
            uuid: String::new(),
        },
    );
    expect_violation(
        |p| p.line_mut(0).unwrap().shots[1].uuid = "x".repeat(129),
        &Violation::InvalidShotUuid {
            line: 0,
            index: 1,
            uuid: "x".repeat(129),
        },
    );
    expect_violation(
        |p| p.line_mut(1).unwrap().shots[0].uuid = "shot-0-2".to_string(),
        &Violation::DuplicateShotUuid {
            uuid: "shot-0-2".to_string(),
            first: (0, 2),
            second: (1, 0),
        },
    );
}

#[test]
fn dangling_line_starts_and_shot_targets_are_reported() {
    expect_violation(
        |p| p.line_mut(2).unwrap().start = 999,
        &Violation::DanglingLineStart {
            line: 2,
            station: 999,
        },
    );
    expect_violation(
        |p| p.line_mut(1).unwrap().shots[1].to = 998,
        &Violation::DanglingShotTarget {
            line: 1,
            index: 1,
            station: 998,
        },
    );
}

#[test]
fn dangling_datum_placement_and_closure_stations_are_reported() {
    expect_violation(
        |p| p.datums.push(ZeroDatum::new(500, gps(10.0, 10.0))),
        &Violation::DanglingDatumStation {
            datum: 0,
            station: 500,
        },
    );
    expect_violation(
        |p| p.placements[0].station = 501,
        &Violation::DanglingPlacementStation {
            placement: 0,
            station: 501,
        },
    );
    expect_violation(
        |p| {
            let a = at(p, 0, 0);
            p.push_closure(a, None, 502, None, ClosureStrategy::Batch);
        },
        &Violation::DanglingClosureEnd {
            closure: 1,
            end: ClosureEnd::B,
            station: 502,
        },
    );
}

#[test]
fn a_via_hint_must_name_a_line_through_the_station() {
    expect_violation(
        |p| {
            let s0 = at(p, 0, 0);
            p.placements.clear();
            let mut datum = ZeroDatum::new(s0, gps(10.0, 10.0));
            datum.via = Some(1);
            p.datums.push(datum);
        },
        &Violation::StaleDatumVia {
            datum: 0,
            station: 0,
            line: 1,
        },
    );
    expect_violation(
        |p| {
            let (s0, s3) = (at(p, 0, 0), at(p, 0, 3));
            p.push_closure(s0, Some(0), s3, Some(1), ClosureStrategy::Batch);
        },
        &Violation::StaleClosureVia {
            closure: 1,
            end: ClosureEnd::B,
            station: 3,
            line: 1,
        },
    );
}

#[test]
fn hand_moves_off_their_line_and_dangling_sections_are_reported() {
    expect_violation(
        |p| {
            let s0 = at(p, 0, 0);
            p.line_mut(1)
                .unwrap()
                .hand_moves
                .insert(s0, [1.0, 0.0, 0.0]);
        },
        &Violation::HandMoveOffLine {
            line: 1,
            station: 0,
        },
    );
    expect_violation(
        |p| p.line_mut(0).unwrap().section_id = Some(9),
        &Violation::DanglingSection {
            line: 0,
            section: 9,
        },
    );
}

#[test]
fn a_chain_passing_a_station_twice_is_reported() {
    expect_violation(
        |p| {
            let s0 = at(p, 0, 0);
            p.line_mut(0).unwrap().shots[2].to = s0;
        },
        &Violation::RepeatedStationInChain {
            line: 0,
            station: 0,
            first: 0,
            second: 3,
        },
    );
}

#[test]
fn a_station_on_no_line_is_reported() {
    expect_violation(
        |p| {
            p.mint_station("stray").unwrap();
        },
        &Violation::OrphanStation { station: 7 },
    );
}

#[test]
fn a_shot_closing_a_loop_is_reported() {
    // L1's last shot re-targeted onto L0's s2 closes s1 -> a -> s2 -> s1.
    expect_violation(
        |p| {
            let s2 = at(p, 0, 2);
            let orphan = at(p, 1, 2);
            p.line_mut(1).unwrap().shots[1].to = s2;
            p.stations.remove(&orphan);
        },
        &Violation::Cycle {
            line: 1,
            index: 1,
            from: 4,
            to: 2,
        },
    );
}

#[test]
fn anchors_are_exactly_one_per_component() {
    expect_violation(
        |p| p.placements.clear(),
        &Violation::UnanchoredComponent { lowest_station: 0 },
    );
    expect_violation(
        |p| {
            p.placements.push(super::Placement {
                station: 3,
                x: 0.0,
                y: 0.0,
            })
        },
        &Violation::ExtraPlacement {
            placement: 1,
            station: 3,
            first: 0,
        },
    );
    expect_violation(
        |p| p.datums.push(ZeroDatum::new(2, gps(10.0, 10.0))),
        &Violation::PlacementInDatumComponent {
            placement: 0,
            station: 0,
        },
    );
}

#[test]
fn closure_ends_must_differ_and_cross_pieces_only_between_datum_pieces() {
    expect_violation(
        |p| {
            p.push_closure(2, None, 2, None, ClosureStrategy::Batch);
        },
        &Violation::ClosureSelfLoop {
            closure: 1,
            station: 2,
        },
    );
    // A second free piece joined to the first by a closure only.
    let (mut p, _) = three_line_junction();
    let other = add(&mut p, StartAt::New, vec![flat(0.0, 3.0)]);
    let o0 = at(&p, other, 0);
    p.set_placement(o0, 0.0, 0.0).unwrap();
    p.push_closure(0, None, o0, None, ClosureStrategy::Batch);
    assert!(p
        .validate()
        .contains(&Violation::ClosureAcrossUnplacedPieces {
            closure: 1,
            a: 0,
            b: o0,
        }));
    // Both pieces placed by datums: allowed.
    p.placements.clear();
    p.datums.push(ZeroDatum::new(0, gps(10.0, 10.0)));
    p.datums.push(ZeroDatum::new(o0, gps(10.001, 10.0)));
    assert_eq!(p.validate(), Vec::new());
}

#[test]
fn a_name_repeated_on_a_line_is_reported_also_through_a_junction() {
    expect_violation(
        |p| p.stations.get_mut(&5).unwrap().name = "1a".to_string(),
        &Violation::DuplicateStationName {
            line: 1,
            name: "1a".to_string(),
            first: 4,
            second: 5,
        },
    );
    // s1 (named "1") is on L0 and L1: a station of L1 named "1" collides.
    expect_violation(
        |p| p.stations.get_mut(&5).unwrap().name = "1".to_string(),
        &Violation::DuplicateStationName {
            line: 1,
            name: "1".to_string(),
            first: 1,
            second: 5,
        },
    );
}

#[test]
fn counters_must_be_above_every_id_in_use() {
    expect_violation(
        |p| p.counters.next_station_id = 3,
        &Violation::CounterNotAbove {
            counter: CounterKind::Station,
            next: 3,
            max_in_use: 6,
        },
    );
    expect_violation(
        |p| {
            p.stations.get_mut(&0).unwrap().aliases.push(StationAlias {
                id: 40,
                name: Some("old".to_string()),
            });
        },
        &Violation::CounterNotAbove {
            counter: CounterKind::Station,
            next: 7,
            max_in_use: 40,
        },
    );
    expect_violation(
        |p| {
            p.push_closure(0, None, 3, None, ClosureStrategy::Batch);
            p.counters.next_closure_id = 1;
        },
        &Violation::CounterNotAbove {
            counter: CounterKind::Closure,
            next: 1,
            max_in_use: 1,
        },
    );
    expect_violation(
        |p| {
            p.sections.push(Section {
                id: 0,
                name: "S".to_string(),
                color: "#fff".to_string(),
            });
        },
        &Violation::CounterNotAbove {
            counter: CounterKind::Section,
            next: 0,
            max_in_use: 0,
        },
    );
}

#[test]
fn non_finite_numbers_are_reported_by_field() {
    let (mut p, _) = three_line_junction();
    p.line_mut(0).unwrap().shots[1].length = f64::NAN;
    p.line_mut(1).unwrap().shots[0].left = Some(f64::INFINITY);
    p.placements[0].y = f64::NEG_INFINITY;
    p.line_mut(2)
        .unwrap()
        .hand_moves
        .insert(3, [0.0, 0.0, f64::NAN]);
    p.settings.surface_elevation = Some(f64::NAN);
    let fields: Vec<NumberField> = p
        .validate()
        .into_iter()
        .filter_map(|v| match v {
            Violation::NonFinite { field, .. } => Some(field),
            _ => None,
        })
        .collect();
    assert_eq!(
        fields,
        vec![
            NumberField::Shot {
                line: 0,
                index: 1,
                field: "length"
            },
            NumberField::Shot {
                line: 1,
                index: 0,
                field: "left"
            },
            NumberField::HandMove {
                line: 2,
                station: 3,
                axis: 2
            },
            NumberField::Placement {
                placement: 0,
                axis: 1
            },
            NumberField::SurfaceElevation,
        ]
    );
}

#[test]
fn a_datum_correction_must_be_finite() {
    let (mut p, _) = three_line_junction();
    p.placements.clear();
    let mut datum = ZeroDatum::new(0, gps(10.0, 10.0));
    datum.correction = Some([f64::NAN, 0.0]);
    p.datums.push(datum);
    assert!(p.validate().iter().any(|v| matches!(
        v,
        Violation::NonFinite {
            field: NumberField::DatumCorrection { datum: 0, axis: 0 },
            ..
        }
    )));
}

#[test]
fn slots_resolve_and_junctions_list_every_slot() {
    let (p, [l0, l1, l2]) = three_line_junction();
    let s1 = at(&p, l0, 1);
    assert_eq!(p.station_at(StationRef::new(l1, 0)), Some(s1));
    assert_eq!(
        p.slots_of(s1),
        &[StationRef::new(l0, 1), StationRef::new(l1, 0)]
    );
    let s3 = at(&p, l0, 3);
    assert_eq!(
        p.slots_of(s3),
        &[StationRef::new(l0, 3), StationRef::new(l2, 1)]
    );
    assert!(p.topology().is_junction(s3));
    assert_eq!(p.topology().components().len(), 1);
    assert_eq!(p.topology().components()[0].lines, vec![l0, l1, l2]);
    assert_eq!(p.station_at(StationRef::new(l2, 2)), None);
}
