//! Shot operations: create (append, insert, prepend), update, kind, delete,
//! split at distance, and the delete preview.

use stalagmight_legacy::models as old;

use std::sync::Arc;

use crate::{
    LineUpdates, ShotData, ShotKind, ShotUpdates, StationRef, SurveyCredit, FEATURE_ID_STRIDE,
};

use super::super::test_helpers::gps;
use super::super::{ClosureStrategy, Project};
use super::edit_test_helpers::{
    add_datum, assert_still, assert_valid, empty, flat_u, line, line_from, names, near, ok,
    refused, run, shot_id, shot_u, st, xy,
};
use super::{EditError, ShotPlace};

const TOL: f64 = 1e-9;

/// One line `0 1 2 3` east, 10 m shots.
fn east3() -> (Project, u32) {
    let mut p = empty();
    let l = line(&mut p, "Main", run("m", 90.0, 10.0, 3));
    (p, l)
}

#[test]
fn append_names_by_the_end_rule_and_keeps_the_uuid_and_ids() {
    let (mut p, l) = east3();
    let before = p.raw_positions();
    let created = ok(p.append_shot(l, flat_u(90.0, 5.0, "new")));
    assert_eq!(created.uuid, "new");
    assert_eq!(created.index, 3);
    assert_eq!(created.shot, l * FEATURE_ID_STRIDE + 3);
    assert_eq!(names(&p, l), ["0", "1", "2", "3", "4"]);
    let after = p.raw_positions();
    assert_still(&before, &after, TOL, &[]);
    near(xy(&after, created.station), [35.0, 0.0], TOL, "new end");
    assert_valid(&p);
}

#[test]
fn insert_after_a_mid_shot_uses_the_mid_rule_and_moves_the_tail() {
    let (mut p, l) = east3();
    let before = p.raw_positions();
    let (s2, s3) = (st(&p, l, 2), st(&p, l, 3));
    let created = ok(p.create_shot(
        l,
        flat_u(90.0, 5.0, "ins"),
        ShotPlace::After(shot_id(&p, l, 0)),
    ));
    assert_eq!(created.index, 1);
    assert_eq!(names(&p, l), ["0", "1", "1a", "2", "3"]);
    let after = p.raw_positions();
    near(xy(&after, st(&p, l, 0)), [0.0, 0.0], TOL, "start stays");
    near(xy(&after, created.station), [15.0, 0.0], TOL, "new station");
    near(
        xy(&after, s2),
        [xy(&before, s2)[0] + 5.0, 0.0],
        TOL,
        "tail moved",
    );
    near(xy(&after, s3), [35.0, 0.0], TOL, "tail moved");
    // Inserting before a mid shot is the same insertion as after the one
    // before it.
    let mut q = east3().0;
    let c2 = ok(q.create_shot(
        l,
        flat_u(90.0, 5.0, "ins"),
        ShotPlace::Before(shot_id(&q, l, 1)),
    ));
    assert_eq!(c2.index, 1);
    assert_eq!(names(&q, l), names(&p, l));
    assert_eq!(q.raw_positions().slots(&q), after.slots(&p));
}

#[test]
fn prepend_names_by_the_prepend_rule_and_keeps_the_old_start_still() {
    let (mut p, l) = east3();
    let before = p.raw_positions();
    let old_start = st(&p, l, 0);
    let created = ok(p.prepend_shot(l, flat_u(90.0, 4.0, "pre")));
    assert_eq!(created.index, 0);
    assert_eq!(st(&p, l, 0), created.station);
    assert_eq!(st(&p, l, 1), old_start);
    assert_eq!(names(&p, l), ["4", "0", "1", "2", "3"]);
    let after = p.raw_positions();
    assert_still(&before, &after, TOL, &[]);
    near(xy(&after, created.station), [-4.0, 0.0], TOL, "new start");
    // Before the first shot is a prepend too.
    let mut q = east3().0;
    let c = ok(q.create_shot(
        l,
        flat_u(90.0, 4.0, "pre"),
        ShotPlace::Before(shot_id(&q, l, 0)),
    ));
    assert_eq!(st(&q, l, 0), c.station);
}

