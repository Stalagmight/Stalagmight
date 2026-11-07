//! [#576 RV6b-2] A display setting lands in the project it was made for, or nowhere.
//!
//! Grid spacing, the geometry mode and the display toggles are per project:
//! `save_display_setting` writes into whichever project database is open when the
//! command runs. The frontend now queues same-key writes (the setting write
//! chain), so a burst of toggles in project A followed by opening project B could
//! execute A's queued writes after B opened: they landed in B's file, invisible
//! this session and shown on B's next open. The command now carries the identity
//! of the project the write was made for (`created_at`, in the RFC 3339 form the
//! frontend holds it in, and - #576 NF-W8FE-3 - the file it is open from) and
//! refuses a write for another project with `PROJECT_CHANGED`.
//!
//! All data is synthetic.

use std::path::Path;

use rusqlite::Connection;
use tempfile::TempDir;

use super::{AppState, DisplaySettingTarget};
use crate::core::{Command, CommandErrorCode};
use crate::models::{serialize_project_ipc, CreationStamp, Project};
use crate::persistence::Database;
use crate::test_fixtures::state_with_project;

/// The identity the frontend holds for `project`: its `created_at` as the wire
/// carries it (the IPC encoding of the project), and its `file_path`.
fn wire_identity(project: &Project) -> DisplaySettingTarget {
    let created_at = serialize_project_ipc(project, serde_json::value::Serializer)
        .expect("serialize the project for the wire")["created_at"]
        .as_str()
        .expect("created_at is a string on the wire")
        .to_string();
    DisplaySettingTarget {
        created_at,
        file_path: project
            .file_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    }
}

/// An identity naming `created_at` and no file.
fn unsaved_identity(created_at: &str) -> DisplaySettingTarget {
    DisplaySettingTarget {
        created_at: created_at.to_string(),
        file_path: None,
    }
}

/// A saved project at `path`, open in `state` with its database.
fn open_saved(state: &AppState, path: &Path, name: &str, created: &str) -> Project {
    let mut project = Project::new(name.to_string(), None);
    project.created_at = CreationStamp::parse(created).expect("timestamp");
    let mut db = Database::open(path).expect("create");
    db.save_project(&project).expect("write");
    drop(db);
    state.set_project_for_tests(project.clone());
    state.set_database_for_tests(Database::open_existing(path).expect("reopen"));
    project
}

