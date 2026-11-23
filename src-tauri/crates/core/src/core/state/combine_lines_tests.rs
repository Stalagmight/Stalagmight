//! Combine Lines through `AppState`, on the station graph: one history
//! entry (0.6.0's label and preview rows), undo and redo exact, refusals
//! that change nothing and push nothing, the candidates' plans, the
//! warnings for the ties a combine removes across one shot (#746) named by
//! their stations before the combine, the frame, the ignores, the Closures
//! Computed View, and a save and a reopen.
//!
//! The model-level oracle is M1's port of the legacy combine tests
//! (`models::survey::edit::combine_port_*_tests`, `combine_tests`); these pin
//! the command layer on top of it.
//!
//! Fate of each 0.6.0 test (`state/combine_lines_tests.rs` at the C0 base):
//! - ported: `a_combine_is_one_undo_step_and_redoes`,
//!   `a_refused_combine_changes_nothing_and_leaves_no_undo_entry` (with the
//!   name rules, the edit gate and the read-only file added),
//!   `a_datum_on_the_other_line_keeps_the_frame`,
//!   `a_combine_survives_a_save_and_a_reopen`,
//!   `an_end_to_end_combine_undoes_the_reverse_too`,
//!   `the_station_query_refuses_unknown_and_out_of_range_stations`,
//!   `keeping_the_higher_id_keeps_it` (its prepend's undo is exact too),
//!   `a_kept_line_without_shot_id_room_is_refused_with_no_undo_entry`,
//!   `a_combine_at_a_junction_keeps_the_branch_and_undoes_exactly`,
//!   `a_combine_at_a_loop_closure_is_one_undo_step` (the junction at the
//!   far end is detached into the loop's closure, S1),
//!   `a_combine_removes_the_merge_it_would_tear_across_one_shot`,
//!   `a_combine_removes_a_closure_its_join_puts_across_one_shot`,
//!   `a_removed_former_snap_is_named_as_the_loop_closure_left_from_a_snap`,
//!   `a_combine_keeps_the_closure_the_solve_takes_over_a_legacy_one` (#749),
//!   `a_side_by_side_combine_keeps_merged_stations_together_through_a_reopen`
//!   (#740), `a_hand_moved_join_station_keeps_its_branch_through_undo_and_a_reopen`
//!   (#755), `shots_combined_into_a_shotless_line_take_its_legacy_surveyor`;
//! - adapted: `an_ignored_warning_follows_its_shot_into_the_kept_line`,
//!   `a_position_keyed_ignore_follows_its_shot_through_a_reversing_combine`,
//!   `an_ignore_on_an_unsaved_shot_follows_it_through_a_combine_and_a_split`
//!   and `an_ignore_on_an_unsaved_shot_survives_an_undone_combine` ->
//!   `an_ignore_follows_its_shot_through_a_combine_its_undo_and_a_reopen`
//!   (an ignore is keyed by the shot's uuid alone since C11, so whether the
//!   uuid is on disk yet no longer matters; the warnings computation is not
//!   on this branch, so the line an ignore reads is checked through
//!   `load_resolved_warnings`); `a_held_component_gauge_survives_undo_redo_and_a_reopen`
//!   -> `the_closures_computed_view_holds_through_undo_redo_and_a_reopen` (the
//!   gauge is the piece's `Placement` station, plan 3.7: nothing to record);
//!   `a_combine_that_would_put_two_datums_on_one_station_is_refused` ->
//!   `two_datums_on_side_by_side_join_stations_are_refused_and_one_station_may_hold_both`
//!   (S8: a merged join is one station already, which may hold both);
//! - retired: `the_record_parity_of_the_other_lines_shots_survives_a_save`,
//!   `an_undone_delete_after_a_reopen_restores_the_shots_counter_parity` and
//!   `an_emptied_counter_parity_set_does_not_wedge_an_older_combine` (record
//!   parity and `counter_parity_shots` are gone, plan 1.3);
//!   `a_huge_held_gauge_list_opens_fast_and_keeps_one_per_component`,
//!   `a_held_gauge_on_the_renamed_copy_of_a_repeated_uuid_holds_its_station`
//!   and `an_undone_line_delete_after_a_reopen_restores_its_held_gauge` (held
//!   gauges are gone, plan 1.3; shot uuids are project-unique, invariant 1);
//!   `undoing_a_prepend_does_not_keep_the_gauge_pair_it_made` (gauge pairs
//!   are gone; the prepend's exact undo is in `keeping_the_higher_id_keeps_it`).

use std::collections::BTreeMap;

use super::topology_kit::{
    add_datum, apart, assert_same, assert_undo_redo_exact, canonical, close, deep, empty, gps,
    history, hold, line_at, line_from, open, positions, preview_rows, project, save_close_reopen,
    shot, slot, snapshot, st, xy,
};
use super::AppState;
use crate::core::{Command, WarningSeverity};
use crate::models::survey::{ClosureStrategy, LineMeta, Project, StartAt};
use crate::models::{
    CombineJoinEnds, CombineJoinKind, CombineLinesOptions, CombinePick, CombineRemovedClosure,
    CombineRemovedKind, KeptMovesReason, LineEnd, LineId, ShotData, StationRef,
    CLOSURE_REMOVED_ACROSS_ONE_SHOT,
};
use crate::persistence::ResolvedWarning;

/// A shot credited to its own surveyor, so the snapshot comparisons below
/// cover per-shot survey credit through the combine, its undo and its redo.
fn credited(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        credit: Some(std::sync::Arc::new(crate::models::SurveyCredit {
            surveyors: crate::models::Person::named(&format!("Surveyor {azimuth}/{length}"))
                .into_iter()
                .collect(),
            explorers: Vec::new(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 4, 1),
        })),
        ..shot(azimuth, length)
    }
}

