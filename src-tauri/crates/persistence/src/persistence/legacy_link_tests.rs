//! The legacy SpeleoDB link reader on files the 0.6.0 save wrote (the frozen
//! writer), on the narrower shapes older builds wrote, and on hostile input.

use std::path::{Path, PathBuf};
use std::time::Duration;

use stalagmight_legacy::models::{SpeleoLink as OldLink, UndoMode as OldMode};
use stalagmight_legacy::{write_v18, LegacyExtras, LegacyProject};

use super::legacy_link::{read_cached_copy, read_link, ReadOptions};
use crate::models::UndoMode;

const ID: &str = "11111111-1111-1111-1111-111111111111";

fn old_link(mode: Option<OldMode>, dirty: bool) -> OldLink {
    OldLink {
        endpoint: "https://sdb.example/".to_owned(),
        project_id: ID.to_owned(),
        project_name: "Synthetic".to_owned(),
        base_commit_sha: Some("abc1234".to_owned()),
        has_unpushed_edits: dirty,
        last_edit_at: dirty.then(|| "2026-11-09T10:00:00Z".to_owned()),
        last_verify_at: None,
        undo_mode: mode,
    }
}

/// A `.stalx` exactly as the 0.6.0 save writes it, with `link`.
fn write_legacy(dir: &Path, name: &str, link: Option<OldLink>) -> PathBuf {
    let extras = LegacyExtras {
        speleo_link: link,
        ..LegacyExtras::default()
    };
    let bytes = write_v18(&LegacyProject::new("Synthetic".to_owned(), None), &extras)
        .expect("the frozen writer writes a synthetic project");
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write the legacy file");
    path
}

#[test]
fn reads_the_link_row_the_0_6_0_save_wrote() {
    let dir = tempfile::tempdir().expect("temp");
    let path = write_legacy(
        dir.path(),
        "a.stalx",
        Some(old_link(Some(OldMode::Disabled), true)),
    );
    let link = read_link(&path, &ReadOptions::default())
        .expect("readable")
        .expect("linked");
    assert_eq!(link.project_id, ID);
    assert_eq!(link.endpoint, "https://sdb.example/");
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc1234"));
    assert!(link.has_unpushed_edits);
    assert_eq!(link.last_edit_at.as_deref(), Some("2026-11-09T10:00:00Z"));
    assert_eq!(link.undo_mode, Some(UndoMode::Disabled));

    let null_mode = write_legacy(dir.path(), "b.stalx", Some(old_link(None, false)));
    let link = read_link(&null_mode, &ReadOptions::default())
        .expect("readable")
        .expect("linked");
    assert_eq!(link.undo_mode, None, "a legacy NULL mode stays NULL");
    assert!(!link.has_unpushed_edits);
}

#[test]
fn an_unlinked_file_reads_as_no_link_and_is_never_written() {
    let dir = tempfile::tempdir().expect("temp");
    let path = write_legacy(dir.path(), "plain.stalx", None);
    let before = std::fs::read(&path).expect("bytes");
    assert_eq!(
        read_link(&path, &ReadOptions::default()).expect("readable"),
        None
    );
    assert_eq!(
        read_link(
            &path,
            &ReadOptions {
                recover: true,
                ..ReadOptions::default()
            }
        )
        .expect("readable"),
        None
    );
    assert_eq!(
        std::fs::read(&path).expect("bytes"),
        before,
        "no byte changed"
    );
}

#[test]
fn a_narrow_pre_v10_link_table_reads_with_its_missing_columns_unset() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("narrow.stalx");
    let conn = rusqlite::Connection::open(&path).expect("create");
    conn.execute_batch(
        "CREATE TABLE speleo_link (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             endpoint TEXT NOT NULL,
             project_id TEXT NOT NULL,
             project_name TEXT NOT NULL,
             base_commit_sha TEXT,
             has_unpushed_edits INTEGER NOT NULL DEFAULT 0
         );
         INSERT INTO speleo_link VALUES (1, 'http://x/', '11111111-1111-1111-1111-111111111111', 'Old', NULL, 1);",
    )
    .expect("narrow table");
    drop(conn);
    let link = read_link(&path, &ReadOptions::default())
        .expect("a narrow table is readable")
        .expect("linked");
    assert_eq!(
        link.base_commit_sha, None,
        "a NULL base is never coerced to \"\""
    );
    assert!(link.has_unpushed_edits);
    assert_eq!(link.last_edit_at, None);
    assert_eq!(link.last_verify_at, None);
    assert_eq!(link.undo_mode, None);
}

#[test]
fn a_file_that_is_not_sqlite_or_is_missing_is_an_error_not_unlinked() {
    let dir = tempfile::tempdir().expect("temp");
    let junk = dir.path().join("junk.stalx");
    std::fs::write(&junk, b"this is not a database at all, not even close").expect("junk");
    assert!(read_link(&junk, &ReadOptions::default()).is_err());
    assert!(read_link(&dir.path().join("missing.stalx"), &ReadOptions::default()).is_err());
}

#[test]
fn a_schema_that_could_run_file_chosen_sql_is_refused() {
    let dir = tempfile::tempdir().expect("temp");
    let path = write_legacy(dir.path(), "view.stalx", Some(old_link(None, false)));
    rusqlite::Connection::open(&path)
        .expect("open")
        .execute_batch("CREATE VIEW sneaky AS SELECT * FROM speleo_link;")
        .expect("add a view");
    assert!(
        read_link(&path, &ReadOptions::default()).is_err(),
        "the frozen schema guard refuses a view before anything is read"
    );
}