#[test]
fn creating_a_shot_refuses_as_today() {
    let (mut p, l) = east3();
    let zero = refused(p.append_shot(l, flat_u(90.0, 0.0, "z")));
    assert_eq!(zero, "The new shot in line \"Main\" has zero length");
    let negative = refused(p.append_shot(l, flat_u(90.0, -2.0, "n")));
    assert_eq!(
        negative,
        "The new shot in line \"Main\" has negative length: -2.0m"
    );
    let nan = refused(p.append_shot(l, flat_u(f64::NAN, 2.0, "x")));
    assert_eq!(nan, "Shot has a non-finite (NaN/inf) value");
    let taken = refused(p.append_shot(l, flat_u(90.0, 2.0, "m-0")));
    assert!(taken.contains("m-0"), "{taken}");
    assert_eq!(
        refused(p.append_shot(99, flat_u(90.0, 2.0, "y"))),
        crate::LINE_GONE
    );
    let gone = refused(p.create_shot(l, flat_u(90.0, 2.0, "y"), ShotPlace::After(12345)));
    assert_eq!(gone, crate::SHOT_GONE);
    // Nothing changed.
    assert_eq!(p, east3().0);
}

#[test]
fn a_new_shot_takes_its_neighbours_credit_and_a_first_shot_the_legacy_one() {
    let mut p = empty();
    let credited = ShotData {
        credit: Some(Arc::new(SurveyCredit::from_legacy(Some("Ann"), None))),
        ..flat_u(90.0, 10.0, "a")
    };
    let l = line(&mut p, "Main", vec![credited, flat_u(90.0, 10.0, "b")]);
    let c = ok(p.create_shot(
        l,
        flat_u(90.0, 1.0, "c"),
        ShotPlace::After(shot_id(&p, l, 0)),
    ));
    let shot = &p.line(l).unwrap().shots[c.index];
    assert_eq!(shot.credit, p.line(l).unwrap().shots[0].credit);
    let pre = ok(p.prepend_shot(l, flat_u(90.0, 1.0, "d")));
    assert_eq!(
        p.line(l).unwrap().shots[pre.index].credit,
        p.line(l).unwrap().shots[1].credit
    );

    // A shot-less line hands its legacy surveyor to its first shot.
    let mut q = empty();
    let lone = line(&mut q, "Lone", Vec::new());
    ok(q.update_line_meta(
        lone,
        &LineUpdates {
            surveyor: Some(Some("Bo".to_string())),
            ..LineUpdates::default()
        },
    ));
    assert_eq!(q.line(lone).unwrap().legacy_surveyor.as_deref(), Some("Bo"));
    ok(q.append_shot(lone, flat_u(0.0, 3.0, "first")));
    let line = q.line(lone).unwrap();
    assert_eq!(line.legacy_surveyor, None);
    assert_eq!(
        line.shots[0].credit.as_deref().map(|c| c.surveyors.len()),
        Some(1)
    );
}

#[test]
fn an_insert_on_a_datum_held_tail_keeps_the_tail_and_moves_the_head() {
    let (mut p, l) = east3();
    let s3 = st(&p, l, 3);
    add_datum(&mut p, s3, gps(10.0, 20.0));
    assert_valid(&p);
    let before = p.raw_positions();
    let s0 = st(&p, l, 0);
    ok(p.create_shot(
        l,
        flat_u(90.0, 5.0, "ins"),
        ShotPlace::After(shot_id(&p, l, 0)),
    ));
    let after = p.raw_positions();
    near(xy(&after, s3), xy(&before, s3), TOL, "datum station stays");
    near(
        xy(&after, s0),
        [xy(&before, s0)[0] - 5.0, xy(&before, s0)[1]],
        TOL,
        "head moved",
    );
}

