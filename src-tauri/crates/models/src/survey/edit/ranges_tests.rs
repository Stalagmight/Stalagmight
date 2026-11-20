//! Delete a shot range: gap-close, split, prefix, suffix, and the preview.

use crate::RangeDeleteKind;

use super::super::test_helpers::gps;
use super::super::{ClosureStrategy, Project};
use super::edit_test_helpers::{
    add_datum, assert_still, assert_valid, empty, line, line_from, names, near, ok, refused, run,
    shot_id, st, xy,
};
use super::RangeDeleteMode;

const TOL: f64 = 1e-9;

/// One line of five 10 m shots east: stations at x = 0, 10, .., 50.
fn east5() -> (Project, u32) {
    let mut p = empty();
    let l = line(&mut p, "Main", run("m", 90.0, 10.0, 5));
    (p, l)
}

fn split(a: &str, b: &str) -> RangeDeleteMode {
    RangeDeleteMode::Split {
        name_a: a.to_string(),
        name_b: b.to_string(),
    }
}

#[test]
fn an_interior_gap_close_removes_the_range_and_its_end_station_and_closes_the_gap() {
    let (mut p, l) = east5();
    let before = p.raw_positions();
    let s: Vec<u32> = (0..=5).map(|i| st(&p, l, i)).collect();
    let (first, last) = (shot_id(&p, l, 2), shot_id(&p, l, 1));
    let preview = p.preview_delete_shot_range(l, first, last).unwrap();
    assert_eq!(preview.kind, RangeDeleteKind::Interior);
    assert_eq!((preview.start, preview.end), (1, 2));
    let removed: Vec<u32> = preview.removed_slots.iter().map(|(_, s)| *s).collect();
    assert_eq!(removed, [s[2], s[3]]);
    assert!(!preview.shift_blocked);
    assert_eq!(preview.closures_removed_across_one_shot, Some(Vec::new()));
    let done = ok(p.delete_shot_range(l, first, last, &RangeDeleteMode::Shift));
    assert_eq!(done.stations_deleted, [s[2], s[3]]);
    assert_eq!(p.line_stations(l).unwrap(), [s[0], s[1], s[4], s[5]]);
    let after = p.raw_positions();
    near(xy(&after, s[1]), xy(&before, s[1]), TOL, "head stays");
    near(xy(&after, s[4]), [20.0, 0.0], TOL, "tail closes the gap");
    near(xy(&after, s[5]), [30.0, 0.0], TOL, "tail closes the gap");
    assert_valid(&p);
}

#[test]
fn a_prefix_and_a_suffix_drop_their_stations_and_nothing_moves() {
    let (mut p, l) = east5();
    let before = p.raw_positions();
    let s: Vec<u32> = (0..=5).map(|i| st(&p, l, i)).collect();
    let done = ok(p.delete_shot_range(
        l,
        shot_id(&p, l, 0),
        shot_id(&p, l, 1),
        &RangeDeleteMode::Shift,
    ));
    assert_eq!(done.kind, RangeDeleteKind::Prefix);
    assert_eq!(p.line_stations(l).unwrap(), &s[2..]);
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    let done = ok(p.delete_shot_range(
        l,
        shot_id(&p, l, 2),
        shot_id(&p, l, 1),
        &RangeDeleteMode::Shift,
    ));
    assert_eq!(done.kind, RangeDeleteKind::Suffix);
    assert_eq!(p.line_stations(l).unwrap(), &s[2..=3]);
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    assert_eq!(names(&p, l), ["2", "3"]);
    assert_valid(&p);
}

#[test]
fn the_whole_line_and_a_split_of_an_end_range_are_refused() {
    let (mut p, l) = east5();
    let whole = refused(p.delete_shot_range(
        l,
        shot_id(&p, l, 0),
        shot_id(&p, l, 4),
        &RangeDeleteMode::Shift,
    ));
    assert_eq!(
        whole,
        "Cannot shift-delete an entire line; delete the line instead."
    );
    let end =
        refused(p.delete_shot_range(l, shot_id(&p, l, 0), shot_id(&p, l, 1), &split("A", "B")));
    assert!(
        end.starts_with("Range is not interior to the line"),
        "{end}"
    );
    let same =
        refused(p.delete_shot_range(l, shot_id(&p, l, 1), shot_id(&p, l, 2), &split("A", "A")));
    assert_eq!(same, "A line named \"A\" already exists");
    assert_eq!(p, east5().0);
}

