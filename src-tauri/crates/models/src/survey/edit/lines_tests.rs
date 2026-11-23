//! Line operations: create, delete, metadata, azimuth reference, reorder.

use chrono::NaiveDate;

use crate::{AzimuthReference, LineUpdates, NorthReference};

use super::super::test_helpers::{gps, meta};
use super::super::{ClosureStrategy, Placement, Project, StartAt};
use super::edit_test_helpers::{
    add_datum, assert_still, assert_valid, empty, flat_u, line, line_at, line_from, names, near,
    ok, refused, run, st, xy,
};

const TOL: f64 = 1e-9;

#[test]
fn a_new_line_starts_a_piece_at_its_placement_and_is_seed_named() {
    let mut p = empty();
    let l = line_at(&mut p, "Main", [5.0, -3.0], run("m", 0.0, 10.0, 2));
    assert_eq!(names(&p, l), ["0", "1", "2"]);
    assert_eq!(
        p.placements,
        vec![Placement {
            station: st(&p, l, 0),
            x: 5.0,
            y: -3.0
        }]
    );
    let positions = p.raw_positions();
    near(xy(&positions, st(&p, l, 2)), [5.0, 17.0], TOL, "end");
    // A second line without a placement starts at the origin.
    let m = line(&mut p, "Second", run("s", 90.0, 1.0, 1));
    near(
        xy(&p.raw_positions(), st(&p, m, 0)),
        [0.0, 0.0],
        TOL,
        "origin",
    );
    assert_valid(&p);
}

#[test]
fn a_line_from_a_station_makes_a_junction_and_moves_nothing() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 3));
    let before = p.raw_positions();
    let j = st(&p, a, 1);
    p.station_mut(j).unwrap().name = "2".to_string();
    p.station_mut(st(&p, a, 2)).unwrap().name = "J".to_string();
    let b = line_from(&mut p, "B", j, run("b", 0.0, 5.0, 3));
    assert!(p.topology().is_junction(j));
    // The new line's stations avoid the start's name on it.
    assert_eq!(names(&p, b), ["2", "1", "2a", "3"]);
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    near(
        xy(&p.raw_positions(), st(&p, b, 3)),
        [10.0, 15.0],
        TOL,
        "branch end",
    );
    assert_eq!(p.placements.len(), 1);
    assert_valid(&p);
}

#[test]
fn creating_a_line_refuses_as_today() {
    let mut p = empty();
    let zero = refused(p.create_line(
        meta("Main"),
        StartAt::New,
        vec![flat_u(0.0, 3.0, "a"), flat_u(0.0, 0.0, "b")],
        None,
    ));
    assert_eq!(zero, "Shot Main: 1 \u{2192} 2 has zero length");
    let negative =
        refused(p.create_line(meta("  "), StartAt::New, vec![flat_u(0.0, -1.5, "a")], None));
    assert_eq!(
        negative,
        "Shot New line: 0 \u{2192} 1 has negative length: -1.5m"
    );
    let nan = refused(p.create_line(
        meta("X"),
        StartAt::New,
        vec![flat_u(f64::NAN, 1.0, "a")],
        None,
    ));
    assert_eq!(nan, "Line has a shot with a non-finite (NaN/inf) value");
    let unknown = refused(p.create_line(meta("Y"), StartAt::Station(77), Vec::new(), None));
    assert!(unknown.contains("77"), "{unknown}");
    assert_eq!(p, empty());
}

#[test]
fn deleting_a_line_keeps_its_junctions_and_deletes_the_rest_with_their_data() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 3));
    let j = st(&p, a, 1);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 2));
    let (b1, b2) = (st(&p, b, 1), st(&p, b, 2));
    p.station_mut(j).unwrap().comment = Some("hub".to_string());
    p.station_mut(b2).unwrap().comment = Some("end".to_string());
    add_datum(&mut p, j, gps(10.0, 20.0));
    add_datum(&mut p, b1, gps(10.0001, 20.0));
    p.push_closure(b2, Some(b), st(&p, a, 3), Some(a), ClosureStrategy::Batch);
    assert_valid(&p);
    let before = p.raw_positions();
    let deleted = ok(p.delete_line(b));
    assert_eq!(deleted.position, 1);
    assert_eq!(deleted.stations_deleted, vec![b1, b2]);
    assert_eq!(deleted.datums_removed.len(), 1);
    assert_eq!(deleted.closures_removed.len(), 1);
    assert_eq!(deleted.comments_removed, vec![(b2, "end".to_string())]);
    assert_eq!(p.station(j).unwrap().comment.as_deref(), Some("hub"));
    assert_eq!(p.datums.len(), 1);
    assert_eq!(p.datums[0].station, j);
    assert!(!p.topology().is_junction(j));
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    assert_valid(&p);
}