fn options() -> CombineLinesOptions {
    CombineLinesOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

fn joining(kept: LineEnd, other: LineEnd) -> CombineLinesOptions {
    CombineLinesOptions {
        join: Some(CombineJoinEnds { kept, other }),
        ..options()
    }
}

/// K (two shots east) with O (two shots north, then a short one) starting on
/// K's end: a junction.
fn kos() -> (Project, LineId, LineId) {
    let mut p = empty("Combine");
    let k = line_at(
        &mut p,
        "K",
        [0.0, 0.0],
        vec![credited(90.0, 10.0), credited(90.0, 5.0)],
    );
    let end = st(&p, k, 2);
    let o = line_from(
        &mut p,
        "O",
        end,
        vec![credited(0.0, 5.0), credited(0.0, 5.0), credited(0.0, 0.1)],
    );
    (p, k, o)
}

fn shot_counts(state: &AppState) -> Vec<(LineId, usize)> {
    project(state)
        .lines()
        .iter()
        .map(|l| (l.id, l.shots.len()))
        .collect()
}

/// Run a combine that must be refused: it fails, the project is unchanged
/// to the bit, no history entry is pushed and the dirty flag is as it was.
fn assert_refused(
    state: &AppState,
    kept: LineId,
    other: LineId,
    options: CombineLinesOptions,
    what: &str,
) -> String {
    let before = snapshot(state);
    let depth = state.get_undo_stack().len();
    let dirty = state.has_unsaved_changes();
    let result = state.execute_combine_lines(kept, other, options);
    assert!(!result.success, "{what}: the combine was accepted");
    assert!(
        snapshot(state) == before,
        "{what}: a refused combine changed the project"
    );
    assert_eq!(state.get_undo_stack().len(), depth, "{what}: an undo entry");
    assert_eq!(state.has_unsaved_changes(), dirty, "{what}: dirtied");
    result.error.unwrap_or_default()
}

#[test]
fn a_combine_is_one_undo_step_and_redoes() {
    let (p, k, o) = kos();
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.line_id, Some(k));
    assert_eq!(result.line_ids, vec![k]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.reversed_lines, None, "no reverse is reported");
    assert_eq!(shot_counts(state), vec![(k, 5)]);
    let combined = project(state);
    let line = combined.line(k).unwrap();
    assert_eq!(
        (line.name.as_deref(), line.color.as_str()),
        (Some("Joined"), "#abcdef")
    );
    assert!(combined
        .topology()
        .stations()
        .iter()
        .all(|s| !combined.topology().is_junction(*s)));
    assert_eq!(
        history(state),
        [(
            "batch".to_string(),
            "Combine lines \"K\" and \"O\" into \"Joined\"".to_string()
        )]
    );
    assert_eq!(
        preview_rows(state),
        [
            (
                "Update line \"Joined\" metadata".to_string(),
                String::new(),
                String::new()
            ),
            (
                "Delete line \"O\"".to_string(),
                String::new(),
                String::new()
            ),
        ]
    );
    assert!(state.has_unsaved_changes());
    // Every station stays where it was.
    for (at, was) in positions(&p) {
        let station = st(&p, at.line_id, at.station_index);
        let now = combined.raw_positions().xy(station).unwrap();
        assert!(apart(was, now) < 1e-9, "{at:?} moved");
    }

    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "combine");
    assert_eq!(state.get_undo_stack().len(), 1, "one entry");
}

/// The label says ` into "Name"` only when the combined line takes a name
/// neither line had.
#[test]
fn the_label_names_both_lines_before_the_combine() {
    for (name, label) in [
        ("Joined", "Combine lines \"K\" and \"O\" into \"Joined\""),
        ("O", "Combine lines \"K\" and \"O\""),
        (" K ", "Combine lines \"K\" and \"O\""),
    ] {
        let (p, k, o) = kos();
        let opened = open(&p);
        let state = &opened.state;
        let result = state.execute_combine_lines(
            k,
            o,
            CombineLinesOptions {
                name: name.to_string(),
                ..options()
            },
        );
        assert!(result.success, "{name}: {:?}", result.error);
        assert_eq!(history(state)[0].1, label, "{name}");
        assert_eq!(
            project(state).line(k).unwrap().name.as_deref(),
            Some(name.trim()),
            "{name}"
        );
    }
}

/// Every refusal changes nothing and pushes nothing: two lines each placed
/// by their own zero datum (side by side, two pieces), a line with itself,
/// an unknown line, an empty or another line's name (refused like a rename,
/// after the join's own refusals as in 0.6.0), a SpeleoDB project open
/// without its lock, and a file made with a newer Stalagmight.
#[test]
fn a_refused_combine_changes_nothing_and_leaves_no_undo_entry() {
    let mut p = empty("Placed");
    let k = line_at(
        &mut p,
        "K",
        [0.0, 0.0],
        vec![shot(90.0, 10.0), shot(90.0, 5.0)],
    );
    let o = line_at(
        &mut p,
        "O",
        [15.0, 0.0],
        vec![shot(0.0, 5.0), shot(0.0, 5.0), shot(0.0, 0.1)],
    );
    line_at(&mut p, "Third", [50.0, 50.0], vec![shot(0.0, 3.0)]);
    let (k0, o3) = (st(&p, k, 0), st(&p, o, 3));
    add_datum(&mut p, k0, gps(20.0, -87.0));
    hold(&mut p, o3, gps(20.0, -87.0));
    let opened = open(&p);
    let state = &opened.state;

    let reason = assert_refused(state, k, o, options(), "both placed");
    assert!(reason.contains("zero datum"), "{reason}");
    // The join's refusal comes before the name's, as in 0.6.0.
    let empty_name = CombineLinesOptions {
        name: " ".to_string(),
        ..options()
    };
    let reason = assert_refused(state, k, o, empty_name.clone(), "both placed, no name");
    assert!(reason.contains("zero datum"), "{reason}");
    for (kept, other) in [(k, k), (k, 999)] {
        assert_refused(state, kept, other, options(), "no pair");
    }

    let (mut p, k, o) = kos();
    line_at(&mut p, "Third", [50.0, 50.0], vec![shot(0.0, 3.0)]);
    let opened = open(&p);
    let state = &opened.state;
    let reason = assert_refused(state, k, o, empty_name, "an empty name");
    assert_eq!(reason, "A line name cannot be empty");
    let taken = CombineLinesOptions {
        name: "Third".to_string(),
        ..options()
    };
    let reason = assert_refused(state, k, o, taken, "another line's name");
    assert_eq!(reason, "A line named \"Third\" already exists");

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
    let reason = assert_refused(state, k, o, options(), "no edit session");
    assert!(
        reason.starts_with(crate::core::command::EDIT_DISABLED_CODE),
        "{reason}"
    );

    let (p, k, o) = kos();
    let opened = open(&p);
    let text = std::fs::read_to_string(&opened.path).unwrap();
    let patched = text.replacen("\"requires\":[]", "\"requires\":[\"x-from-the-future\"]", 1);
    std::fs::write(&opened.path, patched.as_bytes()).unwrap();
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject { path: opened.path });
    assert!(r.success, "{:?}", r.error);
    let reason = assert_refused(&state, k, o, options(), "read-only file");
    assert!(reason.starts_with(super::READ_ONLY_FILE_CODE), "{reason}");
}

