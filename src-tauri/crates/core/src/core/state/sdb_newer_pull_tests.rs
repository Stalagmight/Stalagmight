//! [#576 NF-PNEWER-2 / NF-W2F-1] A SpeleoDB download that is a file a NEWER build
//! saved, and the flags that describe the open file after an in-place tree swap.
//!
//! All data is synthetic: a project this build saves, then its `PRAGMA
//! user_version` raised above this build's schema, which is exactly what a newer
//! build's file looks like to this one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::speleodb_cache::stamp_pulled_file;
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;

/// A `.stalx` stamped one schema version above this build's.
fn newer_file(dir: &Path) -> PathBuf {
    let path = dir.join("project.stalx");
    let mut db = Database::open(&path).expect("create the fixture");
    db.save_project(&Project::new("CaveA".to_owned(), None))
        .expect("save the fixture");
    drop(db);
    let conn = rusqlite::Connection::open(&path).expect("open the fixture directly");
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("read the stamp");
    conn.execute_batch(&format!("PRAGMA user_version = {};", current + 1))
        .expect("stamp it newer");
    path
}

fn link() -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://127.0.0.1:9/".to_owned(),
        project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
        project_name: "CaveA".to_owned(),
        base_commit_sha: Some("abc123".to_owned()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

/// Rows the stamp could have written: the link and the display settings.
fn written(path: &Path) -> (i64, i64) {
    let conn = rusqlite::Connection::open(path).expect("open the file directly");
    let links: i64 = conn
        .query_row("SELECT COUNT(*) FROM speleo_link", [], |r| r.get(0))
        .expect("count link rows");
    let settings: i64 = conn
        .query_row("SELECT COUNT(*) FROM display_settings", [], |r| r.get(0))
        .expect("count display settings");
    (links, settings)
}

#[test]
fn a_pulled_newer_file_gets_no_link_row_and_no_display_settings() {
    // [#576 NF-PNEWER-2] The open, fast-forward and revision-swap pulls stamp the
    // downloaded file with this build's link row (INSERT OR REPLACE with this
    // build's columns) and the preserved display settings. Into a file a newer
    // build saved, that write resets any link column the newer build added - the
    // downgrade a newer file is otherwise protected from (#576 B6-D1). Only a
    // first-time Open installs one now (#576 RV4-R-S7-D2; the rest refuse it
    // before the swap), so this is that Open's stamp.
    let dir = TempDir::new().expect("tempdir");
    let path = newer_file(dir.path());
    let before = written(&path);
    let db = Database::open_existing(&path).expect("a newer file still opens");
    assert!(db.newer_schema_version().is_some(), "fixture: newer");
    let view = HashMap::from([("show_labels".to_owned(), "false".to_owned())]);

    let result = stamp_pulled_file(&db, &view, |_| link());
    drop(db);

    let after = written(&path);
    println!(
        "pulled newer file: (link rows, display settings) before {before:?}, after the stamp \
         {after:?}; stamp -> {result:?}"
    );
    assert!(result.is_ok(), "skipping the stamp is not a failure");
    assert_eq!(
        after, before,
        "nothing of this build's is written into a newer build's file"
    );
}

#[test]
fn a_pulled_file_of_this_build_is_still_stamped() {
    // DO-NOT-REGRESS: the ordinary download gets its link row and view back.
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("project.stalx");
    let mut db = Database::open(&path).expect("create");
    db.save_project(&Project::new("CaveA".to_owned(), None))
        .expect("save");
    drop(db);
    let db = Database::open_existing(&path).expect("open");
    let view = HashMap::from([("show_labels".to_owned(), "false".to_owned())]);

    stamp_pulled_file(&db, &view, |_| link()).expect("stamp");
    drop(db);

    assert_eq!(
        written(&path),
        (1, 1),
        "one link row and the restored setting"
    );
}

#[test]
fn the_open_file_flags_describe_the_file_open_now() {
    // [#576 NF-W2F-1] What the frontend re-asks after an in-place tree swap: the
    // flags of the file the backend has open NOW, not the ones the last frontend
    // `loadProject` reported. Driven across opens of different bytes, which is what
    // a swap does at one path.
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let state = super::AppState::new().expect("state");

    let current = dir.path().join("current.stalx");
    let mut db = Database::open(&current).expect("create");
    db.save_project(&Project::new("CaveA".to_owned(), None))
        .expect("save");
    drop(db);
    assert!(state.open_project(current).success);
    let flags = state.open_file_flags();
    println!("current-format file -> {flags:?}");
    assert_eq!(flags.newer_schema_version, None);
    assert!(!flags.speleo_link_unreadable);

    let newer_dir = TempDir::new().expect("tempdir");
    let newer = newer_file(newer_dir.path());
    assert!(state.open_project(newer).success);
    let flags = state.open_file_flags();
    println!("newer-format file -> {flags:?}");
    assert!(flags.newer_schema_version.is_some());

    let broken = dir.path().join("broken-link.stalx");
    let mut db = Database::open(&broken).expect("create");
    db.save_project(&Project::new("CaveA".to_owned(), None))
        .expect("save");
    db.save_speleo_link(Some(&link())).expect("link");
    drop(db);
    rusqlite::Connection::open(&broken)
        .expect("open directly")
        .execute_batch("ALTER TABLE speleo_link RENAME COLUMN project_id TO broken_id;")
        .expect("break the link row");
    assert!(state.open_project(broken).success);
    let flags = state.open_file_flags();
    println!("unreadable-link file -> {flags:?}");
    assert_eq!(flags.newer_schema_version, None);
    assert!(flags.speleo_link_unreadable);
}
