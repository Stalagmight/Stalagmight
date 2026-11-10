//! End to end: a file saved before shot UUIDs were unique across the project
//! opens with every id held once, and its whole undo/redo history still walks.
//!
//! The legacy state is built the way an older build held it: the slot gets a
//! project whose sections repeat each other's ids, and real commands then record
//! a history against those ids (the integrity guard lets edits through on a
//! project that was already critical). Saving writes both; reopening through
//! the load path heals the project and translates the history. Every survey here
//! is synthetic.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use tempfile::TempDir;

use super::AppState;
use crate::core::events::EventSink;
use crate::core::Command;
use crate::models::{
    reminted_shot_uuid, ClosureStrategy, Project, SectionData, SectionId, ShotData, ShotUpdates,
    StationKey, StationRef,
};
use crate::persistence::{Database, ResolvedWarning};

/// The production load path reports progress through a sink; nothing listens.
struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

const X: [&str; 4] = [
    "c0c0c0c0-0000-4000-8000-000000000001",
    "c0c0c0c0-0000-4000-8000-000000000002",
    "c0c0c0c0-0000-4000-8000-000000000003",
    "c0c0c0c0-0000-4000-8000-000000000004",
];

/// Shot 1 is 0.2 m, so every copy raises the ignorable SHORT_SHOT on it.
fn copy() -> SectionData {
    SectionData {
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

fn three_copies() -> Project {
    let mut project = Project::new("Legacy".to_string(), None);
    for _ in 0..3 {
        project.add_section(copy());
    }
    project
}

/// An open session holding the legacy project with an empty history, backed by
/// a file at `path` that holds the same project.
fn legacy_session(dir: &Path) -> (AppState, PathBuf) {
    let path = dir.join("legacy.stalx");
    let mut project = three_copies();
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success, "open the file");
    project.file_path = Some(path.clone());
    state.set_project_for_tests(project);
    state.undo_stack.lock().unwrap().clear();
    (state, path)
}

fn run(state: &AppState, command: Command) {
    let what = format!("{command:?}");
    let result = state.execute(command);
    assert!(result.success, "{what} failed: {:?}", result.error);
}

/// A history touching the copies: every kind of entry a live edit records
/// about a shot, a station or a section, plus one entry left on the redo stack.
fn record_history(state: &AppState) {
    let in_one = |position: u32| crate::models::FEATURE_ID_STRIDE + position;
    run(
        state,
        Command::UpdateShot {
            section_id: 1,
            shot_id: in_one(2),
            updates: ShotUpdates {
                comment: Some(Some("edited on the copy".to_string())),
                ..Default::default()
            },
        },
    );
    run(
        state,
        Command::CreateShot {
            section_id: 1,
            shot: ShotData {
                azimuth_in: 200.0,
                azimuth_out: 200.0,
                length: 6.0,
                depth_in: 2.0,
                depth_out: 2.5,
                ..Default::default()
            },
            after_shot_id: Some(in_one(2)),
            before_shot_id: None,
        },
    );
    run(
        state,
        Command::SetStationComment {
            station_ref: StationRef::new(1, 3),
            comment: Some("on the copy".to_string()),
        },
    );
    run(
        state,
        Command::MergeStations {
            station_a: StationRef::new(1, 0),
            station_b: StationRef::new(0, 2),
        },
    );
    run(
        state,
        Command::CreateLoopClosure {
            station_a: StationRef::new(1, 5),
            station_b: StationRef::new(0, 4),
            closure_strategy: ClosureStrategy::Move,
        },
    );
    run(
        state,
        Command::DeleteShot {
            section_id: 2,
            shot_id: 2 * crate::models::FEATURE_ID_STRIDE + 3,
        },
    );
    run(state, Command::DeleteSection { section_id: 2 });
    assert!(state.undo().success, "leave the delete on the redo stack");
}

/// The open project with every station reference resolved to its position and
/// every shot id dropped: the survey as the user sees it, independent of what
/// the shots are called.
fn survey_state(state: &AppState) -> String {
    let raw = state.capture_project_snapshot().unwrap();
    let mut project: Project = serde_json::from_str(&raw).unwrap();
    project.rebuild_section_index();
    project.rebuild_shot_uuid_index();
    let resolved = project.clone();
    project.map_station_refs(|key| {
        let at = resolved
            .resolve_key(&key)
            .unwrap_or_else(|| panic!("dangling station key {key:?}"));
        StationKey::positional(at.section_id, at.station_index)
    });
    for section in &mut project.sections {
        let position = |uuid: &Option<String>| {
            uuid.as_ref().and_then(|u| {
                section
                    .shots
                    .iter()
                    .position(|s| s.uuid.as_ref() == Some(u))
            })
        };
        let gauge = (
            position(&section.gauge_shot_uuid),
            position(&section.gauge_neighbour_uuid),
        );
        section.gauge_shot_uuid = gauge.0.map(|p| p.to_string());
        section.gauge_neighbour_uuid = gauge.1.map(|p| p.to_string());
        for shot in &mut section.shots {
            shot.uuid = None;
        }
    }
    let value = serde_json::to_value(&project).unwrap();
    super::fuzz_undo_tests::canonicalize(&value, "", true).to_string()
}

/// Every shot id the open project holds is held once, and integrity is clean.
fn assert_ids_unique(state: &AppState, at: &str) {
    let project = state.get_project_state().unwrap();
    let mut seen = std::collections::HashSet::new();
    for section in &project.sections {
        for uuid in section.shots.iter().filter_map(|s| s.uuid.as_deref()) {
            assert!(
                seen.insert(uuid.to_string()),
                "{at}: shot UUID {uuid} held twice"
            );
        }
    }
    assert!(
        project.check_structural_integrity().is_empty(),
        "{at}: {:?}",
        project.check_structural_integrity()
    );
}

/// Undo to the bottom, recording the survey at every depth (top first), then
/// redo back to where it started.
fn walk(state: &AppState, check: &dyn Fn(&AppState, &str)) -> Vec<String> {
    let mut states = vec![survey_state(state)];
    check(state, "top");
    let mut steps = 0;
    while state.undo_stack.lock().unwrap().can_undo() {
        let result = state.undo();
        assert!(result.success, "undo {steps} failed: {:?}", result.error);
        steps += 1;
        check(state, &format!("after undo {steps}"));
        states.push(survey_state(state));
    }
    for step in 0..steps {
        let result = state.redo();
        assert!(result.success, "redo {step} failed: {:?}", result.error);
        check(state, &format!("after redo {step}"));
    }
    assert_eq!(survey_state(state), states[0], "redo returns to the top");
    states
}

fn stack_lengths(state: &AppState) -> (usize, usize) {
    (state.get_undo_stack().len(), state.get_redo_stack().len())
}

fn section_ids(state: &AppState, section: SectionId) -> Vec<String> {
    state
        .get_project_state()
        .unwrap()
        .get_section(section)
        .unwrap()
        .shots
        .iter()
        .filter_map(|s| s.uuid.clone())
        .collect()
}

fn short_shot_resolved(state: &AppState, section: SectionId) -> bool {
    let project = state.get_project_state().unwrap();
    let shot = project.get_section(section).unwrap().shots[1].id;
    state
        .compute_project_warnings(&project, None)
        .iter()
        .find(|w| w.section_id == section && w.shot_id == shot && w.code == "SHORT_SHOT")
        .unwrap_or_else(|| panic!("section {section} must raise SHORT_SHOT"))
        .is_resolved
}

#[test]
fn a_legacy_history_walks_whole_after_the_load_renames_its_repeats() {
    let dir = TempDir::new().unwrap();
    let (state, path) = legacy_session(dir.path());
    record_history(&state);
    state
        .save_resolved_warning(&ResolvedWarning {
            section_id: 1,
            shot_id: crate::models::FEATURE_ID_STRIDE + 1,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: Some(X[1].to_string()),
        })
        .unwrap();
    assert!(
        short_shot_resolved(&state, 1),
        "precondition: the ignore holds"
    );

    // Saved BEFORE the walk is recorded, so the states include what any save
    // does regardless of this change (it captures a gauge pair per section).
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let legacy_states = walk(&state, &|_, _| {});
    let legacy_lengths = stack_lengths(&state);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);

    // Reopen: section 0 keeps the file's ids, section 1 gets the derived ones.
    assert!(state.open_project(path.clone()).success, "reopen");
    assert_eq!(section_ids(&state, 0)[..2], X[..2]);
    assert_eq!(
        section_ids(&state, 1)[0],
        reminted_shot_uuid(1, X[0]),
        "the copy's repeats are renamed"
    );
    assert_eq!(
        stack_lengths(&state),
        legacy_lengths,
        "no entry was dropped"
    );
    assert!(
        short_shot_resolved(&state, 1),
        "the ignore follows the rename"
    );
    assert!(
        !short_shot_resolved(&state, 0),
        "and does not leak to the first copy"
    );

    // The same survey at every depth, every id held once at every depth.
    let healed_states = walk(&state, &|state, at| assert_ids_unique(state, at));
    assert_eq!(healed_states.len(), legacy_states.len());
    for (depth, (healed, legacy)) in healed_states.iter().zip(&legacy_states).enumerate() {
        assert_eq!(
            healed, legacy,
            "the survey differs at depth {depth} from the top"
        );
    }

    // The deleted section comes back from the redo stack under its own ids.
    assert!(state.redo().success, "redo the section delete");
    assert!(state.undo().success, "and undo it again");
    assert_ids_unique(&state, "after the redo stack round trip");
}