/// The frame datum on the other line stays on its station, which the kept
/// line passes now (its `via` follows), and the survey stays framed on it at
/// local (0, 0).
#[test]
fn a_datum_on_the_other_line_keeps_the_frame() {
    let mut p = empty("Frame");
    let k = line_at(&mut p, "K", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let k1 = st(&p, k, 1);
    let o = line_from(&mut p, "O", k1, vec![shot(0.0, 5.0), shot(0.0, 5.0)]);
    let o2 = st(&p, o, 2);
    p.set_datum(o2, Some(o), gps(20.0, -87.0), None).unwrap();
    let opened = open(&p);
    let state = &opened.state;

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    let combined = project(state);
    assert_eq!(combined.datums.len(), 1);
    let datum = combined.datums[0];
    assert_eq!(datum.station, o2, "the datum stays on its station");
    assert_eq!(
        combined.datum_slot(&datum),
        Some(slot(k, 3)),
        "shown on K now"
    );
    let at = combined.raw_positions().xy(o2).unwrap();
    assert!(at[0].hypot(at[1]) < 1e-9, "the frame datum is the origin");
}

#[test]
fn a_combine_survives_a_save_and_a_reopen() {
    let (p, k, o) = kos();
    let opened = open(&p);
    let state = &opened.state;
    assert!(state.execute_combine_lines(k, o, options()).success);
    let before = canonical(state);

    save_close_reopen(state, &opened.path);
    assert_eq!(canonical(state), before);
    let reopened = project(state);
    assert!(reopened.line(o).is_none());
    assert!(reopened.validate().is_empty());
    assert_eq!(
        history(state)[0].1,
        "Combine lines \"K\" and \"O\" into \"Joined\"",
        "the label rides the file"
    );
    let r = state.undo();
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        shot_counts(state),
        vec![(k, 2), (o, 3)],
        "undo after a reopen"
    );
    let reverted = project(state);
    assert_eq!(
        st(&reverted, o, 0),
        st(&reverted, k, 2),
        "the junction is back"
    );
}

/// The line and uuid every ignore reads (the IPC's rows), sorted.
fn ignore_rows(state: &AppState) -> Vec<(LineId, String)> {
    let mut rows: Vec<_> = state
        .load_resolved_warnings()
        .unwrap()
        .into_iter()
        .map(|row| (row.line_id, row.shot_uuid.unwrap_or_default()))
        .collect();
    rows.sort();
    rows
}

fn ignore(state: &AppState, line: LineId, index: usize, by_uuid: bool) -> String {
    let (id, uuid) = {
        let p = project(state);
        let s = &p.line(line).unwrap().shots[index];
        (s.id, s.uuid.clone())
    };
    state
        .save_resolved_warning(&ResolvedWarning {
            line_id: line,
            shot_id: id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: by_uuid.then(|| uuid.clone()),
        })
        .unwrap();
    uuid
}

/// An ignore on one of the other line's shots follows the shot into the
/// kept line (it is keyed by the shot's uuid), back with the undo, again with
/// the redo, and through a save and a reopen; one recorded by position (no
/// uuid) is keyed by the uuid when stored and follows the same way, through a
/// combine that reverses the other line.
#[test]
fn an_ignore_follows_its_shot_through_a_combine_its_undo_and_a_reopen() {
    for reversing in [false, true] {
        let what = format!("reversing {reversing}");
        let mut p = empty("Ignores");
        let k = line_at(&mut p, "K", [0.0, 0.0], vec![shot(90.0, 10.0)]);
        let k1 = st(&p, k, 1);
        let o = if reversing {
            // O ends on K's end: the combine reverses it.
            let o = line_at(
                &mut p,
                "O",
                [0.0, 0.0],
                vec![shot(0.0, 0.1), shot(0.0, 5.0)],
            );
            let o2 = st(&p, o, 2);
            p.merge_stations(o2, k1).unwrap();
            o
        } else {
            line_from(&mut p, "O", k1, vec![shot(0.0, 5.0), shot(0.0, 0.1)])
        };
        let opened = open(&p);
        let state = &opened.state;
        let index = usize::from(!reversing);
        let uuid = ignore(state, o, index, !reversing);
        assert_eq!(ignore_rows(state), [(o, uuid.clone())], "{what}");

        let result = state.execute_combine_lines(k, o, options());
        assert!(result.success, "{what}: {:?}", result.error);
        assert_eq!(ignore_rows(state), [(k, uuid.clone())], "{what}: combined");
        assert!(state.undo().success);
        assert_eq!(ignore_rows(state), [(o, uuid.clone())], "{what}: undone");
        assert!(state.redo().success);
        assert_eq!(ignore_rows(state), [(k, uuid.clone())], "{what}: redone");

        save_close_reopen(state, &opened.path);
        assert_eq!(ignore_rows(state), [(k, uuid)], "{what}: reopened");
    }
}

/// The combine reverses the other line inside the same undo step when they
/// meet end to end; the undo puts its original orientation back exactly.
#[test]
fn an_end_to_end_combine_undoes_the_reverse_too() {
    let mut p = empty("Reverse");
    let k = line_at(&mut p, "K", [0.0, 0.0], vec![credited(90.0, 10.0)]);
    let o = line_at(
        &mut p,
        "O",
        [30.0, 30.0],
        vec![credited(0.0, 5.0), credited(45.0, 2.0)],
    );
    let (k1, o2) = (st(&p, k, 1), st(&p, o, 2));
    p.merge_stations(o2, k1).unwrap();
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.reversed_lines, None);
    let combined = project(state);
    let shots = &combined.line(k).unwrap().shots;
    assert_eq!(shots[1].azimuth_in, 225.0, "O's last shot, reciprocated");

    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "reversing combine");
    let r = state.undo();
    assert!(r.success);
    assert_eq!(r.reversed_lines, None, "nothing to mirror on the frontend");
}

/// The station query answers nothing for an unknown line and for a station
/// that is not a line end, including one past the end (from #713).
#[test]
fn the_station_query_refuses_unknown_and_out_of_range_stations() {
    let (p, k, o) = kos();
    let opened = open(&p);
    let state = &opened.state;
    for station in [
        StationRef::new(999, 0),
        StationRef::new(k, 1),
        StationRef::new(o, 4),
        StationRef::new(o, u32::MAX),
    ] {
        assert_eq!(
            state.combine_candidates(station).unwrap(),
            Vec::new(),
            "{station:?}"
        );
    }
    let at_end = state.combine_candidates(StationRef::new(k, 2)).unwrap();
    assert_eq!(at_end.len(), 1);
    assert_eq!((at_end[0].line_a, at_end[0].line_b), (k, o));
    assert_eq!(at_end[0].join_kind, CombineJoinKind::Merged);
    assert!(at_end[0].merged);
    assert_eq!(state.combine_candidate_for_lines(k, 999).unwrap(), None);
    assert_eq!(state.combine_candidate_for_lines(k, k).unwrap(), None);
    let pair = state.combine_candidate_for_lines(o, k).unwrap().unwrap();
    assert_eq!(pair, at_end[0], "both queries give the same candidate");
    assert!(!state.has_unsaved_changes(), "the queries are read-only");
}

/// Keeping the higher id keeps it: the lower id is the one retired (from
/// #713), K is prepended to O, every station of both stays where it was, and
/// the prepend undoes exactly.
#[test]
fn keeping_the_higher_id_keeps_it() {
    let (p, k, o) = kos();
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result = state.execute_combine_lines(o, k, options());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.line_id, Some(o));
    assert_eq!(shot_counts(state), vec![(o, 5)]);
    let combined = project(state);
    assert!(
        combined.line(k).is_none(),
        "the lower id is the one removed"
    );
    assert_eq!(st(&combined, o, 0), st(&p, k, 0), "K's start starts O now");
    assert!(apart(xy(&combined, slot(o, 0)), xy(&p, slot(k, 0))) < 1e-9);
    assert_eq!(
        history(state)[0].1,
        "Combine lines \"O\" and \"K\" into \"Joined\""
    );

    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "prepend");
}

