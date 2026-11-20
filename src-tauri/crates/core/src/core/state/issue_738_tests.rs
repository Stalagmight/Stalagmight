//! [#738] A loop closure left between merged stations, at the app-state
//! level, on the station graph. A merge is one station now, so such a
//! closure would be `a == b`: invariant 7 refuses it, the merge core removes
//! one a merge folds, and the sweep removes one a promotion folds. Split Line
//! Here no longer reaches the issue at all (the halves share the split
//! station, so no closure becomes the halves' only tie and none is
//! promoted); a file cannot hold one (the decoder refuses it, a load never
//! heals, plan 6.6); the commands whose sweep removes one report it as a
//! warning named by its stations before the edit.
//!
//! Fate of each 0.6.0 test:
//! - adapted: the five Split Line Here tests
//!   (`the_split_removes_the_second_closure_to_the_point_and_its_preview_says_so`,
//!   `the_first_closure_in_the_list_is_the_one_promoted`,
//!   `three_closures_to_one_point_leave_one_merge_and_no_closure`,
//!   `a_cut_along_a_shot_names_the_removed_closure_by_its_stations_before_the_cut`,
//!   `a_split_with_one_closure_across_the_cut_removes_none`) ->
//!   `the_issues_split_keeps_every_closure_and_moves_nothing` (declared
//!   change 12.2: Split Line Here makes a junction);
//!   `a_file_with_closures_between_merged_stations_opens_without_them` ->
//!   `a_file_with_a_closure_on_one_station_is_refused_not_healed`;
//!   `merging_two_placed_pieces_removes_and_reports_the_closure_it_folds` ->
//!   `merging_two_placed_pieces_removes_the_closure_it_folds` (the removal
//!   and the exact undo; the warning is
//!   `merging_two_placed_pieces_reports_the_closure_it_folds`, ignored until
//!   the topology package reports sweep warnings);
//!   `a_closure_elsewhere_between_two_merged_placed_pieces_stays_a_loop`:
//!   ported;
//! - ported and ignored until their commands are ported on this branch
//!   (C2 shots, C3 lines, C5 topology): `delete_shot_and_split_leaves_no_closure_between_merged_stations`,
//!   `delete_shot_range_and_split_reports_the_closure_it_removes`,
//!   `every_command_that_removes_one_reports_it_by_its_stations_before_the_edit`,
//!   `removing_a_zero_datum_reports_the_closure_its_sweep_removes`. Their
//!   expected warnings are 0.6.0's wording with the station graph's names:
//!   a junction has one name on all its lines (plan 5.3), so the end 0.6.0
//!   called `T / 0` is `T / 1` (T starts on S's station 1);
//! - retired: `the_preview_names_a_closure_already_between_merged_stations_apart`
//!   and `a_closure_that_already_was_between_merged_stations_is_reported_as_such`
//!   (a closure on one station cannot exist in a valid model, invariant 7,
//!   and a file holding one is refused; see the file test),
//!   `the_preview_names_the_removed_closure_when_ids_repeat` (a repeated
//!   closure id is a violation, invariant 1).
//!
//! All data is synthetic: projects built in memory, files in a temporary
//! directory.

use super::split_combine_kit::{
    add_datum, apart, assert_same, assert_undo_redo_exact, close, empty, gps, hold, line_at,
    line_from, open, positions, project, shot, slot, snapshot, st,
};
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{ClosureId, ClosureStrategy, LoopClosure, Project};
use crate::models::{
    LineId, SplitAt, SplitLineOptions, SplitSide, StationRef, CLOSURE_REMOVED_MERGED_ENDS,
};

