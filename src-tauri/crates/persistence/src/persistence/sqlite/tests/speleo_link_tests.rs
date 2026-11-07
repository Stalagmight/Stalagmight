//! The SpeleoDB hand-off link row (speleo_link, #189): round trips, migrations and the
//! carry-forward fence (#190).
//!
//! Lines 4875-5680 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`). `speleo_link_column_info` (former
//! lines 5193-5212) moved to `speleo_undo_chain_tests.rs`, its only user.

use super::*;

// ===========================================================================
// SpeleoDB hand-off link (speleo_link, schema v9) - #189 round-trip + is_remote
// and #190 carry-forward fence. No telemetry (these live in persistence tests,
// avoiding the 418 EVENT_NAMES bump). See PRESERVED_SPELEOLINK_COLUMNS.
// ===========================================================================

// --- #189 ---

#[test]
fn speleo_link_roundtrips_through_save_and_load() {
    let db = Database::in_memory().unwrap();
    assert!(db.load_speleo_link().unwrap().is_none(), "no row initially");

    let link = sample_speleo_link(Some("abc123"), true);
    db.save_speleo_link(Some(&link)).unwrap();

    let loaded = db.load_speleo_link().unwrap().expect("row present");
    assert_eq!(loaded, link);

    // Upsert (id=1) overwrites rather than inserting a second row.
    let link2 = sample_speleo_link(Some("def456"), false);
    db.save_speleo_link(Some(&link2)).unwrap();
    assert_eq!(db.load_speleo_link().unwrap().unwrap(), link2);
    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM speleo_link", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "upsert keeps a single row");
}

#[test]
fn speleo_link_last_verify_at_roundtrips() {
    // #202 (schema v11): last_verify_at persists and reloads verbatim, and None
    // stays SQL NULL on disk (never coerced to "").
    let db = Database::in_memory().unwrap();
    let stamp = "2026-07-02T09:00:00+00:00".to_string();
    let mut link = sample_speleo_link(Some("abc123"), true);
    link.last_verify_at = Some(stamp.clone());
    db.save_speleo_link(Some(&link)).unwrap();
    assert_eq!(
        db.load_speleo_link()
            .unwrap()
            .unwrap()
            .last_verify_at
            .as_deref(),
        Some(stamp.as_str()),
    );

    // None round-trips as SQL NULL.
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc123"), false)))
        .unwrap();
    let is_null: i64 = db
        .conn
        .query_row(
            "SELECT last_verify_at IS NULL FROM speleo_link WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(is_null, 1, "None persists as SQL NULL");
}

#[test]
fn speleo_link_null_base_sha_stays_none() {
    let db = Database::in_memory().unwrap();
    // Empty-repo BASE: base_commit_sha is None.
    db.save_speleo_link(Some(&sample_speleo_link(None, false)))
        .unwrap();

    // Typed API: None stays None.
    let loaded = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(loaded.base_commit_sha, None);

    // On-disk: the column is SQL NULL, never the empty string.
    let is_null: i64 = db
        .conn
        .query_row(
            "SELECT base_commit_sha IS NULL FROM speleo_link WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(is_null, 1, "base_commit_sha is SQL NULL, not ''");
}

#[test]
fn speleo_link_clear_removes_row() {
    let db = Database::in_memory().unwrap();
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
        .unwrap();
    assert!(db.load_speleo_link().unwrap().is_some());

    db.save_speleo_link(None).unwrap();
    assert!(db.load_speleo_link().unwrap().is_none(), "row deleted");
    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM speleo_link", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn init_schema_creates_speleo_link_v11() {
    // A freshly-created file (open() → init_schema, no migrate chain) has the v11
    // table immediately.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("fresh.stalx");
    let db = Database::open(&path).unwrap();
    let exists: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='speleo_link'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 1, "init_schema created speleo_link");
    assert_eq!(
        db.get_user_version().unwrap(),
        SCHEMA_VERSION,
        "stamped at current"
    );
}

#[test]
fn migrate_creates_speleo_link_on_legacy_v8() {
    // Simulate a legacy v8 .stalx: create a normal file, then DROP the v9 table
    // and stamp user_version back to 8. open_existing() must migrate it (running
    // migrate_schema), recreating speleo_link via CREATE IF NOT EXISTS.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v8.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Legacy".to_string(), None))
            .unwrap();
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.set_user_version(8).unwrap();
    }

    // Confirm the table is genuinely absent before reopening.
    {
        let db = Database::open_for_metadata(&path).unwrap();
        let exists: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='speleo_link'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0, "v8 file has no speleo_link");
    }

    // open_existing runs ensure_schema_version → migrate_schema, gaining it.
    let db = Database::open_existing(&path).unwrap();
    let exists: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='speleo_link'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 1, "migrate_schema recreated speleo_link");
    assert_eq!(
        db.get_user_version().unwrap(),
        SCHEMA_VERSION,
        "bumped to current"
    );
}

