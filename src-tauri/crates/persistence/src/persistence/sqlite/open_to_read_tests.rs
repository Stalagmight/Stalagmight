//! [#576 RV6b-R1-D1] `Database::open_existing_to_read` writes nothing.
//!
//! An older-format file is upgraded on a copy in memory: the handle reads the
//! upgraded copy, and the file and its folder are left byte-for-byte as they
//! were - where `open_existing` upgrades the file in place and keeps a backup
//! copy beside it. A current file opens as `open_existing` opens it.
//!
//! All data is synthetic: a project saved by this build, re-stamped as older.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::SCHEMA_VERSION;
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;

const OLDER_FORMAT: &str = "UPDATE project SET schema_version = 16; PRAGMA user_version = 16;";

fn project_file(dir: &Path, surgery: &str) -> PathBuf {
    let path = dir.join("CaveR.stalx");
    let mut project = Project::new("CaveR".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionR".to_string()),
        shots: vec![
            ShotData {
                length: 4.0,
                ..Default::default()
            },
            ShotData {
                length: 6.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();
    if !surgery.is_empty() {
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(surgery)
            .unwrap();
    }
    path
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn an_older_file_is_read_through_an_upgraded_copy_and_left_as_it_was() {
    let dir = TempDir::new().unwrap();
    let path = project_file(dir.path(), OLDER_FORMAT);
    let before = std::fs::read(&path).unwrap();

    let db = Database::open_existing_to_read(&path).unwrap();
    let copy_version = db.get_user_version().unwrap();
    let shots = db.load_project().unwrap().sections[0].shots.len();
    drop(db);
    let after = std::fs::read(&path).unwrap();
    let on_disk_version: i32 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    println!(
        "older file opened to read: copy at v{copy_version}, {shots} shots read; file \
         unchanged: {} (still v{on_disk_version}); folder: {:?}",
        after == before,
        files_in(dir.path())
    );
    assert_eq!(
        copy_version, SCHEMA_VERSION,
        "the copy read is the upgraded one"
    );
    assert_eq!(shots, 2);
    assert!(after == before, "the file is not upgraded in place");
    assert_eq!(on_disk_version, 16);
    assert_eq!(
        files_in(dir.path()),
        ["CaveR.stalx"],
        "no backup copy beside it"
    );
}

#[test]
fn an_older_file_in_wal_mode_leaves_nothing_beside_it() {
    // [#576 RV8a-3] The copy into memory is a `VACUUM INTO` run on a second
    // connection to the source. Opened READ-ONLY, that connection created the WAL
    // index of a file in WAL journal mode and could not remove it on close, so
    // `<name>-wal` and `<name>-shm` were left beside a file this open promises to
    // leave as it was, folder included.
    let dir = TempDir::new().unwrap();
    let path = project_file(
        dir.path(),
        &format!("{OLDER_FORMAT} PRAGMA journal_mode = WAL;"),
    );
    let before = std::fs::read(&path).unwrap();
    let folder_before = files_in(dir.path());

    let db = Database::open_existing_to_read(&path).unwrap();
    let copy_version = db.get_user_version().unwrap();
    drop(db);
    let folder_after = files_in(dir.path());
    println!(
        "older WAL-mode file opened to read: copy at v{copy_version}; file unchanged: {}; \
         folder before {folder_before:?}, after {folder_after:?}",
        std::fs::read(&path).unwrap() == before
    );
    assert_eq!(
        copy_version, SCHEMA_VERSION,
        "the copy read is the upgraded one"
    );
    assert_eq!(folder_before, ["CaveR.stalx"], "fixture: nothing beside it");
    assert_eq!(folder_after, folder_before, "and still nothing beside it");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file is unchanged"
    );
}

#[test]
fn a_current_file_opens_as_open_existing_opens_it() {
    let dir = TempDir::new().unwrap();
    let path = project_file(dir.path(), "");
    let before = std::fs::read(&path).unwrap();

    let db = Database::open_existing_to_read(&path).unwrap();
    let file_backed = db.conn.path().is_some_and(|p| !p.is_empty());
    let shots = db.load_project().unwrap().sections[0].shots.len();
    drop(db);
    println!(
        "current file opened to read: file-backed handle: {file_backed}, {shots} shots; \
         file unchanged: {}",
        std::fs::read(&path).unwrap() == before
    );
    assert!(file_backed, "a current file is read in place, not copied");
    assert_eq!(shots, 2);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(files_in(dir.path()), ["CaveR.stalx"]);
}

#[test]
fn a_pre_release_file_is_still_refused() {
    let dir = TempDir::new().unwrap();
    let path = project_file(
        dir.path(),
        "UPDATE project SET schema_version = 1; PRAGMA user_version = 1;",
    );
    let before = std::fs::read(&path).unwrap();
    let refused = Database::open_existing_to_read(&path);
    println!(
        "pre-release file opened to read: refused as pre-release: {}",
        refused.as_ref().err().is_some_and(|e| e
            .downcast_ref::<crate::persistence::PreReleaseFile>()
            .is_some())
    );
    assert!(refused.err().is_some_and(|e| e
        .downcast_ref::<crate::persistence::PreReleaseFile>()
        .is_some()));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
