//! The undo pipeline end to end on the changeset engine (plan 8, C16).
//!
//! Ported from the 0.6.0 `state/tests.rs` sections "Undo/Redo Tests", "State
//! Transition Tests", "Batch Command Tests", "Undo/Redo with Shot
//! Restoration", "Multiple Undo/Redo Sequence", "Command Execution Tests
//! (fn-34-nvq spec)", "Undo/Redo Tests for fn-35-f4v.1", "Undo/Redo Entry
//! Preservation on Failure Tests", "CreateLine / CreateShot undo/redo ID
//! preservation", "Batch Tests", "Coverage gap: batch edge cases" and
//! "Undo-after-restart end-to-end test". Every port keeps what the old test
//! asserted that the user can see, and most add the engine's own oracle on
//! top: one undo and one redo land on the recorded states to the bit
//! (session shot ids included, the id allocators allowed to stand higher).
//!
//! The 0.6.0 failure tests pushed hand-made entries with unparsable payloads
//! (`UndoData::PreviousValues { values_json: "not-json" }`). An entry is a
//! changeset now, so the way an entry fails is its precondition: the project
//! was changed behind the engine's back, and the step is refused and the
//! project left untouched. Those tests tamper with the live project instead.
//!
//! All data is synthetic; every file goes to a temporary directory.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point, and exact float \
              equality is what it checks"
)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::NaiveDate;
use tempfile::TempDir;

use super::topology_kit::{assert_round_trip, assert_same, snapshot};
use super::{AppState, VirtualShotMode};
use crate::core::undo::UndoEntry;
use crate::core::{Command, CommandResult};
use crate::models::survey::changeset::{capture, HistoryRefs, RecordedPreview};
use crate::models::survey::{HistoryEntry, Project, Station};
use crate::models::{
    Coordinates, ExistingShotUpdate, GpsCoordinates, ImportLineConfig, ImportPlacement, LineId,
    LineUpdates, Person, ShotData, ShotId, ShotUpdates, StationId, StationRef, SurveyCredit,
    SurveyDirection,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

// ===========================================================================
// Helpers
// ===========================================================================

/// 0.6.0's `test_shot_data`.
fn test_shot_data(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

/// A level shot on `azimuth`.
fn level(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    assert!(r.success, "{:?}", r.error);
    state
}

fn create_line_command(name: &str, color: &str, shots: Vec<ShotData>) -> Command {
    Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: color.to_string(),
        direction: None,
        shots,
        cave_section_id: None,
    }
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    let r = state.execute(create_line_command(name, "#ff0000", shots));
    assert!(r.success, "CreateLine {name}: {:?}", r.error);
    r.line_id.unwrap()
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn shot_ids(state: &AppState, line: LineId) -> Vec<ShotId> {
    project(state)
        .line(line)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

fn lengths(state: &AppState, line: LineId) -> Vec<f64> {
    project(state)
        .line(line)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect()
}

fn line_ids(state: &AppState) -> Vec<LineId> {
    project(state).lines().iter().map(|l| l.id).collect()
}

fn line_names(state: &AppState) -> Vec<String> {
    project(state)
        .lines()
        .iter()
        .map(|l| l.name.clone().unwrap_or_default())
        .collect()
}

fn station_at(p: &Project, line: LineId, index: u32) -> StationId {
    p.topology()
        .station_at(StationRef::new(line, index))
        .unwrap()
}

/// The raw position of a slot.
fn xyz(p: &Project, line: LineId, index: u32) -> [f64; 3] {
    p.raw_positions()
        .slot(p, StationRef::new(line, index))
        .unwrap()
}

fn assert_close(a: [f64; 3], b: [f64; 3], what: &str) {
    for axis in 0..3 {
        assert!(
            (a[axis] - b[axis]).abs() < 1e-6,
            "{what}: {a:?} against {b:?}"
        );
    }
}

/// Poison `mutex` the way a panicking holder would.
fn poison<T>(mutex: &Mutex<T>) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock();
        panic!("poison the mutex for the lock-failure tests");
    }));
    assert!(mutex.is_poisoned(), "setup: the mutex is poisoned");
}

/// Change the live project behind the engine's back (what a refused step
/// is about: the recorded state is no longer the live one).
fn tamper(state: &AppState, f: impl FnOnce(&mut Project)) {
    let mut guard = state.project.lock().unwrap();
    f(guard.as_mut().unwrap());
}

/// The newest undo entry's `(label, time)`, which identify an entry.
fn top_undo(state: &AppState) -> (String, Option<String>) {
    let item = state.get_undo_stack().into_iter().next().unwrap();
    (item.description, item.timestamp)
}

fn top_redo(state: &AppState) -> (String, Option<String>) {
    let item = state.get_redo_stack().into_iter().next().unwrap();
    (item.description, item.timestamp)
}

