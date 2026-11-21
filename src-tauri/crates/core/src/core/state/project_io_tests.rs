//! Project lifecycle and the file round trip on the station-graph core: create,
//! open, close, save, the persisted history, the guard, the read-only pieces
//! of the save path. Ported from `state/tests.rs` "Project Operations Tests"
//! and "Coverage gap tests: project_io.rs" (the cases whose behaviour still
//! applies; the report of package C13 lists the retired ones with reasons).
//!
//! All data is synthetic: projects built in memory and saved by this build.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test module: panics on a failed assumption are the point; shot fields are \
              compared bit for bit"
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tempfile::TempDir;

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{ShotData, ShotUpdates, SpeleoLink, UndoMode};
use crate::persistence::stalx_v2::{self, Decoded, Extras, Mode, SpeleoDbLink};

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

/// One line of two shots, placed.
fn survey(name: &str) -> Project {
    let mut p = Project::new(name, None);
    let line = p
        .add_line(
            LineMeta {
                name: Some("Main".to_string()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![test_shot_data(5.0), test_shot_data(3.0)],
        )
        .unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    p
}

fn write_v2(path: &Path, project: &Project, extras: &Extras) {
    std::fs::write(
        path,
        stalx_v2::encode(project, extras, Mode::Normal).unwrap(),
    )
    .unwrap();
}

fn linked(undo_mode: Option<UndoMode>) -> Extras {
    Extras {
        speleodb: Some(SpeleoDbLink {
            endpoint: "https://sdb.example".to_string(),
            project_id: "11111111-2222-3333-4444-555555555555".to_string(),
            project_name: "Linked".to_string(),
            undo_mode,
        }),
        ..Extras::default()
    }
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "{:?}", r.error);
    state
}

fn decoded(path: &Path) -> Decoded {
    stalx_v2::decode(
        &std::fs::read(path).unwrap(),
        &stalx_v2::DecodeOptions::default(),
    )
    .unwrap()
}

/// Set the first shot's length (one undo step).
fn set_length(state: &AppState, length: f64) -> CommandResult {
    let (line_id, shot_id) = {
        let p = state.get_project_state().unwrap();
        (p.lines()[0].id, p.lines()[0].shots[0].id)
    };
    state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(length),
            ..ShotUpdates::default()
        },
    })
}

fn first_length(state: &AppState) -> f64 {
    state.get_project_state().unwrap().lines()[0].shots[0].length
}

// === Project operations ===

#[test]
fn creating_a_project_clears_a_previous_projects_discarded_history_flag() {
    let state = AppState::new().unwrap();
    state.set_history_discarded(true);

    let result = state.execute(Command::CreateProject {
        name: "Fresh Cave".to_string(),
        location: None,
    });

    assert!(result.success);
    assert!(
        !state.history_discarded(),
        "a project created from nothing has no persisted history, so it cannot have lost any"
    );
}

/// ...and closing one does too, so the banner cannot outlive the project it describes.
#[test]
fn closing_a_project_clears_the_discarded_history_flag() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Doomed".to_string(),
        location: None,
    });
    state.set_history_discarded(true);

    state.execute(Command::CloseProject);

    assert!(!state.history_discarded());
}

#[test]
fn test_create_project() {
    let state = AppState::new().unwrap();

    let result = state.execute(Command::CreateProject {
        name: "Test Cave".to_string(),
        location: Some("Test Location".to_string()),
    });

    assert!(result.success);
    assert!(result.error.is_none());
    let project = state.get_project_state().unwrap();
    assert_eq!(project.name, "Test Cave");
    assert_eq!(project.location, Some("Test Location".to_string()));
    assert!(project.lines().is_empty());
    assert!(!state.has_unsaved_changes(), "nothing to save yet");
}

#[test]
fn test_create_project_without_location() {
    let state = AppState::new().unwrap();
    let result = state.execute(Command::CreateProject {
        name: "No Location".to_string(),
        location: None,
    });
    assert!(result.success);
    assert_eq!(state.get_project_state().unwrap().location, None);
}

/// A new project's validation thresholds are the user's global ones.
#[test]
fn a_created_project_takes_the_global_validation_thresholds() {
    let state = AppState::new().unwrap();
    let mut validation = state.get_config().validation;
    validation.warn_shot_length_max = 77.0;
    state.config.lock().unwrap().validation = validation.clone();

    state.execute(Command::CreateProject {
        name: "Thresholds".to_string(),
        location: None,
    });

    assert_eq!(
        state.get_project_state().unwrap().settings.validation,
        validation
    );
}