#[test]
fn a_split_keeps_both_boundary_stations_and_the_tail_as_a_new_line() {
    let (mut p, l) = east5();
    let before = p.raw_positions();
    let s: Vec<u32> = (0..=5).map(|i| st(&p, l, i)).collect();
    p.station_mut(s[3]).unwrap().comment = Some("boundary".to_string());
    let tail_uuids: Vec<String> = p.line(l).unwrap().shots[3..]
        .iter()
        .map(|s| s.uuid.clone())
        .collect();
    let done = ok(p.delete_shot_range(
        l,
        shot_id(&p, l, 1),
        shot_id(&p, l, 2),
        &split("Front", "Back"),
    ));
    let b = done.new_line.unwrap();
    assert_eq!(done.stations_deleted, [s[2]]);
    assert_eq!(p.line_stations(l).unwrap(), [s[0], s[1]]);
    assert_eq!(p.line_stations(b).unwrap(), [s[3], s[4], s[5]]);
    assert_eq!(p.line(l).unwrap().name.as_deref(), Some("Front"));
    assert_eq!(p.line(b).unwrap().name.as_deref(), Some("Back"));
    let uuids: Vec<String> = p
        .line(b)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect();
    assert_eq!(uuids, tail_uuids);
    assert_eq!(p.line(b).unwrap().shots[0].id, b * crate::FEATURE_ID_STRIDE);
    assert_eq!(
        p.station(s[3]).unwrap().comment.as_deref(),
        Some("boundary")
    );
    // Nothing moves: the tail is a piece of its own, left where it was.
    assert_still(&before, &p.raw_positions(), TOL, &[]);
    assert_eq!(p.placements.len(), 2);
    assert_valid(&p);
}

#[test]
fn the_shift_is_refused_for_a_loop_a_datum_or_two_held_sides() {
    // A branch from station 1 to station 4 (a second route): a loop.
    let (mut p, l) = east5();
    let (s1, s4) = (st(&p, l, 1), st(&p, l, 4));
    let b = line_from(&mut p, "Branch", s1, run("b", 0.0, 5.0, 1));
    let tip = st(&p, b, 1);
    let mut q = p.clone();
    // Re-route: the branch's tip IS station 4 (a junction loop is a cycle,
    // so make the second route a line ending on 4 instead).
    let c = line_from(&mut q, "Back", s4, run("c", 0.0, 5.0, 1));
    let c1 = st(&q, c, 1);
    q.push_closure(tip, Some(b), c1, Some(c), ClosureStrategy::Batch);
    assert_valid(&q);
    // Closures do not block the range shift (only a shot loop does).
    let preview = q
        .preview_delete_shot_range(l, shot_id(&q, l, 1), shot_id(&q, l, 2))
        .unwrap();
    assert!(
        !preview.shift_blocked,
        "{:?}",
        preview.shift_blocked_reasons
    );

    // A datum on a station the shift deletes.
    let s2 = st(&p, l, 2);
    add_datum(&mut p, s2, gps(10.0, 20.0));
    let text = refused(p.delete_shot_range(
        l,
        shot_id(&p, l, 1),
        shot_id(&p, l, 2),
        &RangeDeleteMode::Shift,
    ));
    assert_eq!(
        text,
        "Deleting this range would remove a zero datum. Use split instead."
    );
    let preview = p
        .preview_delete_shot_range(l, shot_id(&p, l, 1), shot_id(&p, l, 2))
        .unwrap();
    assert!(preview.shift_blocked);
    assert_eq!(preview.removed_datums.len(), 1);
    assert_eq!(preview.closures_removed_across_one_shot, None);
}

#[test]
fn a_gap_close_removes_a_closure_it_leaves_across_one_shot() {
    let (mut p, l) = east5();
    let (s2, s5) = (st(&p, l, 2), st(&p, l, 5));
    let id = p.push_closure(s2, Some(l), s5, Some(l), ClosureStrategy::Batch);
    assert_valid(&p);
    let (first, last) = (shot_id(&p, l, 2), shot_id(&p, l, 3));
    let preview = p.preview_delete_shot_range(l, first, last).unwrap();
    let predicted: Vec<u64> = preview
        .closures_removed_across_one_shot
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(predicted, [id]);
    let edited = p
        .delete_shot_range(l, first, last, &RangeDeleteMode::Shift)
        .unwrap();
    assert_eq!(edited.report.removed_across_one_shot.len(), 1);
    assert!(p.closures.is_empty());
    assert_valid(&p);
}

#[test]
fn a_range_through_a_junction_keeps_the_junction_for_the_other_line() {
    let (mut p, l) = east5();
    let j = st(&p, l, 2);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 2));
    let before = p.raw_positions();
    let preview = p
        .preview_delete_shot_range(l, shot_id(&p, l, 1), shot_id(&p, l, 2))
        .unwrap();
    assert_eq!(preview.junctions_kept, [j]);
    ok(p.delete_shot_range(
        l,
        shot_id(&p, l, 1),
        shot_id(&p, l, 2),
        &RangeDeleteMode::Shift,
    ));
    assert!(p.station(j).is_some());
    assert_eq!(p.line_stations(b).unwrap()[0], j);
    // The branch is its own piece now, left where it was.
    assert_still(
        &before,
        &p.raw_positions(),
        TOL,
        &(0..=5)
            .filter_map(|i| p.line_stations(l).and_then(|c| c.get(i).copied()))
            .filter(|s| *s != st(&p, l, 0) && *s != st(&p, l, 1))
            .collect::<Vec<_>>(),
    );
    near(
        xy(&p.raw_positions(), j),
        xy(&before, j),
        TOL,
        "junction stays",
    );
    assert_valid(&p);
}