fn write_v2(path: &Path, p: &Project) {
    std::fs::write(
        path,
        stalx_v2::encode(p, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "open: {:?}", r.error);
    state
}

fn save_as(state: &AppState, path: &Path) {
    let r = state.execute(Command::SaveProject {
        path: Some(path.to_path_buf()),
    });
    assert!(r.success, "save: {:?}", r.error);
}

// ===========================================================================
// Undo/Redo Tests
// ===========================================================================

#[test]
fn test_undo_create_line() {
    let state = new_state();
    let before = snapshot(&state);
    create(&state, "To Undo", vec![test_shot_data(5.0)]);
    assert_eq!(project(&state).lines().len(), 1);
    assert!(state.can_undo());
    assert_round_trip(&state, &before, "create line");
    assert!(state.undo().success);
    assert!(project(&state).lines().is_empty());
    assert!(project(&state).stations.is_empty(), "its stations went too");
}

#[test]
fn test_undo_delete_line() {
    let state = new_state();
    let line_id = create(&state, "Will Delete", vec![test_shot_data(5.0)]);
    let before = snapshot(&state);
    assert!(state.execute(Command::DeleteLine { line_id }).success);
    assert!(project(&state).lines().is_empty());
    assert_round_trip(&state, &before, "delete line");
    assert!(state.undo().success);
    let p = project(&state);
    assert_eq!(p.lines().len(), 1);
    assert_eq!(p.lines()[0].name.as_deref(), Some("Will Delete"));
}

#[test]
fn test_undo_update_shot() {
    let state = new_state();
    let line_id = create(&state, "Line", vec![test_shot_data(5.0)]);
    let shot_id = shot_ids(&state, line_id)[0];
    let before = snapshot(&state);
    assert!(
        state
            .execute(Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(15.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert_eq!(lengths(&state, line_id), vec![15.0]);
    assert_round_trip(&state, &before, "update shot");
    assert!(state.undo().success);
    assert_eq!(lengths(&state, line_id), vec![5.0]);
}

#[test]
fn test_redo_create_line() {
    let state = new_state();
    create(&state, "Redo Me", vec![]);
    let after = snapshot(&state);
    assert!(state.undo().success);
    assert!(project(&state).lines().is_empty());
    assert!(state.can_redo());
    assert!(state.redo().success);
    assert_eq!(project(&state).lines().len(), 1);
    assert_same(&snapshot(&state), &after, "redo of create line");
}

#[test]
fn test_undo_nothing() {
    let state = new_state();
    assert!(!state.can_undo());
    let r = state.undo();
    assert!(!r.success);
    assert_eq!(r.error.as_deref(), Some("Nothing to undo"));
}

#[test]
fn test_redo_nothing() {
    let state = new_state();
    assert!(!state.can_redo());
    let r = state.redo();
    assert!(!r.success);
    assert_eq!(r.error.as_deref(), Some("Nothing to redo"));
}

#[test]
fn test_redo_cleared_on_new_command() {
    let state = new_state();
    create(&state, "First", vec![]);
    assert!(state.undo().success);
    assert!(state.can_redo());
    create(&state, "Different", vec![]);
    assert!(!state.can_redo(), "a new command clears the redo branch");
    assert!(state.get_redo_stack().is_empty());
}

/// The Full-mode byte budget never evicts the newest entry (0.6.0's
/// `undo_budget_always_keeps_the_newest_entry`): an edit whose entry alone is
/// over the budget stays undoable, and only the older entries go.
#[test]
fn the_byte_budget_keeps_the_newest_entry_even_when_it_alone_is_over() {
    let state = new_state();
    let line_id = create(&state, "Line", vec![test_shot_data(5.0)]);
    let before = snapshot(&state);
    let shot_id = shot_ids(&state, line_id)[0];
    state.undo_stack.lock().unwrap().set_byte_budget(1);
    assert!(
        state
            .execute(Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(15.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(state.can_undo(), "the edit just made stays undoable");
    assert_eq!(state.get_undo_stack().len(), 1, "only the older entry goes");
    assert!(state.undo().success);
    assert_eq!(lengths(&state, line_id), vec![5.0]);
    assert_same(&snapshot(&state), &before, "undo under the byte budget");
    assert!(state.can_redo(), "and the undone entry is redoable");
    assert!(state.redo().success);
    assert_eq!(lengths(&state, line_id), vec![15.0]);
}

/// 0.6.0 refused with "Failed to lock undo stack" when the stack's mutex was
/// poisoned. The foundation takes every state lock poison-tolerantly (a
/// panic elsewhere must not wedge the history), so an undo and a redo still
/// step, and the history the frontend reads (`can_undo`, the stack lists)
/// says the same as the step does.
#[test]
fn a_poisoned_undo_stack_still_undoes_and_redoes() {
    let state = new_state();
    let line_id = create(&state, "Line", vec![test_shot_data(5.0)]);
    let before = snapshot(&state);
    let shot_id = shot_ids(&state, line_id)[0];
    assert!(
        state
            .execute(Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(7.0),
                    ..Default::default()
                },
            })
            .success
    );
    poison(&state.undo_stack);
    assert!(
        state.can_undo(),
        "the poisoned stack still reports its entries"
    );
    assert_eq!(state.get_undo_stack().len(), 2);
    assert_round_trip(&state, &before, "undo and redo through a poisoned stack");
    assert!(state.undo().success);
    assert!(state.can_redo());
    assert_eq!(state.get_redo_stack().len(), 1);
}

/// 0.6.0 refused with "Failed to lock project" when the project mutex was
/// poisoned, and kept the entry. The foundation's undo takes the lock
/// poison-tolerantly, so the step lands, and the entry moves to the other
/// stack exactly as an unpoisoned step would.
#[test]
fn a_poisoned_project_lock_does_not_wedge_the_history() {
    let state = new_state();
    let before = snapshot(&state);
    create(&state, "Line", vec![test_shot_data(5.0)]);
    let after = snapshot(&state);
    poison(&state.project);
    assert!(state.undo().success, "undo through a poisoned project lock");
    assert_same(&snapshot(&state), &before, "the undo landed");
    assert_eq!(state.get_redo_stack().len(), 1);
    assert!(state.redo().success, "redo through a poisoned project lock");
    assert_same(&snapshot(&state), &after, "the redo landed");
    assert_eq!(state.get_undo_stack().len(), 1);
    assert!(state.get_redo_stack().is_empty());
}

/// 0.6.0's `ProjectSnapshotPair` round trip (a whole-project entry, the one
/// Add .stalx and Merge Projects recorded): an Add .stalx brings in a whole
/// other survey as one entry, and one undo and one redo land exactly.
#[test]
fn a_whole_survey_added_from_a_file_undoes_and_redoes_as_one_entry() {
    let dir = TempDir::new().unwrap();
    let source_path = dir.path().join("source.stalx");
    {
        let source = new_state();
        create(&source, "Added A", vec![test_shot_data(2.0)]);
        create(
            &source,
            "Added B",
            vec![test_shot_data(3.0), test_shot_data(4.0)],
        );
        save_as(&source, &source_path);
    }
    let state = new_state();
    create(&state, "Own", vec![test_shot_data(1.0)]);
    let before = snapshot(&state);
    let depth = state.get_undo_stack().len();
    let r = state.execute(Command::add_project_file(source_path));
    assert!(r.success, "add: {:?}", r.error);
    assert_eq!(state.get_undo_stack().len(), depth + 1, "one entry");
    assert_eq!(line_names(&state), ["Own", "Added A", "Added B"]);
    assert_round_trip(&state, &before, "add .stalx");
    assert!(state.undo().success);
    assert_eq!(line_names(&state), ["Own"]);
    assert!(state.can_redo());
    assert!(state.redo().success);
    assert!(state.can_undo() && !state.can_redo());
}

/// A changeset that replaces the whole survey (another name, other lines)
/// steps both ways through the stack like any other entry.
#[test]
fn an_entry_that_replaces_the_whole_survey_steps_both_ways() {
    let state = new_state();
    create(&state, "Before Line", vec![test_shot_data(1.0)]);
    let before = project(&state);
    let mut after = Project::new_at("After", None, before.created_at);
    after
        .add_line(
            crate::models::survey::LineMeta {
                name: Some("After Line".to_string()),
                color: "#00ff00".to_string(),
                direction: SurveyDirection::Out,
                ..Default::default()
            },
            crate::models::survey::StartAt::New,
            vec![test_shot_data(2.0), test_shot_data(3.0)],
        )
        .unwrap();
    let start = after.lines()[0].start;
    after.set_placement(start, 0.0, 0.0).unwrap();
    assert!(after.validate().is_empty());
    let changes = capture(&before, &after);
    tamper(&state, |p| *p = after.clone());
    state.undo_stack.lock().unwrap().clear();
    state
        .undo_stack
        .lock()
        .unwrap()
        .record(HistoryEntry {
            timestamp: chrono::Utc::now(),
            kind: "batch".to_string(),
            label: "Replace the survey".to_string(),
            label_parts: Vec::new(),
            refs: HistoryRefs::default(),
            preview: RecordedPreview::default(),
            changes,
        })
        .unwrap();
    assert!(state.undo().success);
    let undone = project(&state);
    assert_eq!(undone.name, "Test");
    assert_eq!(undone.lines()[0].shots.len(), 1);
    assert!(state.can_redo());
    assert!(state.redo().success);
    let redone = project(&state);
    assert_eq!(redone.name, "After");
    assert_eq!(redone.lines()[0].shots.len(), 2);
    assert!(state.can_undo() && !state.can_redo());
}

// ===========================================================================
// State Transition Tests
// ===========================================================================

#[test]
fn test_project_modification_tracking() {
    let state = new_state();
    let initial = project(&state).modified_at;
    std::thread::sleep(std::time::Duration::from_millis(2));
    create(&state, "Line", vec![]);
    let after_line = project(&state).modified_at;
    assert!(after_line > initial, "an edit stamps the edit time");
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert!(state.undo().success);
    assert!(
        project(&state).modified_at > after_line,
        "an undo stamps it too (plan Deviation C5)"
    );
}

/// 0.6.0's `is_undoable`: the lifecycle commands never record an entry and
/// the edits do. There is no predicate any more (the pipeline records what
/// changed the survey), so the property is measured on the stack.
#[test]
fn lifecycle_commands_record_no_entry_and_edits_record_one() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("lifecycle.stalx");
    let state = new_state();
    assert!(
        state.get_undo_stack().is_empty(),
        "CreateProject records nothing"
    );
    let line_id = create(&state, "Line", vec![test_shot_data(5.0)]);
    assert_eq!(state.get_undo_stack().len(), 1, "CreateLine records one");
    let r = state.execute(Command::CreateShot {
        line_id,
        shot: test_shot_data(6.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(r.success);
    assert_eq!(state.get_undo_stack().len(), 2, "CreateShot records one");
    save_as(&state, &path);
    assert_eq!(
        state.get_undo_stack().len(),
        2,
        "SaveProject records nothing"
    );
    assert!(state.execute(Command::DeleteLine { line_id }).success);
    assert_eq!(state.get_undo_stack().len(), 3, "DeleteLine records one");
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.get_undo_stack().is_empty(), "CloseProject drops it");
    let r = state.execute(Command::OpenProject { path });
    assert!(r.success);
    assert_eq!(
        state.get_undo_stack().len(),
        3,
        "OpenProject loads the file's history and records nothing of its own"
    );
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Fresh".to_string(),
                location: None,
            })
            .success
    );
    assert!(
        state.get_undo_stack().is_empty(),
        "CreateProject starts an empty history"
    );
}

/// The privacy fence (plan 8.2, "privacy (no paths)"): a command that names a
/// filesystem path never enters a batch, and no history entry the file
/// carries holds a path, whatever the edits were (an Add .stalx included).
#[test]
fn path_bearing_commands_never_enter_shared_undo_history() {
    let dir = TempDir::new().unwrap();
    let private = dir.path().join("alice-private-folder");
    std::fs::create_dir_all(&private).unwrap();
    let private_file = private.join("survey.stalx");
    let commands = [
        Command::OpenProject {
            path: private_file.clone(),
        },
        Command::SaveProject {
            path: Some(private_file.clone()),
        },
        Command::MergeProjects {
            source_paths: vec![private_file.clone()],
            output_path: private_file.clone(),
            output_name: "Merged".to_string(),
            archive_originals: false,
        },
        Command::add_project_file(private_file.clone()),
    ];
    let state = new_state();
    for command in commands {
        assert!(
            !AppState::is_batch_allowed(&command),
            "a filesystem-path command must not bypass the privacy fence inside Batch: \
             {command:?}"
        );
        let r = state.execute(Command::Batch {
            commands: vec![command],
            label: None,
        });
        assert!(
            !r.success,
            "a batch holding a path-bearing command is refused"
        );
    }
    assert!(state.get_undo_stack().is_empty());

    // Edits, an Add .stalx from the private folder, a save there.
    let source = private.join("source.stalx");
    {
        let s = new_state();
        create(&s, "From Alice", vec![test_shot_data(2.0)]);
        save_as(&s, &source);
    }
    create(&state, "Own", vec![test_shot_data(1.0)]);
    let added = state.execute(Command::add_project_file(source));
    assert!(added.success, "{:?}", added.error);
    save_as(&state, &private_file);
    let decoded = stalx_v2::decode(
        &std::fs::read(&private_file).unwrap(),
        &stalx_v2::DecodeOptions::default(),
    )
    .unwrap();
    let history = decoded.extras.history;
    assert_eq!(history.undo.len(), 2);
    let folder = dir.path().to_string_lossy().into_owned();
    for entry in history.undo.iter().chain(&history.redo) {
        let text = entry.as_str();
        assert!(
            !text.contains(&folder) && !text.contains("alice-private-folder"),
            "a history entry carries a path: {text}"
        );
    }
}

/// A confirmed push cuts the pushed history (#303): the undo entries the
/// commit carried go, the later ones stay undoable, the redo branch never
/// survives, and the file says the same (history cut, chain pointer and
/// watermark cleared). On a v2 file the cut is a local-only rewrite.
#[test]
fn confirmed_push_persists_the_pruned_undo_and_redo_stacks() {
    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("post-push-history.stalx");
        let mut p = super::stalx_fixtures::line_survey("Post push");
        p.modified_at = p.created_at;
        write_v2(&path, &p);
        let state = open(&path);
        let line_id = project(&state).lines()[0].id;
        for length in [11.0, 12.0, 13.0] {
            let shot_id = shot_ids(&state, line_id)[0];
            assert!(
                state
                    .execute(Command::UpdateShot {
                        line_id,
                        shot_id,
                        updates: ShotUpdates {
                            length: Some(length),
                            ..Default::default()
                        },
                    })
                    .success
            );
        }
        assert!(state.undo().success, "one entry on the redo stack");
        assert!(state.execute(Command::SaveProject { path: None }).success);
        state.speleodb_set_unwound_from(Some("previous-tip"));
        let stamps: Vec<_> = state
            .undo_stack
            .lock()
            .unwrap()
            .undo_entries()
            .map(UndoEntry::timestamp)
            .collect();
        assert_eq!(stamps.len(), 2);
        // Newest first: the commit carried the older one only.
        let cutoff = stamps[1];
        {
            let project = state.project.lock().unwrap();
            state.speleodb_clear_undo_after_push(project.as_ref(), cutoff);
        }
        let stack = state.undo_stack.lock().unwrap();
        assert_eq!(stack.len(), 1, "the late edit remains undoable in memory");
        assert_eq!(stack.redo_len(), 0, "redo never survives a confirmed push");
        drop(stack);
        let decoded = stalx_v2::decode(
            &std::fs::read(&path).unwrap(),
            &stalx_v2::DecodeOptions::default(),
        )
        .unwrap();
        assert_eq!(
            decoded.extras.history.undo.len(),
            1,
            "the pushed entries are cut from the file"
        );
        assert!(decoded.extras.history.redo.is_empty());
        assert_eq!(decoded.extras.local.speleodb.unwound_from, None);
        assert_eq!(decoded.extras.local.speleodb.inherited_through, None);
        assert!(state.undo().success, "the kept entry still undoes");
    });
}

/// The SpeleoDB watermark rule (#303, plan 8.2): an entry recorded after a
/// loaded history is stamped strictly after the inherited watermark, even
/// when the watermark is ahead of this machine's clock, and strictly after
/// the entry before it, so the push's cut of inherited entries never takes
/// it. Undo and redo keep an entry's stamp.
#[test]
fn a_new_entry_is_stamped_after_the_inherited_watermark() {
    let state = new_state();
    let watermark = chrono::Utc::now() + chrono::Duration::days(365);
    state.speleodb_set_inherited_through(Some(&watermark.to_rfc3339()));
    let line_id = create(&state, "Line", vec![test_shot_data(5.0)]);
    create(&state, "Second", vec![test_shot_data(6.0)]);
    let stamps = |state: &AppState| -> Vec<chrono::DateTime<chrono::Utc>> {
        state
            .undo_stack
            .lock()
            .unwrap()
            .undo_entries()
            .map(UndoEntry::timestamp)
            .collect()
    };
    let recorded = stamps(&state);
    assert_eq!(recorded.len(), 2);
    assert!(
        recorded.iter().all(|t| *t > watermark),
        "every new entry is after the watermark: {recorded:?} vs {watermark}"
    );
    assert!(recorded[0] > recorded[1], "strictly increasing stamps");
    // The push's cut at the watermark takes nothing recorded here.
    let mut copy = crate::core::UndoStack::new();
    copy.load_persisted(
        &state.undo_stack.lock().unwrap().persisted(),
        &crate::models::survey::changeset::EntryLimits::default(),
    )
    .unwrap();
    assert_eq!(copy.len(), 2);
    assert_eq!(copy.drop_entries_up_to(watermark), 0);
    assert!(state.undo().success);
    assert!(state.redo().success);
    assert_eq!(stamps(&state), recorded, "a step keeps the entry's stamp");
    // A later entry still lands after both.
    let shot_id = shot_ids(&state, line_id)[0];
    assert!(
        state
            .execute(Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(stamps(&state)[0] > recorded[0]);
}

// ===========================================================================
// Batch Command Tests
// ===========================================================================

#[test]
fn test_batch_create_lines() {
    let state = new_state();
    let before = snapshot(&state);
    let r = state.execute(Command::Batch {
        commands: vec![
            create_line_command("First", "#ff0000", vec![test_shot_data(5.0)]),
            create_line_command("Second", "#00ff00", vec![test_shot_data(7.0)]),
        ],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.line_ids.len(), 2);
    assert_eq!(project(&state).lines().len(), 2);
    assert_eq!(state.get_undo_stack().len(), 1, "one entry for the batch");
    assert_round_trip(&state, &before, "batch of creates");
}

#[test]
fn test_batch_fails_on_first_error() {
    let state = new_state();
    let before = snapshot(&state);
    let r = state.execute(Command::Batch {
        commands: vec![
            create_line_command("First", "#ff0000", vec![]),
            Command::DeleteLine { line_id: 999 },
        ],
        label: None,
    });
    assert!(!r.success);
    assert_same(&snapshot(&state), &before, "the batch rolled back");
    assert!(state.get_undo_stack().is_empty(), "and recorded nothing");
}

// ===========================================================================
// Undo/Redo with Shot Restoration / Multiple Undo/Redo Sequence
// ===========================================================================

#[test]
fn test_undo_delete_shot_restores_at_correct_position() {
    let state = new_state();
    let line_id = create(
        &state,
        "Line",
        vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(9.0),
        ],
    );
    let before = snapshot(&state);
    let ids = shot_ids(&state, line_id);
    assert!(
        state
            .execute(Command::DeleteShot {
                line_id,
                shot_id: ids[1],
            })
            .success
    );
    assert_eq!(lengths(&state, line_id), vec![5.0, 9.0]);
    assert_round_trip(&state, &before, "delete the middle shot");
    assert!(state.undo().success);
    assert_eq!(lengths(&state, line_id), vec![5.0, 7.0, 9.0]);
    assert_eq!(
        shot_ids(&state, line_id),
        ids,
        "at its position, its own id"
    );
}

#[test]
fn test_multiple_undo_redo_sequence() {
    let state = new_state();
    let empty = snapshot(&state);
    let mut states = Vec::new();
    for i in 1..=3 {
        create(&state, &format!("Line {i}"), vec![]);
        states.push(snapshot(&state));
    }
    assert_eq!(project(&state).lines().len(), 3);
    for _ in 0..3 {
        assert!(state.undo().success);
    }
    assert!(project(&state).lines().is_empty());
    assert_same(&snapshot(&state), &empty, "all three undone");
    assert!(state.redo().success);
    assert!(state.redo().success);
    assert_eq!(project(&state).lines().len(), 2);
    assert_same(&snapshot(&state), &states[1], "two redone");
}

// ===========================================================================
// Command Execution Tests (fn-34-nvq spec)
// ===========================================================================

#[test]
fn test_execute_create_line_command() {
    let state = new_state();
    let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
    let r = state.execute(Command::CreateLine {
        name: Some("Main Passage".to_string()),
        surveyor: Some("Alice".to_string()),
        date: Some(date),
        color: "#00ff00".to_string(),
        direction: Some(SurveyDirection::Out),
        shots: vec![
            test_shot_data(10.0),
            test_shot_data(15.0),
            test_shot_data(8.0),
        ],
        cave_section_id: None,
    });
    assert!(r.success, "{:?}", r.error);
    let line_id = r.line_id.expect("Line ID should be returned");
    let p = project(&state);
    let line = p.line(line_id).expect("Line should exist");
    assert_eq!(line.name.as_deref(), Some("Main Passage"));
    assert_eq!(line.color, "#00ff00");
    assert_eq!(line.direction, SurveyDirection::Out);
    let expected = SurveyCredit {
        surveyors: vec![Person::named("Alice").unwrap()],
        date: Some(date),
        ..SurveyCredit::default()
    };
    for s in &line.shots {
        assert_eq!(
            s.credit.as_deref(),
            Some(&expected),
            "the surveyor and the date are every shot's credit"
        );
    }
    assert_eq!(
        line.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![10.0, 15.0, 8.0]
    );
}

#[test]
fn test_execute_merge_stations_command() {
    let state = new_state();
    let a = create(
        &state,
        "Line A",
        vec![test_shot_data(5.0), test_shot_data(7.0)],
    );
    let b = create(
        &state,
        "Line B",
        vec![test_shot_data(4.0), test_shot_data(6.0)],
    );
    let before = snapshot(&state);
    let stations_before = project(&state).stations.len();
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
        keep: None,
        renames: None,
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    let joined = station_at(&p, a, 1);
    assert_eq!(
        station_at(&p, b, 0),
        joined,
        "the merge is one station on both lines"
    );
    assert!(p.topology().is_junction(joined));
    assert_eq!(p.stations.len(), stations_before - 1);
    assert_close(
        xyz(&p, a, 1),
        xyz(&p, b, 0),
        "the moving line is placed on the merge",
    );
    assert_round_trip(&state, &before, "merge");
}

#[test]
fn test_execute_set_zero_datum_command() {
    let state = new_state();
    let line_id = create(&state, "Line", vec![test_shot_data(5.0)]);
    let before = snapshot(&state);
    let gps = GpsCoordinates::new(48.8584, 2.2945, 35.0).unwrap();
    let r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(line_id, 0),
        coordinates: gps,
        accuracy_m: None,
        entrance: None,
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_eq!(p.datums.len(), 1);
    let datum = &p.datums[0];
    assert_eq!(datum.station, station_at(&p, line_id, 0));
    assert!((datum.gps.latitude - 48.8584).abs() < 1e-4);
    assert!((datum.gps.longitude - 2.2945).abs() < 1e-4);
    assert!((datum.gps.depth_reference - 35.0).abs() < 1e-4);
    assert!(p.placements.is_empty(), "the datum places the piece now");
    assert_round_trip(&state, &before, "set zero datum");
}

/// 0.6.0 pinned that the undo restored the exact previous `line_offsets`,
/// key presence included. The placement of a datum-less piece is what an
/// offset was: the undo of a datum that replaced a placement puts the very
/// placement back (its station and coordinates) and takes the datum away.
#[test]
fn test_undo_set_zero_datum_restores_line_offsets() {
    let state = new_state();
    let a = create(&state, "A", vec![test_shot_data(10.0)]);
    let b = create(&state, "B", vec![test_shot_data(10.0)]);
    // Line A's piece sits away from the origin.
    tamper(&state, |p| {
        let start = p.line(a).unwrap().start;
        p.set_placement(start, 100.0, 200.0).unwrap();
    });
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 1),
                station_b: StationRef::new(b, 0),
                keep: None,
                renames: None,
            })
            .success
    );
    state.undo_stack.lock().unwrap().clear();
    let before = snapshot(&state);
    let placements_before = project(&state).placements;
    assert_eq!(placements_before.len(), 1, "one piece, one placement");
    let r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
        accuracy_m: None,
        entrance: None,
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert!(p.placements.is_empty(), "the datum replaced the placement");
    assert_eq!(p.datums.len(), 1);
    assert_round_trip(&state, &before, "set zero datum on a placed piece");
    assert!(state.undo().success);
    let p = project(&state);
    assert_eq!(p.placements, placements_before, "the placement is back");
    assert!(p.datums.is_empty(), "the datum is gone");
}