#[test]
fn deleting_the_bridge_line_of_a_loop_promotes_the_closure() {
    // A and C are joined only through B (A:2 - B - C:0); a closure from A:0
    // to C:1 closes the loop. Deleting B leaves the closure as the only tie:
    // the sweep merges C:1 into A:0, moving C (a's piece moves onto b).
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let at1 = st(&p, a, 2);
    let b = line_from(&mut p, "B", at1, run("b", 0.0, 10.0, 1));
    let at2 = st(&p, b, 1);
    let c = line_from(&mut p, "C", at2, run("c", 270.0, 20.0, 1));
    let (a0, c1) = (st(&p, a, 0), st(&p, c, 1));
    p.push_closure(c1, Some(c), a0, Some(a), ClosureStrategy::Batch);
    assert_valid(&p);
    let before = p.raw_positions();
    let edited = p.delete_line(b).unwrap();
    assert_eq!(edited.report.sweep.promoted.len(), 1);
    assert_eq!(edited.report.sweep.promoted[0].survivor, a0);
    let after = p.raw_positions();
    near(xy(&after, a0), xy(&before, a0), TOL, "A stays");
    // C's start moved by the misclosure: C:1 was at (0, 10), A:0 at (0, 0).
    let c0 = st(&p, c, 0);
    near(xy(&after, c0), [20.0, 0.0], TOL, "C moved");
    assert!(p.closures.is_empty());
    assert_valid(&p);
}

#[test]
fn metadata_updates_follow_todays_rules() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let _b = line(&mut p, "B", run("b", 90.0, 10.0, 1));
    let taken = refused(p.update_line_meta(
        a,
        &LineUpdates {
            name: Some(Some(" B ".to_string())),
            ..LineUpdates::default()
        },
    ));
    assert_eq!(taken, "A line named \"B\" already exists");
    let empty_name = refused(p.update_line_meta(
        a,
        &LineUpdates {
            name: Some(None),
            ..LineUpdates::default()
        },
    ));
    assert_eq!(empty_name, "A line name cannot be empty");
    let date = NaiveDate::from_ymd_opt(2025, 3, 4);
    ok(p.update_line_meta(
        a,
        &LineUpdates {
            name: Some(Some("  Main ".to_string())),
            color: Some("#123456".to_string()),
            quality: Some(f64::NAN),
            surveyor: Some(Some("Ann, Bo".to_string())),
            date: Some(date),
            ..LineUpdates::default()
        },
    ));
    let line = p.line(a).unwrap();
    assert_eq!(line.name.as_deref(), Some("Main"));
    assert_eq!(line.color, "#123456");
    assert_eq!(line.quality, 1.0);
    let (_, surveyors) = crate::legacy_people("Ann, Bo");
    for shot in &line.shots {
        let credit = shot.credit.as_deref().unwrap();
        assert_eq!(credit.surveyors, surveyors);
        assert_eq!(credit.date, date);
    }
}

#[test]
fn a_new_azimuth_reference_turns_the_line_about_its_start() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 0.0, 10.0, 1));
    ok(p.set_line_azimuth_reference(
        a,
        Some(AzimuthReference {
            north: NorthReference::Magnetic,
            correction: 90.0,
            ..AzimuthReference::default()
        }),
    ));
    let positions = p.raw_positions();
    near(xy(&positions, st(&p, a, 0)), [0.0, 0.0], TOL, "start stays");
    near(
        xy(&positions, st(&p, a, 1)),
        [10.0, 0.0],
        1e-9,
        "end turned east",
    );
}