#[test]
fn is_remote_true_when_link_present_false_when_absent() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("remote.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Remote".to_string(), None))
        .unwrap();

    // No link yet → is_remote false.
    assert!(!db.load_metadata(path.clone()).unwrap().is_remote);

    // Stamp a link → is_remote true.
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), false)))
        .unwrap();
    assert!(db.load_metadata(path.clone()).unwrap().is_remote);

    // Clear the link → is_remote false again.
    db.save_speleo_link(None).unwrap();
    assert!(!db.load_metadata(path).unwrap().is_remote);
}

#[test]
fn is_remote_false_on_metadata_open_of_pre_v9_file() {
    // open_for_metadata uses SQLITE_OPEN_READ_ONLY and runs no migration, so a
    // pre-v9 file has no speleo_link table. The EXISTS probe must tolerate that
    // (.ok().unwrap_or(false)) and report is_remote == false, not error.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pre_v9.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("PreV9".to_string(), None))
            .unwrap();
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
    }
    let db = Database::open_for_metadata(&path).unwrap();
    let meta = db.load_metadata(path).unwrap();
    assert!(!meta.is_remote, "missing table → is_remote false, no error");
}

#[test]
fn metadata_open_reads_a_pre_v10_speleo_link_without_last_edit_or_verify_columns() {
    // The missing-TABLE case has coverage (`is_remote_false_on_metadata_open_of_pre_v9_file`);
    // this is the missing-COLUMN one, which wedged projects rather than merely
    // reporting them local.
    //
    // `open_for_metadata` runs NO migration, so a `.stalx` last written by a v9/v10
    // build still has the v9/v10 `speleo_link` shape while the crate is at
    // SCHEMA_VERSION 13. Selecting `last_edit_at` (v10) / `last_verify_at` (v11)
    // unconditionally failed with `no such column`, and that error PROPAGATES - so a
    // perfectly readable legacy cache copy read as an unreadable FILE. Downstream that
    // was permanent: `guard_local_edits` refused every pull / adopt / revision-restore
    // for the project with a factually wrong "has local edits that were never pushed",
    // and the cache scan reported "no local copy" for a project that has one.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("pre_v10.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("PreV10".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc123"), false)))
            .unwrap();
        // Rebuild the table in its narrow, pre-v10 shape (SQLite cannot DROP COLUMN
        // before 3.35, and this is closer to what the old build actually wrote).
        db.conn.execute_batch(
            "DROP TABLE speleo_link;
             CREATE TABLE speleo_link (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 endpoint TEXT NOT NULL,
                 project_id TEXT NOT NULL,
                 project_name TEXT NOT NULL,
                 base_commit_sha TEXT,
                 has_unpushed_edits INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO speleo_link (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits)
             VALUES (1, 'http://10.10.0.220:8000/', '11111111-1111-1111-1111-111111111111', 'Test Cave', 'abc123', 0);",
        ).unwrap();
    }

    let db = Database::open_for_metadata(&path).unwrap();
    let link = db
        .load_speleo_link()
        .expect("a narrow speleo_link is READ, not reported as an unreadable file")
        .expect("the row is there");
    assert_eq!(link.project_id, "11111111-1111-1111-1111-111111111111");
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc123"));
    assert!(!link.has_unpushed_edits, "and known-clean, not unknown");
    // The columns this file predates read as NULL - the same value a migrated row
    // that was never stamped carries.
    assert_eq!(link.last_edit_at, None);
    assert_eq!(link.last_verify_at, None);
}

#[test]
fn metadata_open_still_propagates_a_genuinely_unreadable_file() {
    // The tolerance above must NOT become "swallow every read error". A file that is
    // not a database at all has to keep failing, because `guard_local_edits` fails
    // CLOSED on that error - it is what stops a re-pull overwriting a copy whose
    // un-pushed work could not be read.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("junk.stalx");
    std::fs::write(&path, b"not a sqlite database at all").unwrap();
    let read = Database::open_for_metadata(&path).and_then(|db| db.load_speleo_link());
    assert!(
        read.is_err(),
        "an unreadable FILE must still propagate as an error"
    );
}