/// Out of shot ids on the kept side: refused before anything happens, no undo
/// entry; keeping the other line works.
#[test]
fn a_kept_line_without_shot_id_room_is_refused_with_no_undo_entry() {
    let (p, k, o) = kos();
    let opened = open(&p);
    let state = &opened.state;
    let mut full = project(state);
    full.line_mut(k).unwrap().next_shot_id = (k + 1) * crate::models::FEATURE_ID_STRIDE - 1;
    state.set_project_for_tests(full);

    let reason = assert_refused(state, k, o, options(), "no shot ids left");
    assert!(reason.contains("used up its shot numbers"), "{reason}");
    let result = state.execute_combine_lines(o, k, options());
    assert!(result.success, "{:?}", result.error);
}

/// Every station's position in the Closures Computed View, by station id.
fn adjusted_by_station(state: &AppState) -> BTreeMap<u32, [f64; 2]> {
    let p = project(state);
    state
        .get_adjusted_stations()
        .unwrap()
        .into_iter()
        .map(|s| {
            let id = st(&p, s.station_ref.line_id, s.station_ref.station_index);
            (id, [s.coordinates.x, s.coordinates.y])
        })
        .collect()
}

fn assert_same_adjusted(a: &BTreeMap<u32, [f64; 2]>, b: &BTreeMap<u32, [f64; 2]>, what: &str) {
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "{what}"
    );
    for (id, p) in a {
        let moved = apart(*p, b[id]);
        assert!(
            moved < 1e-6,
            "{what}: station {id} moved {moved} m in the view"
        );
    }
}

