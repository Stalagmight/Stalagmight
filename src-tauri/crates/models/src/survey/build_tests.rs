//! The construction API, naming and identity helpers, and the closure
//! strategy's serde compatibility.

use crate::{ShotData, StationRef, FEATURE_ID_STRIDE};

use super::test_helpers::{add, add_on, at, flat, meta, project, three_line_junction, try_add};
use super::{BuildError, ClosureStrategy, Line, NameConflict, StartAt, Station, StationAlias};

#[test]
fn add_line_mints_one_station_per_shot_and_names_them_by_index() {
    let mut p = project();
    let line = add(&mut p, StartAt::New, vec![flat(0.0, 1.0), flat(0.0, 1.0)]);
    assert_eq!(line, 0);
    assert_eq!(p.line_stations(line), Some(&[0, 1, 2][..]));
    let names: Vec<&str> = (0..3).map(|i| p.stations[&i].name.as_str()).collect();
    assert_eq!(names, ["0", "1", "2"]);
    assert_eq!(p.counters.next_station_id, 3);
    assert_eq!(p.next_line_id(), 1);
}

#[test]
fn a_line_from_an_existing_station_is_a_junction_and_avoids_its_name() {
    let mut p = project();
    let l0 = add(&mut p, StartAt::New, vec![flat(0.0, 1.0), flat(0.0, 1.0)]);
    let s1 = at(&p, l0, 1);
    let l1 = add(
        &mut p,
        StartAt::Station(s1),
        vec![flat(90.0, 1.0), flat(90.0, 1.0)],
    );
    assert_eq!(p.line_stations(l1), Some(&[1, 3, 4][..]));
    // "1" is the start's name, so chain index 1 is "1a".
    assert_eq!(p.stations[&3].name, "1a");
    assert_eq!(p.stations[&4].name, "2");
    assert!(p.topology().is_junction(s1));
    assert_eq!(
        p.slots_of(s1),
        &[StationRef::new(l0, 1), StationRef::new(l1, 0)]
    );
}

#[test]
fn add_line_refusals_change_nothing() {
    let mut p = project();
    add(&mut p, StartAt::New, vec![flat(0.0, 1.0)]);
    let snapshot = p.clone();
    assert_eq!(
        try_add(&mut p, StartAt::Station(40), vec![flat(0.0, 1.0)]),
        Err(BuildError::UnknownStation(40))
    );
    let taken = ShotData {
        uuid: Some("shot-0-0".to_string()),
        ..flat(0.0, 1.0)
    };
    assert_eq!(
        try_add(&mut p, StartAt::New, vec![taken]),
        Err(BuildError::ShotUuidTaken("shot-0-0".to_string()))
    );
    let twice = ShotData {
        uuid: Some("dup".to_string()),
        ..flat(0.0, 1.0)
    };
    assert_eq!(
        try_add(&mut p, StartAt::New, vec![twice.clone(), twice]),
        Err(BuildError::ShotUuidTaken("dup".to_string()))
    );
    assert_eq!(
        p.add_line_on_stations(meta("x"), &[0, 1, 0], vec![flat(0.0, 1.0), flat(0.0, 1.0)]),
        Err(BuildError::RepeatedStation(0))
    );
    assert_eq!(
        p.add_line_on_stations(meta("x"), &[0], vec![flat(0.0, 1.0)]),
        Err(BuildError::ChainLength {
            stations: 1,
            shots: 1
        })
    );
    assert_eq!(p, snapshot);
}

#[test]
fn shot_ids_are_session_handles_and_uuids_come_from_the_data_or_are_minted() {
    let mut p = project();
    add(&mut p, StartAt::New, vec![flat(0.0, 1.0)]);
    let minted = ShotData {
        uuid: None,
        ..flat(0.0, 1.0)
    };
    let line = p
        .add_line(meta("m"), StartAt::New, vec![minted, flat(0.0, 2.0)])
        .unwrap();
    let shots = &p.line(line).unwrap().shots;
    assert_eq!(shots[0].id, FEATURE_ID_STRIDE);
    assert_eq!(shots[1].id, FEATURE_ID_STRIDE + 1);
    assert_eq!(p.line(line).unwrap().next_shot_id, FEATURE_ID_STRIDE + 2);
    assert_eq!(
        shots[0].uuid.len(),
        36,
        "a v4 is minted for a shot without one"
    );
    assert_ne!(shots[0].uuid, shots[1].uuid);
    assert!(p.is_shot_uuid_taken(&shots[0].uuid));
    let uuid = p.mint_shot_uuid();
    assert!(!p.is_shot_uuid_taken(&uuid));
}

#[test]
fn the_line_id_is_one_above_the_highest_and_insert_line_refuses_a_taken_id() {
    let (mut p, _) = three_line_junction();
    assert_eq!(p.next_line_id(), 3);
    let mut line: Line = p.lines()[0].clone();
    assert_eq!(p.insert_line(line.clone()), Err(BuildError::LineIdTaken(0)));
    line.id = 10;
    for shot in &mut line.shots {
        shot.uuid.push_str("-copy");
    }
    assert_eq!(p.insert_line(line), Ok(10));
    assert_eq!(p.next_line_id(), 11);
    assert_eq!(p.line(10).unwrap().shots[2].id, 10 * FEATURE_ID_STRIDE + 2);
}