#[test]
fn save_project_does_not_clear_speleo_link() {
    // D-B13c: save_project wipes/rewrites its owned tables, but speleo_link is
    // intentionally excluded so a local autosave never clears the dirty bit.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("dirty.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Dirty".to_string(), None))
        .unwrap();
    db.save_speleo_link(Some(&sample_speleo_link(Some("abc"), true)))
        .unwrap();

    // A subsequent save_project (the autosave path) must leave the link + dirty
    // bit untouched.
    db.save_project(&Project::new("Dirty Renamed".to_string(), None))
        .unwrap();

    let link = db.load_speleo_link().unwrap().expect("link survives save");
    assert!(link.has_unpushed_edits, "dirty bit NOT cleared by save");
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
}

#[test]
fn carry_forward_four_cell_raw_sql_null_stays_null() {
    // Raw-SQL on-disk: INSERT NULL base + dirty=1, force migrate_schema (drop the
    // table + stamp v8 so open_existing migrates), assert NULL stays NULL and the
    // dirty bit stays 1 - never coerced to '' / 0.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("raw_fence.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Raw".to_string(), None))
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits)
                 VALUES (1, 'http://x', 'pid', 'pname', NULL, 1)",
                [],
            )
            .unwrap();
        // Force the migrate path on next open.
        db.set_user_version(8).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let (is_null, dirty): (i64, i64) = db
        .conn
        .query_row(
            "SELECT base_commit_sha IS NULL, has_unpushed_edits FROM speleo_link WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(is_null, 1, "NULL base survives migrate (not '')");
    assert_eq!(dirty, 1, "dirty bit survives migrate");
}

#[test]
fn carry_forward_four_cell_typed_api() {
    // Typed model API: round-trip None + true through reopen, plus a Some(..) case.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("typed_fence.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Typed".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&sample_speleo_link(None, true)))
            .unwrap();
    }
    {
        let db = Database::open_existing(&path).unwrap();
        let link = db.load_speleo_link().unwrap().unwrap();
        assert_eq!(link.base_commit_sha, None);
        assert!(link.has_unpushed_edits);

        // Second case: Some("abc123") + dirty=false.
        db.save_speleo_link(Some(&sample_speleo_link(Some("abc123"), false)))
            .unwrap();
    }
    let db = Database::open_existing(&path).unwrap();
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc123"));
    assert!(!link.has_unpushed_edits);
}

// --- v10 last_edit_at ---

#[test]
fn last_edit_at_round_trips_and_survives_migration() {
    // Typed Some + None round-trip through save/load, and the value is carried
    // forward verbatim across a forced SCHEMA_VERSION migration (never reset).
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("last_edit_at.stalx");
    let stamp = "2026-06-27T12:34:56+00:00".to_string();
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Edited".to_string(), None))
            .unwrap();
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://x".to_string(),
            project_id: "pid".to_string(),
            project_name: "pname".to_string(),
            base_commit_sha: Some("abc".to_string()),
            has_unpushed_edits: true,
            last_edit_at: Some(stamp.clone()),
            last_verify_at: None,
            undo_mode: None,
        }))
        .unwrap();
        db.set_user_version(8).unwrap(); // force migrate on next open
    }

    let db = Database::open_existing(&path).unwrap();
    let loaded = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        loaded.last_edit_at.as_deref(),
        Some(stamp.as_str()),
        "last_edit_at survives migration verbatim"
    );
    assert!(loaded.has_unpushed_edits);

    // None also round-trips and is SQL NULL on disk (never coerced to "").
    db.save_speleo_link(Some(&SpeleoLink {
        last_edit_at: None,
        ..loaded
    }))
    .unwrap();
    assert_eq!(db.load_speleo_link().unwrap().unwrap().last_edit_at, None);
    let is_null: i64 = db
        .conn
        .query_row(
            "SELECT last_edit_at IS NULL FROM speleo_link WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(is_null, 1, "last_edit_at None is SQL NULL, not ''");
}