/// The station the Closures Computed View holds a piece at is its
/// `Placement` station (plan 3.7), here O's start (O lowest, T between,
/// keep K): a combine keeps it, so the view does not move, through the
/// undo and the redo, a save and a reopen. (0.6.0 recorded it as a held
/// gauge.)
#[test]
fn the_closures_computed_view_holds_through_undo_redo_and_a_reopen() {
    let mut p = empty("Held");
    let o = line_at(&mut p, "O", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let o1 = st(&p, o, 1);
    let t = line_at(
        &mut p,
        "T",
        [40.0, 40.0],
        vec![shot(0.0, 10.0), shot(270.0, 10.0), shot(180.0, 9.0)],
    );
    let k = line_from(&mut p, "K", o1, vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let (k2, t0) = (st(&p, k, 2), st(&p, t, 0));
    p.merge_stations(t0, k2).unwrap();
    close(
        &mut p,
        slot(t, 3),
        slot(k, 1),
        ClosureStrategy::LeastSquaresWeighted,
    );
    assert!(o < t && t < k);
    let gauge = p.placements.clone();
    assert_eq!(gauge.len(), 1);
    let opened = open(&p);
    let state = &opened.state;
    let before = adjusted_by_station(state);

    assert!(state.execute_combine_lines(k, o, options()).success);
    let after = adjusted_by_station(state);
    assert_same_adjusted(&before, &after, "combine");
    assert_eq!(project(state).placements, gauge, "the gauge stays");

    assert!(state.undo().success);
    assert_same_adjusted(&before, &adjusted_by_station(state), "undo");
    assert!(state.redo().success);
    assert_same_adjusted(&after, &adjusted_by_station(state), "redo");

    save_close_reopen(state, &opened.path);
    assert_same_adjusted(&after, &adjusted_by_station(state), "reopen");
    assert_eq!(project(state).placements, gauge);
}

/// At a junction (C starting at K's end, where O starts), combining K and O
/// keeps C on the joined station, the combined line's station 2 whether O is
/// appended to K or K prepended to O; an undo and a redo restore the whole
/// project exactly; and ignores on C (the branch) and on O (the moved line)
/// stay on their shots through it, a save and a reopen.
#[test]
fn a_combine_at_a_junction_keeps_the_branch_and_undoes_exactly() {
    for keep_k in [true, false] {
        let what = format!("keep K {keep_k}");
        let (mut p, k, o) = kos();
        let k2 = st(&p, k, 2);
        let c = line_from(&mut p, "C", k2, vec![shot(180.0, 5.0), shot(180.0, 0.1)]);
        let opened = open(&p);
        let state = &opened.state;
        let uuids = [ignore(state, c, 1, true), ignore(state, o, 2, true)];
        let candidate = state.combine_candidate_for_lines(k, o).unwrap().unwrap();
        assert_eq!(candidate.branches, vec![c], "{what}");
        let before = snapshot(state);

        let (kept, other) = if keep_k { (k, o) } else { (o, k) };
        let result = state.execute_combine_lines(kept, other, options());
        assert!(result.success, "{what}: {:?}", result.error);
        let combined = project(state);
        assert_eq!(st(&combined, kept, 2), k2, "{what}: the joined station");
        assert_eq!(st(&combined, c, 0), k2, "{what}: C stays on it");
        assert!(combined.topology().is_junction(k2), "{what}");
        let lines_of = |state: &AppState| {
            let rows = ignore_rows(state);
            uuids
                .iter()
                .map(|u| rows.iter().find(|(_, x)| x == u).unwrap().0)
                .collect::<Vec<_>>()
        };
        assert_eq!(lines_of(state), [c, kept], "{what}: the ignores");

        let after = snapshot(state);
        assert_undo_redo_exact(state, &before, &after, &what);
        assert_eq!(lines_of(state), [c, kept], "{what}");

        save_close_reopen(state, &opened.path);
        let reopened = project(state);
        assert_eq!(
            st(&reopened, c, 0),
            st(&reopened, kept, 2),
            "{what}: reopened"
        );
        assert_eq!(lines_of(state), [c, kept], "{what}: reopened");
    }
}

/// Two lines closing a loop on each other, combined at the loop closure:
/// the other line moves by the misclosure, the junction at the far end is
/// detached into the loop's closure (S1: O takes a new station there, a
/// `Batch` closure ties it to K's start), and one undo restores the two
/// lines, the junction and the closure exactly.
#[test]
fn a_combine_at_a_loop_closure_is_one_undo_step() {
    let mut p = empty("Loop");
    let k = line_at(&mut p, "K", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let o = line_at(&mut p, "O", [30.0, 30.0], vec![shot(270.0, 9.6)]);
    let (k0, o1) = (st(&p, k, 0), st(&p, o, 1));
    p.merge_stations(o1, k0).unwrap();
    close(&mut p, slot(k, 1), slot(o, 0), ClosureStrategy::Batch);
    let (k0_at, k1_at) = (xy(&p, slot(k, 0)), xy(&p, slot(k, 1)));
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result = state.execute_combine_lines(k, o, joining(LineEnd::End, LineEnd::Start));
    assert!(result.success, "{:?}", result.error);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let combined = project(state);
    assert!(apart(xy(&combined, slot(k, 0)), k0_at) < 1e-9);
    assert!(apart(xy(&combined, slot(k, 1)), k1_at) < 1e-9);
    assert!(apart(xy(&combined, slot(k, 2)), [k0_at[0] + 0.4, k0_at[1]]) < 1e-9);
    let topology = combined.topology();
    assert!(topology
        .stations()
        .iter()
        .all(|s| !topology.is_junction(*s)));
    assert_eq!(combined.closures.len(), 1);
    let closure = combined.closures[0];
    assert_eq!(closure.strategy, ClosureStrategy::Batch);
    let metrics = combined.closure_metrics(closure.a, closure.b).unwrap();
    assert!(
        (metrics.misclosure_distance - 0.4).abs() < 1e-9,
        "{metrics:?}"
    );

    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "loop closure join");
}

/// Combine `kept` and `other` at the given ends through the command, after
/// checking the station menu's plan for that choice (asked at `at`) lists
/// `listed` as the ties it removes across one shot (#746). Returns the
/// command's removed-closure warnings; asserts the combine left no closure
/// across one shot, and that one Undo restores the project exactly and Redo
/// re-applies it.
fn combine_removing(
    state: &AppState,
    at: StationRef,
    (kept, kept_end): (LineId, LineEnd),
    (other, other_end): (LineId, LineEnd),
    listed: &[(StationRef, StationRef, CombineRemovedKind)],
) -> Vec<String> {
    let candidate = state
        .combine_candidates(at)
        .unwrap()
        .into_iter()
        .find(|c| {
            let ends = [(c.line_a, c.end_a), (c.line_b, c.end_b)];
            ends.contains(&(kept, kept_end)) && ends.contains(&(other, other_end))
        })
        .unwrap();
    let plan = if candidate.line_a == kept {
        candidate.plan_keeping_a
    } else {
        candidate.plan_keeping_b
    };
    let named: Vec<_> = plan
        .closures_removed_across_one_shot
        .unwrap_or_default()
        .iter()
        .map(|r| (r.station_a, r.station_b, r.kind))
        .collect();
    assert_eq!(
        named, listed,
        "the dialog lists the ties the combine removes"
    );
    let before = snapshot(state);

    let result = state.execute_combine_lines(kept, other, joining(kept_end, other_end));
    assert!(result.success, "{:?}", result.error);
    let combined = project(state);
    assert!(
        combined.closures_across_one_shot().is_empty(),
        "{:?}",
        combined.closures_across_one_shot()
    );
    let after = snapshot(state);
    assert_undo_redo_exact(
        state,
        &before,
        &after,
        "one undo restores the removed tie too",
    );
    result
        .warnings
        .iter()
        .map(|w| {
            assert_eq!(w.code, CLOSURE_REMOVED_ACROSS_ONE_SHOT);
            assert_eq!(w.severity, WarningSeverity::Warning);
            w.message.clone()
        })
        .collect()
}

/// #746, the issue's shape (a): K 10 m east; a passage X from K's end, one
/// shot 0.4 m further east; O 5 m east from X's end (a junction); K's end
/// and O's start tied by a legacy closure, as a file from before the
/// deferred strategies carries it. Combining K and O there detaches O from
/// X's end (S1) into a closure across X's one shot: the combine removes it
/// inside its one undo step and says so, naming the junction by its slots
/// and its one name before the combine.
#[test]
fn a_combine_removes_the_merge_it_would_tear_across_one_shot() {
    let mut p = empty("Passage");
    let k = line_at(&mut p, "K", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let k1 = st(&p, k, 1);
    let x = line_from(&mut p, "X", k1, vec![shot(90.0, 0.4)]);
    let x1 = st(&p, x, 1);
    let o = line_from(&mut p, "O", x1, vec![shot(90.0, 5.0)]);
    p.push_closure(
        k1,
        Some(k),
        x1,
        Some(o),
        ClosureStrategy::ProportionalByLength,
    );
    let opened = open(&p);
    let state = &opened.state;

    let warnings = combine_removing(
        state,
        slot(k, 1),
        (k, LineEnd::End),
        (o, LineEnd::Start),
        &[(slot(o, 0), slot(x, 1), CombineRemovedKind::Merge)],
    );
    let name = p.stations.get(&x1).unwrap().name.clone();
    assert_eq!(
        warnings,
        vec![format!(
            "Removed the merge of O / {name} and X / {name}: combining the lines left its two \
             stations at the two ends of one shot."
        )]
    );
    let combined = project(state);
    assert!(
        combined.topology().is_junction(k1),
        "K and X still share K's end"
    );
    assert!(!combined.topology().is_junction(x1), "O left X's end");
}

/// The same shape with O merged onto X's end instead of started there: O
/// knows the junction by an alias ("0", its own start before the merge), so
/// the warning names O's slot by O's name, as the Combine dialog lists it
/// (I-3), not by the junction's own name. Injection: naming the slot by the
/// station's own name (`station.name`) fails it with "O / <X's name>".
#[test]
fn a_removed_merge_names_each_slot_as_its_line_shows_it() {
    let mut p = empty("Passage");
    let k = line_at(&mut p, "K", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let k1 = st(&p, k, 1);
    let x = line_from(&mut p, "X", k1, vec![shot(90.0, 0.4)]);
    let x1 = st(&p, x, 1);
    let o = line_at(&mut p, "O", [30.0, 30.0], vec![shot(90.0, 5.0)]);
    p.merge_stations(st(&p, o, 0), x1).unwrap();
    assert_eq!(st(&p, o, 0), x1, "X's end survives the merge");
    assert_eq!(p.shown_name(x1), Some("0"), "O knows it by its own start");
    let own = p.stations.get(&x1).unwrap().name.clone();
    assert_ne!(own, "0", "the junction's own name is X's");
    p.push_closure(
        k1,
        Some(k),
        x1,
        Some(o),
        ClosureStrategy::ProportionalByLength,
    );
    let opened = open(&p);
    let state = &opened.state;

    let warnings = combine_removing(
        state,
        slot(k, 1),
        (k, LineEnd::End),
        (o, LineEnd::Start),
        &[(slot(o, 0), slot(x, 1), CombineRemovedKind::Merge)],
    );
    assert_eq!(
        warnings,
        vec![format!(
            "Removed the merge of O / 0 and X / {own}: combining the lines left its two \
             stations at the two ends of one shot."
        )]
    );
}

/// #746, an ordinary closure the join alone puts across one shot. K runs two
/// shots east; O starts beside K's end (co-located, its own station) and
/// loops back north, west and south onto K's start (a junction); a `Batch`
/// closure ties K's middle station to O's start. Combined at K's end, O's
/// start is K's end and the closure spans K's last shot: the combine removes
/// it, and the junction that would make the combined line pass K's start
/// twice is detached into a closure around the real loop.
#[test]
fn a_combine_removes_a_closure_its_join_puts_across_one_shot() {
    let mut p = empty("Loop");
    let k = line_at(
        &mut p,
        "K",
        [0.0, 0.0],
        vec![shot(90.0, 5.0), shot(90.0, 5.0)],
    );
    let o = line_at(
        &mut p,
        "O",
        [10.0, 0.0],
        vec![shot(0.0, 4.0), shot(270.0, 10.0), shot(180.0, 4.0)],
    );
    let (k0, o3) = (st(&p, k, 0), st(&p, o, 3));
    p.merge_stations(o3, k0).unwrap();
    close(&mut p, slot(k, 1), slot(o, 0), ClosureStrategy::Batch);
    let opened = open(&p);
    let state = &opened.state;

    let warnings = combine_removing(
        state,
        slot(k, 2),
        (k, LineEnd::End),
        (o, LineEnd::Start),
        &[(slot(k, 1), slot(o, 0), CombineRemovedKind::LoopClosure)],
    );
    // By the names before the combine: K is "Joined" now, O is gone. O's
    // start keeps its "0": the junction on K's start is named "0" on both
    // lines, and O knows it by its alias (O's own "3"), so the merge
    // lettered nothing (review findings D-migration-upgrade-8 and UX-3).
    let o0 = p.stations.get(&st(&p, o, 0)).unwrap().name.clone();
    assert_eq!(o0, "0");
    assert_eq!(
        warnings,
        vec![format!(
            "Removed loop closure K / 1 to O / {o0}: combining the lines left its two \
             stations at the two ends of one shot."
        )]
    );
    let combined = project(state);
    let topology = combined.topology();
    assert!(topology
        .stations()
        .iter()
        .all(|s| !topology.is_junction(*s)));
    assert_eq!(combined.closures.len(), 1, "the torn junction's closure");
}

/// A loop closure left from a snap (#736) that a combine removes across one
/// shot is named as what it is, not as a snapped closure, in the warning the
/// toast shows, as the Combine dialog names it.
#[test]
fn a_removed_former_snap_is_named_as_the_loop_closure_left_from_a_snap() {
    let mut p = empty("Labels");
    let k = line_at(
        &mut p,
        "K",
        [0.0, 0.0],
        vec![shot(0.0, 5.0), shot(0.0, 5.0)],
    );
    let o = line_at(&mut p, "O", [20.0, 0.0], vec![shot(90.0, 5.0)]);
    let removed = [
        CombineRemovedClosure {
            station_a: slot(k, 2),
            station_b: slot(o, 0),
            kind: CombineRemovedKind::FormerSnappedClosure,
        },
        CombineRemovedClosure {
            station_a: slot(k, 9),
            station_b: slot(77, 0),
            kind: CombineRemovedKind::SnappedClosure,
        },
    ];
    let warnings = super::command_combine_lines::combine_removed_warnings(&p, &removed);
    assert_eq!(
        warnings
            .iter()
            .map(|w| w.message.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Removed loop closure K / 2 to O / 0, left from a snap whose stations had moved \
             apart: combining the lines left its two stations at the two ends of one shot.",
            "Removed the snapped closure between a station of K and a station: combining the \
             lines left its two stations at the two ends of one shot.",
        ]
    );
}

/// A datum on each of two side-by-side join stations (two pieces, each
/// placed by its own): combining would make them one station holding two
/// placement datums, so it is refused and changes nothing. A merged join is
/// one station already, which may hold both (S8, plan 5.7): that combine
/// goes ahead, keeps both datums on the joined station, and undoes exactly.
#[test]
fn two_datums_on_side_by_side_join_stations_are_refused_and_one_station_may_hold_both() {
    let mut p = empty("Two datums");
    let k = line_at(
        &mut p,
        "K",
        [0.0, 0.0],
        vec![shot(90.0, 10.0), shot(90.0, 5.0)],
    );
    let o = line_at(
        &mut p,
        "O",
        [15.0, 0.0],
        vec![shot(0.0, 5.0), shot(0.0, 5.0)],
    );
    let (k2, o0) = (st(&p, k, 2), st(&p, o, 0));
    add_datum(&mut p, k2, gps(20.0, -87.0));
    hold(&mut p, o0, gps(20.0001, -87.0));
    let opened = open(&p);
    let reason = assert_refused(&opened.state, k, o, options(), "side by side");
    assert!(reason.contains("each hold a zero datum"), "{reason}");

    let (mut p, k, o) = kos();
    let join = st(&p, k, 2);
    add_datum(&mut p, join, gps(20.0, -87.0));
    add_datum(&mut p, join, gps(20.0001, -87.0));
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);
    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    let combined = project(state);
    assert_eq!(combined.datums.len(), 2);
    assert!(combined.datums.iter().all(|d| d.station == join));
    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "merged join with two datums");
}

/// #749, the smallest survey, through the command and a saved file. B
/// (created first, so its start is where the Closures Computed View holds
/// the survey) runs 10 m east, 9.6 m back west and 5 m north; a legacy
/// closure (one the solve does not take, as a file from before the deferred
/// strategies carries it) ties B:2, 0.4 m east of B's start, back to B's
/// start. A runs one shot 4 m south and ends on B:2. Combining A's end with
/// B's start keeps A, which moves 0.4 m onto B's start, and detaches the
/// junction into a `Batch` closure on the legacy closure's two stations
/// (PassesTwice, S2). That closure is the one left there, so the view solves
/// the joined survey as the plan said it would, held where B's start was,
/// and the plan lists the legacy closure as replaced by the torn junction's;
/// one Undo restores the project exactly, Redo re-applies it, and a save and
/// a reopen keep it. No warning: the connection stays.
#[test]
fn a_combine_keeps_the_closure_the_solve_takes_over_a_legacy_one() {
    let mut p = empty("Legacy loop");
    let b = line_at(
        &mut p,
        "B",
        [0.0, 0.0],
        vec![shot(90.0, 10.0), shot(270.0, 9.6), shot(0.0, 5.0)],
    );
    let (b0, b2) = (st(&p, b, 0), st(&p, b, 2));
    let a = line_at(&mut p, "A", [30.0, 30.0], vec![shot(180.0, 4.0)]);
    let a1 = st(&p, a, 1);
    p.merge_stations(a1, b2).unwrap();
    let legacy = p.push_closure(
        b2,
        Some(b),
        b0,
        Some(b),
        ClosureStrategy::ProportionalByLength,
    );
    assert!(b < a);
    let b0_at = xy(&p, slot(b, 0));
    let opened = open(&p);
    let state = &opened.state;

    let candidate = state
        .combine_candidates(slot(a, 1))
        .unwrap()
        .into_iter()
        .find(|c| [c.line_a, c.line_b] == [b, a])
        .unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    let plan = candidate.plan_keeping_b;
    assert_eq!(plan.moving_line, Some(a));
    assert_eq!(
        plan.kept_moves_because,
        Some(KeptMovesReason::AdjustmentAnchor)
    );
    assert_eq!(
        plan.torn_ties.len(),
        1,
        "the junction becomes a loop closure"
    );
    let replaced = plan.closures_replaced.unwrap();
    assert_eq!(replaced.len(), 1);
    assert_eq!(
        (replaced[0].station_a, replaced[0].station_b),
        (slot(b, 2), slot(b, 0)),
        "the dialog lists the legacy closure the torn junction's closure replaces"
    );
    assert_eq!(
        replaced[0].kind,
        crate::models::CombineReplacedKind::LegacyClosure
    );
    assert_eq!(replaced[0].by_kind, CombineRemovedKind::Merge);
    let before = snapshot(state);

    let result = state.execute_combine_lines(a, b, joining(LineEnd::End, LineEnd::Start));
    assert!(result.success, "{:?}", result.error);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let check = |state: &AppState, what: &str| {
        let p = project(state);
        let topology = p.topology();
        assert!(
            topology
                .stations()
                .iter()
                .all(|s| !topology.is_junction(*s)),
            "{what}"
        );
        let left: Vec<_> = p
            .closures
            .iter()
            .map(|c| {
                let mut ends = [
                    p.closure_end_slot(c.a, c.a_via).unwrap(),
                    p.closure_end_slot(c.b, c.b_via).unwrap(),
                ];
                ends.sort_by_key(|s| s.station_index);
                (c.id == legacy, c.strategy, ends)
            })
            .collect();
        assert_eq!(
            left,
            vec![(false, ClosureStrategy::Batch, [slot(a, 1), slot(a, 3)])],
            "{what}: the torn junction's closure, not the legacy one"
        );
        let adjusted = state.get_adjusted_stations().unwrap();
        let report = state.get_adjustment_report().unwrap();
        assert!(report.failures.is_empty(), "{what}: {:?}", report.failures);
        let at = |index: u32| {
            let s = adjusted
                .iter()
                .find(|s| s.station_ref == slot(a, index))
                .unwrap();
            [s.coordinates.x, s.coordinates.y]
        };
        let (joined, b2) = (at(1), at(3));
        assert!(
            apart(joined, b0_at) < 1e-6,
            "{what}: held where B's start was"
        );
        assert!(apart(joined, b2) < 1e-6, "{what}: the view closes the loop");
    };
    check(state, "combine");
    let after = snapshot(state);
    assert_undo_redo_exact(
        state,
        &before,
        &after,
        "one undo restores the legacy closure",
    );
    check(state, "redo");

    save_close_reopen(state, &opened.path);
    check(state, "reopen");
}

/// #740 through the command: M runs north from the origin with O (east)
/// ending on its start (a junction), and K (east) ends 0.6 mm short of O's
/// start: side by side, two pieces. Combining there keeping K moves O by the
/// residual, with M on it, so O's far end (K's station 2 now) stays M's
/// start and K stays put. One Undo restores the project exactly, Redo
/// re-applies it, and a save and a reopen move nothing.
#[test]
fn a_side_by_side_combine_keeps_merged_stations_together_through_a_reopen() {
    let mut p = empty("Side by side");
    let m = line_at(&mut p, "M", [0.0, 0.0], vec![shot(0.0, 10.0)]);
    let o = line_at(&mut p, "O", [-10.0, 0.0], vec![shot(90.0, 10.0)]);
    let (o1, m0) = (st(&p, o, 1), st(&p, m, 0));
    p.merge_stations(o1, m0).unwrap();
    let k = line_at(&mut p, "K", [-20.0006, 0.0], vec![shot(90.0, 10.0)]);
    let gap = apart(xy(&p, slot(k, 1)), xy(&p, slot(o, 0)));
    assert!(
        (gap - 0.0006).abs() < 1e-9,
        "setup: the ends are {gap} m apart"
    );
    let k0 = xy(&p, slot(k, 0));
    let opened = open(&p);
    let state = &opened.state;

    let candidate = state.combine_candidate_for_lines(k, o).unwrap().unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated);
    let plan = if candidate.line_a == k {
        candidate.plan_keeping_a
    } else {
        candidate.plan_keeping_b
    };
    assert_eq!(plan.moving_line, Some(o));
    assert_eq!(plan.moved_lines, vec![o, m]);
    assert!((plan.shift_m - 0.0006).abs() < 1e-9, "{}", plan.shift_m);
    let before = snapshot(state);

    let result = state.execute_combine_lines(k, o, joining(LineEnd::End, LineEnd::Start));
    assert!(result.success, "{:?}", result.error);
    let check = |state: &AppState, what: &str| {
        let p = project(state);
        assert_eq!(
            st(&p, k, 2),
            st(&p, m, 0),
            "{what}: O's far end is M's start"
        );
        assert!(apart(xy(&p, slot(k, 0)), k0) < 1e-12, "{what}: K stays");
    };
    check(state, "combine");
    let combined = positions(&project(state));
    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "side by side");
    check(state, "redo");

    save_close_reopen(state, &opened.path);
    check(state, "reopen");
    let reopened = positions(&project(state));
    assert_eq!(reopened.len(), combined.len());
    for ((a, p), (b, q)) in combined.iter().zip(&reopened) {
        assert_eq!(a, b);
        assert!(apart(*p, *q) < 1e-9, "the reopen moved {a:?}");
    }
}