#[test]
fn a_geometry_update_keeps_the_lines_start_and_a_comment_moves_nothing() {
    let (mut p, l) = east3();
    let before = p.raw_positions();
    let s3 = st(&p, l, 3);
    ok(p.update_shot(
        l,
        shot_id(&p, l, 1),
        &ShotUpdates {
            length: Some(12.0),
            ..ShotUpdates::default()
        },
    ));
    let after = p.raw_positions();
    near(xy(&after, st(&p, l, 0)), [0.0, 0.0], TOL, "start stays");
    near(
        xy(&after, s3),
        [xy(&before, s3)[0] + 2.0, 0.0],
        TOL,
        "end moves",
    );

    let snapshot = p.clone();
    ok(p.update_shot(
        l,
        shot_id(&p, l, 2),
        &ShotUpdates {
            comment: Some(Some("wet".to_string())),
            ..ShotUpdates::default()
        },
    ));
    assert_eq!(p.raw_positions(), snapshot.raw_positions());
    assert_eq!(p.line(l).unwrap().shots[2].comment.as_deref(), Some("wet"));
    // An empty comment clears it (today's rule).
    ok(p.update_shot(
        l,
        shot_id(&p, l, 2),
        &ShotUpdates {
            comment: Some(Some(String::new())),
            ..ShotUpdates::default()
        },
    ));
    assert_eq!(p.line(l).unwrap().shots[2].comment, None);
}

#[test]
fn updates_refuse_as_today() {
    let (mut p, l) = east3();
    let id = shot_id(&p, l, 1);
    let zero = refused(p.update_shot(
        l,
        id,
        &ShotUpdates {
            length: Some(0.0),
            ..ShotUpdates::default()
        },
    ));
    assert_eq!(zero, "Shot Main: 1 \u{2192} 2 has zero length");
    let nan = refused(p.update_shot(
        l,
        id,
        &ShotUpdates {
            depth_in: Some(f64::INFINITY),
            ..ShotUpdates::default()
        },
    ));
    assert_eq!(nan, "Shot update has a non-finite (NaN/inf) value");
    assert_eq!(
        refused(p.update_shot(l, 7777, &ShotUpdates::default())),
        crate::SHOT_GONE
    );
}

#[test]
fn set_kind_changes_only_the_kind() {
    let (mut p, l) = east3();
    let before = p.raw_positions();
    ok(p.set_shot_kind(l, shot_id(&p, l, 1), ShotKind::Jump));
    assert_eq!(p.line(l).unwrap().shots[1].kind, ShotKind::Jump);
    assert_eq!(p.raw_positions(), before);
}

#[test]
fn delete_first_mid_and_last_shot_remove_one_station_each() {
    let (mut p, l) = east3();
    let before = p.raw_positions();
    let (s0, s1, s2, s3) = (st(&p, l, 0), st(&p, l, 1), st(&p, l, 2), st(&p, l, 3));
    // First shot: station 0 goes; the rest stay put.
    let gone = ok(p.delete_shot(l, shot_id(&p, l, 0)));
    assert_eq!(gone.station, s0);
    assert!(gone.station_deleted);
    assert!(p.station(s0).is_none());
    assert_eq!(p.line_stations(l).unwrap(), [s1, s2, s3]);
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    // Mid shot (now shot 0 of 2 is first; add one so a mid exists).
    ok(p.append_shot(l, flat_u(90.0, 10.0, "x")));
    let s4 = st(&p, l, 3);
    let before = p.raw_positions();
    let gone = ok(p.delete_shot(l, shot_id(&p, l, 1)));
    assert_eq!(gone.station, s3);
    assert_eq!(p.line_stations(l).unwrap(), [s1, s2, s4]);
    let after = p.raw_positions();
    near(xy(&after, s1), xy(&before, s1), TOL, "head stays");
    near(
        xy(&after, s4),
        [xy(&before, s4)[0] - 10.0, 0.0],
        TOL,
        "tail closes the gap",
    );
    // Last shot.
    let gone = ok(p.delete_shot(l, shot_id(&p, l, 1)));
    assert_eq!(gone.station, s4);
    assert_eq!(p.line_stations(l).unwrap(), [s1, s2]);
    assert_valid(&p);
}