fn options() -> SplitLineOptions {
    SplitLineOptions {
        name_a: "S-A".to_string(),
        name_b: "S-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    }
}

/// A closure planted between two slots, as a file can carry it (creation
/// refuses a second closure on a pair that has one).
fn plant(p: &mut Project, a: StationRef, b: StationRef) -> ClosureId {
    let (sa, sb) = (
        st(p, a.line_id, a.station_index),
        st(p, b.line_id, b.station_index),
    );
    p.push_closure(
        sa,
        Some(a.line_id),
        sb,
        Some(b.line_id),
        ClosureStrategy::Batch,
    )
}

/// The issue's survey. S runs 10 m east, 10 m north, then 9.9 m back south,
/// so S:3 ends 0.1 m short of S:1; T (and, with `"U"` in `ties`, U) starts
/// on S:1, a junction. Then one closure per entry of `ties`, in that order,
/// from S:3 to the junction: `"S"` on S's slot, `"T"` on T's and `"U"` on
/// U's. Returns the project, S, T and the closure ids in `ties` order.
fn survey(ties: &[&str]) -> (Project, LineId, LineId, Vec<ClosureId>) {
    let mut p = empty("738");
    let s = line_at(
        &mut p,
        "S",
        [0.0, 0.0],
        vec![shot(90.0, 10.0), shot(0.0, 10.0), shot(180.0, 9.9)],
    );
    let s1 = st(&p, s, 1);
    let t = line_from(&mut p, "T", s1, vec![shot(90.0, 5.0)]);
    let u = ties
        .contains(&"U")
        .then(|| line_from(&mut p, "U", s1, vec![shot(270.0, 5.0)]));
    let ids = ties
        .iter()
        .map(|tie| match *tie {
            "S" => plant(&mut p, slot(s, 3), slot(s, 1)),
            "T" => plant(&mut p, slot(t, 0), slot(s, 3)),
            _ => plant(&mut p, slot(s, 3), slot(u.unwrap(), 0)),
        })
        .collect();
    (p, s, t, ids)
}

/// The closures as the split must leave them: ids, ends and strategies.
fn closure_ends(p: &Project) -> Vec<(ClosureId, u32, u32, ClosureStrategy)> {
    p.closures
        .iter()
        .map(|c| (c.id, c.a, c.b, c.strategy))
        .collect()
}

/// Every split of the issue's survey, at S:2 or along the north shot, keeps
/// every closure (the halves share the cut station, so the loop through S:3
/// is still a loop and no closure is the halves' only tie), moves nothing,
/// and its preview says so; whatever the closures' order or number, the
/// same survey gives the same preview, and one undo and one redo are exact.
#[test]
fn the_issues_split_keeps_every_closure_and_moves_nothing() {
    for ties in [&["S", "T"][..], &["T", "S"], &["S", "T", "U"], &["S"]] {
        for along_shot in [false, true] {
            let what = format!("{ties:?}, along the shot {along_shot}");
            let (p, s, _, ids) = survey(ties);
            let opened = open(&p);
            let state = &opened.state;
            let at = if along_shot {
                SplitAt::Shot {
                    shot_id: p.line(s).unwrap().shots[1].id,
                    distance: 4.0,
                }
            } else {
                SplitAt::Station { station_index: 2 }
            };
            let before = snapshot(state);
            let preview = state.split_line_preview(s, at, &options()).unwrap();
            assert!(
                snapshot(state) == before,
                "{what}: the preview changed the project"
            );
            assert_eq!(preview.closures_to_merges, 0, "{what}");
            assert_eq!(preview.moved_side, None, "{what}");
            assert_eq!(preview.moved_m, 0.0, "{what}");
            assert!(preview.removed_closures.is_empty(), "{what}");
            assert!(preview.removed_already_one_point.is_empty(), "{what}");
            let again = open(&p);
            assert_eq!(
                again.state.split_line_preview(s, at, &options()).unwrap(),
                preview,
                "{what}: deterministic"
            );

            let result = state.execute_split_line(s, at, options());
            assert!(result.success, "{what}: {:?}", result.error);
            let after_project = project(state);
            assert_eq!(closure_ends(&after_project), closure_ends(&p), "{what}");
            assert_eq!(after_project.closures.len(), ids.len(), "{what}");
            for (at, was) in positions(&p) {
                let id = st(&p, at.line_id, at.station_index);
                let now = after_project.raw_positions().xy(id).unwrap();
                assert!(apart(was, now) < 1e-9, "{what}: {at:?} moved");
            }
            assert!(after_project.validate().is_empty(), "{what}");
            let after = snapshot(state);
            assert_undo_redo_exact(state, &before, &after, &what);
        }
    }
}

/// A closure whose two ends are one station is invalid (invariant 7), so
/// the writer cannot save one and a file that holds one anyway (written by
/// an older build or by hand) is refused at open by the corrupt-file route,
/// untouched: a load never heals (plan 6.6). (0.6.0 removed such closures on
/// open and wrote the removal at the next save.)
#[test]
fn a_file_with_a_closure_on_one_station_is_refused_not_healed() {
    let (p, s, t, ids) = survey(&["T"]);
    let opened = open(&p);
    let text = std::fs::read_to_string(&opened.path).unwrap();
    let closure = p.closures.iter().find(|c| c.id == ids[0]).unwrap();
    let (junction, far) = (st(&p, t, 0), st(&p, s, 3));
    assert_eq!((closure.a, closure.b), (junction, far));
    let record = format!("\"b\":{far},");
    assert_eq!(
        text.matches(&record).count(),
        1,
        "setup: one closure record"
    );
    let edited = text.replacen(&record, &format!("\"b\":{junction},"), 1);
    std::fs::write(&opened.path, edited.as_bytes()).unwrap();

    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: opened.path.clone(),
    });
    assert!(!r.success, "a closure on one station opens");
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .contains("cannot be opened normally"),
        "{:?}",
        r.error
    );
    assert_eq!(
        std::fs::read_to_string(&opened.path).unwrap(),
        edited,
        "the file is untouched"
    );
    assert!(state.get_project_state().is_none());
}

