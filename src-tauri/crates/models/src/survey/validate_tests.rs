//! Every invariant of plan section 2 is detected, with a precise violation.

use crate::{Section, StationRef, MAX_SECTION_ID, MAX_STATION_ID};

use super::test_helpers::{add, at, flat, gps, project, three_line_junction};
use super::{
    ClosureEnd, ClosureStrategy, CounterKind, NumberField, Project, StartAt, Station, StationAlias,
    Violation, ZeroDatum, MAX_SHOTS_PER_LINE,
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
            });
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

#[test]
fn a_non_finite_validation_threshold_is_reported() {
    let (mut p, _) = three_line_junction();
    p.settings.validation.pitch_diff_threshold = f64::INFINITY;
    assert_eq!(
        p.validate(),
        vec![Violation::NonFinite {
            field: NumberField::ValidationThreshold {
                field: "pitch_diff_threshold"
            },
            value: f64::INFINITY,
        }]
    );
}

#[test]
fn an_alias_id_outside_the_station_id_space_is_reported() {
    expect_violation(
        |p| {
            let host = *p.stations.keys().next().unwrap();
            p.counters.next_station_id = MAX_STATION_ID;
            p.station_mut(host).unwrap().aliases.push(StationAlias {
                id: MAX_STATION_ID,
                name: None,
            });
        },
        &Violation::AliasOutOfRange {
            station: 0,
            alias: MAX_STATION_ID,
        },
    );
}

#[test]
fn a_line_holds_at_most_99999_shots() {
    let mut p = project();
    let full = add(
        &mut p,
        StartAt::New,
        vec![flat(0.0, 1.0); MAX_SHOTS_PER_LINE],
    );
    p.set_placement(at(&p, full, 0), 0.0, 0.0).unwrap();
    assert_eq!(p.validate(), Vec::new(), "99,999 shots is the most");
    let over = add(
        &mut p,
        StartAt::New,
        vec![flat(0.0, 1.0); MAX_SHOTS_PER_LINE + 1],
    );
    p.set_placement(at(&p, over, 0), 5.0, 0.0).unwrap();
    assert_eq!(
        p.validate(),
        vec![Violation::TooManyShots {
            line: over,
            count: MAX_SHOTS_PER_LINE + 1,
        }]
    );
}

#[test]
fn station_and_section_counters_stay_within_their_id_spaces() {
    let (mut p, _) = three_line_junction();
    p.counters.next_station_id = MAX_STATION_ID;
    p.counters.next_section_id = MAX_SECTION_ID;
    assert_eq!(
        p.validate(),
        Vec::new(),
        "at the bound: the ids are used up"
    );
    expect_violation(
        |p| p.counters.next_station_id = MAX_STATION_ID + 1,
        &Violation::CounterOutOfRange {
            counter: CounterKind::Station,
            next: u64::from(MAX_STATION_ID) + 1,
        },
    );
    expect_violation(
        |p| p.counters.next_section_id = u32::MAX,
        &Violation::CounterOutOfRange {
            counter: CounterKind::Section,
            next: u64::from(u32::MAX),
        },
    );
}

// ----- the fast checks against the map-based ones they replace (D2) ---------