#[test]
fn test_start_new_load_cancels_previous_and_clear_is_identity_safe() {
    let state = AppState::new().unwrap();

    let first = state.start_new_load();
    assert!(!first.load(Ordering::SeqCst));

    let second = state.start_new_load();
    assert!(first.load(Ordering::SeqCst));
    assert!(!second.load(Ordering::SeqCst));

    state.clear_active_load(&first);
    let guard = state.active_loading.lock().unwrap();
    assert!(guard.as_ref().is_some());
    assert!(Arc::ptr_eq(guard.as_ref().unwrap(), &second));
    drop(guard);

    state.clear_active_load(&second);
    assert!(state.active_loading.lock().unwrap().is_none());
}

/// The file state replaced the database handle; a poisoned lock on it must
/// not stop a close.
#[test]
fn test_close_project_succeeds_with_a_poisoned_file_state_lock() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = state.file.lock().unwrap();
        panic!("poison the file-state lock");
    }));

    let result = state.close_project();

    assert!(result.success);
    assert!(state.get_project_state().is_none());
}

#[test]
fn test_close_project() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    let result = state.execute(Command::CloseProject);
    assert!(result.success);
    assert!(state.get_project_state().is_none());
}

#[test]
fn test_close_project_clears_all_state() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("close.stalx");
    write_v2(&path, &survey("ToClose"), &Extras::default());
    let state = open(&path);
    assert!(set_length(&state, 6.0).success);
    assert!(state.get_adjusted_stations().is_some());
    assert!(state.derived.lock().unwrap().is_some(), "the cache is warm");

    let result = state.close_project();

    assert!(result.success);
    assert!(state.get_project_state().is_none());
    assert_eq!(state.undo_stack.lock().unwrap().len(), 0);
    assert!(
        state.derived.lock().unwrap().is_none(),
        "derived views dropped"
    );
    assert!(!state.has_unsaved_changes());
    assert_eq!(state.open_project_path(), None);
}

#[test]
fn test_open_project_missing_file() {
    let state = AppState::new().unwrap();
    let result = state.open_project(PathBuf::from("/nonexistent/path/project.stalx"));
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("File not found"));
}

#[test]
fn test_open_project_fails_on_invalid_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("invalid.stalx");
    std::fs::write(&path, b"not a database").unwrap();

    let result = AppState::new().unwrap().open_project(path.clone());

    assert!(!result.success);
    assert_eq!(std::fs::read(&path).unwrap(), b"not a database");
}

#[test]
fn test_open_project_clears_previous_state() {
    let dir = TempDir::new().unwrap();
    let path_a = dir.path().join("a.stalx");
    write_v2(&path_a, &survey("Project A"), &Extras::default());
    let mut b = survey("Project B");
    b.add_line(LineMeta::default(), StartAt::New, vec![test_shot_data(4.0)])
        .unwrap();
    let start = b.lines()[1].start;
    b.set_placement(start, 9.0, 9.0).unwrap();
    let path_b = dir.path().join("b.stalx");
    write_v2(&path_b, &b, &Extras::default());

    let state = open(&path_b);
    assert!(set_length(&state, 8.0).success);
    let result = state.open_project(path_a.clone());

    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.name, "Project A");
    assert_eq!(project.lines().len(), 1);
    assert!(!state.can_undo(), "B's history is gone");
    assert!(!state.has_unsaved_changes());
    assert_eq!(state.open_project_path(), Some(path_a));
}

// === The persisted history ===

/// The undo and redo stacks are written by the save and read back by the
/// open; the steps replay exactly after the reopen.
#[test]
fn test_open_project_restores_undo_redo() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("restore.stalx");
    write_v2(&path, &survey("Restore"), &Extras::default());
    let state = open(&path);
    assert!(set_length(&state, 6.0).success);
    assert!(set_length(&state, 7.0).success);
    assert!(state.undo().success);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let history = decoded(&path).extras.history;
    assert_eq!((history.undo.len(), history.redo.len()), (1, 1));

    let reopened = open(&path);

    assert!(reopened.can_undo() && reopened.can_redo());
    assert_eq!(first_length(&reopened), 6.0);
    assert!(reopened.redo().success);
    assert_eq!(first_length(&reopened), 7.0);
    assert!(reopened.undo().success);
    assert!(reopened.undo().success);
    assert_eq!(first_length(&reopened), 5.0);
    assert!(!reopened.can_undo());
}

