//! Coverage gap tests: project_io.rs.
//!
//! Lines 17011-18045 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// ============================================================================
// Coverage gap tests: project_io.rs
// ============================================================================

#[test]
fn test_save_project_no_project_open() {
    let state = AppState::new().unwrap();
    let result = state.save_project(Some(PathBuf::from("/tmp/test.stalx")));
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No project open"));
}

#[test]
fn test_save_project_no_path_specified() {
    let state = state_with_project("Test");
    // No file_path on project and no path argument
    let result = state.save_project(None);
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No save path"));
}

#[test]
fn test_save_project_implicit_first_save_autorename_when_target_exists() {
    let temp_dir = TempDir::new().unwrap();
    let existing_path = temp_dir.path().join("existing.stalx");
    let expected_autorenamed_path = temp_dir.path().join("existing_1.stalx");
    std::fs::write(&existing_path, b"ORIGINAL").unwrap();

    let state = state_with_project("Test");

    // Simulate deferred-create behavior: reserved file_path, no open database yet,
    // and the explicit flag `create_project_full` sets for exactly this state.
    {
        let mut guard = state.project.lock();
        let project = guard.as_mut().unwrap();
        project.file_path = Some(existing_path.clone());
    }
    {
        let mut db_guard = state.database.lock();
        *db_guard = None;
    }
    state.set_initial_save_deferred_for_tests(true);

    let result = state.save_project(None);
    assert!(
        result.success,
        "unexpected save failure: {:?}",
        result.error
    );

    // Existing file must remain untouched.
    let bytes = std::fs::read(&existing_path).unwrap();
    assert_eq!(bytes, b"ORIGINAL");

    // Autosave should pick an available suffixed filename.
    assert!(expected_autorenamed_path.exists());
    let project = state.get_project_state().unwrap();
    assert_eq!(project.file_path.as_ref(), Some(&expected_autorenamed_path));
}

/// A saved project with one shot, an "Ignore" on that shot and a stored display
/// setting, whose live database handle is then dropped exactly the way the
/// `DATABASE_NOT_REOPENED` branch of `save_project` leaves it: the file was
/// renamed into place, `file_path` points at it, the deferred flag is cleared,
/// and the handle is `None`. Returns `(state, path, _temp)`.
fn session_after_a_failed_reopen() -> (AppState, PathBuf, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("X.stalx");
    let state = state_with_project("X");
    assert!(
        state
            .execute(Command::CreateSection {
                name: Some("First".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![shot(5.0)],
            })
            .success
    );
    assert!(state.save_project(Some(path.clone())).success);

    let project = state.get_project_state().unwrap();
    let section = &project.sections[0];
    let shot = &section.shots[0];
    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: section.id,
            shot_id: shot.id,
            warning_code: "LONG_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: Some("checked".to_string()),
            shot_uuid: shot.uuid.clone(),
        })
        .unwrap();
    state.save_display_setting("stationSize", "7").unwrap();

    *state.database.lock() = None;
    assert!(!state.initial_save_deferred());
    assert!(
        state
            .execute(Command::CreateSection {
                name: Some("Second".to_string()),
                surveyor: None,
                date: None,
                color: "#00ff00".to_string(),
                direction: None,
                shots: vec![crate::test_fixtures::shot(3.0)],
            })
            .success
    );
    (state, path, temp_dir)
}