#[test]
fn test_execute_update_shot_command() {
    let state = new_state();
    let line_id = create(
        &state,
        "Line",
        vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 270.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 6.0,
            ..Default::default()
        }],
    );
    let shot_id = shot_ids(&state, line_id)[0];
    let before = snapshot(&state);
    let r = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            azimuth_in: Some(45.0),
            length: Some(20.0),
            left: Some(Some(1.5)),
            right: Some(Some(2.0)),
            up: Some(Some(3.0)),
            down: Some(Some(0.5)),
            comment: Some(Some("Narrow squeeze".to_string())),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    let s = &p.line(line_id).unwrap().shots[0];
    assert_eq!(s.azimuth_in, 45.0);
    assert_eq!(s.azimuth_out, 270.0, "unchanged");
    assert_eq!(s.length, 20.0);
    assert_eq!(s.depth_in, 5.0, "unchanged");
    assert_eq!(s.left, Some(1.5));
    assert_eq!(s.right, Some(2.0));
    assert_eq!(s.up, Some(3.0));
    assert_eq!(s.down, Some(0.5));
    assert_eq!(s.comment.as_deref(), Some("Narrow squeeze"));
    assert_round_trip(&state, &before, "update several fields");
}

/// A non-finite value hidden in a nested `Option<Option<f64>>` field is
/// refused, and atomically: the valid field beside it is not applied.
#[test]
fn test_update_shot_rejects_non_finite_nested_field_atomically() {
    let state = new_state();
    let line_id = create(
        &state,
        "Line",
        vec![ShotData {
            azimuth_in: 10.0,
            length: 5.0,
            ..Default::default()
        }],
    );
    let shot_id = shot_ids(&state, line_id)[0];
    let before = snapshot(&state);
    let r = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            azimuth_in: Some(45.0),
            pitch_in: Some(Some(f64::NAN)),
            ..Default::default()
        },
    });
    assert!(!r.success, "a non-finite nested field must be rejected");
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .contains("non-finite"),
        "{:?}",
        r.error
    );
    assert_same(&snapshot(&state), &before, "no partial apply");
    assert_eq!(state.get_undo_stack().len(), 1, "no entry");
}

