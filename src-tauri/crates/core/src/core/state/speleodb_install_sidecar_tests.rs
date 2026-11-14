//! A stale SQLite sidecar beside a cached copy never reaches the copy installed
//! over it, on any install path, and a hot journal is never thrown away unasked.
//!
//! All data is synthetic: every database is built here, and a "crash" is a copy
//! of a live database and its journal taken mid-transaction.

use std::path::{Path, PathBuf};

use super::speleodb_cache::{InheritedUndo, LocalEditsPolicy};
use super::speleodb_sidecars::{has_hot_journal, sidecar_path, SetAsideSidecars};
use super::AppState;
use crate::models::{Project, SpeleoLink};
use crate::persistence::Database;
use crate::speleodb::SpeleoDbError;

const ID: &str = "11111111-1111-1111-1111-111111111111";
const SERVER_NAME: &str = "Server copy";
const LOCAL_NAME: &str = "Committed local work";

fn link(dirty: bool) -> SpeleoLink {
    SpeleoLink {
        endpoint: "https://example.com".into(),
        project_id: ID.into(),
        project_name: "Remote project".into(),
        base_commit_sha: Some("abc1234".into()),
        has_unpushed_edits: dirty,
        last_edit_at: dirty.then(|| "2026-01-01T00:00:00Z".into()),
        last_verify_at: None,
        undo_mode: None,
    }
}