#[test]
fn reopening_without_saving_derives_the_same_ids_and_a_save_persists_them() {
    let dir = TempDir::new().unwrap();
    let (state, path) = legacy_session(dir.path());
    record_history(&state);
    state
        .save_resolved_warning(&ResolvedWarning {
            section_id: 1,
            shot_id: crate::models::FEATURE_ID_STRIDE + 1,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: Some(X[1].to_string()),
        })
        .unwrap();
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);

    let opened = |state: &AppState| -> (Vec<Vec<String>>, Vec<String>) {
        let project = state.get_project_state().unwrap();
        let ids = project
            .sections
            .iter()
            .map(|s| s.shots.iter().filter_map(|x| x.uuid.clone()).collect())
            .collect();
        let history = state
            .get_undo_stack()
            .iter()
            .map(|item| item.description.clone())
            .collect();
        (ids, history)
    };

    assert!(state.open_project(path.clone()).success);
    let first = opened(&state);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path.clone()).success);
    assert_eq!(opened(&state), first, "two opens of the unsaved file agree");

    // A save writes the derived ids and re-keys the ignore to them.
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let rows = Database::open_existing(&path)
        .unwrap()
        .load_resolved_warnings()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].shot_uuid.as_deref(),
        Some(reminted_shot_uuid(1, X[1]).as_str())
    );
    assert!(state.execute(Command::CloseProject).success);

    // The production load path agrees, and nothing is renamed a second time.
    state.set_event_sink(Arc::new(NullSink));
    state.do_load_project_full(path.clone()).unwrap();
    assert_eq!(opened(&state), first, "the saved file reads back unchanged");
    assert!(short_shot_resolved(&state, 1));
    assert_ids_unique(&state, "after the second open");
}