#[test]
fn test_update_shot_rejects_positive_infinity() {
    let state = new_state();
    let line_id = create(
        &state,
        "Line",
        vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
    );
    let shot_id = shot_ids(&state, line_id)[0];
    let before = snapshot(&state);
    let r = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            left: Some(Some(f64::INFINITY)),
            ..Default::default()
        },
    });
    assert!(!r.success, "+inf must be rejected");
    assert!(r
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("non-finite"));
    assert_same(&snapshot(&state), &before, "poisoned left not written");
}

fn import_config(name: &str, shots: Vec<ShotData>, placement: ImportPlacement) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: None,
        reverse: false,
        line_index: 0,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#333333".to_string(),
        direction: None,
        quality: 1.0,
        shots,
        station_comments: Vec::new(),
        station_names: Vec::new(),
        station_external: Vec::new(),
        placement,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

fn import(
    state: &AppState,
    lines: Vec<ImportLineConfig>,
    updates: Vec<ExistingShotUpdate>,
) -> CommandResult {
    state.execute_import_lines(
        lines,
        Vec::new(),
        None,
        Vec::new(),
        updates,
        VirtualShotMode::Auto,
    )
}

/// Two existing-shot updates where the SECOND carries +inf: the import is
/// refused whole, so neither shot changes, not even the first (valid) one.
#[test]
fn test_import_shot_updates_reject_atomically_when_one_is_poisoned() {
    let state = new_state();
    let r = import(
        &state,
        vec![import_config(
            "Existing",
            vec![test_shot_data(5.0), test_shot_data(6.0)],
            ImportPlacement::Origin,
        )],
        Vec::new(),
    );
    assert!(r.success, "seed import: {:?}", r.error);
    let line_id = project(&state).lines()[0].id;
    let ids = shot_ids(&state, line_id);
    let before = snapshot(&state);
    let r = import(
        &state,
        vec![import_config(
            "Incoming",
            vec![test_shot_data(3.0)],
            ImportPlacement::Origin,
        )],
        vec![
            ExistingShotUpdate {
                line_id,
                shot_id: ids[0],
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            },
            ExistingShotUpdate {
                line_id,
                shot_id: ids[1],
                updates: ShotUpdates {
                    length: Some(f64::INFINITY),
                    ..Default::default()
                },
            },
        ],
    );
    assert!(
        !r.success,
        "a batch with one poisoned update must be rejected"
    );
    assert_same(&snapshot(&state), &before, "neither update applied");
    assert_eq!(state.get_undo_stack().len(), 1, "no entry");
}

/// A batch that creates more than one zero datum names the first line and
/// counts the rest on the receipt.
#[test]
fn multiple_zero_datum_placements_name_the_first_and_count_the_rest() {
    let state = new_state();
    let make = |name: &str| {
        import_config(
            name,
            vec![test_shot_data(5.0)],
            ImportPlacement::ZeroDatum {
                coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
                station_index: 0,
            },
        )
    };
    let r = import(
        &state,
        vec![make("First Anchor"), make("Second Anchor")],
        Vec::new(),
    );
    assert!(r.success, "{:?}", r.error);
    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert_eq!(
        receipt.zero_datum_created.as_deref(),
        Some("First Anchor (and 1 more)"),
        "must name the first line and count the rest"
    );
}

#[test]
fn import_with_no_zero_datum_placement_leaves_receipt_field_none() {
    let state = new_state();
    let r = import(
        &state,
        vec![import_config(
            "Plain",
            vec![test_shot_data(5.0)],
            ImportPlacement::Origin,
        )],
        Vec::new(),
    );
    assert!(r.success, "{:?}", r.error);
    let receipt = r.import_receipt.expect("import must carry a receipt");
    assert_eq!(receipt.zero_datum_created, None);
}

#[test]
fn test_execute_delete_shot_command() {
    let state = new_state();
    let line_id = create(
        &state,
        "Line",
        vec![
            test_shot_data(5.0),
            test_shot_data(7.0),
            test_shot_data(9.0),
        ],
    );
    let ids = shot_ids(&state, line_id);
    let before = snapshot(&state);
    let r = state.execute(Command::DeleteShot {
        line_id,
        shot_id: ids[1],
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(shot_ids(&state, line_id), vec![ids[0], ids[2]]);
    assert_eq!(lengths(&state, line_id), vec![5.0, 9.0]);
    assert!(
        project(&state).validate().is_empty(),
        "the line stays a chain"
    );
    assert_round_trip(&state, &before, "delete shot");
}

#[test]
fn test_execute_create_shot_command() {
    let state = new_state();
    let line_id = create(
        &state,
        "Line",
        vec![test_shot_data(5.0), test_shot_data(10.0)],
    );
    let first = shot_ids(&state, line_id)[0];
    let before = snapshot(&state);
    let r = state.execute(Command::CreateShot {
        line_id,
        shot: test_shot_data(7.5),
        after_shot_id: Some(first),
        before_shot_id: None,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(lengths(&state, line_id), vec![5.0, 7.5, 10.0]);
    assert_round_trip(&state, &before, "insert a shot");
}

#[test]
fn test_execute_delete_line_command() {
    let state = new_state();
    let a = create(&state, "Line A", vec![test_shot_data(5.0)]);
    let b = create(&state, "Line B", vec![test_shot_data(7.0)]);
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
                accuracy_m: None,
                entrance: None,
            })
            .success
    );
    let p = project(&state);
    assert_eq!(p.lines().len(), 2);
    assert_eq!(p.datums.len(), 1);
    let before = snapshot(&state);
    let r = state.execute(Command::DeleteLine { line_id: a });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_eq!(line_ids(&state), vec![b]);
    assert_eq!(p.lines()[0].name.as_deref(), Some("Line B"));
    assert!(
        p.datums.is_empty(),
        "the datum on the deleted line went with it"
    );
    assert!(p.validate().is_empty());
    assert_round_trip(&state, &before, "delete a datum line");
}

#[test]
fn test_execute_update_line_metadata_command() {
    let state = new_state();
    let initial = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
    let r = state.execute(Command::CreateLine {
        name: Some("Original Name".to_string()),
        surveyor: Some("Bob".to_string()),
        date: Some(initial),
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![test_shot_data(5.0)],
        cave_section_id: None,
    });
    let line_id = r.line_id.unwrap();
    let before = snapshot(&state);
    let updated = NaiveDate::from_ymd_opt(2024, 2, 20).unwrap();
    let r = state.execute(Command::UpdateLineMetadata {
        line_id,
        updates: LineUpdates {
            name: Some(Some("Updated Name".to_string())),
            surveyor: Some(Some("Carol".to_string())),
            date: Some(Some(updated)),
            color: Some("#0000ff".to_string()),
            direction: Some(SurveyDirection::Out),
            quality: Some(1.25),
        },
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    let line = p.line(line_id).unwrap();
    assert_eq!(line.name.as_deref(), Some("Updated Name"));
    let credit = line.shots[0].credit.as_deref().unwrap();
    assert_eq!(credit.surveyors, vec![Person::named("Carol").unwrap()]);
    assert_eq!(credit.date, Some(updated));
    assert_eq!(line.color, "#0000ff");
    assert_eq!(line.direction, SurveyDirection::Out);
    assert_eq!(line.quality, 1.25);
    assert_eq!(line.shots.len(), 1, "shots unaffected");
    assert_eq!(line.shots[0].length, 5.0);
    assert_round_trip(&state, &before, "update line metadata");
}

// ===========================================================================
// Undo/Redo Tests for fn-35-f4v.1
// ===========================================================================

#[test]
fn test_undo_delete_shot() {
    let state = new_state();
    let line_id = create(
        &state,
        "Line",
        vec![test_shot_data(5.0), test_shot_data(10.0)],
    );
    let ids = shot_ids(&state, line_id);
    assert!(
        state
            .execute(Command::DeleteShot {
                line_id,
                shot_id: ids[0],
            })
            .success
    );
    assert_eq!(lengths(&state, line_id), vec![10.0]);
    assert!(state.undo().success);
    assert_eq!(lengths(&state, line_id), vec![5.0, 10.0]);
    assert_eq!(shot_ids(&state, line_id), ids);
}

#[test]
fn test_undo_merge_stations() {
    let state = new_state();
    let a = create(
        &state,
        "Line A",
        vec![test_shot_data(5.0), test_shot_data(7.0)],
    );
    let b = create(
        &state,
        "Line B",
        vec![test_shot_data(4.0), test_shot_data(6.0)],
    );
    let p = project(&state);
    assert!(p
        .topology()
        .stations()
        .iter()
        .all(|&s| !p.topology().is_junction(s)));
    let before = snapshot(&state);
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 1),
                station_b: StationRef::new(b, 0),
                keep: None,
                renames: None,
            })
            .success
    );
    let p = project(&state);
    assert_eq!(station_at(&p, a, 1), station_at(&p, b, 0));
    assert!(state.undo().success);
    let p = project(&state);
    assert_ne!(
        station_at(&p, a, 1),
        station_at(&p, b, 0),
        "the merge is gone"
    );
    assert!(p
        .topology()
        .stations()
        .iter()
        .all(|&s| !p.topology().is_junction(s)));
    assert_same(&snapshot(&state), &before, "exactly");
}