#[test]
fn inserting_a_station_raises_the_counter_past_it_and_its_aliases() {
    let mut p = project();
    let mut station = Station::new(10, "ten");
    station.aliases.push(StationAlias {
        id: 25,
        name: Some("old".to_string()),
    });
    p.insert_station(station).unwrap();
    assert_eq!(p.counters.next_station_id, 26);
    assert_eq!(
        p.insert_station(Station::new(10, "again")),
        Err(BuildError::StationIdTaken(10))
    );
    assert_eq!(
        p.insert_station(Station::new(25, "alias")),
        Err(BuildError::StationIdTaken(25))
    );
    assert_eq!(p.mint_station("next"), Ok(26));
    assert_eq!(p.mint_closure_id(), 1);
    assert_eq!(p.mint_section_id(), Ok(0));
}

#[test]
fn set_placement_replaces_the_components_placement_in_place() {
    let (mut p, [_, l1, _]) = three_line_junction();
    let other = add(&mut p, StartAt::New, vec![flat(0.0, 1.0)]);
    p.set_placement(at(&p, other, 0), 1.0, 1.0).unwrap();
    assert_eq!(p.set_placement(at(&p, l1, 2), 5.0, 6.0), Ok(0));
    assert_eq!(p.placements.len(), 2);
    assert_eq!(p.placements[0].station, at(&p, l1, 2));
    assert_eq!(
        p.set_placement(99, 0.0, 0.0),
        Err(BuildError::UnknownStation(99))
    );
}

#[test]
fn the_caches_follow_every_mutable_borrow_of_the_lines() {
    let (mut p, [l0, l1, _]) = three_line_junction();
    let s2 = at(&p, l0, 2);
    assert_eq!(p.slots_of(s2).len(), 1);
    // Re-target L1's last shot onto s2 through `line_mut`: the slot index
    // must see it at once.
    let old = at(&p, l1, 2);
    p.line_mut(l1).unwrap().shots[1].to = s2;
    assert_eq!(p.slots_of(s2).len(), 2);
    assert!(p.slots_of(old).is_empty());
    p.lines_mut().pop();
    assert_eq!(p.next_line_id(), 2);
    assert!(p.line(2).is_none());
}

#[test]
fn junction_names_are_checked_on_every_line_they_are_on() {
    let (mut p, [l0, l1, _]) = three_line_junction();
    let s1 = at(&p, l0, 1);
    let a = at(&p, l1, 1);
    assert!(p.name_conflicts(s1).is_empty());
    // Renaming s1 to "1a" collides with L1's station "1a" (s1 is on L1 too).
    assert_eq!(
        p.conflicts_with_name(s1, "1a"),
        vec![NameConflict { line: l1, other: a }]
    );
    p.stations.get_mut(&s1).unwrap().name = "1a".to_string();
    assert_eq!(
        p.name_conflicts(a),
        vec![NameConflict {
            line: l1,
            other: s1
        }]
    );
    // The letter rule makes the renamed station unique on all its lines.
    assert_eq!(p.uniquify_station_name(s1), Some("1aa".to_string()));
    assert!(p.name_conflicts(s1).is_empty());
    assert_eq!(p.uniquify_station_name(s1), None);
    assert!(p.is_valid());
}

#[test]
fn uniquify_line_names_keeps_the_first_holder_and_suffixes_later_ones() {
    let (mut p, [l0, _, l2]) = three_line_junction();
    for i in 1..4 {
        let station = at(&p, l0, i);
        p.stations.get_mut(&station).unwrap().name = "5".to_string();
    }
    // s3 is also on L2, whose other station is "x0".
    p.stations.get_mut(&at(&p, l2, 0)).unwrap().name = "5b".to_string();
    let renamed = p.uniquify_line_names(l0);
    assert_eq!(renamed, vec![at(&p, l0, 2), at(&p, l0, 3)]);
    let names: Vec<&str> = (1..4)
        .map(|i| p.stations[&at(&p, l0, i)].name.as_str())
        .collect();
    assert_eq!(names, ["5", "5a", "5c"]);
    assert!(p.is_valid());
}

#[test]
fn duplicate_shot_uuids_are_listed_with_every_place() {
    let (mut p, [l0, l1, _]) = three_line_junction();
    assert!(p.duplicate_shot_uuids().is_empty());
    p.line_mut(l1).unwrap().shots[0].uuid = "shot-0-1".to_string();
    let duplicates = p.duplicate_shot_uuids();
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0].0, "shot-0-1");
    let places: Vec<(u32, usize)> = duplicates[0].1.iter().map(|l| (l.line, l.index)).collect();
    assert_eq!(places, vec![(l0, 1), (l1, 0)]);
    assert_eq!(p.shot_by_uuid("shot-0-1").map(|l| l.line), Some(l0));
}