#[test]
fn unignoring_a_renamed_shot_clears_the_row_stored_against_its_old_id() {
    let dir = TempDir::new().unwrap();
    let (state, path) = legacy_session(dir.path());
    state
        .save_resolved_warning(&ResolvedWarning {
            section_id: 1,
            shot_id: crate::models::FEATURE_ID_STRIDE + 1,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: Some(X[1].to_string()),
        })
        .unwrap();
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success);
    assert!(short_shot_resolved(&state, 1));

    let live = reminted_shot_uuid(1, X[1]);
    assert!(
        state.shot_uuid_is_persisted(1, &live),
        "a derived id counts as persisted: every open derives it again"
    );
    state
        .delete_resolved_warning(
            1,
            crate::models::FEATURE_ID_STRIDE + 1,
            "SHORT_SHOT",
            Some(&live),
        )
        .unwrap();
    assert!(!short_shot_resolved(&state, 1), "the ignore is gone");
}

#[test]
fn create_and_merge_renames_a_client_id_the_project_already_holds() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("one.stalx");
    let mut project = Project::new("One".to_string(), None);
    project.add_section(copy());
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success);

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
    let new_section = result.section_id.unwrap();
    assert_eq!(section_ids(&state, 0), X);
    assert_eq!(
        section_ids(&state, new_section),
        X.iter()
            .map(|id| reminted_shot_uuid(new_section, id))
            .collect::<Vec<_>>()
    );
    assert_ids_unique(&state, "after create-and-merge");

    // The merge anchored on the renamed start, so undo and redo both hold.
    assert!(state.undo().success);
    assert!(state.redo().success);
    assert_ids_unique(&state, "after undo + redo");
}

#[test]
fn undoing_a_repair_of_a_cross_section_repeat_restores_it_exactly() {
    let dir = TempDir::new().unwrap();
    let (state, _path) = legacy_session(dir.path());
    let before = section_ids(&state, 1);
    assert_eq!(before, X);

    let repaired = state.repair_open_project_integrity();
    assert!(repaired.changed && repaired.remaining == 0, "{repaired:?}");
    assert_ids_unique(&state, "after repair");

    // Undo puts the recorded state back verbatim, repeats included; the next
    // load heals them with the same rule.
    assert!(state.undo().success);
    assert_eq!(section_ids(&state, 1), before);
    assert!(state.redo().success);
    assert_ids_unique(&state, "after redoing the repair");
}