/// [#576 B35(c)] After the reopen that follows a successful rename fails, the next
/// implicit save (every autosave) must still write the open file. It used to take
/// the missing handle for "first implicit save" and go through
/// `next_available_project_path`, writing `X_1.stalx` without the ignores or the
/// stored display settings and silently moving the session there.
#[test]
fn an_implicit_save_after_a_failed_reopen_stays_on_the_open_file() {
    let (state, path, temp_dir) = session_after_a_failed_reopen();
    let renamed = temp_dir.path().join("X_1.stalx");

    let result = state.save_project(None);
    assert!(result.success, "save failed: {:?}", result.error);

    let session_path = state.get_project_state().unwrap().file_path;
    let db = Database::open_existing(&path).unwrap();
    let sections = db.load_project().unwrap().sections.len();
    let ignores = db.load_resolved_warnings_verbatim().unwrap().len();
    let setting = db.get_display_setting("stationSize").unwrap();
    eprintln!("saved_path: {:?}", result.saved_path);
    eprintln!("session file_path: {session_path:?}");
    eprintln!("X_1.stalx exists: {}", renamed.exists());
    eprintln!("X.stalx: {sections} sections (expected 2), {ignores} ignores (expected 1), stationSize {setting:?} (expected Some(\"7\"))");
    if renamed.exists() {
        let moved = Database::open_existing(&renamed).unwrap();
        eprintln!(
            "X_1.stalx: {} ignores, stationSize {:?}",
            moved.load_resolved_warnings_verbatim().unwrap().len(),
            moved.get_display_setting("stationSize").unwrap()
        );
    }
    assert!(!renamed.exists(), "the save moved the session to X_1.stalx");
    assert_eq!(session_path.as_ref(), Some(&path));
    assert_eq!(sections, 2, "the edit did not reach X.stalx");
    assert_eq!(ignores, 1, "the ignore was dropped");
    assert_eq!(
        setting.as_deref(),
        Some("7"),
        "the display setting was dropped"
    );
    assert!(
        state.database.lock().is_some(),
        "the session must have its database handle back"
    );
}

/// [#576 B35(c)] A display setting changed while the handle was gone lands in the
/// in-memory cache; reopening the file must not let the file's older value win.
#[test]
fn a_display_setting_changed_without_a_handle_survives_the_reopen() {
    let (state, path, _temp_dir) = session_after_a_failed_reopen();
    state.save_display_setting("stationSize", "9").unwrap();

    assert!(state.save_project(None).success);

    let stored = Database::open_existing(&path)
        .unwrap()
        .get_display_setting("stationSize")
        .unwrap();
    assert_eq!(stored.as_deref(), Some("9"));
}

/// [#576 B35(c)] When the missing handle cannot be reopened either, an implicit
/// save is refused - writing the file without the side tables it alone holds would
/// destroy them - and the file on disk is left as it was. Save As to another file
/// stays available, so the edits are never stranded.
#[test]
fn an_implicit_save_is_refused_when_the_missing_handle_cannot_be_reopened() {
    let (state, path, temp_dir) = session_after_a_failed_reopen();
    let renamed = temp_dir.path().join("X_1.stalx");
    // Whatever holds the file shut, the reopen fails the way it did after the
    // rename. A file SQLite cannot read is the portable way to make it fail
    // (mode bits do not stop root).
    std::fs::write(&path, b"not a database").unwrap();

    let result = state.save_project(None);
    eprintln!("save success: {} error: {:?}", result.success, result.error);
    eprintln!("X_1.stalx exists: {}", renamed.exists());
    eprintln!(
        "session file_path: {:?}",
        state.get_project_state().unwrap().file_path
    );
    assert!(!result.success, "the save must be refused");
    assert!(!renamed.exists(), "the save wrote X_1.stalx");
    assert_eq!(std::fs::read(&path).unwrap(), b"not a database");
    assert_eq!(
        state.get_project_state().unwrap().file_path.as_ref(),
        Some(&path)
    );

    let elsewhere = temp_dir.path().join("Rescue.stalx");
    let rescue = state.save_project(Some(elsewhere.clone()));
    assert!(
        rescue.success,
        "Save As must still work: {:?}",
        rescue.error
    );
    assert_eq!(
        Database::open_existing(&elsewhere)
            .unwrap()
            .load_project()
            .unwrap()
            .sections
            .len(),
        2
    );
}

#[test]
fn test_save_project_adds_stalx_extension() {
    let temp_dir = TempDir::new().unwrap();
    let state = state_with_project("Test");
    // Save without .stalx extension
    let save_path = temp_dir.path().join("myproject");
    let result = state.save_project(Some(save_path));
    assert!(result.success);
    assert!(temp_dir.path().join("myproject.stalx").exists());
}

/// True when `dir` tells `a.stalx` and `a.STALX` apart. On a case-insensitive
/// filesystem (the macOS and Windows defaults) the two spellings ARE one file, so
/// the B33 tests below have nothing to prove there and return early.
fn dir_is_case_sensitive(dir: &std::path::Path) -> bool {
    let probe = dir.join("case-probe.stalx");
    std::fs::write(&probe, b"probe").unwrap();
    let sensitive = !dir.join("case-probe.STALX").exists();
    std::fs::remove_file(&probe).unwrap();
    if !sensitive {
        eprintln!("skipping: {} is case-insensitive", dir.display());
    }
    sensitive
}