/// A save with no new edit writes the history again (it is part of the file,
/// not a delta).
#[test]
fn a_second_save_with_no_new_edit_still_persists_the_undo_history() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("twice.stalx");
    write_v2(&path, &survey("Twice"), &Extras::default());
    let state = open(&path);
    assert!(set_length(&state, 6.0).success);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let first = std::fs::read(&path).unwrap();

    assert!(state.execute(Command::SaveProject { path: None }).success);

    assert_eq!(
        std::fs::read(&path).unwrap(),
        first,
        "same model, same bytes"
    );
    assert_eq!(decoded(&path).extras.history.undo.len(), 1);
}

#[test]
fn test_undo_after_reload_restores_exactly_the_edited_field() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("exact.stalx");
    let original = survey("Exact");
    write_v2(&path, &original, &Extras::default());
    let state = open(&path);
    assert!(set_length(&state, 9.25).success);
    assert!(state.execute(Command::SaveProject { path: None }).success);

    let reopened = open(&path);
    assert!(reopened.undo().success);

    let after = reopened.get_project_state().unwrap();
    let mut expected = original;
    expected.file_path = after.file_path.clone();
    expected.modified_at = after.modified_at;
    assert_eq!(after, expected, "every other field as it was");
}

/// A count-window project (no SpeleoDB link) reads at most the configured
/// number of entries; a Full-mode linked project reads its whole history.
#[test]
fn the_open_reads_the_history_window_its_mode_allows() {
    let dir = TempDir::new().unwrap();
    let edits = |path: &Path, extras: &Extras| {
        write_v2(path, &survey("Window"), extras);
        let state = open(path);
        if extras.speleodb.is_some() {
            state.set_edit_enabled(true);
        }
        for i in 0..5 {
            assert!(set_length(&state, 6.0 + f64::from(i)).success);
        }
        assert!(state.execute(Command::SaveProject { path: None }).success);
        assert_eq!(decoded(path).extras.history.undo.len(), 5);
    };
    let local = dir.path().join("local.stalx");
    edits(&local, &Extras::default());
    let full = dir.path().join("full.stalx");
    edits(&full, &linked(Some(UndoMode::Full)));

    let depth = |path: &Path| {
        let state = AppState::new().unwrap();
        // What a settings change does (`update_config`): the window follows.
        state.undo_stack.lock().unwrap().set_history_limit(3);
        assert!(
            state
                .execute(Command::OpenProject {
                    path: path.to_path_buf()
                })
                .success
        );
        state.get_undo_stack().len()
    };

    assert_eq!(
        depth(&local),
        3,
        "the count window keeps the newest entries"
    );
    assert_eq!(
        depth(&full),
        5,
        "Full mode is bounded by bytes, not the window"
    );
}

/// Full mode persists both stacks: the undo history rides the next push, and
/// redo survives a reopen.
#[test]
fn full_mode_save_persists_both_stacks() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("full-mode-undo.stalx");
    write_v2(&path, &survey("Linked"), &linked(Some(UndoMode::Full)));
    let state = open(&path);
    state.set_edit_enabled(true);
    assert!(set_length(&state, 6.0).success);
    assert!(set_length(&state, 7.0).success);
    assert!(state.undo().success);
    assert!(
        state.can_redo(),
        "the undo must leave a redo branch in memory"
    );

    let saved = state.execute(Command::SaveProject { path: None });

    assert!(saved.success, "save failed: {:?}", saved.error);
    let history = decoded(&path).extras.history;
    assert!(
        !history.undo.is_empty(),
        "Full mode persists the undo history"
    );
    assert!(!history.redo.is_empty(), "and redo too");
}

/// No local path reaches the persisted history (it rides SpeleoDB pushes).
#[test]
fn a_persisted_history_entry_carries_no_local_filesystem_path() {
    let dir = TempDir::new().unwrap();
    let private = dir.path().join("alice-private");
    std::fs::create_dir(&private).unwrap();
    let path = private.join("secret-sump.stalx");
    write_v2(&path, &survey("Leaky"), &Extras::default());
    let state = open(&path);
    assert!(set_length(&state, 6.0).success);
    let line_id = state.get_project_state().unwrap().lines()[0].id;
    assert!(state.execute(Command::DeleteLine { line_id }).success);

    assert!(state.execute(Command::SaveProject { path: None }).success);

    let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
    let history = &text[text.find("\"history\":").unwrap()..];
    assert!(!history.contains("alice"), "path leaked: {history}");
    assert!(!history.contains("secret-sump"), "path leaked: {history}");
}