/// Two surveys, each placed by its own datum (A east from the frame, B
/// north from a second datum that holds it where it is), tied by two
/// closures: A:1-B:1 and A:2-B:2. Since #741 the two may merge (B's datum
/// becomes a GPS fix). Returns the project, A, B and the two closure ids.
fn two_placed_pieces() -> (Project, LineId, LineId, [ClosureId; 2]) {
    let mut p = empty("placed");
    let a = line_at(
        &mut p,
        "A",
        [0.0, 0.0],
        vec![shot(90.0, 10.0), shot(90.0, 10.0)],
    );
    let b = line_at(
        &mut p,
        "B",
        [3.0, -12.0],
        vec![shot(0.0, 10.0), shot(0.0, 10.0)],
    );
    let (a0, b0) = (st(&p, a, 0), st(&p, b, 0));
    add_datum(&mut p, a0, gps(20.5, -87.4));
    hold(&mut p, b0, gps(20.5001, -87.4));
    let folded = close(&mut p, slot(a, 1), slot(b, 1), ClosureStrategy::Batch);
    let elsewhere = close(&mut p, slot(a, 2), slot(b, 2), ClosureStrategy::Batch);
    (p, a, b, [folded, elsewhere])
}

/// The merge command, plain or in the Batch the Merge dialog's snap sends.
fn merge(state: &AppState, a: StationRef, b: StationRef, batched: bool) -> CommandResult {
    let merge = Command::MergeStations {
        station_a: a,
        station_b: b,
    };
    state.execute(if batched {
        Command::Batch {
            commands: vec![merge],
            label: None,
        }
    } else {
        merge
    })
}

/// Merging two placed pieces at the two ends of a closure (#741) makes that
/// closure tie one station to itself: the merge removes it (#738), the
/// other datum becomes the merged piece's GPS fix, and one Undo brings back
/// both the merge and the closure; Redo merges again. The Merge dialog's
/// snap sends the merge in a Batch, which does the same.
#[test]
fn merging_two_placed_pieces_removes_the_closure_it_folds() {
    for batched in [false, true] {
        let what = format!("batched {batched}");
        let (p, a, b, [folded, elsewhere]) = two_placed_pieces();
        let opened = open(&p);
        let state = &opened.state;
        let before = snapshot(state);
        let result = merge(state, slot(a, 1), slot(b, 1), batched);
        assert!(result.success, "{what}: {:?}", result.error);
        let merged = project(state);
        assert_eq!(st(&merged, a, 1), st(&merged, b, 1), "{what}: one station");
        assert_eq!(merged.datums.len(), 2, "{what}");
        assert_eq!(
            merged.placement_datums(),
            vec![Some(0)],
            "{what}: B's is a fix"
        );
        let left: Vec<ClosureId> = merged.closures.iter().map(|c| c.id).collect();
        assert_eq!(left, [elsewhere], "{what}: {folded} went");
        assert!(merged.closures.iter().all(|c| c.a != c.b), "{what}");
        let after = snapshot(state);
        assert_undo_redo_exact(state, &before, &after, &what);
    }
}