#[test]
fn reordering_lines_moves_nothing_and_needs_every_line_once() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let at3 = st(&p, a, 1);
    let b = line_from(&mut p, "B", at3, run("b", 0.0, 10.0, 2));
    let c = line(&mut p, "C", run("c", 45.0, 3.0, 1));
    let bits = |q: &Project| -> Vec<[u64; 2]> {
        q.raw_positions()
            .iter()
            .map(|(_, [x, y, _])| [x.to_bits(), y.to_bits()])
            .collect()
    };
    let before = bits(&p);
    ok(p.reorder_lines(&[c, b, a]));
    let order: Vec<u32> = p.lines().iter().map(|l| l.id).collect();
    assert_eq!(order, [c, b, a]);
    assert_eq!(bits(&p), before);
    let text = refused(p.reorder_lines(&[a, b]));
    assert_eq!(text, "The new line order must name every line exactly once");
    let text = refused(p.reorder_lines(&[a, a, b]));
    assert_eq!(text, "The new line order must name every line exactly once");
}

/// A `Tie` the migration demoted from a 0.6.0 merge cycle, on the junction
/// a deleted line leaves, goes with the line, as 0.6.0's Delete Line dropped
/// the merge (review finding A-model-edits-1). Kept, the orphan sweep promoted
/// it as the only bridge and moved all of A by the 42 m legacy misclosure.
#[test]
fn delete_line_takes_a_member_tie_on_the_junction_it_leaves() {
    // A east 0..4 from the origin; B north from A:2 (J), three shots; C east
    // from B:3, three shots; a `Tie` from J (via B) to C:3 (via C).
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 4));
    let j = st(&p, a, 2);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 3));
    let b3 = st(&p, b, 3);
    let c = line_from(&mut p, "C", b3, run("c", 90.0, 10.0, 3));
    let c3 = st(&p, c, 3);
    let tie = p.push_closure(j, Some(b), c3, Some(c), ClosureStrategy::Tie);
    assert_valid(&p);
    let before = p.raw_positions();

    let edited = p.delete_line(b).unwrap();
    assert_eq!(
        edited
            .value
            .closures_removed
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [tie],
        "the demoted merge leaves with the line"
    );
    assert!(
        edited.report.sweep.promoted.is_empty(),
        "nothing is promoted: {:?}",
        edited.report.sweep.promoted
    );
    assert!(p.closures.is_empty());
    let after = p.raw_positions();
    for i in 0..=4 {
        let s = st(&p, a, i);
        near(xy(&after, s), xy(&before, s), 1e-9, "A stays where it was");
    }
    assert_valid(&p);
}

/// Two closures join the junction J to the far station F, one via the kept
/// line A and one via the deleted line B. Deleting B must not leave the same
/// pair twice (Create Loop Closure refuses it as a duplicate): 0.6.0 skipped
/// re-anchoring the doubled closure (`are_stations_in_closure`) and its
/// `cleanup_line_references` dropped it (review finding A-model-edits-2).
#[test]
fn delete_line_drops_a_closure_the_kept_line_doubles() {
    // A east 0..4; B north from A:2 (J); E north from A:0, with F = E:3.
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 4));
    let j = st(&p, a, 2);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 3));
    let a0 = st(&p, a, 0);
    let e = line_from(&mut p, "E", a0, run("e", 0.0, 10.0, 3));
    let f = st(&p, e, 3);
    let kept = p.push_closure(j, Some(a), f, Some(e), ClosureStrategy::Batch);
    let doubled = p.push_closure(j, Some(b), f, Some(e), ClosureStrategy::Batch);
    assert_valid(&p);
    assert!(
        p.clone()
            .create_closure(j, Some(a), f, Some(e), ClosureStrategy::Batch, None, &[])
            .is_err(),
        "the pair is a duplicate once B is gone"
    );
    let before = p.raw_positions();

    let edited = p.delete_line(b).unwrap();
    assert_eq!(
        edited
            .value
            .closures_removed
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [doubled],
        "the doubled closure leaves with the line"
    );
    assert_eq!(
        p.closures.iter().map(|c| c.id).collect::<Vec<_>>(),
        [kept],
        "one closure for the pair"
    );
    assert!(edited.report.sweep.promoted.is_empty());
    let after = p.raw_positions();
    for s in [a0, st(&p, a, 4), f] {
        near(xy(&after, s), xy(&before, s), TOL, "nothing moves");
    }
    assert_valid(&p);
}