// === The save ===

/// The save refuses a model that breaks an invariant (the old save healed
/// dangling references; the station graph never holds one, so reaching this
/// is a bug), and the file is not touched.
#[test]
fn a_save_refuses_a_survey_that_breaks_an_invariant() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("refused.stalx");
    write_v2(&path, &survey("Sound"), &Extras::default());
    let before = std::fs::read(&path).unwrap();
    let state = open(&path);
    let mut broken = state.get_project_state().unwrap();
    broken.lines_mut()[0].shots[0].length = f64::NAN;
    state.set_project_for_tests(broken);

    let result = state.save_project(None);

    assert!(!result.success);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .starts_with("Can't save: "),
        "{result:?}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn test_save_project_no_project_open() {
    let state = AppState::new().unwrap();
    let result = state.save_project(Some(PathBuf::from("/tmp/test.stalx")));
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No project open"));
}

#[test]
fn test_save_project_no_path_specified() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    // No file_path on project and no path argument.
    let result = state.save_project(None);
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("The project has no file to save to")
    );
}

/// A project created with its first save deferred (#92) whose reserved path
/// was taken meanwhile never replaces that file: the implicit first save
/// picks the next free name, as 0.6.0 did
/// (`test_save_project_implicit_first_save_autorename_when_target_exists`).
#[test]
fn a_deferred_first_save_never_replaces_a_file_that_appeared_at_its_path() {
    let dir = TempDir::new().unwrap();
    let existing = dir.path().join("existing.stalx");
    let autorenamed = dir.path().join("existing_1.stalx");
    std::fs::write(&existing, b"ORIGINAL").unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });
    state.project.lock().unwrap().as_mut().unwrap().file_path = Some(existing.clone());

    let result = state.save_project(None);

    assert!(
        result.success,
        "unexpected save failure: {:?}",
        result.error
    );
    // Existing file must remain untouched.
    assert_eq!(std::fs::read(&existing).unwrap(), b"ORIGINAL");
    // The save picks an available suffixed filename, and the session follows.
    assert!(autorenamed.exists());
    assert_eq!(result.saved_path.as_deref(), autorenamed.to_str());
    let project = state.get_project_state().unwrap();
    assert_eq!(project.file_path.as_ref(), Some(&autorenamed));
}

/// The save writes through `<file>.stalx.tmp` and leaves nothing beside the
/// project; the session follows the file.
#[test]
fn test_save_project_leaves_no_temp_and_follows_the_file() {
    let dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    state.set_project_for_tests(survey("Sec"));
    let save_path = dir.path().join("project.stalx");

    let result = state.save_project(Some(save_path.clone()));

    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        result.saved_path.as_deref(),
        Some(save_path.to_str().unwrap())
    );
    assert!(!dir.path().join("project.stalx.tmp").exists());
    assert_eq!(
        state.get_project_state().unwrap().file_path.as_ref(),
        Some(&save_path)
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "only the project"
    );
}

/// A second save replaces the file with the new state.
#[test]
fn test_save_project_twice_writes_the_latest_state() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("atomic.stalx");
    write_v2(&path, &survey("Original"), &Extras::default());
    let state = open(&path);
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(set_length(&state, 12.5).success);

    assert!(state.execute(Command::SaveProject { path: None }).success);

    assert_eq!(decoded(&path).project.lines()[0].shots[0].length, 12.5);
    assert!(!state.has_unsaved_changes());
}

/// The display settings ride the save in `local.display`, in key order.
#[test]
fn the_save_writes_the_display_settings_in_key_order() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("display.stalx");
    write_v2(&path, &survey("Display"), &Extras::default());
    let state = open(&path);
    state.save_display_setting("zeta", "1").unwrap();
    state.save_display_setting("alpha", "2").unwrap();
    state.save_display_setting("showStations", "false").unwrap();

    assert!(set_length(&state, 6.0).success);
    assert!(state.execute(Command::SaveProject { path: None }).success);

    let display = decoded(&path).extras.local.display;
    let keys: Vec<&str> = display.keys().map(String::as_str).collect();
    assert_eq!(keys, ["alpha", "showStations", "zeta"]);
    let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
    let (a, s, z) = (
        text.find("\"alpha\"").unwrap(),
        text.find("\"showStations\"").unwrap(),
        text.find("\"zeta\"").unwrap(),
    );
    assert!(a < s && s < z, "written in key order");
}