#[test]
fn test_multiple_undo_sequence() {
    let state = new_state();
    for name in ["First", "Second", "Third"] {
        create(&state, name, vec![]);
    }
    for (left, gone) in [(2, "Third"), (1, "Second"), (0, "First")] {
        assert!(state.undo().success);
        let names = line_names(&state);
        assert_eq!(names.len(), left);
        assert!(!names.iter().any(|n| n == gone), "LIFO: {gone} first");
    }
}

#[test]
fn test_redo_clears_on_new_command() {
    let state = new_state();
    create(&state, "Original", vec![]);
    assert!(state.undo().success);
    assert!(state.can_redo());
    create(&state, "New", vec![]);
    assert!(!state.can_redo());
    assert_eq!(line_names(&state), ["New"]);
}

#[test]
fn test_undo_stack_limits() {
    let state = new_state();
    state.config.lock().unwrap().general.undo_history_limit = 3;
    state.undo_stack.lock().unwrap().set_history_limit(3);
    for i in 0..5 {
        create(&state, &format!("Line {i}"), vec![]);
    }
    assert_eq!(project(&state).lines().len(), 5);
    for left in [4, 3, 2] {
        assert!(state.undo().success);
        assert_eq!(project(&state).lines().len(), left);
    }
    assert!(!state.undo().success, "the limit dropped the oldest two");
    assert_eq!(line_names(&state), ["Line 0", "Line 1"]);
}

// ===========================================================================
// Undo/Redo Entry Preservation on Failure Tests (fn-28-rpu.2)
// ===========================================================================

/// One line with one shot of 5 m, and an edit of it to `to` m.
fn edited_line(state: &AppState, to: f64) -> (LineId, ShotId) {
    let line_id = create(state, "Line", vec![test_shot_data(5.0)]);
    let shot_id = shot_ids(state, line_id)[0];
    set_length(state, line_id, shot_id, to);
    (line_id, shot_id)
}

fn set_length(state: &AppState, line_id: LineId, shot_id: ShotId, length: f64) {
    let r = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(length),
            ..Default::default()
        },
    });
    assert!(r.success, "{:?}", r.error);
}

fn set_live_length(state: &AppState, line_id: LineId, index: usize, length: f64) {
    tamper(state, |p| {
        p.line_mut(line_id).unwrap().shots[index].length = length;
    });
}

#[test]
fn test_undo_failure_preserves_entry_on_undo_stack() {
    let state = new_state();
    let (line_id, _) = edited_line(&state, 15.0);
    state.undo_stack.lock().unwrap().clear();
    set_length(&state, line_id, shot_ids(&state, line_id)[0], 16.0);
    // The live value is no longer the one the entry recorded.
    set_live_length(&state, line_id, 0, 20.0);
    let before = snapshot(&state);
    let r = state.undo();
    assert!(!r.success);
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .starts_with("Undo failed: "),
        "{:?}",
        r.error
    );
    assert_same(&snapshot(&state), &before, "a refused step changes nothing");
    assert_eq!(state.get_undo_stack().len(), 1, "the entry stays");
    assert!(state.can_undo());
    assert!(!state.can_redo(), "nothing moved to the redo stack");
}

#[test]
fn test_redo_failure_preserves_entry_on_redo_stack() {
    let state = new_state();
    let (line_id, _) = edited_line(&state, 15.0);
    state.undo_stack.lock().unwrap().clear();
    set_length(&state, line_id, shot_ids(&state, line_id)[0], 16.0);
    assert!(state.undo().success);
    set_live_length(&state, line_id, 0, 20.0);
    let before = snapshot(&state);
    let r = state.redo();
    assert!(!r.success);
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .starts_with("Redo failed: "),
        "{:?}",
        r.error
    );
    assert_same(&snapshot(&state), &before, "a refused step changes nothing");
    assert_eq!(state.get_redo_stack().len(), 1, "the entry stays");
    assert!(state.can_redo());
    assert!(!state.can_undo(), "nothing moved to the undo stack");
}

#[test]
fn test_undo_failure_preserves_entry_at_top_lifo_order() {
    let state = new_state();
    let (line_id, shot_id) = edited_line(&state, 15.0);
    set_length(&state, line_id, shot_id, 16.0);
    let top = top_undo(&state);
    set_live_length(&state, line_id, 0, 20.0);
    assert!(!state.undo().success);
    assert_eq!(
        state.get_undo_stack().len(),
        3,
        "every entry is still there"
    );
    assert_eq!(top_undo(&state), top, "the refused entry is still on top");
}

#[test]
fn test_redo_failure_preserves_entry_at_top_lifo_order() {
    let state = new_state();
    let (line_id, shot_id) = edited_line(&state, 15.0);
    set_length(&state, line_id, shot_id, 16.0);
    assert!(state.undo().success);
    assert!(state.undo().success);
    let top = top_redo(&state);
    set_live_length(&state, line_id, 0, 20.0);
    assert!(!state.redo().success);
    assert_eq!(
        state.get_redo_stack().len(),
        2,
        "every entry is still there"
    );
    assert_eq!(top_redo(&state), top, "the refused entry is still on top");
}

/// Two lines: A (two shots, for a reverse) and B (one shot of 5 m), and
/// three entries: B to 6 m, B to 7 m and a reverse of A, the reverse last
/// (`reverse_first == false`: the first step of an undo jump) or first (the
/// first step of a redo jump).
fn three_entries(state: &AppState, reverse_first: bool) -> (LineId, LineId) {
    let a = create(state, "A", vec![test_shot_data(3.0), test_shot_data(4.0)]);
    let b = create(state, "B", vec![test_shot_data(5.0)]);
    state.undo_stack.lock().unwrap().clear();
    let reverse = || {
        let r = state.execute(Command::ReverseLine {
            line_id: a,
            swap_left_right: false,
        });
        assert!(r.success);
    };
    if reverse_first {
        reverse();
    }
    let shot = shot_ids(state, b)[0];
    set_length(state, b, shot, 6.0);
    set_length(state, b, shot, 7.0);
    if !reverse_first {
        reverse();
    }
    (a, b)
}

/// `undo_to` stops at the step that is refused, says which step of how many
/// and why, keeps the refused entry on top, and reports the reversed lines
/// of the steps it did apply (the frontend mirrors them).
#[test]
fn test_undo_to_partial_completion_stops_at_failed_step() {
    let state = new_state();
    let (a, b) = three_entries(&state, false);
    let refused_entry = state.get_undo_stack()[1].description.clone();
    // Step 2 (B back to 6 m) reads 7 m; the live value is 8 m.
    set_live_length(&state, b, 0, 8.0);
    let r = state.undo_to(2);
    assert!(!r.success);
    let error = r.error.as_deref().unwrap();
    assert!(
        error.starts_with("Undo failed at step 2 of 3: "),
        "the step count and the reason: {error}"
    );
    let reversed = r.reversed_lines.expect("the applied step's reverse");
    assert_eq!(reversed.len(), 1);
    assert_eq!((reversed[0].line_id, reversed[0].shot_count), (a, 2));
    // Step 1 landed, step 2 did not.
    assert_eq!(lengths(&state, a), vec![3.0, 4.0], "A is un-reversed");
    assert_eq!(lengths(&state, b), vec![8.0], "B is untouched");
    assert_eq!(state.get_undo_stack().len(), 2);
    assert_eq!(state.get_redo_stack().len(), 1);
    assert_eq!(
        state.get_undo_stack()[0].description,
        refused_entry,
        "the refused entry is on top"
    );
}

#[test]
fn test_redo_to_partial_completion_stops_at_failed_step() {
    let state = new_state();
    let (a, b) = three_entries(&state, true);
    assert!(state.undo_to(2).success);
    assert_eq!(lengths(&state, b), vec![5.0]);
    let refused_entry = state.get_redo_stack()[1].description.clone();
    // Redo step 2 (B to 6 m) reads 5 m; the live value is 9 m.
    set_live_length(&state, b, 0, 9.0);
    let r = state.redo_to(2);
    assert!(!r.success);
    let error = r.error.as_deref().unwrap();
    assert!(
        error.starts_with("Redo failed at step 2 of 3: "),
        "the step count and the reason: {error}"
    );
    let reversed = r.reversed_lines.expect("the applied step's reverse");
    assert_eq!((reversed[0].line_id, reversed[0].shot_count), (a, 2));
    assert_eq!(lengths(&state, a), vec![4.0, 3.0], "A is reversed again");
    assert_eq!(lengths(&state, b), vec![9.0], "B is untouched");
    assert_eq!(state.get_redo_stack().len(), 2);
    assert_eq!(state.get_undo_stack().len(), 1);
    assert_eq!(
        state.get_redo_stack()[0].description,
        refused_entry,
        "the refused entry is on top"
    );
}