/// #755 through the command, with the hand move as a migrated file carries
/// it (no command makes one): K's end and O's start side by side, O's start
/// moved 0.3 m east and 0.2 m down by hand on O, T starting there (a
/// junction). The joined station keeps K's hand move (none), so the plan
/// moves T with it by the 0.3 m; the command's result is exactly that, with
/// T on the joined station and no shot changed; one Undo restores every
/// byte and Redo repeats it, and a save and a reopen move nothing.
#[test]
fn a_hand_moved_join_station_keeps_its_branch_through_undo_and_a_reopen() {
    // Level shots, each line at its own depth: K 2 m, O 3 m, T 5 m.
    let level = |azimuth: f64, length: f64, depth: f64| deep(azimuth, length, depth, depth);
    let mut p = empty("Hand moved");
    let k = line_at(&mut p, "K", [0.0, 0.0], vec![level(90.0, 10.0, 2.0)]);
    let o = line_at(&mut p, "O", [10.3, 0.0], vec![level(90.0, 10.0, 3.0)]);
    let o0 = st(&p, o, 0);
    p.line_mut(o)
        .unwrap()
        .hand_moves
        .insert(o0, [0.3, 0.0, 0.2]);
    let t = line_from(&mut p, "T", o0, vec![level(0.0, 5.0, 5.0)]);
    let opened = open(&p);
    let state = &opened.state;
    let before_project = project(state);
    let depth = |p: &Project, at: StationRef| p.raw_positions().slot(p, at).unwrap()[2];
    // The junction reads the average of O's and T's readings; O's slot adds
    // its hand move's 0.2 m.
    assert!((depth(&before_project, slot(o, 0)) - 4.2).abs() < 1e-9);
    assert!((depth(&before_project, slot(t, 0)) - 4.0).abs() < 1e-9);
    let candidate = state.combine_candidate_for_lines(k, o).unwrap().unwrap();
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moved_lines, vec![o]);
    assert!(plan.shift_m < 1e-9, "the chain gap is 0: {}", plan.shift_m);
    let joined = plan.moved_with_joined_station.clone().unwrap();
    assert_eq!(joined.lines, vec![t], "T moves with the joined station");
    assert!((joined.shift_m - 0.3).abs() < 1e-9, "{}", joined.shift_m);
    let before = snapshot(state);
    let t0_was = xy(&before_project, slot(t, 0));

    let result = state.execute_combine_lines(k, o, options());
    assert!(result.success, "{:?}", result.error);
    let combined = project(state);
    // K stays, O's start lands on K's end and its far end stays, T moves the
    // 0.3 m with the joined station and stays on it.
    for (at, want) in [
        (slot(k, 0), [0.0, 0.0]),
        (slot(k, 1), [10.0, 0.0]),
        (slot(k, 2), [20.0, 0.0]),
        (slot(t, 0), [10.0, 0.0]),
        (slot(t, 1), [10.0, 5.0]),
    ] {
        let now = xy(&combined, at);
        assert!(apart(now, want) < 1e-9, "{at:?} at {now:?}");
    }
    assert!(
        (apart(xy(&combined, slot(t, 0)), t0_was) - joined.shift_m).abs() < 1e-9,
        "T moved as planned, no more"
    );
    assert_eq!(
        st(&combined, t, 0),
        st(&combined, k, 1),
        "T on the joined station"
    );
    assert!(
        combined.lines().iter().all(|l| l.hand_moves.is_empty()),
        "the joined station keeps K's hand move: none"
    );
    let lengths: Vec<f64> = combined
        .line(k)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect();
    assert_eq!(lengths, vec![10.0, 10.0], "no shot changes");
    // The joined station reads the average of the three readings there.
    let joined_depth = (2.0 + 3.0 + 5.0) / 3.0;
    for at in [slot(k, 1), slot(t, 0)] {
        let now = depth(&combined, at);
        assert!((now - joined_depth).abs() < 1e-9, "{at:?} at depth {now}");
    }
    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "hand-moved join");

    let shown = positions(&project(state));
    save_close_reopen(state, &opened.path);
    let reopened = project(state);
    assert_eq!(positions(&reopened), shown, "the reopen moves nothing");
    for at in [slot(k, 1), slot(t, 0)] {
        let now = depth(&reopened, at);
        assert!(
            (now - joined_depth).abs() < 1e-9,
            "reopened, {at:?} at depth {now}"
        );
    }
}