#[test]
fn a_deleted_station_takes_its_comment_datums_and_closures_and_a_junction_stays() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 4));
    let j = st(&p, a, 2);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 2));
    let (a1, a4) = (st(&p, a, 1), st(&p, a, 4));
    let b2 = st(&p, b, 2);
    p.station_mut(a4).unwrap().comment = Some("sump".to_string());
    p.station_mut(j).unwrap().comment = Some("junction".to_string());
    add_datum(&mut p, a4, gps(10.0, 20.0));
    // Two loop closures from the branch's end.
    p.push_closure(b2, Some(b), a4, Some(a), ClosureStrategy::Batch);
    p.push_closure(b2, Some(b), a1, Some(a), ClosureStrategy::Batch);
    assert_valid(&p);

    // A's last shot takes A:4 (not a junction) with its comment, its datum
    // and the closure on it.
    let preview = p.preview_delete_shot(a, shot_id(&p, a, 3)).unwrap();
    assert_eq!(preview.removed_station, a4);
    assert!(preview.station_stays_on.is_empty());
    assert_eq!(preview.removed_comment.as_deref(), Some("sump"));
    assert_eq!(preview.removed_datums.len(), 1);
    assert_eq!(preview.removed_closures.len(), 1);
    assert!(!preview.shift_blocked);
    let gone = ok(p.delete_shot(a, shot_id(&p, a, 3)));
    assert!(gone.station_deleted);
    assert_eq!(gone.closures_removed.len(), 1);
    assert_eq!(gone.datums_removed.len(), 1);
    assert!(p.datums.is_empty());
    assert_eq!(p.closures.len(), 1);
    assert_valid(&p);

    // B's first shot takes the junction off B only: it stays on A with its
    // comment; B is cut loose from A, so the closure B:2 - A:1 is now the
    // only tie between them and the sweep promotes it: B moves onto A:1.
    let preview = p.preview_delete_shot(b, shot_id(&p, b, 0)).unwrap();
    assert_eq!(preview.removed_station, j);
    assert_eq!(preview.station_stays_on, vec![a]);
    assert_eq!(preview.removed_comment, None);
    assert_eq!(preview.promoted_closures.len(), 1);
    let before = p.raw_positions();
    let edited = p.delete_shot(b, shot_id(&p, b, 0)).unwrap();
    assert!(!edited.value.station_deleted);
    assert_eq!(p.station(j).unwrap().comment.as_deref(), Some("junction"));
    assert_eq!(edited.report.sweep.promoted.len(), 1);
    assert!(p.closures.is_empty());
    let after = p.raw_positions();
    let promotion = &edited.report.sweep.promoted[0];
    assert_eq!(promotion.survivor, a1);
    assert_eq!(promotion.retired, b2);
    near(xy(&after, a1), xy(&before, a1), TOL, "A:1 stays");
    assert_still(&before, &after, TOL, &[st(&p, b, 0), a1]);
    assert!(p.topology().is_junction(a1));
    assert_eq!(p.station(a1).unwrap().aliases.len(), 1);
    assert_valid(&p);
}

#[test]
fn a_mid_delete_in_a_loop_or_between_two_datums_is_refused() {
    // Loop: a closure ties the two sides of the shot.
    let (mut p, l) = east3();
    let (s0, s3) = (st(&p, l, 0), st(&p, l, 3));
    let mut looped = p.clone();
    looped.push_closure(s0, Some(l), s3, Some(l), ClosureStrategy::Batch);
    let text = refused(looped.delete_shot(l, shot_id(&looped, l, 1)));
    assert_eq!(
        text,
        "This shot is part of a loop (the two sides are already connected). Use split instead."
    );
    let preview = looped
        .preview_delete_shot(l, shot_id(&looped, l, 1))
        .unwrap();
    assert!(preview.shift_blocked);

    // Both sides held: a placement datum on the head, and the tail reaches
    // another piece's placement datum through a branch and a closure.
    add_datum(&mut p, s0, gps(10.0, 20.0));
    let s2 = st(&p, l, 2);
    let other = line_from(&mut p, "Other", s2, run("o", 0.0, 10.0, 1));
    let tip = st(&p, other, 1);
    // A second component with its own datum, tied to the tip by a closure
    // (allowed between two placed pieces).
    let far = line(&mut p, "Far", run("f", 0.0, 5.0, 1));
    let far0 = st(&p, far, 0);
    add_datum(&mut p, far0, gps(10.0001, 20.0001));
    p.push_closure(tip, Some(other), far0, Some(far), ClosureStrategy::Batch);
    assert_valid(&p);
    let text = refused(p.delete_shot(l, shot_id(&p, l, 1)));
    assert_eq!(
        text,
        "Both sides are anchored to a zero datum. Use split instead."
    );
}

