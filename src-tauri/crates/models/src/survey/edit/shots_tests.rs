//! Shot operations: create (append, insert, prepend), update, kind, delete,
//! split at distance, and the delete preview.

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
