//! Shot UUIDs are unique across the project, by invariant on the station
//! graph (`validate` refuses a repeat). What is left to pin end to end is
//! what the user sees of a file saved before they were:
//!
//! - it opens with every id held once: the frozen 0.6.0 load renames the
//!   repeats (`reminted_shot_uuid`: line 0 keeps the file's ids, the copies
//!   get derived ones) and the migration keeps the result;
//! - its warning ignores follow the rename (plan 7.2.9) and do not leak to
//!   the line that kept the id;
//! - two opens of the unsaved file derive the same ids, the first save writes
//!   them (and the ignore under the new id), and the saved file reads back
//!   with nothing renamed a second time;
//! - un-ignoring a renamed shot clears its ignore;
//! - an edit that brings in a client id the project already holds (Create
//!   and Merge) gets the derived id, and undo and redo keep every id once.
//!
//! Retired with the SQLite engine: the history walks of
//! `a_legacy_history_walks_whole_*` (a legacy history is dropped on the
//! upgrade, plan 7.2.8; the open half is kept), `undoing_a_repair_of_a_cross_
//! line_repeat_restores_it_exactly` (the in-memory repeat it undid back to is
//! unrepresentable: no model state, file or history entry can hold one), and
//! `the_renamed_ids_flag_belongs_to_the_open_project` (the alias-lookup flag
//! is gone: the migration re-keys the ignores itself). Every survey here is
//! synthetic.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use tempfile::TempDir;

use super::robustness_kit::NullSink;
use super::AppState;
use crate::core::Command;
use crate::models::{
    reminted_shot_uuid, ClosureStrategy, LineData, LineId, ShotData, StationRef, FEATURE_ID_STRIDE,
};
use crate::persistence::stalx_v2::{self, DecodeOptions, Format};
use crate::persistence::{Database, ResolvedWarning};

const X: [&str; 4] = [
    "c0c0c0c0-0000-4000-8000-000000000001",
    "c0c0c0c0-0000-4000-8000-000000000002",
    "c0c0c0c0-0000-4000-8000-000000000003",
    "c0c0c0c0-0000-4000-8000-000000000004",
];