#[test]
fn split_at_distance_inserts_a_mid_station_and_keeps_positions() {
    let mut p = empty();
    let l = line(
        &mut p,
        "Main",
        vec![
            shot_u(90.0, 10.0, 0.0, 4.0, "a"),
            shot_u(45.0, 8.0, 4.0, 4.0, "b"),
        ],
    );
    let before = p.raw_positions();
    let id = shot_id(&p, l, 0);
    let made = ok(p.split_shot_at_distance(l, id, 2.5, Some("head".to_string())));
    let line = p.line(l).unwrap();
    assert_eq!(line.shots.len(), 3);
    assert_eq!(line.shots[0].uuid, "head");
    assert_eq!(line.shots[1].uuid, "a");
    assert_eq!(line.shots[1].id, id, "the tail keeps the shot's id");
    assert_eq!(line.shots[0].length, 2.5);
    assert_eq!(line.shots[0].depth_out, 1.0);
    assert_eq!(line.shots[1].depth_in, 1.0);
    assert_eq!(names(&p, l), ["0", "0a", "1", "2"]);
    assert_eq!(made.station, st(&p, l, 1));
    let after = p.raw_positions();
    assert_still(&before, &after, 1e-9, &[]);
    // Refusals, today's split math.
    let short = refused(p.split_shot_at_distance(l, id, 0.005, None));
    assert_eq!(short, "Both sides of the split must be at least 0.01m long");
    let outside = refused(p.split_shot_at_distance(l, id, 50.0, None));
    assert!(
        outside.starts_with("Distance must be strictly between 0"),
        "{outside}"
    );
}

#[test]
fn the_new_station_gets_a_fresh_id_and_ids_are_never_reused() {
    let (mut p, l) = east3();
    let next = p.counters.next_station_id;
    let c = ok(p.append_shot(l, flat_u(90.0, 1.0, "q")));
    assert_eq!(c.station, next);
    ok(p.delete_shot(l, shot_id(&p, l, 3)));
    let c2 = ok(p.append_shot(l, flat_u(90.0, 1.0, "r")));
    assert_eq!(c2.station, next + 1);
}

#[test]
fn a_refused_edit_leaves_the_project_as_it_was() {
    let (mut p, l) = east3();
    let snapshot = p.clone();
    assert!(matches!(p.delete_shot(l, 4242), Err(EditError::Refused(_))));
    assert_eq!(p, snapshot);
    let slot = StationRef::new(l, 0);
    assert_eq!(p.station_at(slot), snapshot.station_at(slot));
}

// ----- a delete at a junction: today's member rules (S13) -------------------

/// The frozen 0.6.0 model (`stalagmight-legacy`).
type Old = stalagmight_legacy::LegacyProject;

/// 0.6.0's twin of a survey: a line per `shots` list (north or east flat
/// 10 m shots by azimuth), then each `(moving, onto)` merge.
fn today(lines: &[Vec<f64>], merges: &[((u32, u32), (u32, u32))]) -> Old {
    let mut old = Old::new("legacy".to_string(), None);
    for azimuths in lines {
        let shots = azimuths
            .iter()
            .map(|&azimuth| old::ShotData {
                azimuth_in: azimuth,
                azimuth_out: azimuth,
                length: 10.0,
                ..old::ShotData::default()
            })
            .collect();
        old.add_line(old::LineData {
            shots,
            quality: 1.0,
            ..old::LineData::default()
        });
    }
    for &((la, ia), (lb, ib)) in merges {
        old.merge_stations(old::StationRef::new(la, ia), old::StationRef::new(lb, ib))
            .unwrap();
    }
    old
}

/// Every slot of `p` sits where 0.6.0 puts the same slot in `old`.
fn assert_like_today(p: &Project, old: &Old) {
    let today: std::collections::BTreeMap<StationRef, [f64; 2]> = old
        .compute_station_coordinates()
        .into_iter()
        .map(|s| {
            (
                StationRef::new(s.station_ref.line_id, s.station_ref.station_index),
                [s.coordinates.x, s.coordinates.y],
            )
        })
        .collect();
    let slots = p.raw_positions().slots(p);
    assert_eq!(slots.len(), today.len(), "slot count");
    for slot in slots {
        near(
            [slot.x, slot.y],
            today[&slot.slot],
            1e-9,
            &format!("{:?}", slot.slot),
        );
    }
}

