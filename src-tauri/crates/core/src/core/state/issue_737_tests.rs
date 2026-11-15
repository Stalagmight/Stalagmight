//! [#737] Loop closures across one shot, at the app-state level: the warning
//! and its identity, the adjustment report and the failure warning read from
//! the memoized solve, the closure dialog's guard, and the delete-with-shift
//! that removes a closure it leaves across one shot - inside the same undo
//! unit as the delete.
//!
//! All data is synthetic: projects built in memory, no fixture read or written.

use std::collections::HashSet;

use super::command_shot::closure_removed_message;
use super::AppState;
use crate::core::{Command, WarningSeverity};
use crate::models::{
    ClosureStrategy, GpsCoordinates, LoopClosure, Project, SectionData, SectionId, ShotData,
    ShotId, StationRef, CLOSURE_ACROSS_ONE_SHOT, CLOSURE_ADJUSTMENT_FAILED,
    CLOSURE_REMOVED_ACROSS_ONE_SHOT,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, shots: Vec<ShotData>) -> SectionId {
    project
        .add_section(SectionData {
            shots,
            ..Default::default()
        })
        .id
}

fn st(section: SectionId, index: u32) -> StationRef {
    StationRef::new(section, index)
}

fn state_with(project: Project) -> AppState {
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    state
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn shot_id(state: &AppState, section: SectionId, position: usize) -> ShotId {
    project(state).get_section(section).unwrap().shots[position].id
}

/// Plant a closure directly, as a file written before the creation-time
/// refusal can carry it.
fn plant(project: &mut Project, a: StationRef, b: StationRef) -> u64 {
    let (station_a, station_b) = (
        project.key_for_ref(a).unwrap(),
        project.key_for_ref(b).unwrap(),
    );
    let id = project
        .loop_closures
        .iter()
        .map(|lc| lc.id + 1)
        .max()
        .unwrap_or(1)
        .max(project.next_closure_id);
    project.next_closure_id = id + 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a,
        station_b,
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    id
}

/// The issue's loop: A two shots east, B north/east/south, merged at their
/// starts, closed B:3-A:2.
fn issue_loop() -> (Project, SectionId, SectionId) {
    let mut p = Project::new("737".to_string(), None);
    let a = add(&mut p, vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let b = add(
        &mut p,
        vec![shot(0.0, 10.0), shot(90.0, 20.5), shot(180.0, 10.0)],
    );
    p.merge_stations(st(a, 0), st(b, 0)).unwrap();
    p.create_loop_closure(st(b, 3), st(a, 2), ClosureStrategy::Batch)
        .unwrap();
    (p, a, b)
}

/// A straight section of `n` shots with varied bearings (so a gap-close moves
/// something), and nothing else.
fn chain(n: usize) -> (Project, SectionId) {
    let mut p = Project::new("chain".to_string(), None);
    let shots = (0..n)
        .map(|i| shot(30.0 * i as f64, 4.0 + i as f64))
        .collect();
    let a = add(&mut p, shots);
    (p, a)
}

fn closure_endpoints(p: &Project, id: u64) -> Option<(StationRef, StationRef)> {
    let lc = p.loop_closures.iter().find(|lc| lc.id == id)?;
    Some((p.resolve_key(&lc.station_a)?, p.resolve_key(&lc.station_b)?))
}

// ---- the warning ---------------------------------------------------------------

#[test]
fn a_closure_across_one_shot_raises_one_unignorable_error_on_its_shot() {
    let (mut p, a, _) = issue_loop();
    let id = plant(&mut p, st(a, 1), st(a, 2));
    let state = state_with(p);
    let warnings = state.compute_project_warnings(&project(&state), None);
    let ours: Vec<_> = warnings
        .iter()
        .filter(|w| w.code == CLOSURE_ACROSS_ONE_SHOT)
        .collect();
    assert_eq!(ours.len(), 1, "{warnings:?}");
    let w = ours[0];
    assert_eq!(w.closure_id, Some(id));
    assert_eq!((w.section_id, w.shot_index), (a, 1));
    assert_eq!(w.shot_id, shot_id(&state, a, 1));
    assert_eq!(w.severity, WarningSeverity::Error);
    assert!(!w.is_resolved);
    assert!(!crate::models::warning_is_ignorable(
        CLOSURE_ACROSS_ONE_SHOT
    ));
    assert_eq!(
        w.message,
        format!(
            "Shot {a}:2 has both ends joined by loop closure {a}:1 to {a}:2, which would squeeze \
             the shot to zero length, so the Closures Computed View skips that closure. Remove \
             the closure, or close the loop at two stations that are not the ends of a single \
             shot."
        )
    );
    // The DetailPanel pill parses the leading shot reference.
    assert!(w.message.starts_with(&format!("Shot {a}:2 ")));
}

#[test]
fn two_closures_across_the_same_shot_are_two_warnings_told_apart_by_closure_id() {
    let (mut p, a, _) = issue_loop();
    let c = add(&mut p, vec![shot(0.0, 5.0)]);
    p.merge_stations(st(c, 0), st(a, 1)).unwrap();
    let first = plant(&mut p, st(a, 1), st(a, 2));
    let second = plant(&mut p, st(c, 0), st(a, 2));
    let state = state_with(p);
    let warnings = state.compute_project_warnings(&project(&state), None);
    let ids: HashSet<Option<u64>> = warnings
        .iter()
        .filter(|w| w.code == CLOSURE_ACROSS_ONE_SHOT)
        .map(|w| {
            assert_eq!((w.section_id, w.shot_index), (a, 1));
            w.closure_id
        })
        .collect();
    assert_eq!(ids, HashSet::from([Some(first), Some(second)]));
    // Every other warning carries no closure id.
    assert!(warnings
        .iter()
        .filter(|w| w.code != CLOSURE_ACROSS_ONE_SHOT)
        .all(|w| w.closure_id.is_none()));
}

#[test]
fn removing_the_closure_clears_the_warning() {
    let (mut p, a, _) = issue_loop();
    let id = plant(&mut p, st(a, 1), st(a, 2));
    let state = state_with(p);
    let removed = state.execute(Command::RemoveLoopClosure { closure_id: id });
    assert!(removed.success, "{:?}", removed.error);
    let warnings = state.compute_project_warnings(&project(&state), None);
    assert!(warnings.iter().all(|w| w.code != CLOSURE_ACROSS_ONE_SHOT));
    // Undo brings the closure and its warning back.
    assert!(state.undo().success);
    let warnings = state.compute_project_warnings(&project(&state), None);
    assert!(warnings.iter().any(|w| w.code == CLOSURE_ACROSS_ONE_SHOT));
}

// ---- the closure dialog's guard -------------------------------------------------

#[test]
fn the_connectivity_check_carries_the_backend_refusal() {
    let (p, a, b) = issue_loop();
    let state = state_with(p);
    let across = state.check_merge_connectivity(st(a, 1), st(a, 2)).unwrap();
    assert!(across.connected && !across.can_merge);
    let refusal = across.closure_refusal.expect("a refusal for one shot");
    assert!(refusal.starts_with(&format!(
        "This closure would join the two ends of one shot ({a}:1 to {a}:2) and squeeze that \
         shot to zero length."
    )));
    // The very words the create command answers with.
    let created = state.execute(Command::CreateLoopClosure {
        station_a: st(a, 1),
        station_b: st(a, 2),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(!created.success);
    assert_eq!(created.error.as_deref(), Some(refusal.as_str()));

    // A real loop: no refusal, and the key is absent on the wire.
    let fine = state.check_merge_connectivity(st(a, 1), st(b, 2)).unwrap();
    assert!(fine.closure_refusal.is_none());
    let wire = serde_json::to_value(&fine).unwrap();
    assert!(wire.get("closure_refusal").is_none(), "{wire}");
    // Not connected: a merge is offered, and no closure refusal.
    let mut other = project(&state);
    let lone = add(&mut other, vec![shot(0.0, 3.0)]);
    state.set_project_for_tests(other);
    let merge = state
        .check_merge_connectivity(st(a, 1), st(lone, 0))
        .unwrap();
    assert!(merge.can_merge && merge.closure_refusal.is_none());
}

#[test]
fn a_redundant_tie_gets_no_refusal_and_is_created() {
    // Only a closure across ONE shot is refused (#737). C:0 is merged onto B:3,
    // which the loop's closure already ties to A:2, so a closure C:0-A:2 adds
    // nothing - the dialog offers Create and the command creates it, as before.
    let (mut p, a, b) = issue_loop();
    let c = add(&mut p, vec![shot(0.0, 5.0)]);
    p.merge_stations(st(c, 0), st(b, 3)).unwrap();
    let state = state_with(p);
    let check = state.check_merge_connectivity(st(c, 0), st(a, 2)).unwrap();
    assert!(check.connected && !check.can_merge);
    assert!(
        check.closure_refusal.is_none(),
        "{:?}",
        check.closure_refusal
    );
    let created = state.execute(Command::CreateLoopClosure {
        station_a: st(c, 0),
        station_b: st(a, 2),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(created.success, "{:?}", created.error);
    let after = project(&state);
    assert_eq!(after.loop_closures.len(), 2);
    // No warning, and the adjusted view solves cleanly.
    let warnings = state.compute_project_warnings(&after, None);
    assert!(warnings
        .iter()
        .all(|w| w.code != CLOSURE_ACROSS_ONE_SHOT && w.code != CLOSURE_ADJUSTMENT_FAILED));
    state.get_adjusted_stations().unwrap();
    let report = state.get_adjustment_report().unwrap();
    assert!(
        report.skipped_closures.is_empty() && report.failures.is_empty(),
        "{report:?}"
    );
}

// ---- the adjustment report and the failure warning ------------------------------

#[test]
fn the_report_lists_skipped_closures_and_failures_only_from_a_current_solve() {
    let (mut p, a, _) = issue_loop();
    let skipped_id = plant(&mut p, st(a, 1), st(a, 2));
    // A second component that cannot be adjusted: two datum-anchored sections
    // tied datum to datum by a closure, which creation allows.
    let x = add(&mut p, vec![shot(90.0, 10.0)]);
    let y = add(&mut p, vec![shot(0.0, 10.0)]);
    p.set_zero_datum(st(x, 0), GpsCoordinates::new(0.0, 0.0, 0.0).unwrap())
        .unwrap();
    p.set_zero_datum(st(y, 0), GpsCoordinates::new(0.0001, 0.0001, 0.0).unwrap())
        .unwrap();
    p.create_loop_closure(st(x, 0), st(y, 0), ClosureStrategy::Batch)
        .unwrap();
    let state = state_with(p);

    // Before any solve: the skipped closure is known, the failure is not.
    let report = state.get_adjustment_report().unwrap();
    assert_eq!(report.skipped_closures.len(), 1);
    let skipped = &report.skipped_closures[0];
    assert_eq!(skipped.closure_id, skipped_id);
    assert_eq!((skipped.shot_section_id, skipped.shot_index), (a, 1));
    assert!(report.failures.is_empty(), "no solve has run yet");
    let warnings = state.compute_project_warnings(&project(&state), None);
    assert!(warnings.iter().all(|w| w.code != CLOSURE_ADJUSTMENT_FAILED));

    // The view solves; now the failure is reported, in plain words.
    state.get_adjusted_stations().unwrap();
    let report = state.get_adjustment_report().unwrap();
    assert_eq!(report.failures.len(), 1, "{report:?}");
    let failure = &report.failures[0];
    let mut expected = vec![x, y];
    expected.sort_unstable();
    assert_eq!(failure.section_ids, expected);
    assert!(
        failure
            .reason
            .starts_with("loop closures tie two zero datums"),
        "{}",
        failure.reason
    );
    assert!(
        !failure.reason.contains("constraint group"),
        "raw text stays in the log"
    );

    let warnings = state.compute_project_warnings(&project(&state), None);
    let failed: Vec<_> = warnings
        .iter()
        .filter(|w| w.code == CLOSURE_ADJUSTMENT_FAILED)
        .collect();
    assert_eq!(failed.len(), 1, "{warnings:?}");
    let w = failed[0];
    assert!(expected.contains(&w.section_id));
    // Advisory: it lives only while the cached solve matches this version, so
    // it must not gate like an error (the upload gate counts errors).
    assert_eq!(w.severity, WarningSeverity::Warning);
    assert!(crate::models::warning_is_ignorable(
        CLOSURE_ADJUSTMENT_FAILED
    ));
    assert!(!w.is_resolved);
    assert!(
        w.message.starts_with(&format!(
            "Shot {}:{} is in a part of the survey (2 sections)",
            w.section_id,
            w.shot_index + 1
        )),
        "{}",
        w.message
    );
    // An older project version never borrows the cached failure.
    let mut older = project(&state);
    older.touch();
    let hash = state.current_adjusted_config_hash();
    assert!(state.cached_adjustment_failures(&older, hash).is_empty());
    // Nor does the current version under another config, while the current
    // version under the current config does.
    assert!(state
        .cached_adjustment_failures(&project(&state), hash.wrapping_add(1))
        .is_empty());
    assert_eq!(
        state
            .cached_adjustment_failures(&project(&state), hash)
            .len(),
        1
    );

    // Any edit drops the cached solve, and with it the failure.
    let edit = state.execute(Command::RemoveLoopClosure {
        closure_id: skipped_id,
    });
    assert!(edit.success);
    let report = state.get_adjustment_report().unwrap();
    assert!(report.skipped_closures.is_empty());
    assert!(report.failures.is_empty(), "stale failure after an edit");
}

#[test]
fn the_report_is_empty_without_a_project() {
    let state = AppState::new().unwrap();
    assert!(state.get_adjustment_report().is_none());
}

// ---- closures that arrive without create_loop_closure ----------------------------

/// A saved `.stalx` holding the issue's loop plus a closure across the one shot
/// A:1-A:2, written as a file from before the refusal (or another tool) would
/// be. Synthetic, in a temp dir; never a fixture.
fn file_with_closure_across_one_shot(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let (mut p, a, _) = issue_loop();
    p.name = name.to_string();
    plant(&mut p, st(a, 1), st(a, 2));
    let path = dir.join(format!("{name}.stalx"));
    let mut db = crate::persistence::Database::open(&path).unwrap();
    db.save_project(&p).unwrap();
    path
}

/// Assert `p` carries exactly one closure across one shot, that it is warned
/// about, and that the solve skips it rather than failing.
fn assert_skipped_and_warned(p: Project) {
    let skipped = p.closures_across_one_shot();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    let state = state_with(p);
    let warnings = state.compute_project_warnings(&project(&state), None);
    let ours: Vec<_> = warnings
        .iter()
        .filter(|w| w.code == CLOSURE_ACROSS_ONE_SHOT)
        .collect();
    assert_eq!(ours.len(), 1, "{warnings:?}");
    assert_eq!(ours[0].closure_id, Some(skipped[0].closure_id));
    state.get_adjusted_stations().unwrap();
    let report = state.get_adjustment_report().unwrap();
    assert_eq!(report.skipped_closures.len(), 1);
    assert!(report.failures.is_empty(), "{report:?}");
}

#[test]
fn adding_a_project_file_with_a_closure_across_one_shot_warns_and_skips_it() {
    // Add .stalx copies closures straight across (`project_append`), past the
    // creation-time refusal: the solve skip and the warning cover them.
    let dir = tempfile::tempdir().unwrap();
    let source = file_with_closure_across_one_shot(dir.path(), "source");
    let mut target = Project::new("target".to_string(), None);
    add(&mut target, vec![shot(45.0, 7.0)]);
    let target_path = dir.path().join("target.stalx");
    crate::persistence::Database::open(&target_path)
        .unwrap()
        .save_project(&target)
        .unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(target_path).success, "open the target");
    let added = state.execute(Command::add_project_file(source));
    assert!(added.success, "{:?}", added.error);
    assert_skipped_and_warned(project(&state));
}

#[test]
fn merging_project_files_with_a_closure_across_one_shot_warns_and_skips_it() {
    // Merge Projects appends every source the same way.
    let dir = tempfile::tempdir().unwrap();
    let one = file_with_closure_across_one_shot(dir.path(), "one");
    let mut other = Project::new("two".to_string(), None);
    add(&mut other, vec![shot(10.0, 4.0)]);
    let two = dir.path().join("two.stalx");
    crate::persistence::Database::open(&two)
        .unwrap()
        .save_project(&other)
        .unwrap();
    let out = dir.path().join("merged.stalx");
    let state = AppState::new().unwrap();
    let merged = state.merge_projects(vec![one, two], out.clone(), "merged".to_string(), false);
    assert!(merged.success, "{:?}", merged.error);
    let merged = crate::persistence::Database::open_existing(&out)
        .unwrap()
        .load_project()
        .unwrap();
    assert_skipped_and_warned(merged);
}

// ---- delete with shift removes the closure it degenerates ------------------------

/// Assert `result` reported exactly the removal of closures with the given
/// endpoint labels, in that order.
fn assert_removed(result: &crate::core::CommandResult, labels: &[&str]) {
    let removed: Vec<&str> = result
        .warnings
        .iter()
        .filter(|w| w.code == CLOSURE_REMOVED_ACROSS_ONE_SHOT)
        .map(|w| {
            assert_eq!(w.severity, WarningSeverity::Warning);
            w.message.as_str()
        })
        .collect();
    let expected: Vec<String> = labels
        .iter()
        .map(|l| {
            format!(
                "Removed loop closure {l}: closing the gap left its two stations at the two ends \
                 of one shot."
            )
        })
        .collect();
    assert_eq!(removed, expected, "{:?}", result.warnings);
}

#[test]
fn a_range_gap_close_removes_a_closure_it_leaves_across_one_shot_and_one_undo_restores_it() {
    // Closure A:1-A:4 closes three shots. Gap-closing shots 1..2 removes
    // stations 2 and 3, so A:4 becomes A:2 and the closure would span the one
    // shot A:1-A:2.
    let (mut p, a) = chain(5);
    let id = p
        .create_loop_closure(st(a, 1), st(a, 4), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let state = state_with(p);
    let before = project(&state);
    let (s1, s2) = (shot_id(&state, a, 1), shot_id(&state, a, 2));

    let result = state.execute_delete_shot_range_shift(a, s1, s2);
    assert!(result.success, "{:?}", result.error);
    // Named by the stations it was created with, not the renumbered A:1-A:2.
    assert_removed(&result, &[&format!("{a}:1 to {a}:4")]);
    let after = project(&state);
    assert_eq!(after.get_section(a).unwrap().shots.len(), 3);
    assert!(after.loop_closures.iter().all(|lc| lc.id != id));
    assert!(after.closures_across_one_shot().is_empty());

    // ONE undo restores the shots and the closure, exactly.
    assert!(state.undo().success);
    let undone = project(&state);
    assert_eq!(undone.get_section(a).unwrap().shots.len(), 5);
    assert_eq!(closure_endpoints(&undone, id), Some((st(a, 1), st(a, 4))));
    assert_eq!(
        serde_json::to_value(&undone.loop_closures).unwrap(),
        serde_json::to_value(&before.loop_closures).unwrap()
    );
    // Redo removes it again.
    assert!(state.redo().success);
    let redone = project(&state);
    assert_eq!(redone.get_section(a).unwrap().shots.len(), 3);
    assert!(redone.loop_closures.iter().all(|lc| lc.id != id));
    // And undo once more is still the same restore.
    assert!(state.undo().success);
    assert_eq!(
        closure_endpoints(&project(&state), id),
        Some((st(a, 1), st(a, 4)))
    );
}

#[test]
fn a_one_shot_range_does_what_the_single_delete_refuses() {
    // The single DeleteShot refuses to shift a shot that is part of ANY loop,
    // closure loops included, so it never shortens a closure to one shot. The
    // range gap-close only refuses merge loops, so a range of the one middle
    // shot under closure A:1-A:3 is allowed - and removes the closure.
    let (mut p, a) = chain(3);
    let id = p
        .create_loop_closure(st(a, 1), st(a, 3), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let state = state_with(p);
    let middle = shot_id(&state, a, 1);

    let single = state.execute(Command::DeleteShot {
        section_id: a,
        shot_id: middle,
    });
    assert!(!single.success, "the single delete is shift-blocked");
    assert!(project(&state).loop_closures.iter().any(|lc| lc.id == id));

    let range = state.execute_delete_shot_range_shift(a, middle, middle);
    assert!(range.success, "{:?}", range.error);
    assert_removed(&range, &[&format!("{a}:1 to {a}:3")]);
    assert!(project(&state).loop_closures.is_empty());
    assert!(state.undo().success);
    assert_eq!(
        closure_endpoints(&project(&state), id),
        Some((st(a, 1), st(a, 3)))
    );
}

#[test]
fn a_closure_through_a_merge_is_removed_by_the_shot_it_collapses() {
    // B branches off A:1. Closure B:0-A:4 (B:0 = A:1) closes three shots of A;
    // gap-closing A's shots 1..2 leaves B:0-A:2 across the one shot A:1-A:2.
    // The message names the closure's own ends, not the shot's.
    let (mut p, a) = chain(5);
    let b = add(&mut p, vec![shot(200.0, 6.0)]);
    p.merge_stations(st(b, 0), st(a, 1)).unwrap();
    let id = p
        .create_loop_closure(st(b, 0), st(a, 4), ClosureStrategy::LeastSquaresWeighted)
        .unwrap()
        .id;
    let state = state_with(p);
    let (s1, s2) = (shot_id(&state, a, 1), shot_id(&state, a, 2));
    let result = state.execute_delete_shot_range_shift(a, s1, s2);
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &[&format!("{b}:0 to {a}:4")]);
    assert!(project(&state).loop_closures.iter().all(|lc| lc.id != id));
    assert!(state.undo().success);
    assert_eq!(
        closure_endpoints(&project(&state), id),
        Some((st(b, 0), st(a, 4)))
    );
}

#[test]
fn a_gap_close_that_keeps_a_real_loop_keeps_its_closure() {
    // Closure A:0-A:3 over a one-shot range in the middle becomes A:0-A:2:
    // still two shots, still a loop. Nothing is removed or reported.
    let (mut p, a) = chain(4);
    let id = p
        .create_loop_closure(st(a, 0), st(a, 3), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let state = state_with(p);
    let middle = shot_id(&state, a, 1);
    let result = state.execute_delete_shot_range_shift(a, middle, middle);
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &[]);
    assert_eq!(
        closure_endpoints(&project(&state), id),
        Some((st(a, 0), st(a, 2)))
    );
}

#[test]
fn a_delete_leaves_a_closure_that_already_spanned_one_shot_alone() {
    // A closure that was across one shot BEFORE the delete keeps its warning;
    // the delete only undoes what it caused.
    let (mut p, a, _) = issue_loop();
    let old = plant(&mut p, st(a, 1), st(a, 2));
    let c = add(
        &mut p,
        vec![shot(10.0, 3.0), shot(20.0, 3.0), shot(30.0, 3.0)],
    );
    p.merge_stations(st(c, 0), st(a, 0)).unwrap();
    let state = state_with(p);
    let tail = shot_id(&state, c, 2);
    let result = state.execute(Command::DeleteShot {
        section_id: c,
        shot_id: tail,
    });
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &[]);
    assert!(project(&state).loop_closures.iter().any(|lc| lc.id == old));
}

/// The reviewer's case, through the preview AND the command: section C goes
/// east 5, east 5, south 5, west 5, north 4.8 and closure C:1-C:5 closes the
/// loop. Close Gap & Delete on shots C:2..C:4 (positions 1..=3) leaves the
/// closure as C:1-C:2. The bulk-delete dialog must list it BEFORE the user
/// confirms, the toast after, and both by the ids the user knows: C:1 to C:5.
#[test]
fn the_range_preview_lists_the_closure_the_gap_close_removes_by_its_pre_delete_ids() {
    let mut p = Project::new("preview".to_string(), None);
    let c = add(
        &mut p,
        vec![
            shot(90.0, 5.0),
            shot(90.0, 5.0),
            shot(180.0, 5.0),
            shot(270.0, 5.0),
            shot(0.0, 4.8),
        ],
    );
    let id = p
        .create_loop_closure(st(c, 1), st(c, 5), ClosureStrategy::Batch)
        .unwrap()
        .id;
    let state = state_with(p);
    let (first, last) = (shot_id(&state, c, 1), shot_id(&state, c, 3));

    let impact = project(&state)
        .preview_delete_shot_range(c, first, last)
        .unwrap();
    assert!(!impact.shift_blocked, "{:?}", impact.shift_blocked_reasons);
    let predicted = impact
        .closures_removed_across_one_shot
        .clone()
        .expect("a gap-close is offered, so it is predicted");
    assert_eq!(predicted.len(), 1);
    assert_eq!(predicted[0].closure_id, id);
    assert_eq!(
        (predicted[0].station_a, predicted[0].station_b),
        (st(c, 1), st(c, 5))
    );
    // The IPC path consumes its own clone; same answer, field for field.
    let owned = project(&state)
        .into_preview_delete_shot_range(c, first, last)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&owned).unwrap(),
        serde_json::to_value(&impact).unwrap()
    );
    // On the wire for the dialog.
    let wire = serde_json::to_value(&impact).unwrap();
    assert_eq!(
        wire["closures_removed_across_one_shot"][0]["station_b"]["station_index"],
        5
    );

    let result = state.execute_delete_shot_range_shift(c, first, last);
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &[&format!("{c}:1 to {c}:5")]);
    assert_eq!(
        result.warnings[0].message,
        closure_removed_message(&predicted[0])
    );
    assert!(project(&state).loop_closures.is_empty());
    // Undo puts it back as C:1-C:5.
    assert!(state.undo().success);
    assert_eq!(
        closure_endpoints(&project(&state), id),
        Some((st(c, 1), st(c, 5)))
    );
}

#[test]
fn the_range_preview_predicts_nothing_where_nothing_goes() {
    // A gap-close that keeps a real loop: predicted empty, not absent.
    let (mut p, a) = chain(4);
    p.create_loop_closure(st(a, 0), st(a, 3), ClosureStrategy::Batch)
        .unwrap();
    let state = state_with(p);
    let middle = shot_id(&state, a, 1);
    let impact = project(&state)
        .preview_delete_shot_range(a, middle, middle)
        .unwrap();
    assert!(impact
        .closures_removed_across_one_shot
        .is_some_and(|v| v.is_empty()));
    // The whole section offers no gap-close, so nothing is predicted at all,
    // and the key stays off the wire.
    let (first, last) = (shot_id(&state, a, 0), shot_id(&state, a, 3));
    let whole = project(&state)
        .preview_delete_shot_range(a, first, last)
        .unwrap();
    assert!(whole.closures_removed_across_one_shot.is_none());
    let wire = serde_json::to_value(&whole).unwrap();
    assert!(
        wire.get("closures_removed_across_one_shot").is_none(),
        "{wire}"
    );
    // A blocked shift offers no gap-close either: here it would drop the zero
    // datum on station 2, which the one-shot range removes.
    let (mut p, a) = chain(4);
    p.create_loop_closure(st(a, 1), st(a, 3), ClosureStrategy::Batch)
        .unwrap();
    p.set_zero_datum(st(a, 2), GpsCoordinates::new(0.0, 0.0, 0.0).unwrap())
        .unwrap();
    let state = state_with(p);
    let middle = shot_id(&state, a, 1);
    let impact = project(&state)
        .preview_delete_shot_range(a, middle, middle)
        .unwrap();
    assert!(impact.shift_blocked, "the fixture must block the shift");
    assert!(impact.closures_removed_across_one_shot.is_none());
}

/// Adversarial: random small surveys (chains, side passages merged on,
/// closures through the public API) and random single and range deletes. After
/// every delete that succeeds, no closure may be across one shot that was not
/// before it, every removal is reported, and one undo restores the closures
/// exactly. Exercises whatever shapes are reachable rather than only the ones
/// above.
#[test]
fn no_delete_leaves_a_new_closure_across_one_shot() {
    let mut seed: u64 = 0x737_d1e7e;
    let mut next = |m: u64| -> u64 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % m.max(1)
    };
    let mut removals = 0;
    let mut agreed = 0;
    for _case in 0..40 {
        let mut p = Project::new("fuzz".to_string(), None);
        let mut sections: Vec<SectionId> = Vec::new();
        for i in 0..(2 + next(2)) {
            let shots = (0..(3 + next(4)))
                .map(|_| shot(next(360) as f64, 3.0 + next(8) as f64))
                .collect();
            let id = add(&mut p, shots);
            if i > 0 {
                let prev = sections[next(sections.len() as u64) as usize];
                let len = p.get_section(prev).unwrap().shots.len() as u64;
                let _ = p.merge_stations(st(id, 0), st(prev, next(len + 1) as u32));
            }
            sections.push(id);
        }
        let stations: Vec<StationRef> = p
            .sections
            .iter()
            .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| st(s.id, i)))
            .collect();
        for _ in 0..(2 + next(4)) {
            let x = stations[next(stations.len() as u64) as usize];
            let y = stations[next(stations.len() as u64) as usize];
            let _ = p.create_loop_closure(x, y, ClosureStrategy::Batch);
        }
        let state = state_with(p);
        let before = project(&state);
        let before_across: HashSet<u64> = before
            .closures_across_one_shot()
            .into_iter()
            .map(|c| c.closure_id)
            .collect();
        let section = sections[next(sections.len() as u64) as usize];
        let n = before.get_section(section).unwrap().shots.len();
        let mut predicted = None;
        let result = if next(2) == 0 {
            let pos = next(n as u64) as usize;
            state.execute(Command::DeleteShot {
                section_id: section,
                shot_id: shot_id(&state, section, pos),
            })
        } else {
            let start = 1 + next((n - 2) as u64) as usize;
            let end = (start + next(2) as usize).min(n - 2);
            let (first, last) = (
                shot_id(&state, section, start),
                shot_id(&state, section, end),
            );
            // What the bulk-delete dialog will show, taken before the delete.
            predicted = before
                .preview_delete_shot_range(section, first, last)
                .ok()
                .and_then(|impact| impact.closures_removed_across_one_shot);
            state.execute_delete_shot_range_shift(section, first, last)
        };
        if !result.success {
            continue;
        }
        // The preview predicts exactly what the gap-close removed, by the same
        // pre-delete names (#737).
        if let Some(predicted) = predicted {
            let shown: Vec<String> = predicted.iter().map(closure_removed_message).collect();
            let removed: Vec<String> = result
                .warnings
                .iter()
                .filter(|w| w.code == CLOSURE_REMOVED_ACROSS_ONE_SHOT)
                .map(|w| w.message.clone())
                .collect();
            assert_eq!(shown, removed, "preview and execute disagree");
            agreed += usize::from(!removed.is_empty());
        }
        let after = project(&state);
        for c in after.closures_across_one_shot() {
            assert!(
                before_across.contains(&c.closure_id),
                "closure {} is newly across one shot after {:?}",
                c.closure_id,
                result.warnings
            );
        }
        let reported = result
            .warnings
            .iter()
            .filter(|w| w.code == CLOSURE_REMOVED_ACROSS_ONE_SHOT)
            .count();
        removals += reported;
        assert!(state.undo().success);
        assert_eq!(
            serde_json::to_value(&project(&state).loop_closures).unwrap(),
            serde_json::to_value(&before.loop_closures).unwrap(),
            "undo did not restore the closures"
        );
    }
    // The invariant has to be exercised, not only vacuously true.
    assert!(
        removals > 0,
        "no case removed a closure; widen the generator"
    );
    assert!(agreed > 0, "no removal was predicted by a preview");
}