/// The project name stored in the `.stalx` at `path`.
fn stored_project_name(path: &std::path::Path) -> String {
    Database::open_existing(path)
        .unwrap()
        .load_project()
        .unwrap()
        .name
}

/// [#576 B33] Save As typed as `CaveA.STALX` must write THAT file. The backend used
/// to re-extension any path whose extension was not exactly `stalx`, so on a
/// case-sensitive filesystem it wrote `CaveA.stalx` instead - a different file,
/// which the rename replaced without a prompt (the frontend's `exists()` check and
/// the native dialog only ever saw `CaveA.STALX`).
#[test]
fn save_as_with_an_uppercase_extension_does_not_replace_the_lowercase_project() {
    let temp_dir = TempDir::new().unwrap();
    if !dir_is_case_sensitive(temp_dir.path()) {
        return;
    }
    let lower = temp_dir.path().join("CaveA.stalx");
    let upper = temp_dir.path().join("CaveA.STALX");

    let first = state_with_project("Project One");
    assert!(first.save_project(Some(lower.clone())).success);
    let lower_before = std::fs::read(&lower).unwrap();

    let second = state_with_project("Project Two");
    let result = second.save_project(Some(upper.clone()));
    assert!(result.success, "Save As failed: {:?}", result.error);

    let saved_path = result.saved_path.unwrap_or_default();
    let lower_name = stored_project_name(&lower);
    eprintln!("Save As target: {}", upper.display());
    eprintln!("backend saved_path: {saved_path}");
    eprintln!("CaveA.stalx now holds: {lower_name:?} (expected \"Project One\")");
    eprintln!("CaveA.STALX exists: {}", upper.exists());
    assert!(
        std::fs::read(&lower).unwrap() == lower_before,
        "Save As to CaveA.STALX replaced CaveA.stalx, which now holds {lower_name:?}"
    );
    assert_eq!(saved_path, upper.display().to_string());
    assert_eq!(stored_project_name(&upper), "Project Two");
}

/// [#576 B33] A project opened as `CaveB.STALX` (copied from a case-insensitive
/// filesystem) must save back to `CaveB.STALX`, not to a `CaveB.stalx` next to it.
#[test]
fn saving_a_project_opened_with_an_uppercase_extension_writes_that_file() {
    let temp_dir = TempDir::new().unwrap();
    if !dir_is_case_sensitive(temp_dir.path()) {
        return;
    }
    let upper = temp_dir.path().join("CaveB.STALX");
    let lower = temp_dir.path().join("CaveB.stalx");
    {
        let mut db = Database::open(&upper).unwrap();
        db.save_project(&Project::new("CaveB".to_string(), None))
            .unwrap();
    }
    {
        let mut db = Database::open(&lower).unwrap();
        db.save_project(&Project::new("Other project".to_string(), None))
            .unwrap();
    }
    let lower_before = std::fs::read(&lower).unwrap();

    let state = AppState::new().unwrap();
    assert!(state.open_project(upper.clone()).success);
    assert!(
        state
            .execute(Command::CreateSection {
                name: Some("Edit".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![shot(5.0)],
            })
            .success
    );
    let result = state.save_project(None);
    assert!(result.success, "save failed: {:?}", result.error);

    let session_path = state.get_project_state().unwrap().file_path;
    let lower_name = stored_project_name(&lower);
    let upper_sections = Database::open_existing(&upper)
        .unwrap()
        .load_project()
        .unwrap()
        .sections
        .len();
    eprintln!("opened: {}", upper.display());
    eprintln!("session file_path after save: {session_path:?}");
    eprintln!("CaveB.stalx now holds: {lower_name:?} (expected \"Other project\")");
    eprintln!("sections in CaveB.STALX: {upper_sections} (expected 1)");
    assert!(
        std::fs::read(&lower).unwrap() == lower_before,
        "saving CaveB.STALX replaced CaveB.stalx, which now holds {lower_name:?}"
    );
    assert_eq!(session_path.as_ref(), Some(&upper));
    assert_eq!(upper_sections, 1, "the edit never reached CaveB.STALX");
}

#[test]
fn test_save_project_cleans_up_temp_on_reopen_failure() {
    let temp_dir = TempDir::new().unwrap();
    let state = state_with_project("Test");

    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    let save_path = temp_dir.path().join("project.stalx");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    // Verify temp file was cleaned up
    let temp_path = save_path.with_extension("stalx.tmp");
    assert!(
        !temp_path.exists(),
        "temp file should be cleaned up after save"
    );

    // Verify the project file_path was updated
    let project = state.get_project_state().unwrap();
    assert_eq!(project.file_path.as_ref(), Some(&save_path));
}

#[test]
fn test_save_project_atomic_rename_preserves_original_on_temp_failure() {
    let temp_dir = TempDir::new().unwrap();
    let state = state_with_project("Original");
    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });

    // First save
    let save_path = temp_dir.path().join("atomic.stalx");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    // Modify project
    state.execute(Command::CreateSection {
        name: Some("Sec2".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });

    // Save again - should succeed and update the file
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    // Reload and verify both sections are there
    let db = Database::open_readonly(&save_path).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.sections.len(), 2);
}