/// A linked project with enough undo rows that a transaction touching them
/// spills pages into the database file.
fn seed_local(path: &Path, dirty: bool) {
    let mut db = Database::open(path).expect("local");
    db.save_project(&Project::new(LOCAL_NAME.into(), None))
        .expect("project");
    db.save_speleo_link(Some(&link(dirty))).expect("link");
    db.save_undo_stack(
        (0..400)
            .map(|i| {
                (
                    format!(r#"{{"c":{i},"p":"{}"}}"#, "x".repeat(2000)),
                    "{}".to_owned(),
                )
            })
            .collect(),
    )
    .expect("undo rows");
}

/// The cache copy at `<dir>/cache/project.stalx` exactly as a kill mid-save
/// leaves it: the database file with an unfinished transaction's pages already
/// written into it, and the HOT journal that holds what they overwrote. The
/// COMMITTED state (what a read-write open recovers) is `LOCAL_NAME` with the
/// given dirty bit.
fn crashed_copy(dir: &Path, dirty: bool) -> PathBuf {
    let live = dir.join("live").join("project.stalx");
    std::fs::create_dir_all(live.parent().expect("parent")).expect("live dir");
    seed_local(&live, dirty);
    let dest = dir.join("cache").join("project.stalx");
    std::fs::create_dir_all(dest.parent().expect("parent")).expect("cache dir");
    let conn = rusqlite::Connection::open(&live).expect("live connection");
    // A one-page cache forces the spill that arms the journal and writes the
    // uncommitted pages into the database file.
    conn.execute_batch(
        "PRAGMA cache_size = 1; BEGIN IMMEDIATE; \
         UPDATE project SET name = 'Torn write'; DELETE FROM undo_stack;",
    )
    .expect("open a spilling write transaction");
    std::fs::copy(&live, &dest).expect("copy the database mid-transaction");
    std::fs::copy(
        sidecar_path(&live, "-journal"),
        sidecar_path(&dest, "-journal"),
    )
    .expect("copy its live journal");
    conn.execute_batch("ROLLBACK;")
        .expect("roll the original back");
    assert!(
        has_hot_journal(&dest),
        "precondition: the copied journal is armed, so the copy is a real crash"
    );
    dest
}

/// A native download at `<dir>/<name>` whose project is `SERVER_NAME`.
fn server_copy(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut db = Database::open(&path).expect("server");
    db.save_project(&Project::new(SERVER_NAME.into(), None))
        .expect("project");
    db.save_speleo_link(Some(&link(false))).expect("link");
    path
}

/// `PRAGMA integrity_check` through a READ-WRITE connection: the open that
/// would replay any journal sitting beside the file.
fn integrity(path: &Path) -> String {
    rusqlite::Connection::open(path)
        .and_then(|conn| conn.query_row("PRAGMA integrity_check", [], |row| row.get(0)))
        .unwrap_or_else(|e| format!("could not check: {e}"))
}

/// Every name beside `dest` that is a sidecar of it, or a parked sidecar.
fn leftovers(dest: &Path) -> Vec<String> {
    let file = dest
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    let mut names: Vec<String> = std::fs::read_dir(dest.parent().expect("parent"))
        .expect("cache dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with(&file) && *name != file)
        .collect();
    names.sort();
    names
}

/// After an install: no sidecar of the old file survived, the installed copy
/// is sound when opened read-write, and it is the server's copy byte for byte.
fn assert_installed_exactly(dest: &Path, server_bytes: &[u8], label: &str) {
    assert_eq!(
        leftovers(dest),
        Vec::<String>::new(),
        "{label}: sidecars left"
    );
    assert_eq!(integrity(dest), "ok", "{label}: installed copy is corrupt");
    let installed = Database::open_existing(dest).expect("installed copy opens");
    assert_eq!(
        installed.load_project().expect("project").name,
        SERVER_NAME,
        "{label}: the old file's pages were mixed into the installed copy"
    );
    drop(installed);
    assert_eq!(
        std::fs::read(dest).expect("installed bytes"),
        server_bytes,
        "{label}: the installed copy is not the server's bytes"
    );
}

/// The hazard itself, so the tests below cannot pass vacuously: rename a fresh
/// file over a crashed copy WITHOUT touching its journal, and the next
/// read-write open rolls the old file's pages into the new one.
#[test]
fn control_a_bare_rename_over_a_crashed_copy_replays_its_journal() {
    let dir = tempfile::tempdir().expect("temp");
    let dest = crashed_copy(dir.path(), false);
    let server = server_copy(dest.parent().expect("cache dir"), "download.tmp");
    std::fs::rename(&server, &dest).expect("bare rename");
    let sound = integrity(&dest) == "ok"
        && Database::open_existing(&dest)
            .and_then(|db| db.load_project())
            .is_ok_and(|project| project.name == SERVER_NAME);
    assert!(
        !sound,
        "a stale journal beside a renamed-in file must replay into it, or this \
         suite proves nothing"
    );
}

/// Bug: a cache install renamed the download over the cache file and left the
/// crashed copy's journal beside it, so the next open replayed the old pages
/// into the server copy. Under the confirmed discard every install path sets
/// the journal aside first and deletes it after.
#[test]
fn a_discarding_install_never_replays_a_stale_journal_on_any_path() {
    crate::test_helpers::with_isolated_config(|| {
        for path in ["session-bound", "prepared"] {
            let dir = tempfile::tempdir().expect("temp");
            let dest = crashed_copy(dir.path(), true);
            let source = server_copy(dir.path(), "download.stalx");
            let server_bytes = std::fs::read(&source).expect("server bytes");
            let state = AppState::new().expect("state");
            if path == "session-bound" {
                let generation = state.reserve_download_generation();
                state
                    .replace_cache_for_session(
                        &source,
                        &dest,
                        ID,
                        LocalEditsPolicy::Discard,
                        InheritedUndo::Keep,
                        generation,
                    )
                    .expect("session-bound install");
            } else {
                // The fast-forward / revision-swap chokepoint: a prepared file
                // staged beside the cache copy, renamed over it.
                let staged = dest.with_file_name("staged.tmp");
                std::fs::copy(&source, &staged).expect("stage");
                let prepared = Database::open_existing(&staged).expect("prepared");
                drop(
                    state
                        .install_prepared_database(&staged, &dest, prepared, None)
                        .expect("prepared install"),
                );
            }
            assert_installed_exactly(&dest, &server_bytes, path);
        }
    });
}

/// Policy: a hot journal is part of the copy's committed state, so under
/// `Protect` it makes that state UNKNOWN - every protected install refuses with
/// the same typed signal an unreadable copy gets, even when the committed dirty
/// bit says clean, and leaves the copy and its journal exactly as they were.
#[test]
fn a_hot_journal_refuses_every_protected_install_and_survives_it() {
    crate::test_helpers::with_isolated_config(|| {
        let dir = tempfile::tempdir().expect("temp");
        let dest = crashed_copy(dir.path(), false);
        let journal = sidecar_path(&dest, "-journal");
        let before = (
            std::fs::read(&dest).expect("copy"),
            std::fs::read(&journal).expect("journal"),
        );
        let source = server_copy(dir.path(), "download.stalx");
        let state = AppState::new().expect("state");

        let generation = state.reserve_download_generation();
        let refusals = [
            (
                "session-bound",
                state
                    .replace_cache_for_session(
                        &source,
                        &dest,
                        ID,
                        LocalEditsPolicy::Protect,
                        InheritedUndo::Drop,
                        generation,
                    )
                    .map(drop),
            ),
            // The guard the fast-forward and the revision swap run before their
            // prepared install.
            ("guard", state.guard_local_edits(ID, &dest)),
        ];
        for (path, result) in refusals {
            assert!(
                matches!(
                    &result,
                    Err(SpeleoDbError::WouldDiscardLocalEdits {
                        last_edit_at: None,
                        ..
                    })
                ),
                "{path}: a hot journal must read as unknown local state, got {result:?}"
            );
            assert_eq!(
                (
                    std::fs::read(&dest).expect("copy"),
                    std::fs::read(&journal).expect("journal")
                ),
                before,
                "{path}: a refusal must leave the copy AND its journal untouched"
            );
        }

        // ...and what was kept is the user's work: a read-write open recovers it.
        assert_eq!(integrity(&dest), "ok");
        let recovered = Database::open_existing(&dest).expect("recovered copy");
        assert_eq!(recovered.load_project().expect("project").name, LOCAL_NAME);
    });
}

/// Sidecars that hold nothing (a journal SQLite never armed, a WAL index with no
/// WAL) neither block a protected install nor survive it.
#[test]
fn inert_sidecars_do_not_block_a_protected_install_and_do_not_survive_it() {
    crate::test_helpers::with_isolated_config(|| {
        let dir = tempfile::tempdir().expect("temp");
        let dest = dir.path().join("cache").join("project.stalx");
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("cache dir");
        seed_local(&dest, false);
        std::fs::write(sidecar_path(&dest, "-journal"), [0u8; 512]).expect("unarmed journal");
        std::fs::write(sidecar_path(&dest, "-shm"), vec![0; 32768]).expect("lone shm");
        assert!(!has_hot_journal(&dest), "precondition: nothing is armed");
        let source = server_copy(dir.path(), "download.stalx");
        let server_bytes = std::fs::read(&source).expect("server bytes");
        let state = AppState::new().expect("state");
        let generation = state.reserve_download_generation();
        state
            .replace_cache_for_session(
                &source,
                &dest,
                ID,
                LocalEditsPolicy::Protect,
                InheritedUndo::Keep,
                generation,
            )
            .expect("a clean copy with inert sidecars installs");
        assert_installed_exactly(&dest, &server_bytes, "inert");
    });
}

/// The cache copy at `<dir>/cache/project.stalx` as a kill leaves a WAL-mode
/// database: COMMITTED frames still in its `-wal`, never checkpointed.
fn crashed_wal_copy(dir: &Path) -> PathBuf {
    let live = dir.join("live").join("project.stalx");
    std::fs::create_dir_all(live.parent().expect("parent")).expect("live dir");
    seed_local(&live, true);
    let dest = dir.join("cache").join("project.stalx");
    std::fs::create_dir_all(dest.parent().expect("parent")).expect("cache dir");
    let conn = rusqlite::Connection::open(&live).expect("live connection");
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("wal");
    conn.pragma_update(None, "wal_autocheckpoint", 0)
        .expect("no checkpoint");
    conn.execute("UPDATE project SET name = 'Committed in the WAL'", [])
        .expect("commit into the WAL");
    std::fs::copy(&live, &dest).expect("copy the database");
    std::fs::copy(sidecar_path(&live, "-wal"), sidecar_path(&dest, "-wal")).expect("copy its WAL");
    dest
}

/// A WAL can carry COMMITTED frames, and SQLite applies them to whatever file
/// now bears the name. A download must not inherit the old copy's frames,
/// whichever journal mode the download itself is in.
#[test]
fn a_stale_wal_is_not_replayed_into_the_installed_copy() {
    crate::test_helpers::with_isolated_config(|| {
        for mode in ["DELETE", "WAL"] {
            let dir = tempfile::tempdir().expect("temp");
            let dest = crashed_wal_copy(dir.path());
            let source = server_copy(dir.path(), "download.stalx");
            rusqlite::Connection::open(&source)
                .expect("server connection")
                .pragma_update(None, "journal_mode", mode)
                .expect("server journal mode");
            let server_bytes = std::fs::read(&source).expect("server bytes");
            let state = AppState::new().expect("state");
            let generation = state.reserve_download_generation();
            state
                .replace_cache_for_session(
                    &source,
                    &dest,
                    ID,
                    LocalEditsPolicy::Discard,
                    InheritedUndo::Keep,
                    generation,
                )
                .unwrap_or_else(|e| panic!("{mode}: install failed: {e}"));
            assert_installed_exactly(&dest, &server_bytes, mode);
        }
    });
}

/// A failed install puts the old file's sidecars back beside the old bytes, so
/// a crashed copy stays exactly as recoverable as it was.
#[test]
fn a_failed_install_puts_the_journal_back_beside_the_original() {
    crate::test_helpers::with_isolated_config(|| {
        let dir = tempfile::tempdir().expect("temp");
        let dest = crashed_copy(dir.path(), true);
        let journal = sidecar_path(&dest, "-journal");
        let before = (
            std::fs::read(&dest).expect("copy"),
            std::fs::read(&journal).expect("journal"),
        );
        // Prepared, then broken after the fact: the rename succeeds and the
        // reopen of the installed file fails, which is the path that has to put
        // the original back.
        let staged = dest.with_file_name("staged.tmp");
        std::fs::copy(server_copy(dir.path(), "download.stalx"), &staged).expect("stage");
        let prepared = Database::open_existing(&staged).expect("prepared");
        std::fs::write(&staged, b"not a database").expect("break the staged file");
        let state = AppState::new().expect("state");
        assert!(state
            .install_prepared_database(&staged, &dest, prepared, None)
            .is_err());
        assert_eq!(
            (
                std::fs::read(&dest).expect("copy"),
                std::fs::read(&journal).expect("journal")
            ),
            before,
            "the original bytes and their journal are back"
        );
        assert_eq!(leftovers(&dest), vec!["project.stalx-journal".to_owned()]);
        let recovered = Database::open_existing(&dest).expect("still recoverable");
        assert_eq!(recovered.load_project().expect("project").name, LOCAL_NAME);
        assert!(
            recovered
                .load_speleo_link()
                .expect("link")
                .expect("linked")
                .has_unpushed_edits,
            "the un-pushed work the journal protected is still there"
        );
    });
}

#[test]
fn hot_journal_detection_follows_the_journal_header() {
    let dir = tempfile::tempdir().expect("temp");
    let db = dir.path().join("project.stalx");
    let journal = sidecar_path(&db, "-journal");
    assert!(!has_hot_journal(&db), "no journal");
    std::fs::write(&journal, b"").expect("empty");
    assert!(!has_hot_journal(&db), "empty journal");
    std::fs::write(&journal, [0u8; 512]).expect("zero header");
    assert!(!has_hot_journal(&db), "a header SQLite never armed");
    let mut armed = vec![0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];
    armed.extend_from_slice(&[0u8; 504]);
    std::fs::write(&journal, &armed).expect("armed");
    assert!(has_hot_journal(&db), "armed journal");
}

#[test]
fn set_aside_sidecars_round_trip_and_discard() {
    let dir = tempfile::tempdir().expect("temp");
    let db = dir.path().join("project.stalx");
    std::fs::write(&db, b"main").expect("main");
    for suffix in ["-journal", "-wal", "-shm"] {
        std::fs::write(sidecar_path(&db, suffix), suffix).expect("sidecar");
    }
    let all = vec![
        "project.stalx-journal".to_owned(),
        "project.stalx-shm".to_owned(),
        "project.stalx-wal".to_owned(),
    ];

    let set_aside = SetAsideSidecars::take(&db).expect("set aside");
    assert!(
        leftovers(&db)
            .iter()
            .all(|name| name.ends_with(".set-aside")),
        "nothing is left under a name SQLite opens: {:?}",
        leftovers(&db)
    );
    set_aside.restore();
    assert_eq!(leftovers(&db), all, "restore puts every sidecar back");
    for suffix in ["-journal", "-wal", "-shm"] {
        assert_eq!(
            std::fs::read(sidecar_path(&db, suffix)).expect("restored"),
            suffix.as_bytes()
        );
    }

    SetAsideSidecars::take(&db).expect("set aside").discard();
    assert_eq!(leftovers(&db), Vec::<String>::new(), "discard removes them");
    assert_eq!(
        std::fs::read(&db).expect("main"),
        b"main",
        "the database itself is untouched"
    );
}