/// The decoded-entry guard (plan Deviation U10): an entry read from a file
/// has no session ids, and one whose step passes its preconditions yet would
/// leave an invalid survey (only a crafted file can hold one) is rolled back
/// from the project kept before the step: refused, the project untouched,
/// the entry kept.
#[test]
fn a_decoded_entry_that_would_leave_an_invalid_survey_is_refused_and_kept() {
    let state = new_state();
    create(&state, "Line", vec![test_shot_data(5.0)]);
    state.undo_stack.lock().unwrap().clear();
    let before_project = project(&state);
    let mut invalid = before_project.clone();
    // A station on no line (invariant 4).
    let id = invalid.counters.next_station_id;
    invalid.insert_station(Station::new(id, "orphan")).unwrap();
    assert!(
        !invalid.validate().is_empty(),
        "setup: the target is invalid"
    );
    let mut changes = capture(&before_project, &invalid);
    changes.forget_session_ids();
    let entry = UndoEntry::new(HistoryEntry {
        timestamp: chrono::Utc::now(),
        kind: "batch".to_string(),
        label: "Crafted".to_string(),
        label_parts: Vec::new(),
        refs: HistoryRefs::default(),
        preview: RecordedPreview::default(),
        changes,
    })
    .unwrap();
    state.undo_stack.lock().unwrap().push_redo(entry);
    let before = snapshot(&state);
    let r = state.redo();
    assert!(!r.success);
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .contains("the step would leave the survey invalid"),
        "{:?}",
        r.error
    );
    assert_same(&snapshot(&state), &before, "rolled back exactly");
    assert_eq!(state.get_redo_stack().len(), 1, "the entry is kept");
    assert!(!state.can_undo());
}

/// The same guard when the inverse step would be refused (E-undo-pipeline-4):
/// a step checks only the side it reads, so a crafted entry can pair a
/// valid-looking target with a source side the inverse refuses (here the
/// undo side's length of an edited shot, 5 m, tampered to 5.5 m). The
/// rollback is the project kept aside before the step, so the survey is
/// left valid and exactly as it was: it still saves and takes edits.
#[test]
fn a_decoded_entry_whose_inverse_is_refused_is_still_rolled_back() {
    let state = new_state();
    let line_id = create(
        &state,
        "Line",
        vec![test_shot_data(5.0), test_shot_data(7.0)],
    );
    state.undo_stack.lock().unwrap().clear();
    let before_project = project(&state);
    let mut invalid = before_project.clone();
    let id = invalid.counters.next_station_id;
    invalid.insert_station(Station::new(id, "orphan")).unwrap();
    invalid.line_mut(line_id).unwrap().shots[0].length = 6.0;
    assert!(
        !invalid.validate().is_empty(),
        "setup: the target is invalid"
    );
    let mut changes = capture(&before_project, &invalid);
    changes.forget_session_ids();
    let entry = UndoEntry::new(HistoryEntry {
        timestamp: chrono::Utc::now(),
        kind: "batch".to_string(),
        label: "Crafted".to_string(),
        label_parts: Vec::new(),
        refs: HistoryRefs::default(),
        preview: RecordedPreview::default(),
        changes,
    })
    .unwrap();
    let text = entry.raw().as_str().to_string();
    let tampered = text.replacen("[5.0]", "[5.5]", 1);
    assert_ne!(
        tampered, text,
        "setup: the undo side's length is in the entry"
    );
    let decoded = UndoEntry::decode(
        &stalx_v2::RawJson::parse(&tampered).unwrap(),
        &crate::models::survey::changeset::EntryLimits::default(),
    )
    .expect("the crafted entry decodes");
    state.undo_stack.lock().unwrap().push_redo(decoded);

    let before = snapshot(&state);
    let r = state.redo();
    assert!(!r.success);
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .contains("the step would leave the survey invalid"),
        "{:?}",
        r.error
    );
    assert!(
        state.integrity_violation_messages().is_empty(),
        "the survey is left valid: {:?}",
        state.integrity_violation_messages()
    );
    assert_same(&snapshot(&state), &before, "rolled back exactly");
    assert_eq!(state.get_redo_stack().len(), 1, "the entry is kept");
    let dir = TempDir::new().unwrap();
    let save = state.execute(Command::SaveProject {
        path: Some(dir.path().join("kept.stalx")),
    });
    assert!(save.success, "it still saves: {:?}", save.error);
    let shot_id = shot_ids(&state, line_id)[1];
    let edit = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(8.0),
            ..Default::default()
        },
    });
    assert!(edit.success, "it still takes edits: {:?}", edit.error);
}

/// Moving a datum (a GPS edit of an existing datum) records the previous
/// GPS: the entry's changeset holds the datum before and after, and nothing
/// of a line (a datum edit moves no shot).
#[test]
fn test_set_zero_datum_update_captures_previous_gps_and_offsets() {
    let state = new_state();
    let a = create(&state, "A", vec![test_shot_data(10.0)]);
    let at = StationRef::new(a, 0);
    let set = |lat: f64, lon: f64| {
        let r = state.execute(Command::SetZeroDatum {
            station_ref: at,
            coordinates: GpsCoordinates::new(lat, lon, 50.0).unwrap(),
            accuracy_m: None,
            entrance: None,
        });
        assert!(r.success, "{:?}", r.error);
    };
    set(25.0, -80.0);
    state.undo_stack.lock().unwrap().clear();
    set(26.0, -81.0);
    let stack = state.undo_stack.lock().unwrap();
    let digest = stack.peek_undo().unwrap().entry().changes.digest();
    drop(stack);
    assert_eq!(digest.datums.removed.len(), 1);
    assert_eq!(digest.datums.added.len(), 1);
    let (old, new) = (&digest.datums.removed[0].gps, &digest.datums.added[0].gps);
    assert_eq!((old.latitude, old.longitude), (25.0, -80.0), "previous GPS");
    assert_eq!((new.latitude, new.longitude), (26.0, -81.0));
    assert!(
        digest.lines_created.is_empty()
            && digest.lines_deleted.is_empty()
            && digest.shot_cells.is_empty(),
        "no line is part of a datum move"
    );
}

#[test]
fn test_set_zero_datum_update_undo_restores_previous_gps_and_offsets() {
    let state = new_state();
    let a = create(&state, "A", vec![test_shot_data(10.0)]);
    let at = StationRef::new(a, 0);
    let set = |lat: f64, lon: f64| {
        let r = state.execute(Command::SetZeroDatum {
            station_ref: at,
            coordinates: GpsCoordinates::new(lat, lon, 50.0).unwrap(),
            accuracy_m: None,
            entrance: None,
        });
        assert!(r.success, "{:?}", r.error);
    };
    set(25.0, -80.0);
    let before = snapshot(&state);
    set(26.0, -81.0);
    assert_eq!(project(&state).datums[0].gps.latitude, 26.0);
    assert_round_trip(&state, &before, "move the datum");
    assert!(state.undo().success);
    let gps = project(&state).datums[0].gps;
    assert_eq!((gps.latitude, gps.longitude), (25.0, -80.0));
}

/// 0.6.0 always recorded an entry for a SetZeroDatum, even one that changed
/// nothing (the no-change guard was the dialog's). The pipeline records
/// what changed (plan 8.1): an edit to the same GPS is a provable no-op, so
/// it succeeds, records nothing and keeps the redo branch.
#[test]
fn test_set_zero_datum_no_change_records_no_entry() {
    let state = new_state();
    let a = create(&state, "A", vec![test_shot_data(10.0)]);
    let command = || Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(25.0, -80.0, 50.0).unwrap(),
        accuracy_m: None,
        entrance: None,
    };
    assert!(state.execute(command()).success);
    create(&state, "B", vec![]);
    assert!(state.undo().success);
    let depth = state.get_undo_stack().len();
    let before = snapshot(&state);
    let r = state.execute(command());
    assert!(r.success, "the same GPS again succeeds: {:?}", r.error);
    assert_eq!(state.get_undo_stack().len(), depth, "and records nothing");
    assert!(state.can_redo(), "the redo branch is kept");
    assert_same(&snapshot(&state), &before, "nothing changed");
}

// ===========================================================================
// CreateLine undo/redo ID preservation tests
// ===========================================================================

#[test]
fn test_create_line_undo_redo_preserves_id() {
    let state = new_state();
    let line_id = create(
        &state,
        "ID Preservation",
        vec![test_shot_data(5.0), test_shot_data(10.0)],
    );
    let ids = shot_ids(&state, line_id);
    assert_eq!(ids.len(), 2);
    assert!(state.undo().success);
    assert!(project(&state).lines().is_empty());
    assert!(state.redo().success);
    assert_eq!(line_ids(&state), vec![line_id], "the line id is preserved");
    assert_eq!(line_names(&state), ["ID Preservation"]);
    assert_eq!(shot_ids(&state, line_id), ids, "the shot ids are preserved");
}

#[test]
fn test_create_line_double_undo_redo_cycle() {
    let state = new_state();
    let line_id = create(
        &state,
        "Double Cycle",
        vec![
            test_shot_data(3.0),
            test_shot_data(7.0),
            test_shot_data(11.0),
        ],
    );
    let after = snapshot(&state);
    let ids = shot_ids(&state, line_id);
    for cycle in 0..2 {
        assert!(state.undo().success);
        assert!(project(&state).lines().is_empty());
        assert!(state.redo().success);
        assert_eq!(line_ids(&state), vec![line_id], "cycle {cycle}");
        assert_eq!(shot_ids(&state, line_id), ids, "cycle {cycle}");
        assert_same(&snapshot(&state), &after, "cycle {cycle}");
    }
}

#[test]
fn test_create_line_undo_redo_then_edit() {
    let state = new_state();
    let line_id = create(&state, "Editable", vec![test_shot_data(5.0)]);
    assert!(state.undo().success);
    assert!(state.redo().success);
    let shot_id = shot_ids(&state, line_id)[0];
    set_length(&state, line_id, shot_id, 99.0);
    assert_eq!(lengths(&state, line_id), vec![99.0]);
}

#[test]
fn test_create_line_undo_redo_then_delete() {
    let state = new_state();
    let line_id = create(&state, "Deletable", vec![]);
    assert!(state.undo().success);
    assert!(state.redo().success);
    assert!(state.execute(Command::DeleteLine { line_id }).success);
    assert!(project(&state).lines().is_empty());
}

