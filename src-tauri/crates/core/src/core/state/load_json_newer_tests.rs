//! [#576 P-NEWER open question 1] The JSON `load_project_full` result says when the
//! file was saved by a newer build.
//!
//! The binary envelope (what the frontend loads through) has carried
//! `newer_schema_version` since B6-D1; the JSON result skipped it to keep its
//! generated binding unchanged, so a client of the registered JSON command was
//! handed a newer-format project with nothing saying it is read-only. All data is
//! synthetic.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::models::Project;
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

fn project_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut db = Database::open(&path).expect("create");
    db.save_project(&Project::new("CaveA".to_owned(), None))
        .expect("save");
    path
}

/// The JSON the `load_project_full` command sends for `path`.
fn load_json(path: PathBuf) -> serde_json::Value {
    let state = state_with_null_sink();
    let loaded = state
        .do_load_project_full(path)
        .unwrap_or_else(|e| panic!("load failed: {e:?}"));
    serde_json::to_value(&loaded).expect("serialize the load result")
}

#[test]
fn the_json_load_result_carries_the_newer_schema_version() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let path = project_file(dir.path(), "newer.stalx");
    let conn = rusqlite::Connection::open(&path).expect("open directly");
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("read the stamp");
    conn.execute_batch(&format!("PRAGMA user_version = {};", current + 1))
        .expect("stamp it newer");
    drop(conn);

    let json = load_json(path);

    let field = json.get("newerSchemaVersion").cloned();
    println!(
        "JSON load_project_full of a file stamped {}: newerSchemaVersion = {field:?}",
        current + 1
    );
    assert_eq!(
        field,
        Some(serde_json::json!(current + 1)),
        "the JSON result names the newer schema, as the binary envelope does"
    );
}

#[test]
fn the_json_load_result_of_a_current_file_says_null() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let json = load_json(project_file(dir.path(), "current.stalx"));
    assert_eq!(
        json.get("newerSchemaVersion"),
        Some(&serde_json::Value::Null)
    );
}