/// Shot 1 is 0.2 m, so every copy raises the ignorable SHORT_SHOT on it.
fn copy() -> LineData {
    LineData {
        name: Some("Copy".to_string()),
        color: "#4ecdc4".to_string(),
        shots: X
            .iter()
            .enumerate()
            .map(|(i, uuid)| ShotData {
                uuid: Some((*uuid).to_string()),
                azimuth_in: 20.0 + 35.0 * i as f64,
                azimuth_out: 20.0 + 35.0 * i as f64,
                length: if i == 1 { 0.2 } else { 10.0 },
                depth_in: i as f64,
                depth_out: i as f64 + 0.5,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// The SHORT_SHOT ignore of the second copy's shot 1, keyed as 0.6.0 keyed
/// it (the file's id, which the copy shares with line 0).
fn ignore_on_the_copy() -> ResolvedWarning {
    ResolvedWarning {
        line_id: 1,
        shot_id: FEATURE_ID_STRIDE + 1,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some(X[1].to_string()),
    }
}

/// A legacy file of `copies` lines that repeat each other's ids, as an
/// older build wrote it, with the copy's ignore when `ignored`.
fn legacy_file(dir: &Path, copies: usize, ignored: bool) -> PathBuf {
    let path = dir.join("legacy.stalx");
    let mut project = crate::models::Project::new("Legacy".to_string(), None);
    for _ in 0..copies {
        project.add_line(copy());
    }
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    if ignored {
        db.save_resolved_warning(&ignore_on_the_copy()).unwrap();
    }
    path
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "open: {:?}", r.error);
    state
}

/// The same, through the full load the UI uses.
fn load_full(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(NullSink));
    state.do_load_project_full(path.to_path_buf()).unwrap();
    state
}

fn line_ids(state: &AppState, line: LineId) -> Vec<String> {
    state
        .get_project_state()
        .unwrap()
        .line(line)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect()
}

fn all_ids(state: &AppState) -> Vec<Vec<String>> {
    state
        .get_project_state()
        .unwrap()
        .lines()
        .iter()
        .map(|l| l.shots.iter().map(|s| s.uuid.clone()).collect())
        .collect()
}

/// Every shot id the open project holds is held once, and it is valid.
fn assert_ids_unique(state: &AppState, at: &str) {
    let mut seen = BTreeSet::new();
    for uuid in all_ids(state).into_iter().flatten() {
        assert!(
            seen.insert(uuid.clone()),
            "{at}: shot UUID {uuid} held twice"
        );
    }
    let violations = state.integrity_violation_messages();
    assert!(violations.is_empty(), "{at}: {violations:?}");
}

fn short_shot_resolved(state: &AppState, line: LineId) -> bool {
    let project = state.get_project_state().unwrap();
    let shot = project.line(line).unwrap().shots[1].id;
    state
        .compute_project_warnings(&project)
        .iter()
        .find(|w| w.line_id == line && w.shot_id == shot && w.code == "SHORT_SHOT")
        .unwrap_or_else(|| panic!("line {line} must raise SHORT_SHOT"))
        .is_resolved
}

fn check_the_opened_copies(state: &AppState, how: &str) {
    assert_eq!(line_ids(state, 0), X, "{how}: line 0 keeps the file's ids");
    assert_eq!(
        line_ids(state, 1),
        X.iter()
            .map(|id| reminted_shot_uuid(1, id))
            .collect::<Vec<_>>(),
        "{how}: the copy's repeats are renamed"
    );
    assert_ids_unique(state, how);
    assert!(
        short_shot_resolved(state, 1),
        "{how}: the ignore follows the rename"
    );
    assert!(
        !short_shot_resolved(state, 0),
        "{how}: and does not leak to the first copy"
    );
    assert!(
        state.get_undo_stack().is_empty(),
        "{how}: a legacy history is not carried"
    );
}

#[test]
fn a_file_with_repeated_ids_opens_with_every_id_held_once() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = legacy_file(dir.path(), 3, true);
    check_the_opened_copies(&open(&path), "OpenProject");
    check_the_opened_copies(&load_full(&path), "the full load");
}

#[test]
fn reopening_without_saving_derives_the_same_ids_and_a_save_persists_them() {
    let _config = crate::test_helpers::isolated_config();
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let path = legacy_file(dir.path(), 3, true);
    let first = all_ids(&open(&path));
    let state = open(&path);
    assert_eq!(
        all_ids(&state),
        first,
        "two opens of the unsaved file agree"
    );

    // The first save upgrades the file: the derived ids, and the ignore
    // under the copy's new id.
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "{:?}", saved.error);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(stalx_v2::sniff(&bytes), Format::V2);
    let decoded = stalx_v2::decode(&bytes, &DecodeOptions::default()).unwrap();
    let ignores: Vec<(String, String)> = decoded
        .extras
        .ignored_warnings
        .iter()
        .map(|i| (i.shot.clone(), i.code.clone()))
        .collect();
    assert_eq!(
        ignores,
        vec![(reminted_shot_uuid(1, X[1]), "SHORT_SHOT".to_string())]
    );

    // The saved file reads back unchanged, nothing renamed a second time.
    let reopened = load_full(&path);
    assert_eq!(all_ids(&reopened), first);
    assert!(short_shot_resolved(&reopened, 1));
    assert_ids_unique(&reopened, "after the second open");
}

#[test]
fn unignoring_a_renamed_shot_clears_its_ignore() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = legacy_file(dir.path(), 3, true);
    let state = open(&path);
    assert!(short_shot_resolved(&state, 1));

    let live = reminted_shot_uuid(1, X[1]);
    assert!(state.shot_uuid_is_persisted(1, &live));
    state
        .delete_resolved_warning(1, FEATURE_ID_STRIDE + 1, "SHORT_SHOT", Some(&live))
        .unwrap();
    assert!(!short_shot_resolved(&state, 1), "the ignore is gone");
    assert!(
        state.has_unsaved_changes(),
        "an un-ignore rides the next save (plan 6.5)"
    );
}

#[test]
fn create_and_merge_renames_a_client_id_the_project_already_holds() {
    let _config = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = legacy_file(dir.path(), 1, false);
    let state = open(&path);
    assert_eq!(line_ids(&state, 0), X);

    let result = state.execute_create_and_merge(
        Some("Again".to_string()),
        None,
        None,
        "#ff0000".to_string(),
        None,
        copy().shots,
        0,
        StationRef::new(0, 2),
        ClosureStrategy::Move,
    );
    assert!(result.success, "{:?}", result.error);
    let new_line = result.line_id.unwrap();
    assert_eq!(line_ids(&state, 0), X, "the project keeps its ids");
    assert_eq!(
        line_ids(&state, new_line),
        X.iter()
            .map(|id| reminted_shot_uuid(new_line, id))
            .collect::<Vec<_>>(),
        "the incoming repeats get the derived ids"
    );
    assert_ids_unique(&state, "after create-and-merge");

    assert!(state.undo().success);
    assert_ids_unique(&state, "after undo");
    assert!(state.redo().success);
    assert_ids_unique(&state, "after redo");
    assert_eq!(
        line_ids(&state, new_line),
        X.iter()
            .map(|id| reminted_shot_uuid(new_line, id))
            .collect::<Vec<_>>(),
        "redo brings the same derived ids back"
    );
}