/// A project never saved keeps its display settings in memory; the first
/// save writes them.
#[test]
fn a_new_projects_display_settings_ride_its_first_save() {
    let dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Fresh".to_string(),
        location: None,
    });
    state
        .save_display_setting("test_key", "test_value")
        .unwrap();
    assert_eq!(
        state.get_display_setting("test_key").unwrap().as_deref(),
        Some("test_value")
    );
    assert_eq!(
        state
            .load_display_settings()
            .unwrap()
            .get("test_key")
            .map(String::as_str),
        Some("test_value")
    );

    let path = dir.path().join("fresh.stalx");
    assert!(state.save_project(Some(path.clone())).success);

    assert_eq!(
        decoded(&path)
            .extras
            .local
            .display
            .get("test_key")
            .map(String::as_str),
        Some("test_value")
    );
}

// === The SpeleoDB undo mode (a project property in `speleodb`) ===

/// [#303] Changing the undo mode needs the edit gate on a linked project.
#[test]
fn setting_the_undo_mode_on_a_linked_project_needs_the_edit_gate() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("undo-mode-gate.stalx");
    write_v2(&path, &survey("Linked"), &linked(Some(UndoMode::Full)));
    let state = open(&path);
    assert!(!state.edit_enabled(), "a remote open starts read-only");

    let err = state
        .speleodb_set_undo_mode(UndoMode::Disabled)
        .expect_err("a read-only session must not change what the project ships");
    assert!(err.message.contains("start editing"), "{err:?}");

    state.set_edit_enabled(true);
    state
        .speleodb_set_undo_mode(UndoMode::Disabled)
        .expect("with an edit session the override is settable");

    state.set_edit_enabled(false);
    state
        .speleodb_set_undo_mode(UndoMode::Full)
        .expect_err("a released session cannot change the project policy");
}

/// The mode change is a local-only durable save: in the file at once, with
/// the dirty bit, and it waits for a full save holding the project lock.
#[test]
fn setting_the_undo_mode_is_durable_and_serializes_with_full_saves() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("undo-mode-save-lock.stalx");
    write_v2(&path, &survey("Linked"), &linked(Some(UndoMode::Full)));
    let state = Arc::new(open(&path));
    state.set_edit_enabled(true);
    let save_guard = state.project.lock().unwrap();
    let worker_state = Arc::clone(&state);
    let (sent, received) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = worker_state.speleodb_set_undo_mode(UndoMode::Disabled);
        sent.send(result.is_ok()).unwrap();
    });

    assert!(
        received
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "the mode write ran while a full-file save owned the project lock"
    );
    drop(save_guard);
    assert!(received
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("mode write completes after the save lock is released"));
    worker.join().unwrap();

    let on_disk = decoded(&path).extras;
    assert_eq!(
        on_disk.speleodb.unwrap().undo_mode,
        Some(UndoMode::Disabled)
    );
    assert!(on_disk.local.speleodb.unpushed_edits);
    let link: SpeleoLink = state.speleo_link().unwrap();
    assert!(link.has_unpushed_edits);
}

/// The `history` member of the file at `path`, byte for byte.
fn history_member(path: &Path) -> String {
    let text = String::from_utf8(std::fs::read(path).unwrap()).unwrap();
    let at = text.find("\"history\":").expect("a history member");
    let end = text[at..].find("\n\"end\"").map_or(text.len(), |e| at + e);
    text[at..end].to_string()
}

/// A display toggle (a local-only save) never erases a history this build
/// did not load (review findings C-file-format-3, J-docs-tests-truth-2,
/// H-frontend-wire-2): entries it cannot decode, and the whole history of a
/// read-only file made by a newer build, stay in the file byte for byte for
/// a build that can still undo them. A real save writes the stacks again.
#[test]
fn a_display_toggle_keeps_a_history_this_build_did_not_load() {
    let _config = crate::test_helpers::isolated_config();
    crate::app_dirs::init_for_test();
    let first = chrono::DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for read_only in [false, true] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history.stalx");
        let mut extras = Extras::default();
        extras.history.undo = super::stalx_fixtures::raw_entries(first, 3, 8);
        write_v2(&path, &survey("History"), &extras);
        if read_only {
            let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
            let text = text.replacen("\"requires\":[]", "\"requires\":[\"zz-future\"]", 1);
            assert!(
                text.contains("zz-future"),
                "the file now requires a future feature"
            );
            std::fs::write(&path, text).unwrap();
        }
        let before = history_member(&path);
        let state = open(&path);
        assert_eq!(state.read_only_reason().is_some(), read_only);
        assert!(!state.can_undo(), "this build could not load the entries");

        state.save_display_setting("showGrid", "true").unwrap();
        assert_eq!(
            history_member(&path),
            before,
            "read_only={read_only}: the toggle kept the file's history"
        );
        assert_eq!(
            decoded(&path)
                .extras
                .local
                .display
                .get("showGrid")
                .map(String::as_str),
            Some("true"),
            "and wrote the toggle"
        );
        if !read_only {
            // An edit and a real save make the stacks the file's history.
            assert!(set_length(&state, 6.0).success);
            assert!(state.execute(Command::SaveProject { path: None }).success);
            assert_eq!(decoded(&path).extras.history.undo.len(), 1);
            state.save_display_setting("showGrid", "false").unwrap();
            assert_eq!(decoded(&path).extras.history.undo.len(), 1);
        }
    }
}