#[test]
fn closure_strategies_read_the_shared_spellings_and_add_tie() {
    for (text, expected) in [
        ("\"Batch\"", ClosureStrategy::Batch),
        ("\"KeepRaw\"", ClosureStrategy::Batch),
        ("\"ManualAdjustment\"", ClosureStrategy::Batch),
        ("\"Sequential\"", ClosureStrategy::LeastSquaresWeighted),
        (
            "\"LeastSquaresWeighted\"",
            ClosureStrategy::LeastSquaresWeighted,
        ),
        ("\"Move\"", ClosureStrategy::Move),
        (
            "\"ProportionalByLength\"",
            ClosureStrategy::ProportionalByLength,
        ),
        ("\"EqualDistribution\"", ClosureStrategy::EqualDistribution),
        ("\"Tie\"", ClosureStrategy::Tie),
    ] {
        let read: ClosureStrategy = serde_json::from_str(text).unwrap();
        assert_eq!(read, expected, "{text}");
        assert_eq!(
            ClosureStrategy::from_str_lossy(text.trim_matches('"')),
            expected
        );
        if let Some(shared) = expected.to_shared() {
            assert_eq!(ClosureStrategy::from(shared), expected);
            // The shared enum writes the same spelling.
            assert_eq!(
                serde_json::to_string(&shared).unwrap(),
                serde_json::to_string(&expected).unwrap()
            );
        }
    }
    assert_eq!(ClosureStrategy::Tie.to_shared(), None);
    assert_eq!(ClosureStrategy::Tie.as_str(), "Tie");
    assert_eq!(
        ClosureStrategy::from_str_lossy("bogus"),
        ClosureStrategy::Batch
    );
}

#[test]
fn a_shot_round_trips_through_its_measurement_record() {
    let mut p = project();
    let line = add(
        &mut p,
        StartAt::New,
        vec![super::test_helpers::shot(12.5, 3.0, 1.0, 2.0)],
    );
    let shot = p.line(line).unwrap().shots[0].clone();
    let data = shot.to_shot_data();
    assert_eq!(data.uuid.as_deref(), Some(shot.uuid.as_str()));
    let back = super::Shot::from_data(shot.id, data, shot.to);
    assert_eq!(back, shot);
}

#[test]
fn a_line_over_given_stations_can_end_on_a_junction() {
    let (mut p, [l0, _, _]) = three_line_junction();
    let s0 = at(&p, l0, 0);
    let fresh = p.mint_station("n").unwrap();
    let line = add_on(&mut p, &[fresh, s0], vec![flat(180.0, 2.0)]);
    assert_eq!(p.line_stations(line), Some(&[fresh, s0][..]));
    assert!(p.is_valid());
}

#[test]
fn stale_vias_and_hand_moves_are_dropped_by_the_epilogue_helper() {
    use super::test_helpers::gps;
    let (mut p, [l0, l1, _]) = three_line_junction();
    let (s0, s2, s3) = (at(&p, l0, 0), at(&p, l0, 2), at(&p, l0, 3));
    p.placements.clear();
    p.push_datum(super::ZeroDatum::new(s0, gps(20.0, -87.0)));
    let mut fix = super::ZeroDatum::new(s3, gps(20.0001, -86.9999));
    fix.via = Some(l0);
    p.push_datum(fix);
    p.push_closure(s2, Some(l0), s3, Some(l0), ClosureStrategy::Batch);
    p.line_mut(l0)
        .unwrap()
        .hand_moves
        .insert(s3, [1.0, 0.0, 0.0]);
    let a = at(&p, l1, 1);
    p.line_mut(l1)
        .unwrap()
        .hand_moves
        .insert(a, [0.0, 1.0, 0.0]);
    assert!(p.is_valid());
    assert_eq!(
        p.drop_stale_attachments(),
        super::StaleAttachments::default()
    );
    // Delete L0's last shot: s3 stays (it is on L2) but L0 no longer passes
    // it, so the fix's hint, the closure end's hint and L0's hand move there
    // are stale. The epilogue drops them, then settles the anchors.
    let before = p.anchor_snapshot();
    p.line_mut(l0).unwrap().shots.pop();
    let dropped = p.drop_stale_attachments();
    assert_eq!(dropped.datum_vias, vec![1]);
    assert_eq!(dropped.closure_vias, vec![(1, super::ClosureEnd::B)]);
    assert_eq!(dropped.hand_moves, vec![(l0, s3)]);
    assert_eq!(p.datums[1].via, None);
    assert_eq!(p.closures[0].a_via, Some(l0));
    assert!(p.line(l0).unwrap().hand_moves.is_empty());
    assert_eq!(p.line(l1).unwrap().hand_moves.len(), 1);
    let report = p.settle_anchors(&before, &super::EditContext::geometry(l0));
    assert_eq!(report.placed_on_gps, vec![1]);
    assert_eq!(p.validate(), Vec::new());
}
