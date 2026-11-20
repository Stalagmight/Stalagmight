//! Split Line Here (#335) through `AppState`, on the station graph: one undo
//! entry covers the whole operation (bit-exact both ways), a cut along a shot
//! inserts a station first, a cut near a shot's end snaps to that end's
//! station, the preview refuses exactly what the split refuses, and the
//! halves share the split station (plan 5.4, declared change 12.2).
//!
//! The model-level oracle is `models::survey::edit::split_tests`; these pin
//! the command layer: the gates, the frozen-shot refusal (S12), the history
//! entry (kind, label, preview rows), the ignores, a save and a reopen.
//!
//! Fate of each 0.6.0 test (`state/split_line_tests.rs` at the C0 base):
//! - ported: `a_station_split_is_one_undo_step_and_redoes`,
//!   `a_cut_along_a_shot_inserts_a_station_and_undoes_in_one_step`,
//!   `a_cut_near_a_shot_end_snaps_to_that_station`,
//!   `a_cut_that_snaps_to_the_lines_end_is_refused_and_changes_nothing`,
//!   `a_distance_off_the_shot_is_refused_before_it_snaps`,
//!   `an_unknown_line_is_refused`, `an_out_of_range_station_is_refused`,
//!   `a_shot_from_another_line_is_refused`,
//!   `a_loop_closure_on_the_split_station_stays_on_the_first_half`,
//!   `a_split_survives_a_save_and_a_reopen`,
//!   `a_split_with_no_line_id_left_is_refused`,
//!   `a_cut_inside_a_shot_with_no_shot_id_left_is_refused`,
//!   `a_cut_inside_a_shot_of_a_full_line_is_refused`,
//!   `a_plain_split_previews_no_move`,
//!   `the_preview_refuses_exactly_what_the_split_refuses` (the two "datum on
//!   both halves" rows are accepted now, S9; a read-only file row added);
//! - adapted: `a_datum_on_both_halves_is_refused_before_the_shot_splits` ->
//!   `the_comment_and_datum_sides_choose_nothing` (S9: the datum is on the
//!   one shared station); `a_loop_closure_across_the_cut_is_previewed_as_a_move`
//!   -> `a_loop_closure_across_the_cut_stays_a_loop_and_nothing_moves` (the
//!   halves stay joined, so the closure is no sole tie and nothing is
//!   promoted); `an_ignored_warning_follows_its_shot_into_the_second_half` and
//!   `a_position_keyed_ignore_follows_its_shot_into_the_second_half` ->
//!   `an_ignore_follows_its_shot_into_the_second_half` (an ignore is keyed by
//!   the shot's uuid alone since C11, a position-keyed one is pinned at once,
//!   and the warnings computation is not on this branch, so the line the
//!   ignore reads is checked through `load_resolved_warnings`);
//!   `a_refused_split_leaves_position_keyed_ignores_unpinned` ->
//!   `a_refused_split_changes_nothing_the_file_state_included` (there is no
//!   pin any more; the property is that a refusal leaves no trace);
//!   `a_split_at_a_loaded_station_undoes_and_redoes_exactly` (one station
//!   carries the comment, name, datum and the junction for both halves; the
//!   hand move is copied, plan 5.6);
//! - retired: `a_delete_and_split_keeps_the_parity_and_the_gauge_on_the_second_line`
//!   (reverse parity and gauge pairs are gone, plan 1.3; Delete Shot & Split
//!   is C2's) and `a_delete_and_split_keeps_the_component_anchor` (C2's
//!   command; its Split Line Here twin is
//!   `a_split_keeps_the_closures_computed_view`).

use super::split_combine_kit::{
    apart, assert_undo_redo_exact, canonical, close, deep, empty, history, line_at, line_from,
    open, positions, preview_rows, project, save_close_reopen, shot, slot, snapshot, st, xy,
};
use super::AppState;
use crate::core::Command;
use crate::models::survey::{ClosureStrategy, Project, ZeroDatum};
use crate::models::{
    GpsCoordinates, LineId, SplitAt, SplitLineOptions, SplitSide, FEATURE_ID_STRIDE, MAX_LINE_ID,
};

