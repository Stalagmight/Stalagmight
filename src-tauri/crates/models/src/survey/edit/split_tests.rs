//! Split Line Here: the halves share the split station.

use crate::SplitAt;

use super::super::test_helpers::gps;
use super::super::ClosureStrategy;
use super::edit_test_helpers::{
    add_datum, assert_still, assert_valid, empty, line, line_from, names, ok, refused, run,
    shot_id, st,
};
use super::SplitOptions;

fn options(a: &str, b: &str) -> SplitOptions {
    SplitOptions {
        name_a: a.to_string(),
        name_b: b.to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
    }
}

#[test]
fn a_split_at_a_station_shares_it_and_moves_nothing() {
    let mut p = empty();
    let l = line(&mut p, "Main", run("m", 90.0, 10.0, 4));
    let k = st(&p, l, 2);
    p.station_mut(k).unwrap().comment = Some("cut".to_string());
    let branch = line_from(&mut p, "Branch", k, run("b", 0.0, 5.0, 1));
    let at1 = st(&p, l, 4);
    add_datum(&mut p, at1, gps(10.0, 20.0));
    p.datums[0].via = Some(l);
    p.line_mut(l)
        .unwrap()
        .hand_moves
        .insert(k, [0.0, 0.0, 0.25]);
    let tip = st(&p, branch, 1);
    p.push_closure(
        tip,
        Some(branch),
        st(&p, l, 4),
        Some(l),
        ClosureStrategy::Batch,
    );
    assert_valid(&p);
    let before = p.raw_positions();
    let preview = p
        .preview_split_line(
            l,
            SplitAt::Station { station_index: 2 },
            &options("Front", "Back"),
        )
        .unwrap();
    assert_eq!(preview.station, Some(k));
    assert_eq!((preview.shots_a, preview.shots_b), (2, 2));
    assert!(preview.hand_move_copied);
    assert!(preview.promoted_closures.is_empty());
    let second = ok(p.split_line(
        l,
        SplitAt::Station { station_index: 2 },
        &options("Front", "Back"),
    ));
    assert_eq!(second, preview.new_line);
    assert_eq!(p.line_stations(l).unwrap().last(), Some(&k));
    assert_eq!(p.line_stations(second).unwrap()[0], k);
    assert_eq!(names(&p, second), ["2", "3", "4"]);
    assert!(p.topology().is_junction(k));
    assert_eq!(p.station(k).unwrap().comment.as_deref(), Some("cut"));
    assert_eq!(p.line(second).unwrap().hand(k), [0.0, 0.0, 0.25]);
    assert_eq!(p.line(l).unwrap().hand(k), [0.0, 0.0, 0.25]);
    // The datum and the closure end follow their station to the second half.
    assert_eq!(p.datums[0].via, Some(second));
    assert_eq!(p.closures[0].b_via, Some(second));
    assert_eq!(p.line(l).unwrap().color, "#111111");
    assert_eq!(p.line(second).unwrap().color, "#222222");
    assert_still(&before, &p.raw_positions(), 1e-12, &[]);
    for (s, [_, _, z]) in before.iter() {
        assert_eq!(p.raw_positions().z(s), Some(z), "depth of {s}");
    }
    assert_valid(&p);
}

#[test]
fn a_split_inside_a_shot_inserts_the_shared_station_or_snaps_to_an_end() {
    let mut p = empty();
    let l = line(&mut p, "Main", run("m", 90.0, 10.0, 3));
    let before = p.raw_positions();
    let id = shot_id(&p, l, 1);
    let second = ok(p.split_line(
        l,
        SplitAt::Shot {
            shot_id: id,
            distance: 4.0,
        },
        &options("A", "B"),
    ));
    assert_eq!(p.line(l).unwrap().shots.len(), 2);
    assert_eq!(p.line(second).unwrap().shots.len(), 2);
    let shared = st(&p, second, 0);
    assert_eq!(p.station(shared).unwrap().name, "1a");
    assert_still(&before, &p.raw_positions(), 1e-9, &[]);
    // Within MIN_SPLIT_CHILD_LENGTH_M of the end: the cut snaps to station 1.
    let mut q = empty();
    let m = line(&mut q, "Main", run("m", 90.0, 10.0, 3));
    let id = shot_id(&q, m, 1);
    let second = ok(q.split_line(
        m,
        SplitAt::Shot {
            shot_id: id,
            distance: 0.005,
        },
        &options("A", "B"),
    ));
    assert_eq!(st(&q, second, 0), st(&q, m, 1));
    assert_eq!(q.line(m).unwrap().shots.len(), 1);
}

#[test]
fn a_split_refuses_as_today() {
    let mut p = empty();
    let l = line(&mut p, "Main", run("m", 90.0, 10.0, 3));
    let _other = line(&mut p, "Other", run("o", 0.0, 1.0, 1));
    let end = refused(p.split_line(l, SplitAt::Station { station_index: 3 }, &options("A", "B")));
    assert_eq!(
        end,
        "Station 3 is the first or last station of Main; split at a station in the middle"
    );
    let first = refused(p.split_line(l, SplitAt::Station { station_index: 0 }, &options("A", "B")));
    assert!(
        first.starts_with("Station 0 is the first or last"),
        "{first}"
    );
    let nameless =
        refused(p.split_line(l, SplitAt::Station { station_index: 1 }, &options(" ", "B")));
    assert_eq!(nameless, "Each half of a split needs a name");
    let taken = refused(p.split_line(
        l,
        SplitAt::Station { station_index: 1 },
        &options("Other", "B"),
    ));
    assert_eq!(taken, "A line named \"Other\" already exists");
    let id = shot_id(&p, l, 0);
    let outside = refused(p.split_line(
        l,
        SplitAt::Shot {
            shot_id: id,
            distance: 11.0,
        },
        &options("A", "B"),
    ));
    assert_eq!(outside, "Distance 11 is outside the shot (0 to 10 m)");
    // Keeping the line's own name on a half is fine.
    ok(p.split_line(
        l,
        SplitAt::Station { station_index: 1 },
        &options("Main", "Main 2"),
    ));
}