/// The reports of the checks `validate` makes with sorted fast paths and a
/// dense union-find (uuid repeats, dangling starts and targets, repeated
/// chain stations, orphans, cycles, repeated names), computed the way they
/// were before: ordered maps keyed by uuid, station id and name. In
/// `validate`'s order.
fn map_based_reports(p: &Project) -> Vec<Violation> {
    use std::collections::BTreeMap;
    let topology = p.topology();
    let mut out = Vec::new();
    let mut first_seen: BTreeMap<&str, (crate::LineId, usize)> = BTreeMap::new();
    for line in p.lines() {
        for (index, shot) in line.shots.iter().enumerate() {
            if let Some(&first) = first_seen.get(shot.uuid.as_str()) {
                out.push(Violation::DuplicateShotUuid {
                    uuid: shot.uuid.clone(),
                    first,
                    second: (line.id, index),
                });
            } else {
                first_seen.insert(&shot.uuid, (line.id, index));
            }
        }
    }
    for line in p.lines() {
        if !p.stations.contains_key(&line.start) {
            out.push(Violation::DanglingLineStart {
                line: line.id,
                station: line.start,
            });
        }
        for (index, shot) in line.shots.iter().enumerate() {
            if !p.stations.contains_key(&shot.to) {
                out.push(Violation::DanglingShotTarget {
                    line: line.id,
                    index,
                    station: shot.to,
                });
            }
        }
    }
    for line in p.lines() {
        let mut seen: BTreeMap<crate::StationId, usize> = BTreeMap::new();
        for (index, station) in line.stations().enumerate() {
            if let Some(&first) = seen.get(&station) {
                out.push(Violation::RepeatedStationInChain {
                    line: line.id,
                    station,
                    first,
                    second: index,
                });
            } else {
                seen.insert(station, index);
            }
        }
    }
    for &station in p.stations.keys() {
        if !topology.contains(station) {
            out.push(Violation::OrphanStation { station });
        }
    }
    let mut parent: BTreeMap<crate::StationId, crate::StationId> = BTreeMap::new();
    fn find(
        parent: &BTreeMap<crate::StationId, crate::StationId>,
        x: crate::StationId,
    ) -> crate::StationId {
        let mut root = x;
        while let Some(&p) = parent.get(&root) {
            if p == root {
                break;
            }
            root = p;
        }
        root
    }
    for line in p.lines() {
        parent.entry(line.start).or_insert(line.start);
        let mut from = line.start;
        for (index, shot) in line.shots.iter().enumerate() {
            parent.entry(shot.to).or_insert(shot.to);
            let (ra, rb) = (find(&parent, from), find(&parent, shot.to));
            if ra == rb {
                out.push(Violation::Cycle {
                    line: line.id,
                    index,
                    from,
                    to: shot.to,
                });
            } else {
                parent.insert(ra.max(rb), ra.min(rb));
            }
            from = shot.to;
        }
    }
    for (position, line) in p.lines().iter().enumerate() {
        let Some(chain) = topology.chain_at(position) else {
            continue;
        };
        let mut seen: BTreeMap<&str, crate::StationId> = BTreeMap::new();
        for &station in chain {
            let Some(name) = p.stations.get(&station).map(|s| s.name.as_str()) else {
                continue;
            };
            match seen.get(name) {
                Some(&first) if first != station => out.push(Violation::DuplicateStationName {
                    line: line.id,
                    name: name.to_string(),
                    first,
                    second: station,
                }),
                Some(_) => {}
                None => {
                    seen.insert(name, station);
                }
            }
        }
    }
    out
}

fn is_rewritten_report(v: &Violation) -> bool {
    matches!(
        v,
        Violation::DuplicateShotUuid { .. }
            | Violation::DanglingLineStart { .. }
            | Violation::DanglingShotTarget { .. }
            | Violation::RepeatedStationInChain { .. }
            | Violation::OrphanStation { .. }
            | Violation::Cycle { .. }
            | Violation::DuplicateStationName { .. }
    )
}

/// Random corruptions of a branching survey (repeated uuids, dangling and
/// orphaned stations, a station twice on a chain, shots closing loops,
/// repeated names): `validate` reports exactly what the map-based checks
/// report, in the same order. Injection: drop a fast path's fallback (e.g.
/// `continue` unconditionally in `check_chains`) and this fails.
#[test]
fn the_fast_checks_report_what_the_map_based_ones_did() {
    let mut reported = 0;
    for seed in 0..300_u64 {
        let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut below = |n: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % n as u64) as usize
        };
        let (mut p, lines) = three_line_junction();
        let extra = add(&mut p, StartAt::New, vec![flat(30.0, 4.0); 6]);
        let lines = [lines[0], lines[1], lines[2], extra];
        let stations: Vec<crate::StationId> = p.stations.keys().copied().collect();
        for _ in 0..=below(3) {
            let line = lines[below(lines.len())];
            let shots = p.line(line).unwrap().shots.len();
            if shots == 0 {
                continue;
            }
            let index = below(shots);
            match below(6) {
                0 => {
                    let other = lines[below(lines.len())];
                    let uuid = p.line(other).unwrap().shots[0].uuid.clone();
                    p.line_mut(line).unwrap().shots[index].uuid = uuid;
                }
                1 => p.line_mut(line).unwrap().shots[index].to = 900_000 + below(3) as u32,
                2 => {
                    let station = stations[below(stations.len())];
                    p.line_mut(line).unwrap().shots[index].to = station;
                }
                3 => {
                    let orphan = 800_000 + below(5) as u32;
                    p.stations
                        .insert(orphan, Station::new(orphan, format!("o{orphan}")));
                }
                4 => {
                    let station = stations[below(stations.len())];
                    let name = p.stations[&station].name.clone();
                    let to = p.line(line).unwrap().shots[index].to;
                    if let Some(s) = p.stations.get_mut(&to) {
                        s.name = name;
                    }
                }
                _ => {
                    let start = stations[below(stations.len())];
                    p.line_mut(line).unwrap().start = start;
                }
            }
        }
        let fast: Vec<Violation> = p
            .validate()
            .into_iter()
            .filter(is_rewritten_report)
            .collect();
        assert_eq!(fast, map_based_reports(&p), "seed {seed}");
        reported += usize::from(!fast.is_empty());
    }
    assert!(reported > 200, "most corruptions are reported: {reported}");
}