fn options() -> SplitLineOptions {
    SplitLineOptions {
        name_a: "Main-A".to_string(),
        name_b: "Main-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    }
}

/// One line "Main" of three shots, 5, 7 and 3 metres, north-east and
/// sloping down.
fn main_survey() -> (Project, LineId) {
    let mut p = empty("Split");
    let main = line_at(
        &mut p,
        "Main",
        [0.0, 0.0],
        vec![
            deep(45.0, 5.0, 10.0, 11.0),
            deep(45.0, 7.0, 10.0, 11.0),
            deep(45.0, 3.0, 10.0, 11.0),
        ],
    );
    (p, main)
}

/// Shot lengths of every line, in line order.
fn lengths(state: &AppState) -> Vec<Vec<f64>> {
    project(state)
        .lines()
        .iter()
        .map(|l| l.shots.iter().map(|s| s.length).collect())
        .collect()
}

fn shot_id(state: &AppState, line: LineId, index: usize) -> u32 {
    project(state).line(line).unwrap().shots[index].id
}

/// The line after the cut station shares it with the first half.
fn assert_joined(p: &Project, first: LineId, second: LineId) {
    let end = p.line(first).unwrap().shots.len() as u32;
    let shared = st(p, first, end);
    assert_eq!(st(p, second, 0), shared, "the halves share the cut station");
    assert!(p.topology().is_junction(shared));
}

#[test]
fn a_station_split_is_one_undo_step_and_redoes() {
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result = state.execute_split_line(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.line_id.unwrap();
    assert_eq!(result.line_ids, vec![main, second]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(lengths(state), vec![vec![5.0], vec![7.0, 3.0]]);
    let after_project = project(state);
    assert_joined(&after_project, main, second);
    let (a, b) = (
        after_project.line(main).unwrap(),
        after_project.line(second).unwrap(),
    );
    assert_eq!(
        (a.name.as_deref(), a.color.as_str()),
        (Some("Main-A"), "#111111")
    );
    assert_eq!(
        (b.name.as_deref(), b.color.as_str()),
        (Some("Main-B"), "#222222")
    );
    assert_eq!(
        history(state),
        [("batch".to_string(), "Split line \"Main\" at 1".to_string())]
    );
    assert_eq!(
        preview_rows(state),
        [(
            "Create line \"Main-B\"".to_string(),
            String::new(),
            "Name: Main-B".to_string()
        )]
    );
    assert!(state.has_unsaved_changes());
    // Nothing moved: every station keeps its measured position.
    for (at, was) in positions(&p) {
        let station = st(&p, at.line_id, at.station_index);
        let now = after_project.raw_positions().xy(station).unwrap();
        assert!(apart(was, now) < 1e-9, "{at:?} moved");
    }

    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "station split");
    assert_eq!(state.get_undo_stack().len(), 1, "one entry");
}

#[test]
fn a_cut_along_a_shot_inserts_a_station_and_undoes_in_one_step() {
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    let middle = shot_id(state, main, 1);
    let before = snapshot(state);

    let result = state.execute_split_line(
        main,
        SplitAt::Shot {
            shot_id: middle,
            distance: 2.0,
        },
        options(),
    );
    assert!(result.success, "{:?}", result.error);
    let second = result.line_id.unwrap();
    assert_eq!(lengths(state), vec![vec![5.0, 2.0], vec![5.0, 3.0]]);
    let after_project = project(state);
    assert_joined(&after_project, main, second);
    // The new station sits on the shot it cut, and nothing else moved.
    let (from, to) = (xy(&p, slot(main, 1)), xy(&p, slot(main, 2)));
    let cut = xy(&after_project, slot(second, 0));
    assert!((apart(from, cut) + apart(cut, to) - apart(from, to)).abs() < 1e-9);
    assert!(apart(xy(&after_project, slot(second, 2)), xy(&p, slot(main, 3))) < 1e-9);
    // Labelled by the station the cut made, read after the cut.
    let name = after_project
        .stations
        .get(&st(&after_project, second, 0))
        .unwrap()
        .name
        .clone();
    assert_eq!(
        history(state),
        [(
            "batch".to_string(),
            format!("Split line \"Main\" at {name}")
        )]
    );

    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "cut along a shot");
    assert_eq!(state.get_undo_stack().len(), 1);
}

#[test]
fn a_cut_near_a_shot_end_snaps_to_that_station() {
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    let middle = shot_id(state, main, 1);

    let result = state.execute_split_line(
        main,
        SplitAt::Shot {
            shot_id: middle,
            distance: 6.995,
        },
        options(),
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        lengths(state),
        vec![vec![5.0, 7.0], vec![3.0]],
        "no sliver shot: the cut snapped to the shot's end station"
    );
    assert_eq!(
        history(state),
        [("batch".to_string(), "Split line \"Main\" at 2".to_string())]
    );
}

/// Run a split that must be refused before anything mutates: the split and
/// its preview fail with one reason, the project is unchanged to the bit,
/// no history entry is pushed and the project stays clean.
fn assert_refused(
    state: &AppState,
    line: LineId,
    at: SplitAt,
    options: &SplitLineOptions,
    what: &str,
) -> String {
    let before = snapshot(state);
    let depth = state.get_undo_stack().len();
    let dirty = state.has_unsaved_changes();
    let preview = state.split_line_preview(line, at, options);
    let result = state.execute_split_line(line, at, options.clone());
    assert!(!result.success, "{what}: the split was accepted");
    assert_eq!(
        preview.err(),
        result.error,
        "{what}: preview and split agree"
    );
    assert!(
        snapshot(state) == before,
        "{what}: a refused split changed the project"
    );
    assert_eq!(state.get_undo_stack().len(), depth, "{what}: an undo entry");
    assert_eq!(state.has_unsaved_changes(), dirty, "{what}: dirtied");
    result.error.unwrap_or_default()
}

#[test]
fn a_cut_that_snaps_to_the_lines_end_is_refused_and_changes_nothing() {
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    let first = shot_id(state, main, 0);
    let reason = assert_refused(
        state,
        main,
        SplitAt::Shot {
            shot_id: first,
            distance: 0.001,
        },
        &options(),
        "a snap to the start",
    );
    assert!(reason.contains("first or last station"), "{reason}");
    assert!(!state.has_unsaved_changes());
}

#[test]
fn a_distance_off_the_shot_is_refused_before_it_snaps() {
    for (distance, what) in [
        (f64::NAN, "NaN"),
        (-1.0, "negative"),
        (7.5, "past the shot's end"),
    ] {
        let (p, main) = main_survey();
        let opened = open(&p);
        let middle = shot_id(&opened.state, main, 1);
        let reason = assert_refused(
            &opened.state,
            main,
            SplitAt::Shot {
                shot_id: middle,
                distance,
            },
            &options(),
            what,
        );
        assert!(reason.contains("is outside the shot"), "{what}: {reason}");
    }
}

#[test]
fn an_unknown_line_is_refused() {
    let (p, main) = main_survey();
    let opened = open(&p);
    let reason = assert_refused(
        &opened.state,
        main + 99,
        SplitAt::Station { station_index: 1 },
        &options(),
        "unknown line",
    );
    assert_eq!(reason, crate::models::LINE_GONE);
}

#[test]
fn an_out_of_range_station_is_refused() {
    let (p, main) = main_survey();
    let opened = open(&p);
    assert_refused(
        &opened.state,
        main,
        SplitAt::Station {
            station_index: u32::MAX,
        },
        &options(),
        "station index u32::MAX",
    );
}

#[test]
fn a_shot_from_another_line_is_refused() {
    let (mut p, main) = main_survey();
    let other = line_at(&mut p, "Other", [50.0, 0.0], vec![shot(0.0, 4.0)]);
    let opened = open(&p);
    let reason = assert_refused(
        &opened.state,
        main,
        SplitAt::Shot {
            shot_id: shot_id(&opened.state, other, 0),
            distance: 2.0,
        },
        &options(),
        "a shot from another line",
    );
    assert_eq!(reason, crate::models::SHOT_GONE);
}

/// S9: the dialog's comment and datum sides choose nothing any more. A datum
/// and a comment on the cut station stay on that one station, which both
/// halves pass, whatever the sides say; a datum "on both halves" (refused in
/// 0.6.0, which duplicated the station) is simply what happens.
#[test]
fn the_comment_and_datum_sides_choose_nothing() {
    for (comment_side, datum_side) in [
        (SplitSide::A, SplitSide::Both),
        (SplitSide::B, SplitSide::B),
        (SplitSide::Both, SplitSide::A),
    ] {
        let what = format!("comment {comment_side:?}, datum {datum_side:?}");
        let (mut p, main) = main_survey();
        let cut = st(&p, main, 1);
        p.stations.get_mut(&cut).unwrap().comment = Some("the cut".to_string());
        p.set_datum(
            cut,
            Some(main),
            GpsCoordinates::new(20.5, -87.4, 0.0).unwrap(),
            None,
        )
        .unwrap();
        let opened = open(&p);
        let state = &opened.state;
        let result = state.execute_split_line(
            main,
            SplitAt::Station { station_index: 1 },
            SplitLineOptions {
                comment_side,
                datum_side,
                ..options()
            },
        );
        assert!(result.success, "{what}: {:?}", result.error);
        let second = result.line_id.unwrap();
        let after = project(state);
        assert_eq!(after.datums.len(), 1, "{what}: one datum");
        assert_eq!(after.datums[0].station, cut, "{what}");
        assert_eq!(
            after.stations.get(&cut).unwrap().comment.as_deref(),
            Some("the cut"),
            "{what}"
        );
        // Both halves' slots show it (W2): one station.
        let wire = after.project_state_wire(false);
        for at in [slot(main, 1), slot(second, 0)] {
            let station = wire.stations.iter().find(|s| s.station_ref == at).unwrap();
            assert!(station.is_zero_datum, "{what}: {at:?}");
            assert_eq!(
                station.comment.as_deref(),
                Some("the cut"),
                "{what}: {at:?}"
            );
        }
    }
}

/// A loop closure on the split station stays on it, attached to the first
/// half (its `via`), and nothing moves. Main runs east past the closure
/// station into a dead end; Side leaves Main's start and comes back 1 m short
/// of station 2. In 0.6.0 a copy of the closure on the second half would
/// have been the dead end's only tie; on the station graph the halves share
/// the station, so there is no copy to promote.
#[test]
fn a_loop_closure_on_the_split_station_stays_on_the_first_half() {
    let mut p = empty("Split");
    let main = line_at(
        &mut p,
        "Main",
        [0.0, 0.0],
        vec![
            shot(90.0, 10.0),
            shot(90.0, 10.0),
            shot(90.0, 5.0),
            shot(90.0, 5.0),
        ],
    );
    let start = st(&p, main, 0);
    let side = line_from(
        &mut p,
        "Side",
        start,
        vec![shot(0.0, 10.0), shot(90.0, 21.0), shot(180.0, 10.0)],
    );
    let closure = close(&mut p, slot(main, 2), slot(side, 3), ClosureStrategy::Batch);
    assert!(
        apart(xy(&p, slot(main, 2)), xy(&p, slot(side, 3))) > 0.9,
        "setup: 1 m"
    );
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result = state.execute_split_line(main, SplitAt::Station { station_index: 2 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.line_id.unwrap();
    let after = project(state);
    for i in 0..=2 {
        let was = xy(&p, slot(main, 2 + i));
        let now = xy(&after, slot(second, i));
        assert!(apart(was, now) < 1e-9, "the second half moved at {i}");
    }
    assert_eq!(after.closures.len(), 1);
    let c = after.closures[0];
    assert_eq!(c.id, closure);
    let shared = st(&after, main, 2);
    assert_eq!(
        (c.a, c.a_via),
        (shared, Some(main)),
        "the closure end stays"
    );
    assert_eq!(after.closure_end_slot(c.a, c.a_via), Some(slot(main, 2)));
    assert_eq!(after.closure_end_slot(c.b, c.b_via), Some(slot(side, 3)));

    let snap = snapshot(state);
    assert_undo_redo_exact(state, &before, &snap, "closure on the cut station");
}

/// A loop closure crossing the cut: Main loops back on itself, closing from
/// station 10 onto station 2 with a 0.5 m misclosure, and is split at station
/// 5. In 0.6.0 the closure became the halves' only tie and turned into a
/// merge, moving one half by the misclosure. On the station graph the halves
/// share station 5, so the closure still closes a loop: the preview reports
/// nothing beyond the cut, the closure stays, and nothing moves.
#[test]
fn a_loop_closure_across_the_cut_stays_a_loop_and_nothing_moves() {
    let mut p = empty("Loop");
    let main = line_at(
        &mut p,
        "Main",
        [0.0, 0.0],
        vec![
            shot(90.0, 5.0),
            shot(90.0, 5.0),
            shot(0.0, 4.0),
            shot(0.0, 4.0),
            shot(270.0, 4.0),
            shot(270.0, 4.0),
            shot(180.0, 4.0),
            shot(180.0, 4.0),
            shot(90.0, 4.0),
            shot(90.0, 3.5),
            shot(180.0, 5.0),
        ],
    );
    let closure = close(
        &mut p,
        slot(main, 2),
        slot(main, 10),
        ClosureStrategy::Batch,
    );
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);
    let at = SplitAt::Station { station_index: 5 };

    let preview = state.split_line_preview(main, at, &options()).unwrap();
    assert!(snapshot(state) == before, "the preview changed the project");
    assert_eq!(preview.closures_to_merges, 0);
    assert_eq!(preview.moved_side, None);
    assert_eq!(preview.moved_m, 0.0);
    assert!(preview.removed_closures.is_empty());
    assert!(preview.removed_already_one_point.is_empty());

    let result = state.execute_split_line(main, at, options());
    assert!(result.success, "{:?}", result.error);
    let after = project(state);
    assert_eq!(
        after.closures.iter().map(|c| c.id).collect::<Vec<_>>(),
        [closure],
        "the closure stays a loop closure"
    );
    for (at, was) in positions(&p) {
        let now = after
            .raw_positions()
            .xy(st(&p, at.line_id, at.station_index))
            .unwrap();
        assert!(apart(was, now) < 1e-9, "{at:?} moved");
    }
    assert!(after.closures_across_one_shot().is_empty());
}

/// A split that leaves every tie in place moves nothing, and says so.
#[test]
fn a_plain_split_previews_no_move() {
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    let preview = state
        .split_line_preview(main, SplitAt::Station { station_index: 1 }, &options())
        .unwrap();
    assert_eq!(preview.closures_to_merges, 0);
    assert_eq!(preview.moved_side, None);
    assert_eq!(preview.moved_m, 0.0);
    assert!(!state.has_unsaved_changes(), "a preview is read-only");

    let refused = state.split_line_preview(main, SplitAt::Station { station_index: 0 }, &options());
    assert!(refused.unwrap_err().contains("first or last station"));
}

/// The ignores of a shot past the cut follow it into the second half: an
/// ignore is keyed by the shot's uuid (C11), so the line it reads is wherever
/// the shot is, in the session and after a save and a reopen. One recorded
/// by position (the IPC's `line_id` + `shot_id`, no uuid) is keyed by the
/// shot's uuid when stored, so it follows the same way.
#[test]
fn an_ignore_follows_its_shot_into_the_second_half() {
    use crate::persistence::ResolvedWarning;
    let mut p = empty("Split");
    let main = line_at(
        &mut p,
        "Main",
        [0.0, 0.0],
        vec![shot(90.0, 5.0), shot(90.0, 5.0), shot(90.0, 0.1)],
    );
    let opened = open(&p);
    let state = &opened.state;
    let (short_id, short_uuid) = {
        let p = project(state);
        let s = &p.line(main).unwrap().shots[2];
        (s.id, s.uuid.clone())
    };
    for (code, uuid) in [
        ("SHORT_SHOT", Some(short_uuid.clone())),
        ("LRUD_MISSING", None),
    ] {
        state
            .save_resolved_warning(&ResolvedWarning {
                line_id: main,
                shot_id: short_id,
                warning_code: code.to_string(),
                resolved_at: chrono::Utc::now(),
                resolved_comment: None,
                shot_uuid: uuid,
            })
            .unwrap();
    }
    let lines_read = |state: &AppState| -> Vec<(LineId, Option<String>)> {
        let mut rows: Vec<_> = state
            .load_resolved_warnings()
            .unwrap()
            .into_iter()
            .map(|row| (row.line_id, row.shot_uuid))
            .collect();
        rows.sort();
        rows
    };
    let pinned = vec![(main, Some(short_uuid.clone())); 2];
    assert_eq!(lines_read(state), pinned, "both keyed by the uuid");

    let result = state.execute_split_line(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.line_id.unwrap();
    let moved = vec![(second, Some(short_uuid)); 2];
    assert_eq!(lines_read(state), moved, "in the session");

    save_close_reopen(state, &opened.path);
    assert_eq!(lines_read(state), moved, "after a save and a reopen");
}

/// A split survives a save and a reopen: both halves, the station they
/// share, a comment past the cut on its station, and the history entry.
#[test]
fn a_split_survives_a_save_and_a_reopen() {
    let (mut p, main) = main_survey();
    let past = st(&p, main, 2);
    p.stations.get_mut(&past).unwrap().comment = Some("past the cut".to_string());
    let opened = open(&p);
    let state = &opened.state;
    let result = state.execute_split_line(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.line_id.unwrap();
    let before = canonical(state);

    save_close_reopen(state, &opened.path);
    assert_eq!(canonical(state), before);
    let reopened = project(state);
    assert_joined(&reopened, main, second);
    let comment = reopened
        .stations
        .get(&st(&reopened, second, 1))
        .and_then(|s| s.comment.clone());
    assert_eq!(comment.as_deref(), Some("past the cut"));
    assert!(reopened.validate().is_empty());
    assert_eq!(
        history(state),
        [("batch".to_string(), "Split line \"Main\" at 1".to_string())],
        "the label rides the file"
    );
    let r = state.undo();
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        lengths(state),
        vec![vec![5.0, 7.0, 3.0]],
        "undo after a reopen"
    );
}

/// The split mints the second half's id (#555): with no id left it is
/// refused before anything changes.
#[test]
fn a_split_with_no_line_id_left_is_refused() {
    let (mut p, _) = main_survey();
    let mut last = p.lines()[0].clone();
    last.id = MAX_LINE_ID;
    last.start = p.mint_station("0".to_string()).unwrap();
    last.shots.clear();
    last.hand_moves.clear();
    last.name = Some("Last".to_string());
    p.insert_line(last).unwrap();
    let lone = p.line(MAX_LINE_ID).unwrap().start;
    p.set_placement(lone, 40.0, 0.0).unwrap();
    let main = p.lines()[0].id;
    let opened = open(&p);
    let reason = assert_refused(
        &opened.state,
        main,
        SplitAt::Station { station_index: 1 },
        &options(),
        "no line id left",
    );
    assert!(reason.contains("maximum number of lines"), "{reason}");
}

/// A cut inside a shot inserts a station: a new shot of the line, with an
/// id from the line's window. With no id left there the cut is refused
/// before anything changes; a cut at an existing station needs no new shot
/// id and still works.
#[test]
fn a_cut_inside_a_shot_with_no_shot_id_left_is_refused() {
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    let mut full = project(state);
    full.line_mut(main).unwrap().next_shot_id = (main + 1) * FEATURE_ID_STRIDE;
    state.set_project_for_tests(full);

    let reason = assert_refused(
        state,
        main,
        SplitAt::Shot {
            shot_id: shot_id(state, main, 1),
            distance: 2.0,
        },
        &options(),
        "no shot id left",
    );
    assert!(reason.contains("used up its shot numbers"), "{reason}");

    let result = state.execute_split_line(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
}

/// The shot cap, exactly at its edge: a line of 99,999 shots (the most a
/// line may hold) cannot take the station a cut inside a shot inserts.
#[test]
fn a_cut_inside_a_shot_of_a_full_line_is_refused() {
    let mut p = empty("Full");
    let full = line_at(
        &mut p,
        "Full",
        [0.0, 0.0],
        (0..99_999).map(|_| shot(90.0, 1.0)).collect(),
    );
    let state = AppState::new().unwrap();
    state.set_project_for_tests(p);

    let reason = assert_refused(
        &state,
        full,
        SplitAt::Shot {
            shot_id: shot_id(&state, full, 10),
            distance: 0.5,
        },
        &options(),
        "a full line",
    );
    assert!(reason.contains("maximum of 99999 shots"), "{reason}");
    let after = project(&state);
    let line = after.line(full).unwrap();
    assert_eq!(line.shots.len(), 99_999);
}

/// S12: a cut inside a frozen shot (on a loop a snap closes, in a piece a
/// zero datum holds: an adjusted network) is refused by the command layer,
/// at the point 0.6.0 refused it, by the preview too; a cut at a station of
/// the same line, which inserts nothing, still goes ahead.
#[test]
fn a_cut_inside_a_frozen_shot_is_refused() {
    let mut p = empty("Frozen");
    let main = line_at(
        &mut p,
        "Main",
        [0.0, 0.0],
        vec![
            shot(90.0, 10.0),
            shot(0.0, 10.0),
            shot(270.0, 10.0),
            shot(180.0, 10.0),
        ],
    );
    let start = st(&p, main, 0);
    close(&mut p, slot(main, 4), slot(main, 0), ClosureStrategy::Move);
    p.set_datum(
        start,
        Some(main),
        GpsCoordinates::new(20.5, -87.4, 0.0).unwrap(),
        None,
    )
    .unwrap();
    assert!(p.is_shot_frozen(main, 1), "setup: a frozen shot");
    let opened = open(&p);
    let state = &opened.state;

    let reason = assert_refused(
        state,
        main,
        SplitAt::Shot {
            shot_id: shot_id(state, main, 1),
            distance: 2.0,
        },
        &options(),
        "a frozen shot",
    );
    assert_eq!(
        reason,
        "Cannot split a frozen shot. This shot is part of an adjusted survey network."
    );
    // The names' refusal still comes first, as in 0.6.0.
    let reason = assert_refused(
        state,
        main,
        SplitAt::Shot {
            shot_id: shot_id(state, main, 1),
            distance: 2.0,
        },
        &SplitLineOptions {
            name_b: " ".to_string(),
            ..options()
        },
        "a frozen shot without a name",
    );
    assert_eq!(reason, "Each half of a split needs a name");

    let result = state.execute_split_line(main, SplitAt::Station { station_index: 1 }, options());
    assert!(result.success, "{:?}", result.error);
}

/// Every station's position in the Closures Computed View, by station id.
fn adjusted_by_station(state: &AppState) -> std::collections::BTreeMap<u32, (f64, f64)> {
    let p = project(state);
    state
        .get_adjusted_stations()
        .unwrap()
        .into_iter()
        .map(|s| {
            let id = st(&p, s.station_ref.line_id, s.station_ref.station_index);
            (id, (s.coordinates.x, s.coordinates.y))
        })
        .collect()
}

/// The Split Line Here twin of 0.6.0's delete-and-split anchor test: S (five
/// shots east) has a bypass X from S:1 that comes back 1 m short of S:4,
/// closed by a loop closure, so the Closures Computed View solves the piece.
/// Splitting S anywhere along the loop keeps every station where the view
/// put it, and the piece's gauge (its placement station) stays put.
#[test]
fn a_split_keeps_the_closures_computed_view() {
    for at in [2, 3] {
        let mut p = empty("Bypass");
        let s = line_at(
            &mut p,
            "S",
            [0.0, 0.0],
            (0..5).map(|_| shot(90.0, 10.0)).collect(),
        );
        let s1 = st(&p, s, 1);
        let x = line_from(
            &mut p,
            "X",
            s1,
            vec![shot(0.0, 10.0), shot(90.0, 31.0), shot(180.0, 10.0)],
        );
        close(&mut p, slot(x, 3), slot(s, 4), ClosureStrategy::Batch);
        let gauge = p.placements.clone();
        let opened = open(&p);
        let state = &opened.state;
        let before = adjusted_by_station(state);

        let result = state.execute_split_line(s, SplitAt::Station { station_index: at }, options());
        assert!(result.success, "at {at}: {:?}", result.error);
        assert_eq!(project(state).placements, gauge, "at {at}: the gauge");
        let after = adjusted_by_station(state);
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>()
        );
        for (id, p) in &before {
            let q = after[id];
            let moved = (p.0 - q.0).hypot(p.1 - q.1);
            assert!(
                moved < 1e-9,
                "at {at}: station {id} moved {moved} m in the view"
            );
        }
    }
}

/// The Split dialog's preview refuses exactly what the split refuses, with
/// the same reason, and accepts what it accepts. Each row opens a fresh
/// project, asks for the preview, then runs the split.
#[test]
fn the_preview_refuses_exactly_what_the_split_refuses() {
    type Setup = fn(&AppState, LineId) -> (LineId, SplitAt, SplitLineOptions);
    let rows: Vec<(&str, Setup, bool)> = vec![
        (
            "a mid station",
            |_, m| (m, SplitAt::Station { station_index: 1 }, options()),
            true,
        ),
        (
            "inside a shot",
            |s, m| {
                let shot_id = shot_id(s, m, 1);
                (
                    m,
                    SplitAt::Shot {
                        shot_id,
                        distance: 2.0,
                    },
                    options(),
                )
            },
            true,
        ),
        (
            "an unknown line",
            |_, m| (m + 99, SplitAt::Station { station_index: 1 }, options()),
            false,
        ),
        (
            "the first station",
            |_, m| (m, SplitAt::Station { station_index: 0 }, options()),
            false,
        ),
        (
            "the last station",
            |_, m| (m, SplitAt::Station { station_index: 3 }, options()),
            false,
        ),
        (
            "station u32::MAX",
            |_, m| {
                let at = SplitAt::Station {
                    station_index: u32::MAX,
                };
                (m, at, options())
            },
            false,
        ),
        (
            "a NaN distance",
            |s, m| {
                let shot_id = shot_id(s, m, 1);
                let at = SplitAt::Shot {
                    shot_id,
                    distance: f64::NAN,
                };
                (m, at, options())
            },
            false,
        ),
        (
            "a negative distance",
            |s, m| {
                let shot_id = shot_id(s, m, 1);
                (
                    m,
                    SplitAt::Shot {
                        shot_id,
                        distance: -1.0,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "past the shot's end",
            |s, m| {
                let shot_id = shot_id(s, m, 1);
                (
                    m,
                    SplitAt::Shot {
                        shot_id,
                        distance: 7.5,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "a snap to the line's start",
            |s, m| {
                let shot_id = shot_id(s, m, 0);
                (
                    m,
                    SplitAt::Shot {
                        shot_id,
                        distance: 0.001,
                    },
                    options(),
                )
            },
            false,
        ),
        (
            "a datum on both halves, at a station (S9: accepted)",
            |_, m| {
                let options = SplitLineOptions {
                    datum_side: SplitSide::Both,
                    ..options()
                };
                (m, SplitAt::Station { station_index: 1 }, options)
            },
            true,
        ),
        (
            "a datum on both halves, inside a shot (S9: accepted)",
            |s, m| {
                let shot_id = shot_id(s, m, 1);
                let options = SplitLineOptions {
                    datum_side: SplitSide::Both,
                    ..options()
                };
                (
                    m,
                    SplitAt::Shot {
                        shot_id,
                        distance: 2.0,
                    },
                    options,
                )
            },
            true,
        ),
        (
            "an empty first-half name",
            |_, m| {
                let options = SplitLineOptions {
                    name_a: String::new(),
                    ..options()
                };
                (m, SplitAt::Station { station_index: 1 }, options)
            },
            false,
        ),
        (
            "a whitespace-only second-half name, inside a shot",
            |s, m| {
                let shot_id = shot_id(s, m, 1);
                let options = SplitLineOptions {
                    name_b: " \t ".to_string(),
                    ..options()
                };
                (
                    m,
                    SplitAt::Shot {
                        shot_id,
                        distance: 2.0,
                    },
                    options,
                )
            },
            false,
        ),
        (
            "two halves with one name",
            |_, m| {
                let options = SplitLineOptions {
                    name_b: "Main-A".to_string(),
                    ..options()
                };
                (m, SplitAt::Station { station_index: 1 }, options)
            },
            false,
        ),
        (
            "no shot id left, inside a shot",
            |s, m| {
                let mut project = project(s);
                project.line_mut(m).unwrap().next_shot_id = (m + 1) * FEATURE_ID_STRIDE;
                s.set_project_for_tests(project);
                let shot_id = shot_id(s, m, 1);
                (
                    m,
                    SplitAt::Shot {
                        shot_id,
                        distance: 2.0,
                    },
                    options(),
                )
            },
            false,
        ),
    ];
    for (what, setup, accepted) in rows {
        let (p, main) = main_survey();
        let opened = open(&p);
        let state = &opened.state;
        let (line, at, opts) = setup(state, main);
        let preview = state.split_line_preview(line, at, &opts);
        let result = state.execute_split_line(line, at, opts);
        assert_eq!(preview.is_ok(), accepted, "{what}: preview {preview:?}");
        assert_eq!(result.success, accepted, "{what}: split {:?}", result.error);
        if let Err(reason) = preview {
            assert_eq!(Some(reason), result.error, "{what}");
        }
    }

    // A SpeleoDB project open without its lock: both refuse at the edit gate.
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    state.set_speleo_link_for_tests(Some(&crate::models::SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: "22222222-2222-2222-2222-222222222222".to_string(),
        project_name: "Remote".to_string(),
        base_commit_sha: Some("def456".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }));
    let at = SplitAt::Station { station_index: 1 };
    let reason = assert_refused(state, main, at, &options(), "no edit session");
    assert!(
        reason.starts_with(crate::core::command::EDIT_DISABLED_CODE),
        "{reason}"
    );

    // A file made with a newer Stalagmight opens read-only: both refuse.
    let (p, main) = main_survey();
    let opened = open(&p);
    let text = std::fs::read_to_string(&opened.path).unwrap();
    let patched = text.replacen("\"requires\":[]", "\"requires\":[\"x-from-the-future\"]", 1);
    std::fs::write(&opened.path, patched.as_bytes()).unwrap();
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject { path: opened.path });
    assert!(r.success, "{:?}", r.error);
    let reason = assert_refused(&state, main, at, &options(), "read-only file");
    assert!(reason.starts_with(super::READ_ONLY_FILE_CODE), "{reason}");
}

/// A refusal leaves no trace: the project and the file state (clean, no
/// history, the ignores) are as they were, whichever refusal it was.
#[test]
fn a_refused_split_changes_nothing_the_file_state_included() {
    use crate::persistence::ResolvedWarning;
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    let ignored = {
        let p = project(state);
        p.line(main).unwrap().shots[2].uuid.clone()
    };
    state
        .save_resolved_warning(&ResolvedWarning {
            line_id: main,
            shot_id: shot_id(state, main, 2),
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: Some(ignored),
        })
        .unwrap();
    let ignores = state.load_resolved_warnings().unwrap().len();
    for (at, what) in [
        (SplitAt::Station { station_index: 0 }, "an end station"),
        (
            SplitAt::Shot {
                shot_id: shot_id(state, main, 0),
                distance: 0.001,
            },
            "a cut that snaps to an end",
        ),
        (
            SplitAt::Shot {
                shot_id: shot_id(state, main, 1),
                distance: 9.0,
            },
            "a distance off the shot",
        ),
    ] {
        assert_refused(state, main, at, &options(), what);
        assert_eq!(
            state.load_resolved_warnings().unwrap().len(),
            ignores,
            "{what}"
        );
        assert!(!state.can_undo(), "{what}");
    }
}

/// Main (five shots east) with everything a station can carry on station 2:
/// a junction with Side (which starts there), a comment, a name, a hand move
/// on Main (a migrated file's), a zero datum (Main's) and a loop closure (from
/// Loop, which leaves station 4 and comes back 1 m short of 2); and a comment
/// on station 1, a name and a hand move on station 3.
fn loaded_station() -> (Project, LineId, LineId, LineId) {
    let mut p = empty("Loaded");
    let main = line_at(
        &mut p,
        "Main",
        [0.0, 0.0],
        (0..5).map(|_| shot(90.0, 10.0)).collect(),
    );
    let (s1, s2, s3, s4) = (
        st(&p, main, 1),
        st(&p, main, 2),
        st(&p, main, 3),
        st(&p, main, 4),
    );
    let side = line_from(&mut p, "Side", s2, vec![shot(0.0, 6.0), shot(0.0, 6.0)]);
    let lp = line_from(
        &mut p,
        "Loop",
        s4,
        vec![shot(0.0, 8.0), shot(270.0, 20.0), shot(180.0, 7.0)],
    );
    let j = p.stations.get_mut(&s2).unwrap();
    j.name = "J1".to_string();
    j.comment = Some("the junction".to_string());
    p.stations.get_mut(&s1).unwrap().comment = Some("before the cut".to_string());
    p.stations.get_mut(&s3).unwrap().name = "A3".to_string();
    let line = p.line_mut(main).unwrap();
    line.hand_moves.insert(s2, [0.0, 0.0, 0.25]);
    line.hand_moves.insert(s3, [0.0, 0.0, -0.1]);
    p.push_datum(ZeroDatum {
        via: Some(main),
        ..ZeroDatum::new(s2, GpsCoordinates::new(20.5, -87.4, 0.0).unwrap())
    });
    p.placements.clear();
    close(&mut p, slot(lp, 3), slot(main, 2), ClosureStrategy::Batch);
    (p, main, side, lp)
}

/// A split at the loaded station: the one station keeps its name, comment,
/// datum, closure end and junction with Side for both halves, the hand move
/// is copied to the second half (plan 5.6), the neighbours keep theirs, and
/// one undo and one redo put back exactly the project before and after.
/// Then the same through a cut inside the next shot, which inserts a station
/// first (no comment, no hand move, the Insert Station name).
#[test]
fn a_split_at_a_loaded_station_undoes_and_redoes_exactly() {
    for inside_shot in [false, true] {
        let what = format!("inside a shot {inside_shot}");
        let (p, main, side, lp) = loaded_station();
        let opened = open(&p);
        let state = &opened.state;
        let before = snapshot(state);
        let at = if inside_shot {
            SplitAt::Shot {
                shot_id: shot_id(state, main, 2),
                distance: 4.0,
            }
        } else {
            SplitAt::Station { station_index: 2 }
        };
        let options = SplitLineOptions {
            comment_side: SplitSide::Both,
            datum_side: SplitSide::B,
            ..options()
        };
        let result = state.execute_split_line(main, at, options);
        assert!(result.success, "{what}: {:?}", result.error);
        let second = result.line_id.unwrap();
        let after_project = project(state);
        assert_joined(&after_project, main, second);
        let j = st(&p, main, 2);
        let station = |id| after_project.stations.get(&id).unwrap();
        let hand = |line: LineId, id| {
            after_project
                .line(line)
                .unwrap()
                .hand_moves
                .get(&id)
                .copied()
        };
        assert_eq!(station(j).name, "J1", "{what}");
        assert_eq!(
            station(j).comment.as_deref(),
            Some("the junction"),
            "{what}"
        );
        assert_eq!(st(&after_project, side, 0), j, "{what}: Side stays on it");
        assert_eq!(hand(main, j), Some([0.0, 0.0, 0.25]), "{what}");
        assert_eq!(after_project.datums.len(), 1, "{what}");
        assert_eq!(after_project.datums[0].station, j, "{what}: the datum");
        let closure = after_project.closures[0];
        assert_eq!(closure.b, j, "{what}: the closure end");
        assert_eq!(closure.a, st(&p, lp, 3), "{what}");
        let a3 = st(&p, main, 3);
        if inside_shot {
            let cut = st(&after_project, second, 0);
            assert!(!p.stations.contains_key(&cut), "{what}: a new station");
            assert_eq!(station(cut).comment, None, "{what}");
            assert_eq!(hand(main, cut), None, "{what}");
            assert_eq!(hand(second, cut), None, "{what}");
            assert!(
                station(cut).name.starts_with("J1"),
                "{what}: {}",
                station(cut).name
            );
            assert_eq!(st(&after_project, second, 1), a3, "{what}");
        } else {
            assert_eq!(st(&after_project, second, 0), j, "{what}");
            assert_eq!(hand(second, j), Some([0.0, 0.0, 0.25]), "{what}: copied");
            assert_eq!(st(&after_project, second, 1), a3, "{what}");
        }
        assert_eq!(
            hand(second, a3),
            Some([0.0, 0.0, -0.1]),
            "{what}: A3's moves on"
        );
        assert_eq!(hand(main, a3), None, "{what}");
        assert_eq!(station(a3).name, "A3", "{what}");
        let s1 = st(&p, main, 1);
        assert_eq!(
            station(s1).comment.as_deref(),
            Some("before the cut"),
            "{what}"
        );
        // Nothing moved.
        for (at, was) in positions(&p) {
            let now = after_project
                .raw_positions()
                .xy(st(&p, at.line_id, at.station_index))
                .unwrap();
            assert!(apart(was, now) < 1e-9, "{what}: {at:?} moved");
        }

        let after = snapshot(state);
        assert!(after != before);
        assert_undo_redo_exact(state, &before, &after, &what);
    }
}

/// The label is worded at record time: the line by the name it had before
/// the cut, though the halves are renamed, on the undo and the redo side.
#[test]
fn the_label_names_the_line_by_its_name_before_the_cut() {
    let (p, main) = main_survey();
    let opened = open(&p);
    let state = &opened.state;
    let result = state.execute_split_line(main, SplitAt::Station { station_index: 2 }, options());
    assert!(result.success, "{:?}", result.error);
    // The halves are named Main-A and Main-B now; the entry keeps the name
    // the line had when it was split.
    assert_eq!(history(state)[0].1, "Split line \"Main\" at 2");
    let r = state.undo();
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        state.get_redo_stack()[0].description,
        "Split line \"Main\" at 2"
    );
    let at = slot(main, 2);
    assert_eq!(
        project(state).topology().station_at(at),
        Some(st(&p, main, 2))
    );
}