#[test]
fn deleting_an_end_shot_at_a_junction_leaves_it_as_today() {
    // A east 0..3; B north from A:2 (its first station is the junction).
    // Today the delete drops B's member there and B stays where it is: its
    // new first station is not merged to the junction.
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 3));
    let j = st(&p, a, 2);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 3));
    let mut old = today(&[vec![90.0; 3], vec![0.0; 3]], &[((b, 0), (a, 2))]);
    assert_like_today(&p, &old);
    let before = p.raw_positions();
    let gone = ok(p.delete_shot(b, shot_id(&p, b, 0)));
    assert!(!gone.station_deleted);
    assert_eq!(gone.station, j);
    assert!(p.topology().lines_of(j) == [a]);
    assert!(!p.topology().connected(j, st(&p, b, 0)));
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    let shot = old.lines[1].shots[0].id;
    old.remove_shot_with_rewire(b, shot).unwrap();
    assert_like_today(&p, &old);
    assert_valid(&p);

    // B ending on A:2 (built south from the junction, then reversed).
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 3));
    let j = st(&p, a, 2);
    let b = line_from(&mut p, "B", j, run("b", 180.0, 10.0, 3));
    ok(p.reverse_line(b, false));
    assert_eq!(st(&p, b, 3), j);
    let mut old = today(&[vec![90.0; 3], vec![0.0; 3]], &[((b, 3), (a, 2))]);
    assert_like_today(&p, &old);
    let gone = ok(p.delete_shot(b, shot_id(&p, b, 2)));
    assert_eq!(gone.station, j);
    let shot = old.lines[1].shots[2].id;
    old.remove_shot_with_rewire(b, shot).unwrap();
    assert_like_today(&p, &old);
    assert_valid(&p);

    // A one-shot line: it is left shot-less on its far station.
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 3));
    let j = st(&p, a, 2);
    let c = line_from(&mut p, "C", j, run("c", 0.0, 10.0, 1));
    let tip = st(&p, c, 1);
    let mut old = today(&[vec![90.0; 3], vec![0.0]], &[((c, 0), (a, 2))]);
    ok(p.delete_shot(c, shot_id(&p, c, 0)));
    assert_eq!(p.line_stations(c).unwrap(), [tip]);
    let shot = old.lines[1].shots[0].id;
    old.remove_shot_with_rewire(c, shot).unwrap();
    assert_like_today(&p, &old);
    assert_valid(&p);
}

/// A east 0..4 (placed at the origin); B north from A:2, three shots; a
/// `Tie` from the junction (via B, B's member there in 0.6.0) to `to`.
fn junction_with_a_member_tie(to: impl Fn(&Project, u32) -> u32) -> (Project, u32, u32, u64) {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 4));
    let j = st(&p, a, 2);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 3));
    let far = to(&p, b);
    let id = p.push_closure(j, Some(b), far, Some(b), ClosureStrategy::Tie);
    assert_valid(&p);
    (p, a, b, id)
}

#[test]
fn a_tie_on_the_lines_slot_at_a_junction_leaves_with_it() {
    // A 0.6.0 same-line merge of B (its start with its end, 30 m apart):
    // today the delete took it with B's member and B stayed put.
    let (mut p, a, b, tie) = junction_with_a_member_tie(|p, b| st(p, b, 3));
    let preview = p.preview_delete_shot(b, shot_id(&p, b, 0)).unwrap();
    assert_eq!(
        preview
            .removed_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [tie]
    );
    assert!(preview.promoted_closures.is_empty());
    let impact = p.delete_shot_impact(&preview);
    assert!(impact.closure_to_merge.is_empty());
    assert_eq!(impact.removed_closures.len(), 1);
    let before = p.raw_positions();
    let edited = p.delete_shot(b, shot_id(&p, b, 0)).unwrap();
    assert_eq!(
        edited
            .value
            .closures_removed
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [tie]
    );
    assert!(edited.report.sweep.promoted.is_empty());
    assert!(p.closures.is_empty());
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    assert_eq!(p.topology().lines_of(st(&p, a, 2)), [a]);
    assert_valid(&p);

    // The same `Tie` attached via A (A's member's merge in 0.6.0) is not
    // B's to take: it stays, is the only bridge, and the sweep promotes it.
    let (mut p, a, b, tie) = junction_with_a_member_tie(|p, b| st(p, b, 3));
    p.closures[0].a_via = Some(a);
    let edited = p.delete_shot(b, shot_id(&p, b, 0)).unwrap();
    assert!(edited.value.closures_removed.is_empty());
    assert_eq!(edited.report.sweep.promoted[0].closure.id, tie);
    assert_valid(&p);
}

