//! `legacy_settle::settle` on the two shapes SQLite leaves unfinished (a hot
//! rollback journal, a WAL with committed frames), on a clean file, and on the
//! state a settle that replaced its original passes through: the settled file
//! with the OLD sidecar still beside it.
//!
//! Plain SQLite databases built here; no fixture is read.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::legacy_settle::{settle, sidecar_of};

/// Rows with values large enough that a transaction touching them spans many
/// pages.
fn committed_db(path: &Path) {
    let conn = Connection::open(path).expect("create");
    conn.execute_batch(
        "PRAGMA journal_mode = DELETE;
         CREATE TABLE t(id INTEGER PRIMARY KEY, label TEXT, pad BLOB);",
    )
    .expect("schema");
    for id in 0..40 {
        conn.execute(
            "INSERT INTO t(id, label, pad) VALUES (?1, 'committed', zeroblob(3000))",
            [id],
        )
        .expect("insert");
    }
}

/// The labels the file at `path` holds, read on a private copy of the file
/// alone (no sidecar), so the read cannot itself finish anything.
fn labels_of_the_file_alone(path: &Path) -> Vec<String> {
    let scratch = tempfile::tempdir().expect("temp");
    let copy = scratch.path().join("alone.db");
    std::fs::copy(path, &copy).expect("copy");
    let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("open");
    let mut stmt = conn
        .prepare("SELECT DISTINCT label FROM t ORDER BY label")
        .expect("prepare");
    stmt.query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

/// The file and the hot journal a kill leaves mid-transaction: a one-page
/// cache makes the transaction spill its pages into the file.
fn hot_pair(dir: &Path) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let live = dir.join("live.db");
    committed_db(&live);
    let committed = std::fs::read(&live).expect("read");
    let conn = Connection::open(&live).expect("open");
    conn.execute_batch(
        "PRAGMA cache_size = 1;
         BEGIN IMMEDIATE;
         UPDATE t SET label = 'interrupted', pad = zeroblob(3100);",
    )
    .expect("spill");
    let main = std::fs::read(&live).expect("read main");
    let journal = std::fs::read(sidecar_of(&live, "-journal")).expect("read journal");
    conn.execute_batch("ROLLBACK;").expect("rollback");
    assert_ne!(main, committed, "the transaction reached the file");
    (committed, main, journal)
}

/// The file, its WAL and WAL index as a kill leaves them after a committed
/// transaction that was never checkpointed.
fn wal_trio(dir: &Path) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let live = dir.join("wal.db");
    committed_db(&live);
    let conn = Connection::open(&live).expect("open");
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .expect("wal");
    assert_eq!(mode, "wal");
    conn.execute_batch(
        "PRAGMA wal_autocheckpoint = 0;
         UPDATE t SET label = 'in-the-wal' WHERE id % 2 = 0;",
    )
    .expect("commit into the wal");
    let main = std::fs::read(&live).expect("read main");
    let wal = std::fs::read(sidecar_of(&live, "-wal")).expect("read wal");
    let shm = std::fs::read(sidecar_of(&live, "-shm")).expect("read shm");
    drop(conn);
    (main, wal, shm)
}

fn place(dir: &Path, name: &str, main: &[u8], sidecars: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, main).expect("write main");
    for (suffix, bytes) in sidecars {
        std::fs::write(sidecar_of(&path, suffix), bytes).expect("write sidecar");
    }
    path
}

fn no_sidecars(path: &Path) {
    for suffix in ["-journal", "-wal", "-shm"] {
        assert!(
            !sidecar_of(path, suffix).exists(),
            "{suffix} is left beside the settled copy"
        );
    }
}

#[test]
fn a_hot_journal_is_rolled_back_into_the_file() {
    let dir = tempfile::tempdir().expect("temp");
    let (committed, main, journal) = hot_pair(dir.path());
    let copy = place(dir.path(), "copy.db", &main, &[("-journal", &journal)]);
    assert!(
        labels_of_the_file_alone(&copy).contains(&"interrupted".to_owned()),
        "precondition: the file alone holds the half-written transaction"
    );

    settle(&copy).expect("settles");

    no_sidecars(&copy);
    assert_eq!(labels_of_the_file_alone(&copy), ["committed"]);
    assert_eq!(
        std::fs::read(&copy).expect("read"),
        committed,
        "the rollback restores the committed bytes exactly"
    );
}

/// The state an upgrade passes through after its settled copy replaced the
/// original: the settled file with the original's hot journal still beside
/// it. SQLite replays that journal again (a rollback is idempotent), so the
/// pair still opens as the committed survey and the file does not change.
#[test]
fn the_settled_file_beside_its_old_hot_journal_is_still_the_committed_one() {
    let dir = tempfile::tempdir().expect("temp");
    let (committed, main, journal) = hot_pair(dir.path());
    let first = place(dir.path(), "first.db", &main, &[("-journal", &journal)]);
    settle(&first).expect("settles");
    let settled = std::fs::read(&first).expect("read");
    let again = place(dir.path(), "again.db", &settled, &[("-journal", &journal)]);

    settle(&again).expect("settles again");

    no_sidecars(&again);
    assert_eq!(std::fs::read(&again).expect("read"), committed);
}

#[test]
fn a_wal_is_checkpointed_into_the_file() {
    let dir = tempfile::tempdir().expect("temp");
    let (main, wal, shm) = wal_trio(dir.path());
    let copy = place(
        dir.path(),
        "copy.db",
        &main,
        &[("-wal", &wal), ("-shm", &shm)],
    );
    assert_eq!(
        labels_of_the_file_alone(&copy),
        ["committed"],
        "precondition: the file alone misses the commit in the WAL"
    );

    settle(&copy).expect("settles");

    no_sidecars(&copy);
    assert_eq!(labels_of_the_file_alone(&copy), ["committed", "in-the-wal"]);
}

/// The WAL counterpart of the replay above: the checkpointed file with the
/// original's WAL (and a stale index) still beside it reads the same.
#[test]
fn the_settled_file_beside_its_old_wal_is_still_the_committed_one() {
    let dir = tempfile::tempdir().expect("temp");
    let (main, wal, shm) = wal_trio(dir.path());
    let first = place(
        dir.path(),
        "first.db",
        &main,
        &[("-wal", &wal), ("-shm", &shm)],
    );
    settle(&first).expect("settles");
    let settled = std::fs::read(&first).expect("read");
    let again = place(
        dir.path(),
        "again.db",
        &settled,
        &[("-wal", &wal), ("-shm", &shm)],
    );

    settle(&again).expect("settles again");

    no_sidecars(&again);
    assert_eq!(
        labels_of_the_file_alone(&again),
        ["committed", "in-the-wal"]
    );
}

#[test]
fn a_clean_file_is_left_as_it_was() {
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join("clean.db");
    committed_db(&path);
    let before = std::fs::read(&path).expect("read");

    settle(&path).expect("settles");

    no_sidecars(&path);
    assert_eq!(std::fs::read(&path).expect("read"), before);
}

#[test]
fn a_file_that_is_not_a_database_is_refused() {
    let dir = tempfile::tempdir().expect("temp");
    let path = place(dir.path(), "text.db", b"{\"format\":\"stalx\"}\n", &[]);
    assert!(settle(&path).is_err());
}
