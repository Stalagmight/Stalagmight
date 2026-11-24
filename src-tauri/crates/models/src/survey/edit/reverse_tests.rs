//! Reverse a line (plan 4.4): the chain and every shot flip, nothing moves.

use crate::{ShotData, StationRef};

use super::super::test_helpers::gps;
use super::super::Project;
use super::edit_test_helpers::{
    add_datum, assert_still, assert_valid, empty, line, line_from, names, ok, run, shot_u, st,
};

#[test]
fn a_reverse_flips_the_chain_and_every_shot_and_moves_nothing() {
    let mut p = empty();
    let mut a = shot_u(10.0, 5.0, 1.0, 3.0, "a");
    a.pitch_in = Some(4.0);
    a.pitch_out = Some(5.0);
    a.left = Some(1.0);
    a.right = Some(2.0);
    let l = line(
        &mut p,
        "Main",
        vec![
            a,
            shot_u(100.0, 7.0, 3.0, 2.0, "b"),
            shot_u(200.0, 4.0, 2.0, 6.0, "c"),
        ],
    );
    let before = p.raw_positions();
    let chain: Vec<u32> = p.line_stations(l).unwrap().to_vec();
    let ids: Vec<u32> = p.line(l).unwrap().shots.iter().map(|s| s.id).collect();
    ok(p.reverse_line(l, true));
    let reversed: Vec<u32> = chain.iter().rev().copied().collect();
    assert_eq!(p.line_stations(l).unwrap(), reversed.as_slice());
    assert_eq!(names(&p, l), ["3", "2", "1", "0"]);
    let line = p.line(l).unwrap();
    let uuids: Vec<&str> = line.shots.iter().map(|s| s.uuid.as_str()).collect();
    assert_eq!(uuids, ["c", "b", "a"]);
    let now: Vec<u32> = line.shots.iter().map(|s| s.id).collect();
    assert_eq!(now, ids, "session ids stay with their positions");
    let last = &line.shots[2];
    assert_eq!((last.azimuth_in, last.azimuth_out), (190.0, 190.0));
    assert_eq!((last.depth_in, last.depth_out), (3.0, 1.0));
    assert_eq!((last.pitch_in, last.pitch_out), (Some(-5.0), Some(-4.0)));
    assert_eq!((last.left, last.right), (Some(2.0), Some(1.0)));
    assert_still(&before, &p.raw_positions(), 1e-9, &[]);
    // Depths are the same set at every station.
    for s in &chain {
        assert_eq!(before.z(*s), p.raw_positions().z(*s));
    }
    // Twice is the line back (to the ulp of the 180-degree turn).
    ok(p.reverse_line(l, true));
    assert_eq!(p.line_stations(l).unwrap(), chain.as_slice());
    assert_still(&before, &p.raw_positions(), 1e-9, &[]);
    assert_valid(&p);
}

#[test]
fn a_reverse_keeps_junctions_datums_and_closures_on_their_stations() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 3));
    let j = st(&p, a, 1);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 2));
    let at1 = st(&p, a, 3);
    add_datum(&mut p, at1, gps(10.0, 20.0));
    let before = p.raw_positions();
    let datums = p.datums.clone();
    ok(p.reverse_line(a, false));
    assert_eq!(p.station_at(StationRef::new(a, 2)), Some(j));
    assert_eq!(p.datums, datums);
    assert_eq!(p.line_stations(b).unwrap()[0], j);
    assert_still(&before, &p.raw_positions(), 1e-9, &[]);
    assert_valid(&p);
}

#[test]
fn a_shotless_line_reverses_to_itself_and_a_missing_one_is_refused() {
    let mut p = empty();
    let l = line(&mut p, "Lone", Vec::<ShotData>::new());
    let snapshot: Project = p.clone();
    ok(p.reverse_line(l, true));
    assert_eq!(p, snapshot);
    assert!(p.reverse_line(99, false).is_err());
}

/// Farm finding (sg-metamorphic reverse-twice-is-identity, e.g. seeds
/// 17003130 and 17500744): a shot whose two readings were typed exactly
/// opposite but are one ulp short of it in f64 (`158.1` / `338.1`,
/// `267.1` / `87.1`). The reverse left that shot's bearing unturned, so one
/// reverse moved every station past it by twice its horizontal length (11 m
/// here), and a second reverse turned it the other way: the line ended up
/// 11 m off. 0.6.0 does the same (declared fix, plan Deviations T4-R1).
#[test]
fn a_shot_typed_exactly_opposite_turns_with_its_line() {
    for (azimuth_in, azimuth_out) in [(158.1, 338.1), (267.1, 87.1)] {
        let mut p = empty();
        let mut odd = shot_u(0.0, 5.5, 5.0, 5.0, "b");
        odd.azimuth_in = azimuth_in;
        odd.azimuth_out = azimuth_out;
        let l = line(
            &mut p,
            "Main",
            vec![
                shot_u(0.0, 4.0, 5.0, 5.0, "a"),
                odd,
                shot_u(90.0, 3.0, 5.0, 5.0, "c"),
            ],
        );
        let before = p.raw_positions();
        ok(p.reverse_line(l, false));
        assert_still(&before, &p.raw_positions(), 1e-9, &[]);
        let middle = &p.line(l).unwrap().shots[1];
        assert_eq!(
            (middle.azimuth_in.to_bits(), middle.azimuth_out.to_bits()),
            (azimuth_out.to_bits(), azimuth_in.to_bits()),
            "the degenerate pair is swapped, both values as typed"
        );
        ok(p.reverse_line(l, false));
        assert_still(&before, &p.raw_positions(), 1e-9, &[]);
        let middle = &p.line(l).unwrap().shots[1];
        assert_eq!(
            (middle.azimuth_in.to_bits(), middle.azimuth_out.to_bits()),
            (azimuth_in.to_bits(), azimuth_out.to_bits()),
            "twice is the shot back, to the bit"
        );
        assert_valid(&p);
    }
}