#[test]
fn a_co_located_tie_on_the_lines_slot_is_re_stitched_as_today() {
    // B goes north, east, then back to the junction's point: its last
    // station sits on the junction. Today the delete re-stitched the two
    // co-located members by a replacement merge: the `Tie` stays and the
    // sweep promotes it, moving nothing.
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 4));
    let j = st(&p, a, 2);
    let b = line_from(
        &mut p,
        "B",
        j,
        vec![
            flat_u(0.0, 10.0, "b-0"),
            flat_u(90.0, 10.0, "b-1"),
            flat_u(180.0, 10.0, "b-2"),
            flat_u(270.0, 10.0, "b-3"),
        ],
    );
    let back = st(&p, b, 4);
    let id = p.push_closure(j, Some(b), back, Some(b), ClosureStrategy::Tie);
    assert_valid(&p);
    let before = p.raw_positions();
    let edited = p.delete_shot(b, shot_id(&p, b, 0)).unwrap();
    assert!(edited.value.closures_removed.is_empty());
    let promoted = &edited.report.sweep.promoted;
    assert_eq!(promoted.len(), 1);
    assert_eq!(promoted[0].closure.id, id);
    // B's last station is folded into the junction, which keeps its id.
    assert_eq!(promoted[0].survivor, j);
    assert_eq!(p.topology().lines_of(j), [a, b]);
    assert_eq!(st(&p, b, 3), j);
    assert_still(&before, &p.raw_positions(), 1e-6, &[back]);
    assert_valid(&p);
}

#[test]
fn a_closure_on_the_lines_slot_moves_the_junctions_side_as_today() {
    // A east 0..3; B north from A:1; a closure from the junction (via B) to
    // B:2, closing 20 m of B as a loop. Deleting B's first shot cuts B
    // loose, and the closure is the only bridge: today it was re-anchored
    // onto A's member and promoted, A's side moving onto B:2.
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 3));
    let j = st(&p, a, 1);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 2));
    let b2 = st(&p, b, 2);
    p.push_closure(j, Some(b), b2, Some(b), ClosureStrategy::Batch);
    assert_valid(&p);
    let mut old = today(&[vec![90.0; 3], vec![0.0; 2]], &[((b, 0), (a, 1))]);
    old.create_loop_closure(
        old::StationRef::new(b, 0),
        old::StationRef::new(b, 2),
        old::ClosureStrategy::Batch,
    )
    .unwrap();
    assert_like_today(&p, &old);
    let preview = p.preview_delete_shot(b, shot_id(&p, b, 0)).unwrap();
    assert_eq!(preview.promoted_closures.len(), 1);
    assert_eq!(preview.promoted_closures[0].retired, j);
    let before = p.raw_positions();
    let edited = p.delete_shot(b, shot_id(&p, b, 0)).unwrap();
    assert_eq!(edited.report.sweep.promoted, preview.promoted_closures);
    let after = p.raw_positions();
    near(xy(&after, b2), xy(&before, b2), TOL, "B stays");
    near(xy(&after, b2), [10.0, 20.0], TOL, "A's side moved onto B:2");
    let shot = old.lines[1].shots[0].id;
    old.remove_shot_with_rewire(b, shot).unwrap();
    assert_like_today(&p, &old);
    assert_valid(&p);

    // With a datum on A, A stays and B moves onto the junction instead.
    let mut q = empty();
    let a = line(&mut q, "A", run("a", 90.0, 10.0, 3));
    let j = st(&q, a, 1);
    let a0 = st(&q, a, 0);
    add_datum(&mut q, a0, gps(20.5, -87.3));
    let b = line_from(&mut q, "B", j, run("b", 0.0, 10.0, 2));
    let b2 = st(&q, b, 2);
    q.push_closure(j, Some(b), b2, Some(b), ClosureStrategy::Batch);
    let before = q.raw_positions();
    let edited = q.delete_shot(b, shot_id(&q, b, 0)).unwrap();
    assert_eq!(edited.report.sweep.promoted[0].survivor, j);
    near(xy(&q.raw_positions(), j), xy(&before, j), TOL, "A stays");
    assert_valid(&q);
}

