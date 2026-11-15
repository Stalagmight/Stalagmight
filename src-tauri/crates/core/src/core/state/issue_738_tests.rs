//! [#738] A loop closure left between merged stations, at the app-state level:
//! Split Section Here's preview names the closure the split removes, the split
//! removes exactly that one inside its own undo unit, Delete Shot & Split does
//! the same, and a project file that already holds one opens without it.
//!
//! All data is synthetic: projects built in memory, and the one file written
//! goes to a temporary directory.

use super::AppState;
use crate::models::{
    ClosurePair, ClosureStrategy, LoopClosure, Project, SectionData, SectionId, ShotData, SplitAt,
    SplitPreview, SplitSectionOptions, SplitSide, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, name: &str, shots: Vec<ShotData>) -> SectionId {
    project
        .add_section(SectionData {
            name: Some(name.to_string()),
            shots,
            ..Default::default()
        })
        .id
}

fn st(section: SectionId, index: u32) -> StationRef {
    StationRef::new(section, index)
}

fn options() -> SplitSectionOptions {
    SplitSectionOptions {
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
fn survey(ties: &[&str]) -> (Project, SectionId, SectionId, Vec<u64>) {
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
    s: SectionId,
    at: SplitAt,
    promoted_ends: impl Fn(SectionId) -> [StationRef; 2],
    removed: &[ClosurePair],
) -> SplitPreview {
    let before_project = project(state);
    let before = full_snapshot(state);
    let preview = state.split_section_preview(s, at, &options()).unwrap();
    assert_eq!(
        full_snapshot(state),
        before,
        "the preview changed the project"
    );
    assert_eq!(preview.closures_to_merges, 1);
    assert_eq!(preview.removed_closures, removed, "the preview's list");

    let result = state.execute_split_section(s, at, options());
    assert!(result.success, "{:?}", result.error);
    let second = result.section_id.unwrap();
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
            .split_section_preview(s, SplitAt::Station { station_index: 2 }, &options())
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
        .sections
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
    let north = p.get_section(s).unwrap().shots[1].id;
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
    let north = p.get_section(s).unwrap().shots[1].id;
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

    assert!(
        state
            .execute(crate::core::Command::SaveProject { path: None })
            .success
    );
    assert!(state.execute(crate::core::Command::CloseProject).success);
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