#[test]
fn migrate_v9_dirty_speleo_link_backfills_last_edit_at() {
    // A genuine v9-shaped speleo_link (no last_edit_at) gains the column via the
    // v9→v10 ALTER on open_existing. A row that was already DIRTY under v9
    // (has_unpushed_edits=1) is backfilled to the epoch so a later confirmed push
    // can clear it - WITHOUT the backfill, edits_captured_by_push(None, push_start)
    // is false and the dirty bit would stick forever (#197 review).
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v9_speleo_link.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V9".to_string(), None))
            .unwrap();
        // Re-create the table in its pre-v10 shape and stamp the version back.
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.conn
            .execute(
                "CREATE TABLE speleo_link (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    endpoint TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_name TEXT NOT NULL,
                    base_commit_sha TEXT,
                    has_unpushed_edits INTEGER NOT NULL DEFAULT 0
                )",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits)
                 VALUES (1, 'http://x', 'pid', 'pname', 'abc', 1)",
                [],
            )
            .unwrap();
        db.set_user_version(9).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let has_col: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_edit_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_col, 1, "v9→v10 ALTER added last_edit_at");
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("1970-01-01T00:00:00+00:00"),
        "a dirty v9 row is backfilled to the epoch so a confirmed push can clear it"
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
    assert!(link.has_unpushed_edits, "existing row preserved");
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
}

#[test]
fn migrate_v9_clean_speleo_link_keeps_null_last_edit_at() {
    // The v9→v10 backfill only touches DIRTY rows. A clean v9 row
    // (has_unpushed_edits=0) keeps last_edit_at NULL - there is no un-pushed edit to
    // preserve, and a fresh edit will stamp it at edit time.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v9_clean.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V9c".to_string(), None))
            .unwrap();
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.conn
            .execute(
                "CREATE TABLE speleo_link (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    endpoint TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_name TEXT NOT NULL,
                    base_commit_sha TEXT,
                    has_unpushed_edits INTEGER NOT NULL DEFAULT 0
                )",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits)
                 VALUES (1, 'http://x', 'pid', 'pname', 'abc', 0)",
                [],
            )
            .unwrap();
        db.set_user_version(9).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        link.last_edit_at, None,
        "a clean v9 row keeps last_edit_at NULL (only dirty rows are backfilled)"
    );
    assert!(!link.has_unpushed_edits, "clean row stays clean");
}

#[test]
fn migrate_v10_speleo_link_adds_last_verify_at() {
    // #202 (schema v10→v11): a genuine v10-shaped speleo_link (last_edit_at present,
    // last_verify_at ABSENT) gains the column via the idempotent ALTER on
    // open_existing, defaulting to NULL ("never verified since pull"), WITHOUT
    // disturbing the pre-existing last_edit_at / dirty bit / identity.
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v10_speleo_link.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V10".to_string(), None))
            .unwrap();
        // Re-create the table in its pre-v11 (v10) shape and stamp the version back.
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.conn
            .execute(
                "CREATE TABLE speleo_link (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    endpoint TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_name TEXT NOT NULL,
                    base_commit_sha TEXT,
                    has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                    last_edit_at TEXT
                )",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits, last_edit_at)
                 VALUES (1, 'http://x', 'pid', 'pname', 'abc', 1, '2026-06-27T10:00:00+00:00')",
                [],
            )
            .unwrap();
        db.set_user_version(10).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let has_col: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_verify_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_col, 1, "v10→v11 ALTER added last_verify_at");
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        link.last_verify_at, None,
        "the new column defaults to NULL (never verified since pull)"
    );
    // The pre-existing v10 fields survive the migration untouched.
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("2026-06-27T10:00:00+00:00"),
        "existing last_edit_at preserved across the v10→v11 ALTER"
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
    assert!(link.has_unpushed_edits, "existing dirty bit preserved");
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
}