#[test]
fn a_range_delete_at_a_junction_follows_the_member_rules() {
    // The `Tie` of the first test, taken off by a prefix range (B's first
    // two shots: the junction and B:1 leave B) and by a split.
    let (mut p, _, b, tie) = junction_with_a_member_tie(|p, b| st(p, b, 3));
    let (first, last) = (shot_id(&p, b, 0), shot_id(&p, b, 1));
    let preview = p.preview_delete_shot_range(b, first, last).unwrap();
    assert_eq!(
        preview
            .removed_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [tie]
    );
    assert!(preview.promoted_closures.is_empty());
    let before = p.raw_positions();
    let done = ok(p.delete_shot_range(b, first, last, &super::RangeDeleteMode::Shift));
    assert_eq!(done.closures_removed.len(), 1);
    assert!(p.closures.is_empty());
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    assert_valid(&p);
}

/// Review finding UX-5: the delete preview says what Split would weld. A
/// shot inside a loop closed by a Batch closure: splitting there leaves the
/// closure the only connection, so it becomes a junction and the far piece
/// moves by the misclosure (0.6.0's Delete Shot + Split did the same; the
/// parity test is in persistence's migrate_tests). A plain line welds nothing.
#[test]
fn the_delete_preview_says_what_split_would_weld() {
    // Main east 0..4, held by a datum at 0; Loop north from Main:1 round to
    // 1 m north of Main:3.
    let mut p = empty();
    let main = line(&mut p, "Main", run("m", 90.0, 10.0, 4));
    let m0 = st(&p, main, 0);
    add_datum(&mut p, m0, gps(10.0, 20.0));
    let m1 = st(&p, main, 1);
    let looped = line_from(
        &mut p,
        "Loop",
        m1,
        vec![
            flat_u(0.0, 10.0, "l0"),
            flat_u(90.0, 20.0, "l1"),
            flat_u(180.0, 9.0, "l2"),
        ],
    );
    let (tip, m3) = (st(&p, looped, 3), st(&p, main, 3));
    p.push_closure(tip, Some(looped), m3, Some(main), ClosureStrategy::Batch);
    assert_valid(&p);
    let preview = p.preview_delete_shot(main, shot_id(&p, main, 2)).unwrap();
    let weld = preview.split_weld.expect("an interior shot has a split preview");
    assert_eq!(weld.promoted.len(), 1, "the closure becomes a junction");
    assert!((weld.max_move_m - 1.0).abs() < 1e-9, "{}", weld.max_move_m);

    // The edit does what the preview said.
    let before = p.raw_positions();
    let edited = p
        .delete_shot_range(
            main,
            shot_id(&p, main, 2),
            shot_id(&p, main, 2),
            &super::RangeDeleteMode::Split {
                name_a: "Main A".to_string(),
                name_b: "Main B".to_string(),
            },
        )
        .unwrap();
    assert_eq!(edited.report.sweep.promoted, weld.promoted);
    let m4 = st(&p, edited.value.new_line.unwrap(), 1);
    near(
        xy(&p.raw_positions(), m4),
        [xy(&before, m4)[0], xy(&before, m4)[1] + 1.0],
        1e-9,
        "the far piece moves onto the closure's end",
    );

    // Off the loop: nothing welds, nothing moves; edge shots have no split.
    let mut q = empty();
    let plain = line(&mut q, "Plain", run("p", 90.0, 10.0, 3));
    let preview = q.preview_delete_shot(plain, shot_id(&q, plain, 1)).unwrap();
    let weld = preview.split_weld.unwrap();
    assert!(weld.promoted.is_empty());
    assert_eq!(weld.max_move_m, 0.0);
    let edge = q.preview_delete_shot(plain, shot_id(&q, plain, 0)).unwrap();
    assert!(edge.split_weld.is_none());
}
