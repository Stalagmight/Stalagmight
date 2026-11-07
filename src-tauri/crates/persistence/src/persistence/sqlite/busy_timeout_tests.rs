//! Every `Database` connection waits out a competing writer (#576 B10).
//!
//! A `.stalx` connection that met a lock held by another connection used to
//! be suspected of failing at once with `SQLITE_BUSY` (the SpeleoDB local-state
//! guard reads the link through `open_for_metadata` while a save may hold the
//! file). It does not: rusqlite sets a 5000 ms busy timeout on every connection
//! it opens, and every path below opens through rusqlite. That default is the
//! library's, not ours, so a dependency upgrade could change it without a line
//! of this crate moving; this test pins it on each open path instead of
//! restating it in each.

use tempfile::TempDir;

use crate::models::Project;
use crate::persistence::Database;

/// The minimum wait, in milliseconds, the guard relies on.
const MIN_BUSY_TIMEOUT_MS: i64 = 5000;

fn busy_timeout_ms(db: &Database) -> i64 {
    db.conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn every_open_path_waits_at_least_five_seconds_for_a_busy_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CaveA.stalx");
    let mut created = Database::open(&path).unwrap();
    created
        .save_project(&Project::new("CaveA".to_string(), None))
        .unwrap();

    let opened: Vec<(&str, Database)> = vec![
        ("open (new file)", created),
        ("open (existing file)", Database::open(&path).unwrap()),
        ("open_existing", Database::open_existing(&path).unwrap()),
        (
            "open_existing_refusing_upgrade",
            Database::open_existing_refusing_upgrade(
                &path,
                &crate::persistence::UpgradeRefusal {
                    older_format: "not upgraded here".to_owned(),
                    unstamped: "not completed here".to_owned(),
                },
            )
            .unwrap(),
        ),
        (
            "open_existing + into_read_only",
            Database::open_existing(&path)
                .unwrap()
                .into_read_only(&path)
                .unwrap(),
        ),
        (
            "open_for_metadata",
            Database::open_for_metadata(&path).unwrap(),
        ),
        (
            "open_for_metadata_recovering",
            Database::open_for_metadata_recovering(&path).unwrap(),
        ),
        ("open_readonly", Database::open_readonly(&path).unwrap()),
        // [#608] The hardened open for a file from elsewhere (a server upload).
        (
            "open_existing_untrusted",
            Database::open_existing_untrusted(&path, 1 << 20, std::time::Duration::from_mins(1))
                .unwrap(),
        ),
        (
            "open_for_recovery",
            Database::open_for_recovery(&path).unwrap(),
        ),
        ("in_memory", Database::in_memory().unwrap()),
        (
            "open_existing_to_read",
            Database::open_existing_to_read(&path).unwrap(),
        ),
        // [#576 RV6b-R1-D1] An older file is read through an upgraded copy in
        // memory: that handle waits too.
        ("open_existing_to_read (older file, in memory)", {
            let older = dir.path().join("CaveOld.stalx");
            std::fs::copy(&path, &older).unwrap();
            rusqlite::Connection::open(&older)
                .unwrap()
                .execute_batch("UPDATE project SET schema_version = 16; PRAGMA user_version = 16;")
                .unwrap();
            Database::open_existing_to_read(&older).unwrap()
        }),
        // [#576 NF-FIX5-3] The restore check sets the timeout itself: handed a
        // connection that would not wait at all, it still waits.
        ("for_candidate_check", {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.busy_timeout(std::time::Duration::ZERO).unwrap();
            Database::for_candidate_check(conn).unwrap()
        }),
    ];
    for (how, db) in &opened {
        let timeout = busy_timeout_ms(db);
        println!("{how}: busy_timeout = {timeout} ms");
        assert!(
            timeout >= MIN_BUSY_TIMEOUT_MS,
            "{how} opens with busy_timeout {timeout} ms; a competing writer would fail it \
             at once instead of being waited out (needs >= {MIN_BUSY_TIMEOUT_MS} ms)"
        );
    }
}
