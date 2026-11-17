//! [#555] A project may not mint more line ids than the shot-id encode has
//! room for.
//!
//! A shot id is `line_id * FEATURE_ID_STRIDE + position` in `u32`, so the
//! last line id that can hold a FULL line (`FEATURE_ID_STRIDE - 1` shots)
//! is [`MAX_LINE_ID`] (42_948) and the last usable COUNT is
//! [`MAX_LINES_IN_ID_SPACE`] (42_949). Id 42_949 runs out of ids part-way
//! through filling (its unchecked `next_shot_id += 1` leaves `u32` at sequence
//! 67_296) and id 42_950 overflows `Line::new`'s bare
//! `id * FEATURE_ID_STRIDE` before it holds anything: panic in debug, wrap in
//! release, and a wrapped base ALIASES distinct shots onto one id.
//!
//! Only the TML cut budget was bounded before this (`linearize::cut_budget`),
//! which covers exactly one way of reaching the boundary. The places that
//! actually MINT ids are pinned here: `create_line` (the per-line door
//! every command comes through), `execute_import_lines_inner` (the batch
//! door every importer comes through), `merge_projects`, and the two
//! delete-and-split paths, which mint a line id without going through
//! `create_line` at all.
//!
//! The setup is deliberately arithmetic rather than 42k real lines:
//! `next_line_id` is the project's own record of how many ids are spent, so
//! setting it directly states the same precondition 42_948 creates would, and
//! the boundary is what is under test, not the loop that reaches it. The merge
//! test is the exception - its check sums the SOURCES' line counts, which
//! nothing but real lines can supply.

use tempfile::TempDir;

use super::{AppState, VirtualShotMode};
use crate::core::{Command, CommandResult};
use crate::models::{
    ImportPlacement, ImportLineConfig, Project, LineData, ShotData, MAX_LINES_IN_ID_SPACE,
    MAX_LINE_ID,
};
use crate::persistence::Database;

fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

/// A project whose id counter says `spent` ids have been handed out, with no
/// lines to show for them. Everything under test reads the counter, not the
/// line list.
fn state_with_spent_line_ids(spent: u32) -> AppState {
    let mut project = Project::new("Bound".to_string(), None);
    project.next_line_id = spent;
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    state
}

fn create_one_line(state: &AppState) -> CommandResult {
    state.execute(Command::CreateLine {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    })
}

fn import_config(index: u32) -> ImportLineConfig {
    ImportLineConfig {
        reverse: false,
        line_index: index,
        name: Some(format!("Imported {index}")),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(3.0)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement: ImportPlacement::Origin,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

/// The LAST id is usable: 42_948 * 100_000 + 99_999 = 4_294_899_999, which
/// still fits `u32`. An off-by-one guard would refuse this one.
#[test]
fn the_last_encodable_line_id_is_still_minted() {
    let state = state_with_spent_line_ids(MAX_LINE_ID);

    let result = create_one_line(&state);

    assert!(
        result.success,
        "id {MAX_LINE_ID} still encodes and must be allowed: {:?}",
        result.error
    );
    assert_eq!(
        result.line_id,
        Some(MAX_LINE_ID),
        "the line minted must BE the last encodable id"
    );
}

/// One past it is refused, instead of `Line::new` panicking on the debug
/// multiply or wrapping in release.
#[test]
fn minting_one_line_past_the_encode_is_refused() {
    let state = state_with_spent_line_ids(MAX_LINE_ID + 1);

    let result = create_one_line(&state);

    assert!(
        !result.success,
        "line id {} cannot hold shots and must be refused",
        MAX_LINE_ID + 1
    );
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("maximum number of lines")
            && error.contains(&MAX_LINES_IN_ID_SPACE.to_string()),
        "the refusal must name the limit, got: {error}"
    );
    let project = state.get_project_state().unwrap();
    assert!(
        project.lines.is_empty(),
        "a refused create must not leave a line behind"
    );
}

/// An import is refused as a WHOLE when its line count would cross the
/// bound, before any mutation - not part-way through the loop, and not with a
/// rollback after the work. One id is left here and the file declares two.
#[test]
fn an_import_that_would_cross_the_bound_is_refused_atomically() {
    let state = state_with_spent_line_ids(MAX_LINE_ID);

    let result = state.execute_import_lines(
        vec![import_config(0), import_config(1)],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );

    assert!(!result.success, "2 lines do not fit into 1 remaining id");
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains("would add 2 lines"),
        "the refusal must name what the file declares, got: {error}"
    );
    assert!(
        error.contains(&MAX_LINE_ID.to_string())
            && error.contains(&MAX_LINES_IN_ID_SPACE.to_string()),
        "the refusal must name how many ids are spent and the limit, got: {error}"
    );

    let project = state.get_project_state().unwrap();
    assert!(
        project.lines.is_empty(),
        "nothing may be created by a refused import"
    );
    assert_eq!(
        project.next_line_id, MAX_LINE_ID,
        "and no id may be spent by it either"
    );
    assert!(
        !state.can_undo(),
        "a refused import is not an undoable unit"
    );
}

/// The same import, one line smaller, lands: the batch check is exact, not
/// a conservative margin.
#[test]
fn an_import_that_exactly_fills_the_id_space_is_accepted() {
    let state = state_with_spent_line_ids(MAX_LINE_ID);

    let result = state.execute_import_lines(
        vec![import_config(0)],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );

    assert!(result.success, "import failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.lines.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![MAX_LINE_ID],
        "the last id in the space is spent, exactly once"
    );
}