#[test]
fn test_save_project_persists_undo_redo_stacks() {
    let temp_dir = TempDir::new().unwrap();
    let state = state_with_project("UndoPersist");

    // Create section (pushes to undo stack)
    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    // Create another section, then undo it (creates redo entry)
    state.execute(Command::CreateSection {
        name: Some("Sec2".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    state.undo();

    let save_path = temp_dir.path().join("undo-persist.stalx");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    // Reload and verify undo/redo stacks are persisted
    let db = Database::open_existing(&save_path).unwrap();
    let undo = db.load_undo_stack_with_timestamps().unwrap_or_default();
    let redo = db.load_redo_stack_with_timestamps().unwrap_or_default();
    assert!(!undo.is_empty(), "undo stack should be persisted");
    assert!(!redo.is_empty(), "redo stack should be persisted");
}

#[test]
fn failed_atomic_history_replacement_falls_back_to_empty_stacks() {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("history-fallback.stalx");
    let db = Database::open(&path).unwrap();
    db.save_undo_stack_with_timestamps(vec![(
        r#"{"type":"OldUndo"}"#.to_owned(),
        r#"{"kind":"old"}"#.to_owned(),
        None,
    )])
    .unwrap();
    db.save_redo_stack_with_timestamps(vec![(
        r#"{"type":"OldRedo"}"#.to_owned(),
        r#"{"kind":"old"}"#.to_owned(),
        None,
    )])
    .unwrap();

    let state = state_with_project("History fallback");
    state.execute(Command::CreateSection {
        name: Some("Redo candidate".to_owned()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_owned(),
        direction: None,
        shots: vec![shot(2.0)],
    });
    state.undo();

    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_redo_insert BEFORE INSERT ON redo_stack \
             BEGIN SELECT RAISE(FAIL, 'redo rejected'); END;",
        )
        .unwrap();

    let dropped = state
        .persist_or_clear_undo_stacks_into(&db)
        .expect("the empty-history fallback should succeed");

    assert!(dropped, "the caller must surface the history-loss warning");
    assert!(db.load_undo_stack_with_timestamps().unwrap().is_empty());
    assert!(db.load_redo_stack_with_timestamps().unwrap().is_empty());
}

/// [#576 A3-D2] Undo-chain markers that cannot be read STOP the save. They used to
/// be answered by saving the survey with no undo history at all; the maintainer's
/// policy for the markers is now "stop": nothing is written, the file is untouched,
/// and the live history is kept for a save that can carry them.
#[test]
fn unreadable_undo_markers_refuse_the_save() {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("unreadable-undo-markers.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Marker fallback".to_owned(), None))
            .unwrap();
        db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
        db.save_speleo_undo_chain(Some("older-tip")).unwrap();
        db.save_speleo_undo_watermark(Some("2026-08-01T00:00:00+00:00"))
            .unwrap();
    }

    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    state.undo_stack.lock().push_with_data(
        Command::CreateSection {
            name: Some("Local edit".to_owned()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_owned(),
            direction: None,
            shots: Vec::new(),
        },
        UndoData::None,
    );
    state.undo_stack.lock().push_with_data(
        Command::CreateSection {
            name: Some("Redo candidate".to_owned()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_owned(),
            direction: None,
            shots: Vec::new(),
        },
        UndoData::None,
    );
    state.undo();

    // Keep the table present but remove the column the marker reader requires. The
    // survey database and undo rows remain readable, isolating the marker-failure path.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "DROP TABLE speleo_undo_watermark;
             CREATE TABLE speleo_undo_watermark (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 broken_value TEXT NOT NULL
             );
             INSERT INTO speleo_undo_watermark (id, broken_value) VALUES (1, 'broken');",
        )
        .unwrap();

    let before = std::fs::read(&path).unwrap();
    let result = state.save_project(None);

    assert!(!result.success, "the save must be refused: {result:?}");
    assert!(
        result.code == Some(CommandErrorCode::UndoMarkersUnreadable)
            && result
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("Can't save: ")),
        "the refusal carries its code (and no marker): {result:?}"
    );
    assert!(
        std::fs::read(&path).unwrap() == before,
        "a refused save leaves the file untouched"
    );
    // (The fixture's `undo()` is refused by the read-only gate, so both entries are
    // still on the undo side.)
    assert_eq!(
        state.undo_stack.lock().len(),
        2,
        "the live undo history is kept"
    );
}

/// [#303] Changing the undo mode really does need the edit gate.
///
/// The sibling test in `undo_policy.rs` asserts this with NO project open, so
/// `speleodb_validated_link()?` errors one line before the `edit_enabled()` gate is ever
/// reached - delete the gate and it stays green. This one opens a real linked project so
/// the gate is the thing under test, and its companion source-text tripwire only pins the
/// gate's POSITION, not that it fires.
#[test]
fn setting_the_undo_mode_on_a_linked_project_needs_the_edit_gate() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("undo-mode-gate.stalx");
    {
        let mut db = Database::open(&save_path).unwrap();
        db.save_project(&Project::new("Linked".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
    }

    let state = AppState::new().unwrap();
    assert!(state.open_project(save_path).success);
    // A remote open leaves editing disabled, which is exactly the state under test.
    assert!(!state.edit_enabled(), "a remote open starts read-only");

    let err = state
        .speleodb_set_undo_mode(crate::models::UndoMode::Disabled)
        .expect_err("a read-only session must not change what the project ships");
    assert!(
        err.message.contains("start editing"),
        "the refusal must be the edit-gate one, not the absent-link one, got {err:?}"
    );

    // ...and with the gate open the same call succeeds, so the test is pinning the gate
    // rather than some unrelated refusal.
    state.set_edit_enabled(true);
    state
        .speleodb_set_undo_mode(crate::models::UndoMode::Disabled)
        .expect("with an edit session the override is settable");

    // Force editing is intentionally the same gate. It carries conflict risk, but the
    // user explicitly accepted that mode and its commands may edit this same project file.
    state.set_force_editing(true);
    state
        .speleodb_set_undo_mode(crate::models::UndoMode::Full)
        .expect("force editing may change the project policy too");
}

#[test]
fn setting_the_undo_mode_serializes_with_full_file_saves() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("undo-mode-save-lock.stalx");
    {
        let mut db = Database::open(&save_path).unwrap();
        db.save_project(&Project::new("Linked".to_owned(), None))
            .unwrap();
        db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
    }

    let state = Arc::new(AppState::new().unwrap());
    assert!(state.open_project(save_path).success);
    state.set_edit_enabled(true);
    let save_guard = state.project.lock();
    let worker_state = Arc::clone(&state);
    let (sent, received) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = worker_state.speleodb_set_undo_mode(crate::models::UndoMode::Disabled);
        sent.send(result).unwrap();
    });

    assert!(
        received
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "the mode write ran while a full-file save owned the project lock"
    );
    drop(save_guard);
    received
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("mode write completes after the save lock is released")
        .expect("mode write succeeds");
    worker.join().unwrap();

    let db_guard = state.database.lock();
    let link = db_guard
        .as_ref()
        .unwrap()
        .load_speleo_link()
        .unwrap()
        .unwrap();
    assert_eq!(link.undo_mode, Some(crate::models::UndoMode::Disabled));
    assert!(link.has_unpushed_edits);
}