/// A display toggle on a file a newer build saved keeps that build's unknown
/// per-machine and sync keys (`local`, `local.speleodb`), on a read-only file
/// and on an editable one, and a real save keeps them too (review finding
/// H-frontend-wire-2). Injection: dropping an unknown `local` key in
/// `read_local` again erases both on the first toggle.
#[test]
fn a_display_toggle_keeps_a_newer_builds_local_state() {
    let _config = crate::test_helpers::isolated_config();
    crate::app_dirs::init_for_test();
    for read_only in [false, true] {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("newer-local.stalx");
        let mut extras = Extras::default();
        extras
            .local
            .display
            .insert("showGrid".to_string(), "true".to_string());
        extras.local.speleodb.base_commit = Some("ab".repeat(20));
        write_v2(&path, &survey("Newer local"), &extras);
        let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        let mut text = text
            .replacen(
                "\"local\":{",
                "\"local\":{\"x-future-local\":{\"keep\":1},",
                1,
            )
            .replacen("\"base\":", "\"x-future-sync\":[2],\"base\":", 1);
        if read_only {
            text = text.replacen("\"requires\":[]", "\"requires\":[\"zz-future\"]", 1);
        }
        assert_eq!(text.matches("x-future-").count(), 2);
        std::fs::write(&path, &text).unwrap();
        let state = open(&path);
        assert_eq!(state.read_only_reason().is_some(), read_only);

        state.save_display_setting("showGrid", "false").unwrap();
        let after = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert_ne!(after, text, "read_only={read_only}: the toggle was written");
        assert!(
            after.contains("\"x-future-local\":{\"keep\":1}")
                && after.contains("\"x-future-sync\":[2]"),
            "read_only={read_only}: the newer build's keys survive the toggle:\n{after}"
        );
        if !read_only {
            assert!(set_length(&state, 6.0).success);
            assert!(state.execute(Command::SaveProject { path: None }).success);
            let saved = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
            assert!(
                saved.contains("\"x-future-local\":{\"keep\":1}")
                    && saved.contains("\"x-future-sync\":[2]"),
                "and a real save:\n{saved}"
            );
        }
    }
}

/// Renaming the open project outside the history (Recent Projects, the
/// SpeleoDB name fold) never wedges undo: an earlier Project Settings rename
/// and every older entry still undo, and redo, as 0.6.0's snapshot undo did
/// (review findings F-command-parity-1, E-undo-pipeline-2). Undoing the
/// Settings rename sets the name it recorded.
#[test]
fn an_out_of_band_rename_of_the_open_project_does_not_wedge_undo() {
    let _config = crate::test_helpers::isolated_config();
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("rename.stalx");
    write_v2(&path, &survey("Cave A"), &Extras::default());
    let state = open(&path);
    assert!(set_length(&state, 6.0).success);
    let validation = state.get_config().validation;
    assert!(
        state
            .update_project_settings("Cave B".to_string(), validation)
            .success
    );
    assert!(
        state
            .set_project_name_for_path(path.clone(), "Cave C".to_string())
            .success
    );

    let undone = state.undo();
    assert!(undone.success, "{:?}", undone.error);
    assert_eq!(project_name(&state), "Cave A");
    let undone = state.undo();
    assert!(
        undone.success,
        "the older entry is reachable: {:?}",
        undone.error
    );
    assert!(state.redo().success);
    assert!(state.redo().success);
    assert_eq!(project_name(&state), "Cave B");
}

fn project_name(state: &AppState) -> String {
    state
        .project
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap()
}