/// [#303] Spec item 28, migration half (schema v14→v15): a genuine v14-shaped
/// `speleo_link` (no `undo_mode`) gains the column on `open_existing`, defaulting to
/// NULL (the legacy marker the application materializes on first open), without
/// disturbing any pre-existing field.
#[test]
fn migrate_v14_speleo_link_adds_undo_mode() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v14_speleo_link.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V14".to_string(), None))
            .unwrap();
        // Re-create the table in its pre-v15 (v14) shape and stamp the version back.
        db.conn.execute("DROP TABLE speleo_link", []).unwrap();
        db.conn
            .execute(
                "CREATE TABLE speleo_link (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    endpoint TEXT NOT NULL,
                    project_id TEXT NOT NULL,
                    project_name TEXT NOT NULL,
                    base_commit_sha TEXT,
                    has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                    last_edit_at TEXT,
                    last_verify_at TEXT
                )",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO speleo_link
                    (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits, last_edit_at, last_verify_at)
                 VALUES (1, 'http://x', 'pid', 'pname', 'abc', 1, '2026-06-27T10:00:00+00:00', '2026-06-27T11:00:00+00:00')",
                [],
            )
            .unwrap();
        db.set_user_version(14).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let has_col: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='undo_mode'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_col, 1, "v14→v15 ALTER added undo_mode");
    let link = db.load_speleo_link().unwrap().unwrap();
    assert_eq!(
        link.undo_mode, None,
        "the new column defaults to the legacy initialization marker"
    );
    // Every pre-existing v14 field survives the migration untouched.
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("2026-06-27T10:00:00+00:00")
    );
    assert_eq!(
        link.last_verify_at.as_deref(),
        Some("2026-06-27T11:00:00+00:00")
    );
    assert_eq!(link.base_commit_sha.as_deref(), Some("abc"));
    assert!(link.has_unpushed_edits, "existing dirty bit preserved");
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);

    // And the v15 table gained the Unwind chain pointer, empty (not mid-unwind).
    let has_chain: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='speleo_undo_chain'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_chain, 1, "v15 migration created speleo_undo_chain");
}

/// [#303] Spec item 28, round-trip half: both override values survive save/load, and
/// `None` stays NULL rather than being written as the string "full" - the two are
/// different states (only NULL keeps tracking a later change to the global default).
#[test]
fn speleo_link_undo_mode_roundtrips_and_none_stays_null() {
    use crate::models::UndoMode;

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("undo_mode.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("UndoMode".to_string(), None))
        .unwrap();

    for mode in [UndoMode::Full, UndoMode::Disabled] {
        let mut link = sample_speleo_link(Some("abc"), false);
        link.undo_mode = Some(mode);
        db.save_speleo_link(Some(&link)).unwrap();
        assert_eq!(
            db.load_speleo_link().unwrap().unwrap().undo_mode,
            Some(mode),
            "{mode:?} must survive the round trip"
        );
    }

    let mut link = sample_speleo_link(Some("abc"), false);
    link.undo_mode = None;
    db.save_speleo_link(Some(&link)).unwrap();
    assert_eq!(db.load_speleo_link().unwrap().unwrap().undo_mode, None);
    let is_null: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM speleo_link WHERE id = 1 AND undo_mode IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        is_null, 1,
        "None must clear the column to SQL NULL, never write a default spelling"
    );

    // An unrecognized value (a newer build, or a hand-edit) fails closed without
    // making the whole project unloadable.
    db.conn
        .execute("UPDATE speleo_link SET undo_mode = 'sideways'", [])
        .unwrap();
    let loaded = db.load_speleo_link().unwrap().unwrap().undo_mode;
    assert_eq!(loaded, Some(UndoMode::Disabled));
    assert!(
        !loaded.unwrap().pushes_history(),
        "malformed database values must not publish undo history"
    );
}

/// [#303] Spec item 22, persistence half: the Unwind chain pointer survives a close and
/// reopen (it is a row in the `.stalx`, not session state), and clearing it is a distinct
/// state from never having set one.
///
/// It lives in its own table rather than in `speleo_link` because it is per-MACHINE
/// progress through history: `PUSH_SNAPSHOT_LOCAL_TABLES` strips it, so it never rides to
/// the server. BASE cannot double as it, because a restore stamps BASE at the server tip.
#[test]
fn speleo_undo_chain_round_trips_and_clears() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("chain.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("Chain".to_string(), None))
            .unwrap();
        assert_eq!(
            db.load_speleo_undo_chain().unwrap(),
            None,
            "a fresh project is not mid-unwind"
        );
        db.save_speleo_undo_chain(Some("abc1234")).unwrap();
        assert_eq!(
            db.load_speleo_undo_chain().unwrap().as_deref(),
            Some("abc1234")
        );
    }

    // Survives the reopen - an Unwind interrupted by a restart resumes where it was.
    {
        let db = Database::open_existing(&path).unwrap();
        assert_eq!(
            db.load_speleo_undo_chain().unwrap().as_deref(),
            Some("abc1234"),
            "the pointer is on disk, so a restart mid-unwind does not restart the walk"
        );
        // Overwriting moves the position; clearing ends the walk.
        db.save_speleo_undo_chain(Some("def5678")).unwrap();
        assert_eq!(
            db.load_speleo_undo_chain().unwrap().as_deref(),
            Some("def5678")
        );
        db.save_speleo_undo_chain(None).unwrap();
        assert_eq!(db.load_speleo_undo_chain().unwrap(), None);
    }
}
