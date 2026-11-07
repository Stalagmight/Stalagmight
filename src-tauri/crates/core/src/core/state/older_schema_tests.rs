//! [#576 B1-D1, B6-D2] Opening a project saved by an OLDER Stalagmight.
//!
//! - A file from before the first public release (schema below 2) is refused
//!   with its own load-error code and a message that says to re-import, and is
//!   left byte-for-byte as it was.
//! - An older file that IS supported is upgraded on open, and one copy of it as
//!   it was is kept beside it first (`<name>.stalx.pre-v<old>.backup`).
//!
//! All data is synthetic: a project built in memory, saved by this build, then
//! re-stamped as an older file. No fixture is read or written.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// A current file with one section of two shots, then re-stamped by `surgery`.
fn build_file(dir: &Path, name: &str, surgery: &str) -> PathBuf {
    let path = dir.join(name);
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        color: "#123456".to_string(),
        shots: vec![
            ShotData {
                length: 5.0,
                ..Default::default()
            },
            ShotData {
                length: 7.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(surgery)
        .unwrap();
    path
}

/// [#576 B1-D1] The production load path refuses a pre-release file with the
/// `PRE_RELEASE_FILE` code and a message the loading dialog can show as-is, and
/// writes nothing to the file.
#[test]
fn the_load_path_refuses_a_pre_release_file_with_its_own_code() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = build_file(
        temp.path(),
        "pre_release.stalx",
        "UPDATE project SET schema_version = 1; PRAGMA user_version = 0;",
    );
    let before = std::fs::read(&path).unwrap();

    let state = state_with_null_sink();
    let Err(error) = state.do_load_project_full(path.clone()) else {
        panic!("a pre-release file must not open");
    };
    println!(
        "load_project_full -> code={} message={:?}",
        error.code, error.message
    );
    assert_eq!(error.code, crate::models::CommandErrorCode::PreReleaseFile);
    assert!(
        error
            .message
            .starts_with("This project was saved by a pre-release version"),
        "the message is shown to the user as-is, so it must not carry the code prefix: {:?}",
        error.message
    );
    assert!(
        error.message.contains("import it again"),
        "the message must say what to do: {:?}",
        error.message
    );
    assert!(
        state.get_project_state().is_none(),
        "a refused open leaves no project"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a refused file is left as it was"
    );
}

/// [#576 B6-D2] The production load path upgrades an older file and keeps one
/// copy of it, as it was, beside it - the only way back from a one-way upgrade.
#[test]
fn the_load_path_keeps_a_copy_of_an_older_file_before_upgrading_it() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = build_file(
        temp.path(),
        "CaveA.stalx",
        "UPDATE project SET schema_version = 16; PRAGMA user_version = 16;",
    );
    let before = std::fs::read(&path).unwrap();

    let state = state_with_null_sink();
    let loaded = state
        .do_load_project_full(path.clone())
        .unwrap_or_else(|e| panic!("a supported older file must open: {e:?}"));
    assert_eq!(loaded.project.sections[0].shots.len(), 2);

    let copy = temp.path().join("CaveA.stalx.pre-v16.backup");
    let kept = std::fs::read(&copy).unwrap_or_default();
    println!(
        "copy {:?}: {} bytes, identical to the file before the upgrade: {}",
        copy.file_name().unwrap(),
        kept.len(),
        kept == before
    );
    assert!(kept == before, "the copy is the file exactly as it was");
    let stamped: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert!(stamped > 16, "and the file itself was upgraded");
}