/// [#303] An undo snapshot must not carry this machine's filesystem paths.
///
/// Full mode ships `undo_stack` to the server on purpose, and every `ProjectSnapshot` /
/// `ProjectSnapshotPair` is a whole serialized `Project` - so an unscrubbed `file_path`
/// writes `/home/<user>/…` or `C:\Users\<Real Name>\…` into the project's git history,
/// permanently and readable by every collaborator. Snapshot pairs come from Delete shot,
/// Delete section, Reverse section and every Batch import, so this is routine editing.
#[test]
fn a_project_snapshot_carries_no_local_filesystem_path() {
    let mut project = Project::new("Leaky".to_string(), None);
    project.file_path = Some(PathBuf::from("/home/victim/caves/secret-sump.stalx"));

    let json = AppState::serialize_project_snapshot(&project).expect("serializes");

    assert!(
        !json.contains("victim"),
        "the snapshot leaked a local path: {json}"
    );
    assert!(
        !json.contains("secret-sump.stalx"),
        "the snapshot leaked a local filename: {json}"
    );
    // The survey itself must still be there - scrubbing must not cost the payload.
    assert!(json.contains("Leaky"), "the snapshot lost the project name");
}

/// ...and restoring one must not lose track of where the file lives.
///
/// The path is cleared on the way out, so the way back has to re-apply the live value or
/// an undo would leave the app believing the project was never saved - and the next save
/// would prompt for a location.
#[test]
fn restoring_a_snapshot_keeps_the_live_file_path() {
    let mut live = Project::new("Cave".to_string(), None);
    live.file_path = Some(PathBuf::from("/home/me/cave.stalx"));
    let snapshot = AppState::serialize_project_snapshot(&live).expect("serializes");

    let mut slot = project_slot::ProjectSlot::new(Some(live));
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));

    assert_eq!(
        slot.as_ref().and_then(|p| p.file_path.clone()),
        Some(PathBuf::from("/home/me/cave.stalx")),
        "the restore must carry the live path across, not the snapshot's (absent) one"
    );
}