/// The other closure between the two merged pieces ties two different
/// stations (A:2 and B:2), so it is a genuine loop: it stays on its slots,
/// its ends stay apart, it spans more than one shot, and the Closures
/// Computed View solves it.
#[test]
fn a_closure_elsewhere_between_two_merged_placed_pieces_stays_a_loop() {
    let (p, a, b, [_, elsewhere]) = two_placed_pieces();
    let opened = open(&p);
    let state = &opened.state;
    let result = merge(state, slot(a, 1), slot(b, 1), false);
    assert!(result.success, "{:?}", result.error);
    let after = project(state);
    let kept: LoopClosure = *after.closures.iter().find(|c| c.id == elsewhere).unwrap();
    assert_eq!(
        (
            after.closure_end_slot(kept.a, kept.a_via),
            after.closure_end_slot(kept.b, kept.b_via)
        ),
        (Some(slot(a, 2)), Some(slot(b, 2)))
    );
    assert_ne!(kept.a, kept.b);
    assert!(after.closures_across_one_shot().is_empty());
    let report = state.get_adjustment_report().unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    let adjusted = state.get_adjusted_stations().unwrap();
    let at = |s: StationRef| {
        let station = adjusted.iter().find(|x| x.station_ref == s).unwrap();
        [station.coordinates.x, station.coordinates.y]
    };
    assert!(
        apart(at(slot(a, 2)), at(slot(b, 2))) < 1e-6,
        "the view closes the loop"
    );
}

/// The messages of a result's merged-ends warnings, in order.
fn merged_ends_messages(result: &CommandResult) -> Vec<String> {
    result
        .warnings
        .iter()
        .filter(|w| w.code == CLOSURE_REMOVED_MERGED_ENDS)
        .map(|w| w.message.clone())
        .collect()
}

/// The merge's own warning for the closure it folds (0.6.0's wording).
#[test]
fn merging_two_placed_pieces_reports_the_closure_it_folds() {
    for batched in [false, true] {
        let (p, a, b, _) = two_placed_pieces();
        let opened = open(&p);
        let result = merge(&opened.state, slot(a, 1), slot(b, 1), batched);
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            merged_ends_messages(&result),
            ["Removed loop closure A / 1 to B / 1: the merge made its two stations one point."],
            "batched {batched}"
        );
    }
}

/// The message every promotion-made removal below reports.
fn promoted_away(a: &str, b: &str) -> String {
    format!(
        "Removed loop closure {a} to {b}: another loop closure became a merge, which made its \
         two stations one point."
    )
}

/// Delete Shot & Split reaches the issue's shape: deleting the north shot
/// leaves the south one (S-B) tied on only by the two closures, so the sweep
/// promotes the first (S-B moves the 0.1 m onto the junction) and removes
/// the second, reporting it by its stations before the edit; one undo
/// brings both back.
#[test]
fn delete_shot_and_split_leaves_no_closure_between_merged_stations() {
    let (p, s, _, _) = survey(&["S", "T"]);
    let north = p.line(s).unwrap().shots[1].id;
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result =
        state.execute_delete_shot_and_split(s, north, "S-A".to_string(), "S-B".to_string());
    assert!(result.success, "{:?}", result.error);
    let after_project = project(state);
    assert!(
        after_project.closures.is_empty(),
        "{:?}",
        after_project.closures
    );
    assert!(after_project.validate().is_empty());
    assert_eq!(
        merged_ends_messages(&result),
        [promoted_away("T / 1", "S / 3")]
    );
    let after = snapshot(state);
    assert_undo_redo_exact(state, &before, &after, "delete shot and split");
}

/// Delete Shot Range & Split on the issue's survey (the north shot alone):
/// the same removal, reported the same way.
#[test]
fn delete_shot_range_and_split_reports_the_closure_it_removes() {
    let (p, s, _, _) = survey(&["S", "T"]);
    let north = p.line(s).unwrap().shots[1].id;
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result = state.execute_delete_shot_range_split(
        s,
        north,
        north,
        "S-A".to_string(),
        "S-B".to_string(),
    );
    assert!(result.success, "{:?}", result.error);
    assert!(project(state).closures.is_empty());
    assert_eq!(
        merged_ends_messages(&result),
        [promoted_away("T / 1", "S / 3")]
    );
    let r = state.undo();
    assert!(r.success, "{:?}", r.error);
    assert_same(&snapshot(state), &before, "one undo");
}