/// A line of `shots` shots on a project that has spent EVERY line id
/// (`next_line_id == MAX_LINES_IN_ID_SPACE`, the state the import test
/// above blesses as legal). The line itself is id 0; only the counter says
/// the space is full.
fn state_at_full_id_space_with_one_line(shots: usize) -> (AppState, Vec<u32>) {
    let mut project = Project::new("Full".to_string(), None);
    project.add_line(LineData {
        name: Some("Original".to_string()),
        color: "#4ecdc4".to_string(),
        shots: (0..shots).map(|i| shot(5.0 + i as f64)).collect(),
        ..Default::default()
    });
    let shot_ids: Vec<u32> = project.lines[0].shots.iter().map(|s| s.id).collect();
    project.next_line_id = MAX_LINES_IN_ID_SPACE as u32;
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    (state, shot_ids)
}

fn assert_split_was_refused(state: &AppState, result: &CommandResult, shots: usize) {
    assert!(
        !result.success,
        "a split with no id to mint must be refused"
    );
    let error = result.error.clone().unwrap_or_default();
    assert!(
        error.contains("split it into two")
            && error.contains(&MAX_LINES_IN_ID_SPACE.to_string()),
        "the refusal must say what it could not do and name the limit, got: {error}"
    );
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.lines.len(),
        1,
        "no line B may be minted by a refused split"
    );
    assert_eq!(
        project.lines[0].shots.len(),
        shots,
        "and line A must not be truncated by it"
    );
    assert_eq!(
        project.lines[0].name.as_deref(),
        Some("Original"),
        "nor renamed"
    );
    assert_eq!(
        project.next_line_id, MAX_LINES_IN_ID_SPACE as u32,
        "and no id may be spent"
    );
}

/// The single-shot split mints line B through `Project::add_line`
/// directly, NOT through `create_line`, so it needs its own guard - and it
/// truncates line A first, which is what makes a mid-split refusal
/// unacceptable.
#[test]
fn a_delete_that_splits_a_line_is_refused_when_no_line_id_is_left() {
    let (state, shot_ids) = state_at_full_id_space_with_one_line(5);
    let line_id = state.get_project_state().unwrap().lines[0].id;

    let result = state.execute_delete_shot_and_split(
        line_id,
        shot_ids[2],
        "A".to_string(),
        "B".to_string(),
    );

    assert_split_was_refused(&state, &result, 5);
}

/// Same door, the range flavour: it rewrites connectivity
/// (`preserve_connections_for_removed_stations`) BEFORE it mints, so its guard
/// sits earlier in the function than the single-shot one.
#[test]
fn a_range_delete_that_splits_a_line_is_refused_when_no_line_id_is_left() {
    let (state, shot_ids) = state_at_full_id_space_with_one_line(5);
    let line_id = state.get_project_state().unwrap().lines[0].id;

    let result = state.execute_delete_shot_range_split(
        line_id,
        shot_ids[1],
        shot_ids[2],
        "A".to_string(),
        "B".to_string(),
    );

    assert_split_was_refused(&state, &result, 5);
}

/// One id left is enough for a split, which mints exactly one: the guard is the
/// same exact bound the other doors use, not a margin.
#[test]
fn a_split_with_the_last_line_id_left_still_succeeds() {
    let mut project = Project::new("Last".to_string(), None);
    project.add_line(LineData {
        name: Some("Original".to_string()),
        color: "#4ecdc4".to_string(),
        shots: (0..5).map(|i| shot(5.0 + f64::from(i))).collect(),
        ..Default::default()
    });
    let line_id = project.lines[0].id;
    let split_at = project.lines[0].shots[2].id;
    project.next_line_id = MAX_LINE_ID;
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);

    let result =
        state.execute_delete_shot_and_split(line_id, split_at, "A".to_string(), "B".to_string());

    assert!(result.success, "split failed: {:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.lines.len(), 2, "line B must exist");
    assert_eq!(
        project.lines[1].id, MAX_LINE_ID,
        "and must carry the last encodable id"
    );
}

/// A merge whose sources hold more lines between them than the id space
/// allows is refused before any line is appended, and writes no output file.
///
/// Real lines, because the check sums the sources' line counts - the one
/// place in this file where the id counter cannot stand in for them. They are
/// empty (no shots) so the cost is one row each.
#[test]
fn a_merge_past_the_id_space_is_refused_and_writes_no_output() {
    fn source(name: &str, lines: usize, dir: &TempDir) -> std::path::PathBuf {
        let mut project = Project::new(name.to_string(), None);
        for _ in 0..lines {
            project.add_line(LineData {
                color: "#4ecdc4".to_string(),
                ..Default::default()
            });
        }
        let path = dir.path().join(format!("{name}.stalx"));
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        drop(db);
        path
    }

    let temp_dir = TempDir::new().unwrap();
    // 42_949 + 1: the smallest pair that crosses the bound at all.
    let source_a = source("a", MAX_LINES_IN_ID_SPACE, &temp_dir);
    let source_b = source("b", 1, &temp_dir);
    let output_path = temp_dir.path().join("merged.stalx");

    let state = AppState::new().unwrap();
    let result = state.execute(Command::MergeProjects {
        source_paths: vec![source_a, source_b],
        output_path: output_path.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });

    assert!(!result.success, "the merged project cannot hold that many");
    let error = result.error.unwrap_or_default();
    assert!(
        error.contains(&(MAX_LINES_IN_ID_SPACE + 1).to_string())
            && error.contains(&MAX_LINES_IN_ID_SPACE.to_string()),
        "the refusal must name the combined count and the limit, got: {error}"
    );
    assert!(
        !output_path.exists(),
        "a refused merge must leave no output file"
    );
    assert!(
        !output_path.with_extension("stalx.tmp").exists(),
        "nor a temp file"
    );
}