#[test]
fn persisted_split_snapshot_carries_no_local_filesystem_path() {
    let state = state_with_project("Private split");
    let created = state.execute(Command::CreateSection {
        name: Some("Single".to_string()),
        surveyor: None,
        date: None,
        color: "#123456".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let section_id = created.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;
    state.project.lock().as_mut().unwrap().file_path =
        Some(PathBuf::from("/home/alice/private/split.stalx"));

    let result = state.execute_split_shot_at_distance(section_id, shot_id, 4.0);
    assert!(result.success, "split failed: {:?}", result.error);

    let stack = state.undo_stack.lock();
    let entry = stack.undo_entries().last().expect("split undo entry");
    let (_, persisted_undo, _) = UndoStack::serialize_entry_with_timestamp(entry).unwrap();
    assert!(
        !persisted_undo.contains("alice"),
        "path leaked: {persisted_undo}"
    );
    assert!(
        !persisted_undo.contains("split.stalx"),
        "path leaked: {persisted_undo}"
    );
}

#[test]
fn persisted_delete_last_snapshot_carries_no_local_filesystem_path() {
    let state = state_with_project("Private delete");
    let created = state.execute(Command::CreateSection {
        name: Some("Single".to_string()),
        surveyor: None,
        date: None,
        color: "#654321".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_id = created.section_id.unwrap();
    let shot_id = state.get_project_state().unwrap().sections[0].shots[0].id;
    state.project.lock().as_mut().unwrap().file_path =
        Some(PathBuf::from("/home/bob/private/delete.stalx"));

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(result.success, "delete failed: {:?}", result.error);

    let stack = state.undo_stack.lock();
    let entry = stack.undo_entries().last().expect("delete undo entry");
    let (_, persisted_undo, _) = UndoStack::serialize_entry_with_timestamp(entry).unwrap();
    assert!(
        !persisted_undo.contains("bob"),
        "path leaked: {persisted_undo}"
    );
    assert!(
        !persisted_undo.contains("delete.stalx"),
        "path leaked: {persisted_undo}"
    );
}

/// [#303] A Full-mode linked project persists BOTH stacks, redo included.
///
/// Pinned because the opposite was tried. The byte budget is applied to the undo and
/// redo stacks independently, so a Full-mode save can write up to twice
/// `UNDO_BYTE_BUDGET_BYTES` - which reads like a bug, and was briefly "fixed" by
/// dropping redo from the Full-mode save. It was reversed: 64 MiB is a ceiling rather
/// than growth (each stack is budgeted, and a confirmed push clears the undo history),
/// and persisting redo is what keeps a redo branch alive across a close and reopen.
///
/// So this test exists to fail if someone re-derives that fix. None of the redo half
/// reaches the server in any mode - `redo_stack` is unconditionally in
/// `PUSH_SNAPSHOT_LOCAL_TABLES` - so the cost is local and so is the benefit.
#[test]
fn full_mode_save_persists_both_stacks() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("full-mode-undo.stalx");

    // Seed a linked `.stalx` with the per-project override set explicitly, so the
    // policy does not depend on whatever the global default happens to be.
    {
        let mut db = Database::open(&save_path).unwrap();
        db.save_project(&Project::new("Linked".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&SpeleoLink {
            undo_mode: Some(crate::models::UndoMode::Full),
            ..fixture_speleo_link()
        }))
        .unwrap();
    }

    let state = AppState::new().unwrap();
    assert!(state.open_project(save_path.clone()).success);
    // A remote open leaves editing disabled; this test is about the save path, not the
    // lock, so take the write side the way a lock-acquire would.
    state.set_edit_enabled(true);

    state.execute(Command::CreateSection {
        name: Some("Kept".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    state.execute(Command::CreateSection {
        name: Some("Undone".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    state.undo();
    assert!(
        state.can_redo(),
        "the undo must leave a redo branch in memory"
    );

    let saved = state.save_project(None);
    assert!(saved.success, "save failed: {:?}", saved.error);

    let db = Database::open_existing(&save_path).unwrap();
    assert!(
        !db.load_undo_stack_with_timestamps()
            .unwrap_or_default()
            .is_empty(),
        "Full mode persists the undo history - that is what rides in the next commit"
    );
    assert!(
        !db.load_redo_stack_with_timestamps()
            .unwrap_or_default()
            .is_empty(),
        "Full mode persists redo too - the 2x save cost is an accepted ceiling, and \
         dropping it would cost redo-after-reopen"
    );
}

#[test]
fn test_open_project_missing_file() {
    let state = AppState::new().unwrap();
    let result = state.open_project(PathBuf::from("/nonexistent/path/project.stalx"));
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("File not found"));
}

#[test]
fn test_open_project_clears_previous_state() {
    let temp_dir = TempDir::new().unwrap();
    // Create and save project A
    let state = state_with_project("Project A");
    state.execute(Command::CreateSection {
        name: Some("SecA".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let path_a = temp_dir.path().join("a.stalx");
    state.save_project(Some(path_a.clone()));

    // Create and save project B
    state.execute(Command::CreateProject {
        name: "Project B".to_string(),
        location: None,
    });
    state.execute(Command::CreateSection {
        name: Some("SecB1".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    state.execute(Command::CreateSection {
        name: Some("SecB2".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let path_b = temp_dir.path().join("b.stalx");
    state.save_project(Some(path_b.clone()));

    // Open project A - should replace B's state
    let result = state.open_project(path_a);
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.name, "Project A");
    assert_eq!(project.sections.len(), 1);
}

#[test]
fn test_close_project_clears_all_state() {
    let state = state_with_project("ToClose");
    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    let result = state.close_project();
    assert!(result.success);
    assert!(state.get_project_state().is_none());
    let undo_stack = state.undo_stack.lock();
    assert_eq!(undo_stack.len(), 0);
}

#[test]
fn test_close_project_clears_adjusted_cache() {
    let state = state_with_project("ToClose");
    state.execute(Command::CreateSection {
        name: Some("Sec".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    // Warm the memoized closure-adjusted coordinates cache. For a large project
    // this is a project-scale allocation (one `Station` per station), so closing
    // must drop it for RSS to shrink back.
    assert!(state.get_adjusted_stations().is_some());
    assert!(
        state.adjusted_cache.lock().is_some(),
        "adjusted cache should be warm after get_adjusted_stations"
    );

    let result = state.close_project();
    assert!(result.success);

    // close_project must free both project-scale in-memory caches. The undo stack
    // is reloaded from SQLite on the next open, so clearing it here is safe.
    assert!(
        state.adjusted_cache.lock().is_none(),
        "adjusted cache must be cleared on close"
    );
    assert_eq!(state.undo_stack.lock().len(), 0);
}