#[test]
fn a_write_made_for_the_previous_project_does_not_land_in_the_one_now_open() {
    let temp = TempDir::new().expect("temp dir");
    let state = AppState::new().expect("state");
    let path_a = temp.path().join("CaveA.stalx");
    let path_b = temp.path().join("CaveB.stalx");
    let a = open_saved(&state, &path_a, "CaveA", "2026-01-01T10:00:00.123456789Z");
    let made_for_a = wire_identity(&a);
    // The user opens B while A's write is still queued.
    let b = open_saved(&state, &path_b, "CaveB", "2026-02-02T10:00:00Z");

    let late = state.save_display_setting_for(Some(&made_for_a), "gridSpacing", "50");
    let landed_in_b = Database::open_readonly(&path_b)
        .expect("open B")
        .get_display_setting("gridSpacing")
        .expect("read B");
    println!(
        "[RV6b-2] A's queued write after B opened: answer={:?}; B's gridSpacing={landed_in_b:?} (expected None)",
        late.as_ref().map_err(|e| e.code)
    );

    assert_eq!(
        late.map_err(|e| e.code),
        Err(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(landed_in_b, None);

    // B's own write still lands.
    state
        .save_display_setting_for(Some(&wire_identity(&b)), "gridSpacing", "10")
        .expect("B's write");
    let own = Database::open_readonly(&path_b)
        .expect("open B")
        .get_display_setting("gridSpacing")
        .expect("read B");
    assert_eq!(own.as_deref(), Some("10"));
}

#[test]
fn a_write_for_the_open_unsaved_project_lands_in_its_cache() {
    let state = state_with_project("CaveA");
    let project = state.get_project_state().expect("open project");

    state
        .save_display_setting_for(Some(&wire_identity(&project)), "showStations", "false")
        .expect("write");
    assert_eq!(
        state
            .get_display_setting("showStations")
            .expect("read")
            .as_deref(),
        Some("false")
    );
}

#[test]
fn a_write_without_a_project_lands_only_while_none_is_open() {
    let state = AppState::new().expect("state");
    state
        .save_display_setting_for(None, "showStations", "false")
        .expect("no project on either side");

    state.execute(Command::CreateProject {
        name: "CaveA".to_string(),
        location: None,
    });
    let refused = state.save_display_setting_for(None, "showShots", "false");
    assert_eq!(
        refused.map_err(|e| e.code),
        Err(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(state.get_display_setting("showShots").expect("read"), None);
}

#[test]
fn a_write_for_a_closed_project_or_an_unreadable_identity_is_refused() {
    let state = AppState::new().expect("state");
    let closed =
        state.save_display_setting_for(Some(&unsaved_identity("2026-01-01T10:00:00Z")), "k", "v");
    assert_eq!(
        closed.map_err(|e| e.code),
        Err(CommandErrorCode::ProjectChanged)
    );

    state.execute(Command::CreateProject {
        name: "CaveA".to_string(),
        location: None,
    });
    let garbled =
        state.save_display_setting_for(Some(&unsaved_identity("not a timestamp")), "k", "v");
    assert_eq!(
        garbled.map_err(|e| e.code),
        Err(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(state.get_display_setting("k").expect("read"), None);
}

#[test]
fn a_write_made_for_one_copy_does_not_land_in_another_copy_of_the_same_file() {
    // [#576 NF-W8FE-3] Two copies of one project file share `created_at`, so an
    // identity of `created_at` alone could not tell them apart: a write still in
    // flight when the user switched from copy A to copy B landed in B.
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp dir");
    let path_a = temp.path().join("CaveA.stalx");
    let path_b = temp.path().join("CaveA copy.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    project.created_at = CreationStamp::parse("2026-01-01T10:00:00Z").expect("timestamp");
    Database::open(&path_a)
        .expect("create")
        .save_project(&project)
        .expect("write");
    std::fs::copy(&path_a, &path_b).expect("copy the file");

    let state = AppState::new().expect("state");
    assert!(state.open_project(path_a).success, "open copy A");
    let made_for_a = wire_identity(&state.get_project_state().expect("A is open"));
    // The user opens copy B while A's write is still in flight.
    assert!(state.open_project(path_b.clone()).success, "open copy B");

    let late = state.save_display_setting_for(Some(&made_for_a), "gridSpacing", "50");
    let landed_in_b = Database::open_readonly(&path_b)
        .expect("open B")
        .get_display_setting("gridSpacing")
        .expect("read B");
    println!(
        "[NF-W8FE-3] copy A's write after copy B opened: answer={:?}; copy B's \
         gridSpacing={landed_in_b:?} (expected PROJECT_CHANGED and None)",
        late.as_ref().map_err(|e| e.code)
    );
    assert_eq!(
        late.map_err(|e| e.code),
        Err(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(landed_in_b, None);

    // B's own write still lands.
    let made_for_b = wire_identity(&state.get_project_state().expect("B is open"));
    state
        .save_display_setting_for(Some(&made_for_b), "gridSpacing", "10")
        .expect("B's write");
    let own = Database::open_readonly(&path_b)
        .expect("open B")
        .get_display_setting("gridSpacing")
        .expect("read B");
    assert_eq!(own.as_deref(), Some("10"));
}

#[test]
fn a_write_made_just_before_a_save_as_still_lands_in_the_project() {
    // [#576 NF-W8FE-3] The file is part of the identity, and a Save As moves the
    // open project to another file without a new open: a write the frontend made
    // just before it still names the old file, and is still this project's.
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp dir");
    let path_a = temp.path().join("CaveA.stalx");
    let path_c = temp.path().join("CaveA renamed.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    project.created_at = CreationStamp::parse("2026-01-01T10:00:00Z").expect("timestamp");
    Database::open(&path_a)
        .expect("create")
        .save_project(&project)
        .expect("write");

    let state = AppState::new().expect("state");
    assert!(state.open_project(path_a).success, "open the project");
    let made_before = wire_identity(&state.get_project_state().expect("open"));
    let saved_as = state.save_project(Some(path_c.clone()));
    assert!(saved_as.success, "save as: {:?}", saved_as.error);

    let answer = state.save_display_setting_for(Some(&made_before), "gridSpacing", "25");
    let landed = Database::open_readonly(&path_c)
        .expect("open the saved-as file")
        .get_display_setting("gridSpacing")
        .expect("read it");
    println!(
        "[NF-W8FE-3] a write naming the pre-Save-As file: answer={:?}; the saved-as file's \
         gridSpacing={landed:?} (expected Ok and Some(\"25\"))",
        answer.as_ref().map_err(|e| e.code)
    );
    assert!(answer.is_ok());
    assert_eq!(landed.as_deref(), Some("25"));

    // And one naming the new file lands too.
    let made_after = wire_identity(&state.get_project_state().expect("open"));
    assert_eq!(
        made_after.file_path.as_deref(),
        path_c.to_str(),
        "the open project names its new file"
    );
    state
        .save_display_setting_for(Some(&made_after), "gridSpacing", "30")
        .expect("a write naming the new file");
}

/// The saved-as file's `gridSpacing`, read from disk.
fn grid_spacing_in(path: &Path) -> Option<String> {
    Database::open_readonly(path)
        .expect("open the file")
        .get_display_setting("gridSpacing")
        .expect("read it")
}

#[test]
fn a_write_queued_across_a_second_save_as_still_lands_in_the_project() {
    // [#576 RV10b-4] Every Save As moves the open project to another file without
    // a new open, not only the first: a write made after the first move names the
    // intermediate file, and if it is still queued across the second it names
    // neither the file the project was opened from nor the one it is at now.
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp dir");
    let path_a = temp.path().join("CaveA.stalx");
    let path_b = temp.path().join("CaveA second.stalx");
    let path_c = temp.path().join("CaveA third.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    project.created_at = CreationStamp::parse("2026-01-01T10:00:00Z").expect("timestamp");
    Database::open(&path_a)
        .expect("create")
        .save_project(&project)
        .expect("write");

    let state = AppState::new().expect("state");
    assert!(state.open_project(path_a).success, "open the project");
    let made_at_open = wire_identity(&state.get_project_state().expect("open"));
    let first = state.save_project(Some(path_b.clone()));
    assert!(first.success, "first save as: {:?}", first.error);
    let made_between = wire_identity(&state.get_project_state().expect("open"));
    assert_eq!(made_between.file_path.as_deref(), path_b.to_str());
    let second = state.save_project(Some(path_c.clone()));
    assert!(second.success, "second save as: {:?}", second.error);

    let answer = state.save_display_setting_for(Some(&made_between), "gridSpacing", "35");
    let landed = grid_spacing_in(&path_c);
    println!(
        "[RV10b-4] a write naming the file between two Save As: answer={:?}; the open \
         file's gridSpacing={landed:?} (expected Ok and Some(\"35\"))",
        answer.as_ref().map_err(|e| e.code)
    );
    assert!(answer.is_ok());
    assert_eq!(landed.as_deref(), Some("35"));

    // The file it was opened from is still its own (as after one Save As).
    state
        .save_display_setting_for(Some(&made_at_open), "gridSpacing", "36")
        .expect("a write naming the file the project was opened from");
    assert_eq!(grid_spacing_in(&path_c).as_deref(), Some("36"));

    // A file this session's project never was at - same `created_at`, another
    // copy - is still refused, and nothing is written.
    let elsewhere = DisplaySettingTarget {
        created_at: made_between.created_at,
        file_path: Some(
            temp.path()
                .join("CaveA copy.stalx")
                .to_string_lossy()
                .into_owned(),
        ),
    };
    let refused = state.save_display_setting_for(Some(&elsewhere), "gridSpacing", "99");
    assert_eq!(
        refused.map_err(|e| e.code),
        Err(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(grid_spacing_in(&path_c).as_deref(), Some("36"));
}

#[test]
fn a_new_open_forgets_the_files_the_previous_session_was_saved_through() {
    // [#576 RV10b-4] The files a Save As moved the project off are this SESSION's:
    // once a file is opened again, the copy left behind by an earlier Save As is
    // another copy, and a late write made for it is refused.
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp dir");
    let path_a = temp.path().join("CaveA.stalx");
    let path_b = temp.path().join("CaveA second.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    project.created_at = CreationStamp::parse("2026-01-01T10:00:00Z").expect("timestamp");
    Database::open(&path_a)
        .expect("create")
        .save_project(&project)
        .expect("write");

    let state = AppState::new().expect("state");
    assert!(state.open_project(path_a.clone()).success, "open A");
    let saved_as = state.save_project(Some(path_b));
    assert!(saved_as.success, "save as: {:?}", saved_as.error);
    let made_for_b = wire_identity(&state.get_project_state().expect("open"));
    // The user opens A again while B's write is still in flight.
    assert!(state.open_project(path_a.clone()).success, "open A again");

    let late = state.save_display_setting_for(Some(&made_for_b), "gridSpacing", "50");
    assert_eq!(
        late.map_err(|e| e.code),
        Err(CommandErrorCode::ProjectChanged)
    );
    assert_eq!(grid_spacing_in(&path_a), None);
}

/// The project's `created_at` as `path` stores it.
fn stored_created_at(path: &Path) -> String {
    Connection::open(path)
        .expect("open the file")
        .query_row("SELECT created_at FROM project", [], |row| row.get(0))
        .expect("read it")
}

#[test]
fn a_write_queued_across_a_save_of_a_project_stored_in_another_offset_lands() {
    // [#576 NF-W18-1] `created_at` is the identity a display-setting write names.
    // A project another tool stored with a non-UTC offset is saved while a write
    // the frontend made before the save is still queued: the write is still this
    // project's, and the identity a later open hands the frontend is the same.
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("CaveA.stalx");
    let stored = "2026-01-01T15:00:00+05:00";
    Database::open(&path)
        .expect("create")
        .save_project(&Project::new("CaveA".to_string(), None))
        .expect("write");
    Connection::open(&path)
        .expect("open the file")
        .execute("UPDATE project SET created_at = ?1", [stored])
        .expect("store another tool's created_at");

    let state = AppState::new().expect("state");
    assert!(state.open_project(path.clone()).success, "open the project");
    let made_before = wire_identity(&state.get_project_state().expect("open"));
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save: {:?}", saved.error);
    let after_save = stored_created_at(&path);

    let answer = state.save_display_setting_for(Some(&made_before), "gridSpacing", "40");
    let landed = grid_spacing_in(&path);
    let reread = wire_identity(&state.get_project_state().expect("open"));

    // The same across a Save As to a copy: a write made before it still lands.
    let copy = temp.path().join("CaveA copy.stalx");
    let saved_as = state.save_project(Some(copy.clone()));
    assert!(saved_as.success, "save as: {:?}", saved_as.error);
    let in_copy = stored_created_at(&copy);
    let answer_copy = state.save_display_setting_for(Some(&made_before), "gridSpacing", "45");
    let landed_copy = grid_spacing_in(&copy);
    drop(state);
    let state = AppState::new().expect("state");
    assert!(state.open_project(copy).success, "open the copy");
    let reopened = wire_identity(&state.get_project_state().expect("open"));
    println!(
        "[NF-W18-1] stored {stored:?}; after the save {after_save:?}, in the Save As copy \
         {in_copy:?}; identity made before the save {:?}, re-read after it {:?}, after \
         reopening the copy {:?}; the queued write after the save: answer={:?}, \
         gridSpacing={landed:?} (expected Ok, Some(\"40\")); after the Save As: answer={:?}, \
         gridSpacing={landed_copy:?} (expected Ok, Some(\"45\"))",
        made_before.created_at,
        reread.created_at,
        reopened.created_at,
        answer.as_ref().map_err(|e| e.code),
        answer_copy.as_ref().map_err(|e| e.code)
    );
    assert!(answer.is_ok());
    assert_eq!(landed.as_deref(), Some("40"));
    assert!(answer_copy.is_ok());
    assert_eq!(landed_copy.as_deref(), Some("45"));
    assert_eq!(
        reread.created_at, made_before.created_at,
        "re-read after the save"
    );
    assert_eq!(
        reopened.created_at, made_before.created_at,
        "after a reopen"
    );
}