/// 0.6.0 pinned the `UndoData` variant of each command
/// (`CreatedLineSnapshot`, `Delta`). An entry is one changeset for every
/// command now (plan Deviation U1); what is left to pin is what the history
/// shows the entry as: its kind (the icon) and that it names the line.
#[test]
fn create_line_and_delete_line_entries_name_their_kind_and_line() {
    let state = new_state();
    let line_id = create(&state, "Check Variant", vec![test_shot_data(1.0)]);
    let item = &state.get_undo_stack()[0];
    assert_eq!(item.command_type, "create_line");
    assert!(
        item.description.contains("Check Variant"),
        "{}",
        item.description
    );
    assert!(state.execute(Command::DeleteLine { line_id }).success);
    let item = &state.get_undo_stack()[0];
    assert_eq!(item.command_type, "delete_line");
    assert!(
        item.description.contains("Check Variant"),
        "{}",
        item.description
    );
}

#[test]
fn test_delete_line_undo_redo_still_works() {
    let state = new_state();
    let line_id = create(&state, "Delete Test", vec![test_shot_data(5.0)]);
    let before = snapshot(&state);
    assert!(state.execute(Command::DeleteLine { line_id }).success);
    let after = snapshot(&state);
    assert!(project(&state).lines().is_empty());
    assert!(state.undo().success);
    assert_eq!(line_ids(&state), vec![line_id]);
    assert_eq!(line_names(&state), ["Delete Test"]);
    assert_same(&snapshot(&state), &before, "undo");
    assert!(state.redo().success);
    assert_same(&snapshot(&state), &after, "redo");
    assert!(state.undo().success);
    assert_same(&snapshot(&state), &before, "undo again");
}

// ===========================================================================
// CreateShot undo/redo ID preservation tests
// ===========================================================================

