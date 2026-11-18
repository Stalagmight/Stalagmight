//! [#738] A loop closure left between merged stations, at the app-state level:
//! Split Line Here's preview names the closure the split removes (apart
//! from one that already was between merged stations), the split removes
//! exactly that one inside its own undo unit, every other command whose sweep
//! removes one reports it as a warning named by its stations before the edit,
//! and a project file that already holds one opens without it and loses it on
//! disk at the next save.
//!
//! All data is synthetic: projects built in memory, and the one file written
//! goes to a temporary directory.

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    ClosurePair, ClosureStrategy, GpsCoordinates, LineData, LineId, LoopClosure, Project, ShotData,
    SplitAt, SplitLineOptions, SplitPreview, SplitSide, StationRef, CLOSURE_REMOVED_MERGED_ENDS,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, name: &str, shots: Vec<ShotData>) -> LineId {
    project
        .add_line(LineData {
            name: Some(name.to_string()),
            shots,
            ..Default::default()
        })
        .id
}

fn st(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

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

/// The issue's survey. S runs 10 m east, 10 m north, then 9.9 m back south,
/// so S:3 ends 0.1 m short of S:1; T (and, with `with_u`, U) starts merged at
/// S:1. Then one closure per entry of `ties`, in that order, from S:3 to the
/// one point {S:1, T:0, U:0}: `"S"` ties it to S:1, `"T"` to T:0 and `"U"` to
/// U:0. Returns the project, S, T and the closure ids in `ties` order.
fn survey(ties: &[&str]) -> (Project, LineId, LineId, Vec<u64>) {
    let mut p = Project::new("738".to_string(), None);
    let s = add(
        &mut p,
        "S",
        vec![shot(90.0, 10.0), shot(0.0, 10.0), shot(180.0, 9.9)],
    );
    let t = add(&mut p, "T", vec![shot(90.0, 5.0)]);
    p.merge_stations(st(s, 1), st(t, 0)).unwrap();
    let u = ties.contains(&"U").then(|| {
        let u = add(&mut p, "U", vec![shot(270.0, 5.0)]);
        p.merge_stations(st(s, 1), st(u, 0)).unwrap();
        u
    });
    let ids = ties
        .iter()
        .map(|tie| {
            let (a, b) = match *tie {
                "S" => (st(s, 3), st(s, 1)),
                "T" => (st(t, 0), st(s, 3)),
                _ => (st(s, 3), st(u.unwrap(), 0)),
            };
            p.create_loop_closure(a, b, ClosureStrategy::Batch)
                .unwrap()
                .id
        })
        .collect();
    (p, s, t, ids)
}

fn state_with(project: Project) -> AppState {
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    state
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn full_snapshot(state: &AppState) -> String {
    AppState::serialize_project_snapshot(&project(state)).unwrap()
}

/// Every closure whose two stations are one point (the same station, or one
/// merge group), by id - the state #738 is about. Asked of the BFS
/// `are_stations_merged`, not of the sweep's own union-find.
fn closures_between_merged_stations(project: &Project) -> Vec<u64> {
    project
        .loop_closures
        .iter()
        .filter(|c| {
            match (
                project.resolve_key(&c.station_a),
                project.resolve_key(&c.station_b),
            ) {
                (Some(a), Some(b)) => project.are_stations_merged(a, b),
                _ => false,
            }
        })
        .map(|c| c.id)
        .collect()
}

/// The merges in `after` that `before` did not have, as resolved station
/// pairs (sorted within each pair).
fn new_merges(before: &Project, after: &Project) -> Vec<[StationRef; 2]> {
    let count = before.station_merges.len();
    after.station_merges[count..]
        .iter()
        .map(|m| {
            let mut pair = [
                after.resolve_key(&m.station_a).unwrap(),
                after.resolve_key(&m.station_b).unwrap(),
            ];
            pair.sort();
            pair
        })
        .collect()
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

fn pair(closure_id: u64, a: StationRef, b: StationRef) -> ClosurePair {
    ClosurePair {
        closure_id,
        station_a: a,
        station_b: b,
    }
}

/// Preview the split at `at`, run it, and check what both say against
/// `promoted` (the one closure that becomes a merge, by its post-split ends)
/// and `removed` (what the preview must name). Then one undo puts back the
/// project before - both closures included - and one redo the one after.
fn split_and_check(
    state: &AppState,
    s: LineId,
    at: SplitAt,
    promoted_ends: impl Fn(LineId) -> [StationRef; 2],
    removed: &[ClosurePair],
) -> SplitPreview {
    let before_project = project(state);
    let before = full_snapshot(state);
    let preview = state.split_line_preview(s, at, &options()).unwrap();
    assert_eq!(
        full_snapshot(state),
        before,
        "the preview changed the project"
    );
    assert_eq!(preview.closures_to_merges, 1);
    assert_eq!(preview.removed_closures, removed, "the preview's list");
    assert_eq!(preview.removed_already_one_point, []);

    let result = state.execute_split_line(s, at, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.line_id.unwrap();
    let after_project = project(state);
    let after = full_snapshot(state);
    assert!(
        after_project.loop_closures.is_empty(),
        "one closure became the merge, the preview named the rest: {:?}",
        after_project.loop_closures
    );
    let mut promoted = promoted_ends(second);
    promoted.sort();
    assert_eq!(new_merges(&before_project, &after_project), [promoted]);
    assert_eq!(after_project.stats().loop_count, 0);
    assert!(after_project.check_structural_integrity().is_empty());

    assert!(state.undo().success);
    assert_eq!(
        full_snapshot(state),
        before,
        "one undo restores every closure"
    );
    assert!(state.redo().success);
    assert_eq!(full_snapshot(state), after, "one redo removes them again");
    preview
}

/// The issue's steps: S:3 tied to S:1, then T:0 tied to S:3, split at S:2.
/// The first closure becomes the merge and moves S-B onto S:1; the preview
/// names the second, by the stations it was created with (T:0 and S:3, not
/// S-B's end), and the split removes exactly it.
#[test]
fn the_split_removes_the_second_closure_to_the_point_and_its_preview_says_so() {
    let (p, s, t, ids) = survey(&["S", "T"]);
    let state = state_with(p);
    let preview = split_and_check(
        &state,
        s,
        SplitAt::Station { station_index: 2 },
        |b| [st(b, 1), st(s, 1)],
        &[pair(ids[1], st(t, 0), st(s, 3))],
    );
    assert_eq!(preview.moved_side, Some(SplitSide::B));
    assert!((preview.moved_m - 0.1).abs() < 1e-6, "{}", preview.moved_m);
}

/// The same two closures made the other way round: the first in the list is
/// still the one promoted, so now it is T:0 to S:3 that becomes the merge.
/// Its first station's group (S-A and T) is the side that moves, and the
/// closure S:3 to S:1 is the one removed. Deterministic: the same project
/// gives the same answer every time.
#[test]
fn the_first_closure_in_the_list_is_the_one_promoted() {
    let (p, s, t, ids) = survey(&["T", "S"]);
    let state = state_with(p.clone());
    let preview = split_and_check(
        &state,
        s,
        SplitAt::Station { station_index: 2 },
        |b| [st(t, 0), st(b, 1)],
        &[pair(ids[1], st(s, 3), st(s, 1))],
    );
    assert_eq!(preview.moved_side, Some(SplitSide::A));
    assert!((preview.moved_m - 0.1).abs() < 1e-6, "{}", preview.moved_m);

    let again = state_with(p);
    assert_eq!(
        again
            .split_line_preview(s, SplitAt::Station { station_index: 2 }, &options())
            .unwrap(),
        preview
    );
}

/// Three closures from S:3 to the one point (to S:1, T:0 and U:0): one
/// becomes the merge and both others go, in list order.
#[test]
fn three_closures_to_one_point_leave_one_merge_and_no_closure() {
    let (p, s, t, ids) = survey(&["S", "T", "U"]);
    let u = p
        .lines
        .iter()
        .find(|x| x.name.as_deref() == Some("U"))
        .unwrap()
        .id;
    let state = state_with(p);
    split_and_check(
        &state,
        s,
        SplitAt::Station { station_index: 2 },
        |b| [st(b, 1), st(s, 1)],
        &[
            pair(ids[1], st(t, 0), st(s, 3)),
            pair(ids[2], st(s, 3), st(u, 0)),
        ],
    );
}

/// A cut along the north shot inserts a station first, which renumbers S:2
/// and S:3. The preview still names the removed closure by the stations the
/// user created it with: S:3, not S:4 or S-B's end.
#[test]
fn a_cut_along_a_shot_names_the_removed_closure_by_its_stations_before_the_cut() {
    let (p, s, t, ids) = survey(&["S", "T"]);
    let north = p.get_line(s).unwrap().shots[1].id;
    let state = state_with(p);
    split_and_check(
        &state,
        s,
        SplitAt::Shot {
            shot_id: north,
            distance: 4.0,
        },
        |b| [st(b, 2), st(s, 1)],
        &[pair(ids[1], st(t, 0), st(s, 3))],
    );
}

/// A split that leaves no closure between merged stations names none: one
/// closure across the cut becomes the merge and nothing else changes.
#[test]
fn a_split_with_one_closure_across_the_cut_removes_none() {
    let (p, s, _, _) = survey(&["S"]);
    let state = state_with(p);
    split_and_check(
        &state,
        s,
        SplitAt::Station { station_index: 2 },
        |b| [st(b, 1), st(s, 1)],
        &[],
    );
}

/// Delete Shot & Split reaches the same shape - deleting the north shot
/// leaves the south one tied on only by the two closures - and runs the same
/// sweep: no closure is left between merged stations, and one undo brings
/// both back.
#[test]
fn delete_shot_and_split_leaves_no_closure_between_merged_stations() {
    let (p, s, _, _) = survey(&["S", "T"]);
    let north = p.get_line(s).unwrap().shots[1].id;
    let state = state_with(p);
    let before_project = project(&state);
    let before = full_snapshot(&state);

    let result =
        state.execute_delete_shot_and_split(s, north, "S-A".to_string(), "S-B".to_string());
    assert!(result.success, "{:?}", result.error);
    let after_project = project(&state);
    assert_eq!(
        closures_between_merged_stations(&after_project),
        Vec::<u64>::new()
    );
    assert!(
        after_project.loop_closures.is_empty(),
        "{:?}",
        after_project.loop_closures
    );
    assert_eq!(new_merges(&before_project, &after_project).len(), 1);
    let after = full_snapshot(&state);

    assert_eq!(
        merged_ends_messages(&result),
        [
            "Removed loop closure T / 0 to S / 3: another loop closure became a merge, which made its \
          two stations one point."
        ]
    );

    assert!(state.undo().success);
    assert_eq!(
        full_snapshot(&state),
        before,
        "one undo restores both closures"
    );
    assert_eq!(project(&state).loop_closures.len(), 2);
    assert!(state.redo().success);
    assert_eq!(full_snapshot(&state), after);
}

/// Plant a closure directly, as a file written before #738 can carry it
/// (creation refuses one between merged stations).
fn plant(project: &mut Project, a: StationRef, b: StationRef) -> u64 {
    let id = project.next_closure_id.max(1);
    project.next_closure_id = id + 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a: project.key_for_ref(a).unwrap(),
        station_b: project.key_for_ref(b).unwrap(),
        path_length: 20.0,
        misclosure_distance: 0.1,
        error_percentage: 0.5,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    id
}

/// A project file that already holds closures between merged stations - one
/// on a directly merged pair, one through a chain of two merges - opens
/// without them, and a closure that closes a real loop stays. Saving and
/// opening again changes nothing more.
#[test]
fn a_file_with_closures_between_merged_stations_opens_without_them() {
    let mut p = Project::new("738 file".to_string(), None);
    // R closes a real loop on itself: east, north, west, then south to 0.2 m
    // short of its start.
    let r = add(
        &mut p,
        "R",
        vec![
            shot(90.0, 10.0),
            shot(0.0, 10.0),
            shot(270.0, 10.0),
            shot(180.0, 9.8),
        ],
    );
    let t = add(&mut p, "T", vec![shot(90.0, 5.0)]);
    let u = add(&mut p, "U", vec![shot(45.0, 5.0)]);
    p.merge_stations(st(r, 1), st(t, 0)).unwrap();
    p.merge_stations(st(t, 0), st(u, 0)).unwrap();
    let real = p
        .create_loop_closure(st(r, 4), st(r, 0), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let direct = plant(&mut p, st(t, 0), st(r, 1));
    let chained = plant(&mut p, st(u, 0), st(r, 1));
    assert_eq!(closures_between_merged_stations(&p), [direct, chained]);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("between-merged.stalx");
    crate::persistence::Database::open(&path)
        .unwrap()
        .save_project(&p)
        .unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success, "open");
    let opened = project(&state);
    assert_eq!(
        opened
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [real],
        "the real loop stays, the two between merged stations go"
    );
    assert_eq!(opened.station_merges.len(), 2, "no merge added or lost");
    assert_eq!(opened.stats().loop_count, 1);

    // The open changed memory only: the file still holds all three until a
    // save. An explicit Save writes it, though the open made no edit.
    let on_disk = |path: &std::path::Path| -> Vec<u64> {
        crate::persistence::Database::open(path)
            .unwrap()
            .load_project()
            .unwrap()
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect()
    };
    assert_eq!(on_disk(&path), [real, direct, chained]);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert_eq!(
        on_disk(&path),
        [real],
        "the save wrote the open's removals (read without the open's sweep)"
    );
    assert!(state.open_project(path).success, "reopen");
    assert_eq!(
        project(&state)
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [real]
    );
}

/// The issue's survey with one more closure that ALREADY ties a point to
/// itself (planted, as an undo from a pre-#738 history can restore one): the
/// split's sweep takes it too, and the preview names it apart, so the dialog
/// does not count it among the closures that tie the half.
#[test]
fn the_preview_names_a_closure_already_between_merged_stations_apart() {
    let (mut p, s, t, ids) = survey(&["S", "T"]);
    let w = add(&mut p, "W", vec![shot(0.0, 4.0)]);
    p.merge_stations(st(t, 1), st(w, 0)).unwrap();
    let stale = plant(&mut p, st(t, 1), st(w, 0));
    let state = state_with(p);
    let before = full_snapshot(&state);
    let at = SplitAt::Station { station_index: 2 };

    let preview = state.split_line_preview(s, at, &options()).unwrap();
    assert_eq!(preview.removed_closures, [pair(ids[1], st(t, 0), st(s, 3))]);
    assert_eq!(
        preview.removed_already_one_point,
        [pair(stale, st(t, 1), st(w, 0))]
    );

    let result = state.execute_split_line(s, at, options());
    assert!(result.success, "{:?}", result.error);
    assert!(project(&state).loop_closures.is_empty());
    assert!(state.undo().success);
    assert_eq!(full_snapshot(&state), before, "one undo restores all three");
}

/// A file can repeat a closure id (very old, or written by hand). The preview
/// still names the closure the split removes, not another with its id: the
/// second closure takes the first one's id here, and it is the second that
/// goes.
#[test]
fn the_preview_names_the_removed_closure_when_ids_repeat() {
    let (mut p, s, t, ids) = survey(&["S", "T"]);
    p.loop_closures[1].id = ids[0];
    let state = state_with(p);
    let preview = state
        .split_line_preview(s, SplitAt::Station { station_index: 2 }, &options())
        .unwrap();
    assert_eq!(preview.removed_closures, [pair(ids[0], st(t, 0), st(s, 3))]);
}

/// Delete Shot Range & Split on the issue's survey (the north shot alone):
/// the same removal, reported the same way.
#[test]
fn delete_shot_range_and_split_reports_the_closure_it_removes() {
    let (p, s, _, _) = survey(&["S", "T"]);
    let north = p.get_line(s).unwrap().shots[1].id;
    let state = state_with(p);
    let before = full_snapshot(&state);

    let result = state.execute_delete_shot_range_split(
        s,
        north,
        north,
        "S-A".to_string(),
        "S-B".to_string(),
    );
    assert!(result.success, "{:?}", result.error);
    assert!(project(&state).loop_closures.is_empty());
    assert_eq!(
        merged_ends_messages(&result),
        [
            "Removed loop closure T / 0 to S / 3: another loop closure became a merge, which made its \
          two stations one point."
        ]
    );
    assert!(state.undo().success);
    assert_eq!(full_snapshot(&state), before);
}

/// A branch X hangs off N through Y: N runs 20 m east, M starts merged at N:2,
/// Y runs 3 m north from N:0 and X starts at Y:1, going east, east and back
/// south to 0.1 m short of N:2. X's end is tied to the one point {N:2, M:0}
/// twice: X:3 to N:2, then M:0 to X:3. Cutting X loose - unmerging it from Y,
/// deleting Y, or deleting X's first shot - leaves those two closures as X's
/// only ties. Returns the project and N, M, X, Y.
fn branch_survey() -> (Project, [LineId; 4]) {
    let mut p = Project::new("branch".to_string(), None);
    let n = add(&mut p, "N", vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let m = add(&mut p, "M", vec![shot(0.0, 5.0)]);
    let x = add(
        &mut p,
        "X",
        vec![shot(90.0, 10.0), shot(90.0, 10.0), shot(180.0, 2.9)],
    );
    let y = add(&mut p, "Y", vec![shot(0.0, 3.0)]);
    p.merge_stations(st(n, 2), st(m, 0)).unwrap();
    p.merge_stations(st(n, 0), st(y, 0)).unwrap();
    p.merge_stations(st(y, 1), st(x, 0)).unwrap();
    p.create_loop_closure(st(x, 3), st(n, 2), ClosureStrategy::Batch)
        .unwrap();
    p.create_loop_closure(st(m, 0), st(x, 3), ClosureStrategy::Batch)
        .unwrap();
    (p, [n, m, x, y])
}

/// Every other command whose sweep can remove one - through `execute()`'s
/// epilogue (Unmerge, Delete Line), the single Delete Shot (whose own shot
/// removal sweeps), and Delete Last Shot & Line - reports the closure it
/// removed by the stations it had BEFORE the edit (the first-shot delete
/// renumbers X:3 to X:2; the warning still says 2:3), and one undo brings both
/// closures back.
#[test]
fn every_command_that_removes_one_reports_it_by_its_stations_before_the_edit() {
    let expected = [
        "Removed loop closure M / 0 to X / 3: another loop closure became a merge, \
                     which made its two stations one point.",
    ];
    for what in ["unmerge", "delete line", "delete shot", "delete last shot"] {
        let (p, [_, _, x, y]) = branch_survey();
        let first_of_x = p.get_line(x).unwrap().shots[0].id;
        let only_of_y = p.get_line(y).unwrap().shots[0].id;
        let state = state_with(p);
        let before = full_snapshot(&state);
        let result = match what {
            "unmerge" => state.execute(Command::UnmergeStationPair {
                station_a: st(y, 1),
                station_b: st(x, 0),
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
            project(&state).loop_closures.is_empty(),
            "{what}: one closure became the merge, the other went"
        );
        assert_eq!(merged_ends_messages(&result), expected, "{what}");
        assert!(state.undo().success, "{what}");
        assert_eq!(full_snapshot(&state), before, "{what}: one undo");
    }
}

/// Remove Zero Datum too: two anchored surveys, B tied to A's point {A:1, C:0}
/// twice; once B's datum goes, the first closure becomes the merge and the
/// second is removed and reported.
#[test]
fn removing_a_zero_datum_reports_the_closure_its_sweep_removes() {
    let mut p = Project::new("datums".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 10.0)]);
    let c = add(&mut p, "C", vec![shot(0.0, 3.0)]);
    let b = add(&mut p, "B", vec![shot(0.0, 10.0)]);
    p.merge_stations(st(a, 1), st(c, 0)).unwrap();
    let gps = |lat| GpsCoordinates::new(lat, -87.4, 0.0).unwrap();
    p.set_zero_datum(st(a, 0), gps(20.5)).unwrap();
    p.set_zero_datum(st(b, 0), gps(20.5001)).unwrap();
    p.create_loop_closure(st(b, 1), st(a, 1), ClosureStrategy::Batch)
        .unwrap();
    p.create_loop_closure(st(c, 0), st(b, 1), ClosureStrategy::Batch)
        .unwrap();
    let state = state_with(p);
    let before = full_snapshot(&state);

    let result = state.execute(Command::RemoveZeroDatum {
        station_ref: st(b, 0),
    });
    assert!(result.success, "{:?}", result.error);
    assert!(project(&state).loop_closures.is_empty());
    assert_eq!(
        merged_ends_messages(&result),
        [
            "Removed loop closure C / 0 to B / 1: another loop closure became a merge, which made its \
          two stations one point."
        ]
    );
    assert!(state.undo().success);
    assert_eq!(full_snapshot(&state), before);
}

/// A closure between merged stations that an edit did not cause (planted, as
/// an undo from a pre-#738 history can restore one) is reported as such when
/// the edit's sweep takes it.
#[test]
fn a_closure_that_already_was_between_merged_stations_is_reported_as_such() {
    let (mut p, [n, m, x, y]) = branch_survey();
    p.loop_closures.clear();
    plant(&mut p, st(m, 0), st(n, 2));
    let state = state_with(p);
    let result = state.execute(Command::UnmergeStationPair {
        station_a: st(y, 1),
        station_b: st(x, 0),
    });
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        merged_ends_messages(&result),
        ["Removed loop closure M / 0 to N / 2: its two stations were already one point."]
    );
}

/// Two anchored surveys, each placed by its own datum, tied by two closures:
/// A:1-B:1 and A:2-B:2. Since #741 the two may merge (B's datum becomes a GPS
/// fix). Returns the project, A, B and the two closure ids.
fn two_placed_pieces() -> (Project, LineId, LineId, [u64; 2]) {
    let mut p = Project::new("placed".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let b = add(&mut p, "B", vec![shot(0.0, 10.0), shot(0.0, 10.0)]);
    let gps = |lat| GpsCoordinates::new(lat, -87.4, 0.0).unwrap();
    p.set_zero_datum(st(a, 0), gps(20.5)).unwrap();
    p.set_zero_datum(st(b, 0), gps(20.5001)).unwrap();
    let folded = p
        .create_loop_closure(st(a, 1), st(b, 1), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let elsewhere = p
        .create_loop_closure(st(a, 2), st(b, 2), ClosureStrategy::Batch)
        .unwrap()
        .id;
    (p, a, b, [folded, elsewhere])
}

/// Merging two placed pieces at the two ends of a closure (#741) makes that
/// closure tie one point to itself: the merge command's own sweep removes it
/// and reports it (#738), and one Undo brings back both the merge and the
/// closure. The Merge dialog's snap-to-merge sends the merge in a Batch, which
/// reports the same way.
#[test]
fn merging_two_placed_pieces_removes_and_reports_the_closure_it_folds() {
    for batched in [false, true] {
        let (p, a, b, [folded, elsewhere]) = two_placed_pieces();
        let state = state_with(p);
        let before = full_snapshot(&state);
        let merge = Command::MergeStations {
            station_a: st(a, 1),
            station_b: st(b, 1),
        };
        let result = state.execute(if batched {
            Command::Batch {
                commands: vec![merge],
            }
        } else {
            merge
        });
        assert!(result.success, "batched {batched}: {:?}", result.error);
        let after = project(&state);
        assert!(after.are_stations_merged(st(a, 1), st(b, 1)));
        assert_eq!(after.gps_fix_datums().len(), 1, "batched {batched}");
        let left: Vec<u64> = after.loop_closures.iter().map(|c| c.id).collect();
        assert_eq!(left, [elsewhere], "batched {batched}: {folded} went");
        assert!(closures_between_merged_stations(&after).is_empty());
        assert_eq!(
            merged_ends_messages(&result),
            ["Removed loop closure A / 1 to B / 1: the merge made its two stations one point."],
            "batched {batched}"
        );

        assert!(state.undo().success, "batched {batched}");
        assert_eq!(full_snapshot(&state), before, "batched {batched}: one undo");
        assert!(state.redo().success, "batched {batched}");
        assert_eq!(
            full_snapshot(&state),
            AppState::serialize_project_snapshot(&after).unwrap(),
            "batched {batched}: redo"
        );
    }
}

/// The other closure between the two merged pieces ties two different points
/// (A:2 and B:2), so it is a genuine loop: it stays, its ends stay apart, and
/// the Closures Computed View solves it.
#[test]
fn a_closure_elsewhere_between_two_merged_placed_pieces_stays_a_loop() {
    let (p, a, b, [_, elsewhere]) = two_placed_pieces();
    let state = state_with(p);
    let result = state.execute(Command::MergeStations {
        station_a: st(a, 1),
        station_b: st(b, 1),
    });
    assert!(result.success, "{:?}", result.error);
    let after = project(&state);
    assert!(
        after.loop_closures.iter().any(|c| c.id == elsewhere),
        "the closure elsewhere survives the merge"
    );
    let kept = after
        .loop_closures
        .iter()
        .find(|c| c.id == elsewhere)
        .unwrap();
    let (x, y) = (
        after.resolve_key(&kept.station_a).unwrap(),
        after.resolve_key(&kept.station_b).unwrap(),
    );
    assert_eq!((x, y), (st(a, 2), st(b, 2)));
    assert!(!after.are_stations_merged(x, y));
    assert!(after.closures_across_one_shot().is_empty());
    let solved =
        after.compute_adjusted_stations(&crate::config::ClosureAdjustmentConfig::default());
    assert!(solved.failures.is_empty(), "{:?}", solved.failures);
}