#[test]
fn the_untrusted_caps_refuse_an_oversized_value_and_an_expired_deadline() {
    let dir = tempfile::tempdir().expect("temp");
    let path = write_legacy(
        dir.path(),
        "big.stalx",
        Some(old_link(Some(OldMode::Full), false)),
    );
    rusqlite::Connection::open(&path)
        .expect("open")
        .execute(
            "UPDATE speleo_link SET project_name = ?1",
            ["X".repeat(8192)],
        )
        .expect("oversized synthetic name");
    let capped = ReadOptions {
        max_value_bytes: Some(1024),
        deadline: Some(Duration::from_secs(30)),
        ..ReadOptions::default()
    };
    assert!(read_link(&path, &capped).is_err(), "the value cap");
    let expired = ReadOptions {
        max_value_bytes: Some(64 * 1024 * 1024),
        deadline: Some(Duration::ZERO),
        ..ReadOptions::default()
    };
    assert!(read_link(&path, &expired).is_err(), "the deadline");
    let generous = ReadOptions {
        max_value_bytes: Some(64 * 1024 * 1024),
        deadline: Some(Duration::from_secs(30)),
        ..ReadOptions::default()
    };
    assert_eq!(
        read_link(&path, &generous)
            .expect("within the caps")
            .expect("linked")
            .undo_mode,
        Some(UndoMode::Full)
    );
}

fn journal_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-journal");
    PathBuf::from(name)
}

/// Copy `src` to `dest` with a live HOT rollback journal (a kill mid-write):
/// the page cache must spill for SQLite to write the journal's magic.
fn copy_with_a_hot_journal(src: &Path, dest: &Path) {
    let conn = rusqlite::Connection::open(src).expect("open the source");
    conn.execute_batch("PRAGMA cache_size = 2; BEGIN IMMEDIATE; CREATE TABLE dirty (x);")
        .expect("write transaction");
    {
        let mut insert = conn
            .prepare("INSERT INTO dirty VALUES (?1)")
            .expect("prepare");
        let filler = "x".repeat(400);
        for _ in 0..500 {
            insert.execute([&filler]).expect("spill the cache");
        }
    }
    std::fs::copy(src, dest).expect("copy mid-transaction");
    std::fs::copy(journal_of(src), journal_of(dest)).expect("copy the live journal");
    conn.execute_batch("ROLLBACK;")
        .expect("roll the source back");
}

#[test]
fn a_hot_journal_defeats_the_read_only_read_and_only_the_recovering_one_reads_it() {
    let dir = tempfile::tempdir().expect("temp");
    let src = write_legacy(dir.path(), "src.stalx", Some(old_link(None, true)));
    rusqlite::Connection::open(&src)
        .expect("open")
        .pragma_update(None, "user_version", 1)
        .expect("backdate");
    let hot = dir.path().join("hot.stalx");
    copy_with_a_hot_journal(&src, &hot);
    assert!(
        read_link(&hot, &ReadOptions::default()).is_err(),
        "a read-only connection cannot roll a hot journal back"
    );
    let link = read_link(
        &hot,
        &ReadOptions {
            recover: true,
            ..ReadOptions::default()
        },
    )
    .expect("the recovering read rolls the journal back")
    .expect("linked");
    assert!(link.has_unpushed_edits);
    let version: i64 = rusqlite::Connection::open(&hot)
        .expect("open")
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 1, "the recovering read migrates nothing");
}

#[test]
fn a_cache_copy_at_the_frozen_version_reads_its_link_and_view_toggles() {
    // Review G-speleodb-6: the SpeleoDB cache's reader reads these two tables, not
    // the whole project.
    let dir = tempfile::tempdir().expect("temp");
    let extras = LegacyExtras {
        speleo_link: Some(old_link(Some(OldMode::Full), true)),
        display_settings: [
            ("showJumps".to_owned(), "false".to_owned()),
            ("gridSpacing".to_owned(), "25".to_owned()),
        ]
        .into_iter()
        .collect(),
        ..LegacyExtras::default()
    };
    let bytes = write_v18(&LegacyProject::new("Synthetic".to_owned(), None), &extras)
        .expect("the frozen writer writes a synthetic project");
    let path = dir.path().join("cached.stalx");
    std::fs::write(&path, &bytes).expect("write the legacy file");

    let copy = read_cached_copy(&path, &ReadOptions::default()).expect("readable");
    let link = copy.link.expect("linked");
    assert_eq!(link.project_id, ID);
    assert!(link.has_unpushed_edits);
    assert_eq!(link.undo_mode, Some(UndoMode::Full));
    assert_eq!(
        copy.display.get("showJumps").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        copy.display.get("gridSpacing").map(String::as_str),
        Some("25")
    );
    assert_eq!(
        std::fs::read(&path).expect("bytes"),
        bytes,
        "no byte changed"
    );
}

#[test]
fn a_cache_copy_at_another_schema_version_is_left_to_the_full_open() {
    // The frozen open migrates an older file, renaming and adding view-toggle keys
    // on the way, so the cheap read refuses it rather than answer differently.
    let dir = tempfile::tempdir().expect("temp");
    let path = write_legacy(dir.path(), "old.stalx", Some(old_link(None, false)));
    rusqlite::Connection::open(&path)
        .expect("open")
        .pragma_update(None, "user_version", 12)
        .expect("backdate");
    assert!(read_cached_copy(&path, &ReadOptions::default()).is_err());
    assert!(
        read_link(&path, &ReadOptions::default()).is_ok(),
        "the link alone reads at any version"
    );
}
