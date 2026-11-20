//! [#649] The app's own paths into a project refuse a `.stalx` a newer
//! Stalagmight saved, leave it byte-for-byte as it was, and change no session
//! state: a version 2 file stamped with a higher format version, and an old
//! SQLite file stamped with a newer schema.
//!
//! All data is synthetic: projects built in memory and written by this
//! build, then re-stamped as if a newer build had written them; the SQLite
//! one is a temporary copy of the tracked old-format fixture. No fixture is
//! written.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use super::AppState;
use crate::core::events::EventSink;
use crate::models::survey::Project;
use crate::persistence::stalx_v2::{self, Extras, Mode};

/// The load needs a sink for its progress events; nothing listens here.
struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

/// A version 2 project at `dir/name`, re-stamped format version `version`.
fn project_file(dir: &TempDir, name: &str, version: u32) -> PathBuf {
    let path = dir.path().join(name);
    let bytes = stalx_v2::encode(&Project::new(name, None), &Extras::default(), Mode::Normal)
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let stamped = text.replacen("\"version\":2", &format!("\"version\":{version}"), 1);
    assert!(version == 2 || stamped != text, "setup: re-stamped");
    std::fs::write(&path, stamped).unwrap();
    path
}

/// A copy of the tracked old-format fixture at `dir/name`, its SQLite
/// schema stamped `version`.
fn legacy_file(dir: &TempDir, name: &str, version: i32) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures/fractal_plant_iter3.stalx");
    let path = dir.path().join(name);
    std::fs::copy(&fixture, &path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", version).unwrap();
    drop(conn);
    path
}

fn bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

#[test]
fn the_load_refuses_a_newer_project_with_its_own_code() {
    let dir = TempDir::new().unwrap();
    let path = project_file(&dir, "newer.stalx", 3);
    let before = bytes(&path);
    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(NullSink));

    let Err(err) = state.do_load_project_full(path.clone()) else {
        panic!("a project from a newer format must not load");
    };

    assert_eq!(err.code, "NEWER_SCHEMA", "{}", err.message);
    let newer = crate::models::NewerSchema {
        file: "newer.stalx".to_string(),
        found: 3,
        supported: 2,
    };
    // The payload the frontend renders its string from, and the same wording in
    // the message for anything that only relays it.
    assert_eq!(err.newer_schema.as_ref(), Some(&newer));
    assert_eq!(err.message, newer.to_string());
    assert_eq!(
        err.details.as_deref(),
        Some(path.display().to_string().as_str())
    );
    assert_eq!(bytes(&path), before);
    assert!(
        state.project.lock().unwrap().is_none(),
        "no project was installed"
    );
}

#[test]
fn the_load_refuses_an_old_format_file_from_a_newer_schema() {
    let dir = TempDir::new().unwrap();
    let path = legacy_file(&dir, "legacy.stalx", 1000);
    let before = bytes(&path);
    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(NullSink));

    let Err(err) = state.do_load_project_full(path.clone()) else {
        panic!("a project from a newer schema must not load");
    };
    assert_eq!(err.code, "NEWER_SCHEMA", "{}", err.message);
    let newer = err.newer_schema.unwrap();
    assert_eq!(newer.found, 1000);
    assert!(newer.supported < 1000);
    assert_eq!(bytes(&path), before);
    assert!(state.project.lock().unwrap().is_none());
}

/// The executor's open (`Command::OpenProject`) refuses it too, and names
/// the remedy in its message.
#[test]
fn the_plain_open_refuses_a_newer_project() {
    let dir = TempDir::new().unwrap();
    for path in [
        project_file(&dir, "newer.stalx", 3),
        legacy_file(&dir, "legacy.stalx", 1000),
    ] {
        let before = bytes(&path);
        let state = AppState::new().unwrap();

        let result = state.open_project(path.clone());

        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("Update Stalagmight to open it.")),
            "{:?}",
            result.error
        );
        assert_eq!(bytes(&path), before);
        assert!(state.project.lock().unwrap().is_none());
    }
}

#[test]
#[ignore = "C8: Merge Projects is not ported on this branch"]
fn a_merge_with_a_newer_source_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let current = project_file(&dir, "current.stalx", 2);
    let newer = project_file(&dir, "newer.stalx", 3);
    let (current_before, newer_before) = (bytes(&current), bytes(&newer));
    let output = dir.path().join("merged.stalx");
    let state = AppState::new().unwrap();

    let result = state.merge_projects(
        vec![current.clone(), newer.clone()],
        output.clone(),
        "Merged".to_string(),
        true,
    );

    assert!(!result.success);
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("newer version of Stalagmight")),
        "{:?}",
        result.error
    );
    assert!(!output.exists(), "no merged file");
    assert!(!output.with_extension("stalx.tmp").exists(), "no temp file");
    // `archive_originals` must not have moved either source.
    assert_eq!(bytes(&current), current_before);
    assert_eq!(bytes(&newer), newer_before);
}

/// Renaming a project from the Recents panel writes into the file by path.
#[test]
fn renaming_a_newer_project_by_path_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let path = project_file(&dir, "rename.stalx", 3);
    let before = bytes(&path);
    let state = AppState::new().unwrap();

    let result = state.set_project_name_for_path(path.clone(), "Renamed".to_string());

    assert!(!result.success, "{result:?}");
    assert_eq!(bytes(&path), before);
}