/// Every line's legacy surveyor and every shot's credited surveyors.
fn credits(state: &AppState) -> Vec<(LineId, Option<String>, Vec<Vec<String>>)> {
    project(state)
        .lines()
        .iter()
        .map(|l| {
            let shots = l
                .shots
                .iter()
                .map(|s| {
                    s.credit
                        .as_ref()
                        .map(|c| c.surveyors.iter().map(|p| p.name.clone()).collect())
                        .unwrap_or_default()
                })
                .collect();
            (l.id, l.legacy_surveyor.clone(), shots)
        })
        .collect()
}

/// A shotless line keeps a legacy surveyor until shots arrive. Combining
/// shots into it hands that surveyor to the ones without a credit, as the
/// first appended shot takes it, so no later undo or reload re-reads it onto
/// them and lands on a different state.
#[test]
fn shots_combined_into_a_shotless_line_take_its_legacy_surveyor() {
    let mut p = empty("Legacy combine");
    let kept = p
        .create_line(
            LineMeta {
                name: Some("Empty".to_string()),
                legacy_surveyor: Some("Diver One".to_string()),
                ..LineMeta::default()
            },
            StartAt::New,
            Vec::new(),
            Some([0.0, 0.0]),
        )
        .unwrap()
        .value;
    let start = st(&p, kept, 0);
    let other = line_from(
        &mut p,
        "Line",
        start,
        vec![shot(10.0, 4.0), shot(20.0, 4.0)],
    );
    let opened = open(&p);
    let state = &opened.state;
    assert_eq!(
        credits(state)[0].1.as_deref(),
        Some("Diver One"),
        "premise: the shotless line holds the legacy surveyor"
    );
    let before = snapshot(state);

    let result = state.execute_combine_lines(kept, other, options());
    assert!(result.success, "{:?}", result.error);
    let combined = credits(state);
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].1, None, "the line let the legacy surveyor go");
    assert!(
        combined[0]
            .2
            .iter()
            .all(|s| s == &vec!["Diver One".to_string()]),
        "every arriving shot took it: {combined:?}"
    );
    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "legacy surveyor");

    save_close_reopen(state, &opened.path);
    assert_eq!(credits(state), combined, "a reload hands nothing out again");
}