#[test]
fn test_create_shot_undo_redo_preserves_id() {
    let state = new_state();
    let line_id = create(&state, "Shot Test", vec![test_shot_data(5.0)]);
    let first = shot_ids(&state, line_id)[0];
    let r = state.execute(Command::CreateShot {
        line_id,
        shot: test_shot_data(10.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(r.success);
    let ids = shot_ids(&state, line_id);
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[1], first);
    assert_eq!(r.shot_id, Some(ids[1]), "the new shot's id is returned");
    assert!(state.undo().success);
    assert_eq!(shot_ids(&state, line_id), vec![first]);
    assert!(state.redo().success);
    assert_eq!(shot_ids(&state, line_id), ids, "the shot id is preserved");
    assert_eq!(lengths(&state, line_id), vec![5.0, 10.0]);
}

#[test]
fn test_create_shot_mid_line_undo_redo_preserves_position() {
    let state = new_state();
    let line_id = create(
        &state,
        "Mid Insert",
        vec![
            test_shot_data(1.0),
            test_shot_data(2.0),
            test_shot_data(3.0),
        ],
    );
    let original = shot_ids(&state, line_id);
    let before = snapshot(&state);
    assert!(
        state
            .execute(Command::CreateShot {
                line_id,
                shot: test_shot_data(99.0),
                after_shot_id: Some(original[0]),
                before_shot_id: None,
            })
            .success
    );
    let inserted = shot_ids(&state, line_id);
    assert_eq!(inserted.len(), 4);
    assert_eq!(
        [inserted[0], inserted[2], inserted[3]],
        [original[0], original[1], original[2]]
    );
    assert_eq!(lengths(&state, line_id)[1], 99.0);
    assert!(state.undo().success);
    assert_eq!(shot_ids(&state, line_id), original);
    assert_same(&snapshot(&state), &before, "back to the original order");
    assert!(state.redo().success);
    assert_eq!(shot_ids(&state, line_id), inserted, "same position and id");
    assert_eq!(lengths(&state, line_id)[1], 99.0);
}

#[test]
fn test_create_shot_double_undo_redo_cycle() {
    let state = new_state();
    let line_id = create(&state, "Double Cycle", vec![test_shot_data(5.0)]);
    let r = state.execute(Command::CreateShot {
        line_id,
        shot: test_shot_data(20.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(r.success);
    let created = shot_ids(&state, line_id)[1];
    for cycle in 0..2 {
        assert!(state.undo().success);
        assert_eq!(shot_ids(&state, line_id).len(), 1, "cycle {cycle}");
        assert!(state.redo().success);
        assert_eq!(shot_ids(&state, line_id)[1], created, "cycle {cycle}");
    }
    assert_eq!(lengths(&state, line_id)[1], 20.0);
}

#[test]
fn test_create_shot_undo_redo_then_edit() {
    let state = new_state();
    let line_id = create(&state, "Edit After Redo", vec![]);
    assert!(
        state
            .execute(Command::CreateShot {
                line_id,
                shot: test_shot_data(15.0),
                after_shot_id: None,
                before_shot_id: None,
            })
            .success
    );
    let shot_id = shot_ids(&state, line_id)[0];
    assert!(state.undo().success);
    assert!(state.redo().success);
    set_length(&state, line_id, shot_id, 42.0);
    assert_eq!(lengths(&state, line_id), vec![42.0]);
}

#[test]
fn test_create_shot_undo_redo_then_delete() {
    let state = new_state();
    let line_id = create(&state, "Delete After Redo", vec![]);
    assert!(
        state
            .execute(Command::CreateShot {
                line_id,
                shot: test_shot_data(15.0),
                after_shot_id: None,
                before_shot_id: None,
            })
            .success
    );
    let shot_id = shot_ids(&state, line_id)[0];
    assert!(state.undo().success);
    assert!(state.redo().success);
    let r = state.execute(Command::DeleteShot { line_id, shot_id });
    assert!(r.success, "{:?}", r.error);
    assert!(shot_ids(&state, line_id).is_empty());
}

/// 0.6.0 pinned `CreatedShotSnapshot` against `Delta`; see
/// [`create_line_and_delete_line_entries_name_their_kind_and_line`].
#[test]
fn create_shot_and_delete_shot_entries_name_their_kind() {
    let state = new_state();
    let line_id = create(&state, "Variant Check", vec![test_shot_data(5.0)]);
    let r = state.execute(Command::CreateShot {
        line_id,
        shot: test_shot_data(10.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(r.success);
    assert_eq!(state.get_undo_stack()[0].command_type, "create_shot");
    let shot_id = r.shot_id.unwrap();
    assert!(
        state
            .execute(Command::DeleteShot { line_id, shot_id })
            .success
    );
    assert_eq!(state.get_undo_stack()[0].command_type, "delete_shot");
}

#[test]
fn test_insert_shot_before_first_position_and_undo_redo() {
    let state = new_state();
    let line_id = create(
        &state,
        "Before Insert",
        vec![test_shot_data(10.0), test_shot_data(20.0)],
    );
    let first = shot_ids(&state, line_id)[0];
    let r = state.execute(Command::CreateShot {
        line_id,
        shot: test_shot_data(5.0),
        after_shot_id: None,
        before_shot_id: Some(first),
    });
    assert!(r.success, "{:?}", r.error);
    let new_shot = r.shot_id.expect("shot_id must be returned on success");
    assert_eq!(lengths(&state, line_id), vec![5.0, 10.0, 20.0]);
    assert_eq!(shot_ids(&state, line_id)[0], new_shot);
    assert!(state.undo().success);
    assert_eq!(lengths(&state, line_id), vec![10.0, 20.0]);
    assert!(state.redo().success);
    assert_eq!(lengths(&state, line_id), vec![5.0, 10.0, 20.0]);
    assert_eq!(shot_ids(&state, line_id)[0], new_shot, "same id");
}

#[test]
fn test_prepend_shot_undo_preserves_geometry() {
    let state = new_state();
    let line_id = create(&state, "Prepend Geometry", vec![level(90.0, 10.0)]);
    let p = project(&state);
    let (s0, s1) = (xyz(&p, line_id, 0), xyz(&p, line_id, 1));
    let first = shot_ids(&state, line_id)[0];
    let before = snapshot(&state);
    let r = state.execute(Command::CreateShot {
        line_id,
        shot: level(90.0, 5.0),
        after_shot_id: None,
        before_shot_id: Some(first),
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_close(xyz(&p, line_id, 1), s0, "the old start stays put");
    assert_close(xyz(&p, line_id, 2), s1, "the old end stays put");
    assert!(state.undo().success);
    let p = project(&state);
    assert_close(xyz(&p, line_id, 0), s0, "undo: start");
    assert_close(xyz(&p, line_id, 1), s1, "undo: end");
    assert_same(&snapshot(&state), &before, "undo of a prepend");
}

#[test]
fn test_create_shot_rejects_both_after_and_before() {
    let state = new_state();
    let line_id = create(
        &state,
        "Reject Both",
        vec![test_shot_data(10.0), test_shot_data(20.0)],
    );
    let ids = shot_ids(&state, line_id);
    let r = state.execute(Command::CreateShot {
        line_id,
        shot: test_shot_data(5.0),
        after_shot_id: Some(ids[0]),
        before_shot_id: Some(ids[1]),
    });
    assert!(!r.success);
    assert!(r.error.as_deref().unwrap().contains("Cannot specify both"));
    assert_eq!(state.get_undo_stack().len(), 1, "no entry");
}

#[test]
fn test_delete_shot_undo_redo_still_works() {
    let state = new_state();
    let line_id = create(
        &state,
        "Delete Shot Regression",
        vec![test_shot_data(5.0), test_shot_data(10.0)],
    );
    let shot_id = shot_ids(&state, line_id)[0];
    let before = snapshot(&state);
    assert!(
        state
            .execute(Command::DeleteShot { line_id, shot_id })
            .success
    );
    assert_eq!(lengths(&state, line_id), vec![10.0]);
    assert!(state.undo().success);
    assert_eq!(shot_ids(&state, line_id)[0], shot_id);
    assert_eq!(lengths(&state, line_id), vec![5.0, 10.0]);
    assert!(state.redo().success);
    assert_eq!(lengths(&state, line_id), vec![10.0]);
    assert!(state.undo().success);
    assert_same(&snapshot(&state), &before, "undo again");
}

// ===========================================================================
// Batch Tests / Coverage gap: batch edge cases
// ===========================================================================

#[test]
fn test_batch_rollback_on_failure() {
    let state = new_state();
    create(&state, "Existing", vec![test_shot_data(5.0)]);
    let before = snapshot(&state);
    let r = state.execute(Command::Batch {
        commands: vec![
            create_line_command("New", "#00ff00", vec![]),
            Command::MoveLine {
                line_id: 99_999,
                offset: Coordinates::new(1.0, 1.0, 0.0),
            },
        ],
        label: None,
    });
    assert!(!r.success, "Batch should fail");
    assert_same(&snapshot(&state), &before, "rolled back exactly");
    assert_eq!(state.get_undo_stack().len(), 1, "no entry for the batch");
}

/// A command the batch allowlist does not admit is refused with 0.6.0's
/// words, naming the command, before anything runs.
#[test]
fn test_batch_disallowed_command() {
    let state = new_state();
    let before = snapshot(&state);
    let r = state.execute(Command::Batch {
        commands: vec![Command::OpenProject {
            path: PathBuf::from("/nonexistent"),
        }],
        label: None,
    });
    assert!(!r.success, "Batch should reject non-allowlisted commands");
    let error = r.error.unwrap_or_default();
    assert!(
        error.starts_with("Command '") && error.ends_with("' is not allowed in batch context"),
        "the 0.6.0 refusal: {error}"
    );
    assert_same(&snapshot(&state), &before, "nothing changed");
}

#[test]
fn test_batch_empty_is_noop() {
    let state = new_state();
    create(&state, "Line", vec![]);
    assert!(state.undo().success);
    let r = state.execute(Command::Batch {
        commands: vec![],
        label: None,
    });
    assert!(r.success);
    assert!(state.get_undo_stack().is_empty(), "no entry");
    assert!(state.can_redo(), "and the redo branch is kept");
}

#[test]
fn test_batch_disallowed_command_save_project() {
    let state = new_state();
    let r = state.execute(Command::Batch {
        commands: vec![Command::SaveProject {
            path: Some(PathBuf::from("/tmp/test.stalx")),
        }],
        label: None,
    });
    assert!(!r.success);
    assert!(r
        .error
        .unwrap_or_default()
        .ends_with("is not allowed in batch context"));
}

#[test]
fn test_batch_partial_failure_rolls_back() {
    let state = new_state();
    let before = snapshot(&state);
    let r = state.execute(Command::Batch {
        commands: vec![
            create_line_command("NewSec", "#ff0000", vec![test_shot_data(5.0)]),
            Command::MergeStations {
                station_a: StationRef::new(9999, 0),
                station_b: StationRef::new(9998, 0),
                keep: None,
                renames: None,
            },
        ],
        label: None,
    });
    assert!(!r.success);
    assert_same(&snapshot(&state), &before, "rolled back exactly");
}

/// Withdrawing an abandoned import (#394) removes N lines as ONE undo entry,
/// and one undo brings every one of them back.
#[test]
fn batched_line_deletes_are_one_undo_entry() {
    let state = new_state();
    let a = create(&state, "A", vec![test_shot_data(5.0)]);
    let b = create(&state, "B", vec![test_shot_data(5.0)]);
    let c = create(&state, "C", vec![test_shot_data(5.0)]);
    let before = snapshot(&state);
    let depth = state.get_undo_stack().len();
    let r = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteLine { line_id: a },
            Command::DeleteLine { line_id: b },
        ],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(line_ids(&state), vec![c]);
    assert_eq!(state.get_undo_stack().len(), depth + 1, "ONE entry");
    assert!(state.undo().success);
    assert_eq!(line_ids(&state), vec![a, b, c]);
    assert_same(&snapshot(&state), &before, "every line, exactly");
}

/// Reverse a line and then delete it in one batch: the reverse is reported
/// to nobody (the line is gone), and one undo restores it all.
#[test]
fn a_batch_may_reverse_a_line_and_then_delete_it() {
    let state = new_state();
    let doomed = create(
        &state,
        "Doomed",
        vec![test_shot_data(5.0), test_shot_data(7.0)],
    );
    let keeper = create(&state, "Keeper", vec![test_shot_data(9.0)]);
    let before = snapshot(&state);
    let r = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseLine {
                line_id: doomed,
                swap_left_right: false,
            },
            Command::DeleteLine { line_id: doomed },
        ],
        label: None,
    });
    assert!(r.success, "mixed batch: {:?}", r.error);
    assert!(
        r.reversed_lines
            .as_ref()
            .is_none_or(|entries| entries.iter().all(|e| e.line_id != doomed)),
        "a line deleted in the same batch must not be reported as reversed: {:?}",
        r.reversed_lines
    );
    assert_eq!(line_ids(&state), vec![keeper]);
    assert!(state.undo().success);
    assert_eq!(line_ids(&state), vec![doomed, keeper]);
    assert_same(&snapshot(&state), &before, "one undo restores it all");
}

/// The other order is a clean refusal: the line is gone when the reverse
/// runs, and the batch rolls the delete back.
#[test]
fn a_batch_that_reverses_a_line_after_deleting_it_rolls_back() {
    let state = new_state();
    let line_id = create(&state, "A", vec![test_shot_data(5.0)]);
    let before = snapshot(&state);
    let r = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteLine { line_id },
            Command::ReverseLine {
                line_id,
                swap_left_right: false,
            },
        ],
        label: None,
    });
    assert!(!r.success);
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .contains("no longer exists"),
        "{:?}",
        r.error
    );
    assert_same(&snapshot(&state), &before, "the delete rolled back");
}

/// A batch that adds two zero datums places the SECOND against the frame
/// the FIRST established: the first datum's station lands on local (0, 0)
/// and the second at its GPS offset from the first.
#[test]
fn test_batch_second_zero_datum_lands_on_the_origin_the_first_established() {
    let state = new_state();
    let a = create(&state, "A", vec![level(0.0, 10.0)]);
    let b = create(&state, "B", vec![level(0.0, 4.0)]);
    let gps_a = GpsCoordinates::new(20.4, -87.3, 0.0).unwrap();
    let gps_b = GpsCoordinates::new(20.4, -87.295, 0.0).unwrap();
    let expected_b = crate::models::gps_to_local(&gps_b, &gps_a);
    assert!(expected_b.0.hypot(expected_b.1) > 100.0, "setup: far apart");
    let a1 = xyz(&project(&state), a, 1);
    assert!(
        (a1[1] - 10.0).abs() < 1e-9,
        "setup: A's station 1 is off the origin"
    );
    let before = snapshot(&state);
    let r = state.execute(Command::Batch {
        commands: vec![
            Command::SetZeroDatum {
                station_ref: StationRef::new(a, 1),
                coordinates: gps_a,
                accuracy_m: None,
                entrance: None,
            },
            Command::SetZeroDatum {
                station_ref: StationRef::new(b, 0),
                coordinates: gps_b,
                accuracy_m: None,
                entrance: None,
            },
        ],
        label: None,
    });
    assert!(r.success, "batch failed: {:?}", r.error);
    let p = project(&state);
    let a1 = xyz(&p, a, 1);
    assert!(
        a1[0].abs() < 1e-6 && a1[1].abs() < 1e-6,
        "the first datum establishes the frame: {a1:?}"
    );
    let b0 = xyz(&p, b, 0);
    assert!(
        (b0[0] - expected_b.0).abs() < 1e-6 && (b0[1] - expected_b.1).abs() < 1e-6,
        "the second datum sits at its GPS offset from the first, {expected_b:?}, got {b0:?}"
    );
    assert_round_trip(&state, &before, "two datums in one batch");
}

// ===========================================================================
// Undo-after-restart end-to-end test
// ===========================================================================

#[test]
fn test_undo_actually_works_after_project_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("undo-restart.stalx");
    let line_id;
    let before;
    {
        let state = new_state();
        before = snapshot(&state);
        line_id = create(&state, "Main", vec![test_shot_data(5.0)]);
        save_as(&state, &path);
    }
    let state = open(&path);
    assert_eq!(state.get_undo_stack().len(), 1, "the history is restored");
    assert!(!state.history_discarded());
    assert!(state.undo().success, "undo works after the reopen");
    assert!(
        !line_ids(&state).contains(&line_id),
        "the line is undone after the reopen"
    );
    assert_same(
        &snapshot(&state),
        &before,
        "the reopened undo lands exactly",
    );
}

/// An entry is recorded within the limits the next open reads it with:
/// more references than an entry may carry (an Add of thousands of lines), a
/// long name in the label and preview, and more preview rows are cut, so a
/// reopen loads the history instead of dropping it whole (review finding
/// E-undo-pipeline-3). The changeset itself is untouched: the entry steps.
#[test]
fn a_recorded_entry_always_reads_back_on_the_next_open() {
    use crate::models::survey::changeset::{EntryLimits, PreviewRow};

    let state = new_state();
    let line_id = create(&state, "Line", vec![test_shot_data(5.0)]);
    let limits = EntryLimits::default();
    {
        let mut stack = state.undo_stack.lock().unwrap();
        let mut entry = stack.pop_undo().unwrap().entry().clone();
        entry.refs.lines = (0..limits.max_refs as u32 + 5).collect();
        entry.label = "L".repeat(limits.max_text_chars + 10);
        entry.preview.rows = (0..limits.max_preview_rows + 3)
            .map(|i| PreviewRow {
                field: format!("row {i}"),
                old: "x".repeat(limits.max_text_chars + 1),
                new: String::new(),
            })
            .collect();
        stack.record(entry).unwrap();
        let mut reread = crate::core::UndoStack::new();
        reread
            .load_persisted(&stack.persisted(), &limits)
            .expect("the next open reads the history");
        assert_eq!(reread.len(), 1);
        let back = reread.undo_entries().next().unwrap().entry().clone();
        assert_eq!(back.refs.lines.len(), limits.max_refs);
        assert_eq!(back.label.chars().count(), limits.max_text_chars);
        assert_eq!(back.preview.rows.len(), limits.max_preview_rows);
        assert_eq!(back.preview.more, 3);
    }
    assert!(state.undo().success, "the entry still steps");
    assert!(project(&state).line(line_id).is_none());
}
