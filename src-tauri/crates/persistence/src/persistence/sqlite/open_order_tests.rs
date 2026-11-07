//! [#576 NF-PNEWER-1] `Database::open` decides what a file is BEFORE it writes
//! to it. It used to run `init_schema` (CREATE TABLE IF NOT EXISTS for every
//! table this build knows) on an existing file first, so a file a newer build
//! wrote gained the tables this build knows and the newer schema dropped, and
//! a pre-release file was refused only after its missing tables were
//! recreated in it.
//!
//! All files are synthetic projects this build writes, then reshaped with raw
//! SQL. No fixture is read.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::TempDir;

use super::{Database, SCHEMA_VERSION};
use crate::models::Project;

/// A current project file at `dir/name`, then reshaped by `surgery`.
fn reshaped_file(dir: &Path, name: &str, surgery: &str) -> PathBuf {
    let path = dir.join(name);
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("CaveA".to_string(), None))
        .unwrap();
    drop(db);
    Connection::open(&path)
        .unwrap()
        .execute_batch(surgery)
        .unwrap();
    path
}

fn has_table(path: &Path, table: &str) -> bool {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

/// A file a NEWER build wrote, without a table this build knows (the newer
/// schema dropped it), comes through `Database::open` exactly as it was.
#[test]
fn database_open_leaves_a_newer_file_untouched() {
    let temp = TempDir::new().unwrap();
    let newer = SCHEMA_VERSION + 1;
    let path = reshaped_file(
        temp.path(),
        "CaveA.stalx",
        &format!(
            "DROP TABLE station_comments; UPDATE project SET schema_version = {newer}; \
             PRAGMA user_version = {newer};"
        ),
    );
    let before = std::fs::read(&path).unwrap();

    let opened = Database::open(&path).map(drop);
    let unchanged = std::fs::read(&path).unwrap() == before;
    let recreated = has_table(&path, "station_comments");
    println!(
        "Database::open(newer file) -> {opened:?}; file unchanged: {unchanged}; \
         station_comments recreated: {recreated}"
    );
    assert!(opened.is_ok(), "{opened:?}");
    assert!(
        !recreated,
        "no table this build knows may be added to a newer file"
    );
    assert!(
        unchanged,
        "a newer file must be left byte-for-byte as it was"
    );
}

/// [#576 RV4-R-S7] The header read the SpeleoDB pulls refuse a newer download by
/// gives the open's own answer: for a newer file, for a current one, and for bytes
/// that are no database at all.
#[test]
fn the_newer_version_read_off_the_header_is_the_opens_answer() {
    let temp = TempDir::new().unwrap();
    let newer = SCHEMA_VERSION + 1;
    let newer_path = reshaped_file(
        temp.path(),
        "Newer.stalx",
        &format!("UPDATE project SET schema_version = {newer}; PRAGMA user_version = {newer};"),
    );
    let current_path = reshaped_file(temp.path(), "Current.stalx", "");

    for path in [&newer_path, &current_path] {
        let bytes = std::fs::read(path).unwrap();
        let from_header = Database::newer_schema_version_in_header(&bytes[..Database::HEADER_LEN]);
        let from_open = Database::open_existing(path)
            .unwrap()
            .newer_schema_version();
        println!(
            "{}: header says {from_header:?}, the open says {from_open:?}",
            path.display()
        );
        assert_eq!(from_header, from_open);
    }
    assert_eq!(
        Database::newer_schema_version_in_header(
            &std::fs::read(&newer_path).unwrap()[..Database::HEADER_LEN]
        ),
        Some(newer)
    );
    assert_eq!(
        Database::newer_schema_version_in_header(b"PK\x03\x04 not a database"),
        None
    );
    assert_eq!(
        Database::newer_schema_version_in_header(b"SQLite format 3\0"),
        None,
        "a header cut short says nothing"
    );
}

/// A pre-release file is refused before anything is written to it, on this
/// path too (docs/saving.md: "refused before anything is written to it").
#[test]
fn database_open_refuses_a_pre_release_file_before_writing_to_it() {
    let temp = TempDir::new().unwrap();
    let path = reshaped_file(
        temp.path(),
        "CaveA.stalx",
        "DROP TABLE station_comments; UPDATE project SET schema_version = 1; \
         PRAGMA user_version = 0;",
    );
    let before = std::fs::read(&path).unwrap();

    let refused = Database::open(&path).err();
    let typed = refused
        .as_ref()
        .and_then(|e| e.downcast_ref::<super::PreReleaseFile>().copied());
    let message = refused.map(|e| format!("{e:#}")).unwrap_or_default();
    let unchanged = std::fs::read(&path).unwrap() == before;
    let recreated = has_table(&path, "station_comments");
    println!(
        "Database::open(pre-release file) -> {message}; file unchanged: {unchanged}; \
         station_comments recreated: {recreated}"
    );
    assert!(typed.is_some(), "refused as a pre-release file: {message}");
    assert!(!recreated, "a refused file gains no tables");
    assert!(
        unchanged,
        "a refused file must be left byte-for-byte as it was"
    );
}
