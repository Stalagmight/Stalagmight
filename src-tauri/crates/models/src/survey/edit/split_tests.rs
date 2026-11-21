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

/// The Project stats count distinct stations, as the file's summary does,
/// and junctions (review finding UX-8: they counted slots and the wire's
/// merge pairs, so a Split Line Here, which adds no station and merges
/// nothing, raised Stations by one).
#[test]
fn stats_count_stations_once_and_a_split_adds_a_junction_not_a_station() {
    let mut p = empty();
    let l = line(&mut p, "Main", run("m", 90.0, 10.0, 4));
    let k = st(&p, l, 2);
    line_from(&mut p, "Branch", k, run("b", 0.0, 5.0, 3));
    let stats = |p: &super::super::Project| p.survey_stats(&p.raw_positions(), false);
    let before = stats(&p);
    assert_eq!(before.station_count, 8, "5 on Main, 3 more on Branch");
    assert_eq!(before.slot_count, 9);
    assert_eq!(before.junction_count, 1);
    let wire = before.to_project_stats();
    assert_eq!(
        wire.station_count,
        p.stations.len(),
        "the file summary's count"
    );
    assert_eq!(wire.junction_count, 1);

    ok(p.split_line(
        l,
        SplitAt::Station { station_index: 3 },
        &options("Front", "Back"),
    ));
    let after = stats(&p).to_project_stats();
    assert_eq!(after.station_count, 8, "a split adds no station");
    assert_eq!(
        after.junction_count, 2,
        "the halves share the split station"
    );
    assert_eq!(after.station_count, p.stations.len());
}

/// Review finding UX-4: what a line knew its stations by follows them to the
/// second half. Main's station 3 is a junction Main knows by an alias (it was
/// Branch's station merged into Main's), and the split station 2 is one Main
/// knows by an alias too, its name shared with Main's own station 4: after
/// Split Line Here at 2 the hints name the second half, which holds both, and
/// neither half holds two names that only the exemption allowed.
#[test]
fn a_split_moves_the_alias_hints_with_their_stations() {
    use super::super::StationAlias;
    let mut p = empty();
    let l = line(&mut p, "Main", run("m", 90.0, 10.0, 5));
    let (k, m3, m4) = (st(&p, l, 2), st(&p, l, 3), st(&p, l, 4));
    line_from(&mut p, "Branch", k, run("b", 0.0, 5.0, 1));
    let name4 = p.station(m4).unwrap().name.clone();
    p.station_mut(k).unwrap().name.clone_from(&name4);
    for (station, id) in [(k, 50), (m3, 51)] {
        p.station_mut(station).unwrap().aliases.push(StationAlias {
            id,
            name: Some("old".to_string()),
            line: Some(l),
        });
    }
    p.counters.next_station_id = 52;
    assert_valid(&p);
    let second = ok(p.split_line(
        l,
        SplitAt::Station { station_index: 2 },
        &options("Front", "Back"),
    ));
    assert_valid(&p);
    assert_eq!(p.station(m3).unwrap().aliases[0].line, Some(second));
    assert_eq!(
        p.station(k).unwrap().aliases[0].line,
        Some(second),
        "its clash went to the second half"
    );
    assert_eq!(p.station(k).unwrap().name, name4, "nothing renamed");
}