/// A branch X hangs off N through Y: N runs 20 m east, M starts on N:2 (a
/// junction), Y runs 3 m north from N:0 and X starts on Y:1, going east,
/// east and back south to 0.1 m short of N:2. X's end is tied to the
/// junction {N:2, M:0} twice: X:3 to N:2, then M:0 to X:3. Cutting X loose
/// (detaching it from Y:1, deleting Y, or deleting X's first shot) leaves
/// those two closures as X's only ties. Returns the project and N, M, X, Y.
fn branch_survey() -> (Project, [LineId; 4]) {
    let mut p = empty("branch");
    let n = line_at(
        &mut p,
        "N",
        [0.0, 0.0],
        vec![shot(90.0, 10.0), shot(90.0, 10.0)],
    );
    let (n0, n2) = (st(&p, n, 0), st(&p, n, 2));
    let m = line_from(&mut p, "M", n2, vec![shot(0.0, 5.0)]);
    let y = line_from(&mut p, "Y", n0, vec![shot(0.0, 3.0)]);
    let y1 = st(&p, y, 1);
    let x = line_from(
        &mut p,
        "X",
        y1,
        vec![shot(90.0, 10.0), shot(90.0, 10.0), shot(180.0, 2.9)],
    );
    close(&mut p, slot(x, 3), slot(n, 2), ClosureStrategy::Batch);
    plant(&mut p, slot(m, 0), slot(x, 3));
    (p, [n, m, x, y])
}

/// Every other command whose sweep can remove one (Unmerge, Delete Line,
/// Delete Shot and Delete Last Shot & Line) reports the closure it removed
/// by the stations it had BEFORE the edit (M:0 is the junction, named 2 on
/// every line; the first-shot delete renumbers X:3 to X:2, the name stays
/// 3), and one undo brings both closures back.
#[test]
fn every_command_that_removes_one_reports_it_by_its_stations_before_the_edit() {
    for what in ["unmerge", "delete line", "delete shot", "delete last shot"] {
        let (p, [_, _, x, y]) = branch_survey();
        let first_of_x = p.line(x).unwrap().shots[0].id;
        let only_of_y = p.line(y).unwrap().shots[0].id;
        let opened = open(&p);
        let state = &opened.state;
        let before = snapshot(state);
        let result = match what {
            "unmerge" => state.execute(Command::UnmergeStationPair {
                station_a: slot(y, 1),
                station_b: slot(x, 0),
            }),
            "delete line" => state.execute(Command::DeleteLine { line_id: y }),
            "delete shot" => state.execute(Command::DeleteShot {
                line_id: x,
                shot_id: first_of_x,
            }),
            _ => state.execute_delete_last_shot_and_line(y, only_of_y),
        };
        assert!(result.success, "{what}: {:?}", result.error);
        assert!(
            project(state).closures.is_empty(),
            "{what}: one closure became the merge, the other went"
        );
        assert_eq!(
            merged_ends_messages(&result),
            [promoted_away("M / 2", "X / 3")],
            "{what}"
        );
        let r = state.undo();
        assert!(r.success, "{what}: {:?}", r.error);
        assert_same(&snapshot(state), &before, &format!("{what}: one undo"));
    }
}

/// Remove Zero Datum too: two placed surveys, B tied to the junction {A:1,
/// C:0} twice; once B's datum goes, the first closure becomes the merge and
/// the second is removed and reported.
#[test]
fn removing_a_zero_datum_reports_the_closure_its_sweep_removes() {
    let mut p = empty("datums");
    let a = line_at(&mut p, "A", [0.0, 0.0], vec![shot(90.0, 10.0)]);
    let a1 = st(&p, a, 1);
    let c = line_from(&mut p, "C", a1, vec![shot(0.0, 3.0)]);
    let b = line_at(&mut p, "B", [0.0, -20.0], vec![shot(0.0, 10.0)]);
    let (a0, b0) = (st(&p, a, 0), st(&p, b, 0));
    add_datum(&mut p, a0, gps(20.5, -87.4));
    add_datum(&mut p, b0, gps(20.5001, -87.4));
    close(&mut p, slot(b, 1), slot(a, 1), ClosureStrategy::Batch);
    plant(&mut p, slot(c, 0), slot(b, 1));
    let opened = open(&p);
    let state = &opened.state;
    let before = snapshot(state);

    let result = state.execute(Command::RemoveZeroDatum {
        station_ref: slot(b, 0),
    });
    assert!(result.success, "{:?}", result.error);
    assert!(project(state).closures.is_empty());
    assert_eq!(
        merged_ends_messages(&result),
        [promoted_away("C / 1", "B / 1")]
    );
    let r = state.undo();
    assert!(r.success, "{:?}", r.error);
    assert_same(&snapshot(state), &before, "one undo");
}