/// What the dialog shows is what the command does: for every candidate at a
/// junction of three lines, each choice of the kept line combines with the
/// plan's moving line and removed ties, and one undo puts the project back.
#[test]
fn every_offered_combine_at_a_junction_does_what_its_plan_says() {
    let (mut p, k, _) = kos();
    let k2 = st(&p, k, 2);
    line_from(&mut p, "C", k2, vec![shot(180.0, 5.0), shot(200.0, 3.0)]);
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);
    let candidates = state.combine_candidates(slot(k, 2)).unwrap();
    assert_eq!(candidates.len(), 3, "K with O, K with C, then O with C");
    for candidate in candidates {
        for keep_a in [true, false] {
            let (kept, other, kept_end, other_end, plan, blocked) = if keep_a {
                let (a, b) = (candidate.line_a, candidate.line_b);
                (
                    a,
                    b,
                    candidate.end_a,
                    candidate.end_b,
                    &candidate.plan_keeping_a,
                    &candidate.blocked_keeping_a,
                )
            } else {
                let (a, b) = (candidate.line_b, candidate.line_a);
                (
                    a,
                    b,
                    candidate.end_b,
                    candidate.end_a,
                    &candidate.plan_keeping_b,
                    &candidate.blocked_keeping_b,
                )
            };
            let what = format!("keep {kept} of {kept}/{other}");
            let result = state.execute_combine_lines(kept, other, joining(kept_end, other_end));
            let refusal = candidate.blocked_reason.clone().or_else(|| blocked.clone());
            if let Some(reason) = refusal {
                assert!(!result.success, "{what}");
                assert_eq!(result.error, Some(reason), "{what}");
                continue;
            }
            assert!(result.success, "{what}: {:?}", result.error);
            assert_eq!(
                result.warnings.len(),
                plan.closures_removed_across_one_shot
                    .as_ref()
                    .map_or(0, Vec::len),
                "{what}"
            );
            assert!(project(state).line(other).is_none(), "{what}");
            assert!(state.undo().success, "{what}");
            assert_same(&snapshot(state), &before, &what);
        }
    }
}
