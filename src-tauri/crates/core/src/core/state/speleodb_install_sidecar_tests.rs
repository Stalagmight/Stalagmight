//! A stale SQLite sidecar beside a cached copy never reaches the copy installed
//! over it, on any install path, and a hot journal is never thrown away unasked.
//!
//! A cache copy written by an older build is a SQLite file, and a kill or a
//! power loss mid-save can leave its journal (or a WAL) beside it. Every
//! install now writes a version 2 file, which this build never mixes with a
//! journal, but an older build opening it would: so the outgoing copy's
//! sidecars are set aside before the write and deleted once the new file holds
//! the name, and put back if it never does.
//!
//! All data is synthetic: the "legacy copy" is a file with the SQLite magic and
//! its sidecars are files with the right names (an ARMED journal carries the
//! journal magic). No SQLite database is built here.

use std::path::{Path, PathBuf};

use super::speleodb_cache::{InheritedUndo, LocalEditsPolicy};
use super::speleodb_reload::install_cache_bytes;
use super::speleodb_sidecars::{has_hot_journal, sidecar_path, SetAsideSidecars};
use super::stalx_fixtures::{decoded, fixture_link, name_at, write_v2, FIXTURE_ID};
use super::AppState;
use crate::persistence::stalx_v2::{temp_path, DecodeOptions};
use crate::speleodb::SpeleoDbError;

const ID: &str = FIXTURE_ID;
const SERVER_NAME: &str = "Server copy";
const LOCAL_NAME: &str = "Committed local work";

/// The first bytes of an armed rollback journal (SQLite's `aJournalMagic`).
const ARMED: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

/// The cache copy at `<dir>/cache/project.stalx` as a kill mid-save left an
/// older build's copy: a SQLite file and the HOT journal beside it.
fn crashed_legacy_copy(dir: &Path) -> PathBuf {
    let dest = dir.join("cache").join("project.stalx");
    std::fs::create_dir_all(dest.parent().expect("parent")).expect("cache dir");
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend_from_slice(LOCAL_NAME.as_bytes());
    std::fs::write(&dest, bytes).expect("legacy copy");
    let mut journal = ARMED.to_vec();
    journal.extend_from_slice(&[0u8; 504]);
    std::fs::write(sidecar_path(&dest, "-journal"), journal).expect("hot journal");
    assert!(has_hot_journal(&dest), "precondition: the journal is armed");
    dest
}

/// A version 2 download at `<dir>/<name>` whose project is `SERVER_NAME`.
fn server_copy(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    write_v2(
        &path,
        SERVER_NAME,
        Some(&fixture_link("https://example.com", false)),
    );
    path
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

/// After an install: no sidecar of the old file survived, and the installed
/// copy is the server's survey in a valid version 2 file.
fn assert_installed_exactly(dest: &Path, label: &str) {
    assert_eq!(
        leftovers(dest),
        Vec::<String>::new(),
        "{label}: sidecars left"
    );
    assert_eq!(
        name_at(dest),
        SERVER_NAME,
        "{label}: the installed copy is the server's"
    );
    assert!(
        decoded(dest).extras.speleodb.is_some(),
        "{label}: and it is linked"
    );
}

fn install(
    state: &AppState,
    source: &Path,
    dest: &Path,
    policy: LocalEditsPolicy,
) -> Result<(), SpeleoDbError> {
    let generation = state.reserve_download_generation();
    let prepared = state.prepare_download(
        source,
        fixture_link("https://example.com", false),
        InheritedUndo::Drop,
        &DecodeOptions::default(),
    )?;
    state.replace_cache_for_session(prepared, dest, ID, policy, generation)
}

/// Bug: a cache install renamed the download over the cache file and left the
/// crashed copy's journal beside it, so the next open (by an older build, now)
/// replayed the old pages into the new file. Under the confirmed discard every
/// install path sets the journal aside first and deletes it after.
#[test]
fn a_discarding_install_never_leaves_a_stale_journal_on_any_path() {
    crate::test_helpers::with_isolated_config(|| {
        for path in ["session-bound", "prepared"] {
            let dir = tempfile::tempdir().expect("temp");
            let dest = crashed_legacy_copy(dir.path());
            let source = server_copy(dir.path(), "download.stalx");
            let state = AppState::new().expect("state");
            if path == "session-bound" {
                install(&state, &source, &dest, LocalEditsPolicy::Discard)
                    .expect("session-bound install");
            } else {
                // The fast-forward / revision-swap chokepoint writes through the
                // same install as the prepared reload.
                let mut prepared = state
                    .prepare_download(
                        &source,
                        fixture_link("https://example.com", false),
                        InheritedUndo::Drop,
                        &DecodeOptions::default(),
                    )
                    .expect("prepare");
                let bytes = prepared
                    .cache_bytes(std::collections::BTreeMap::new())
                    .expect("bytes");
                install_cache_bytes(&dest, &bytes).expect("prepared install");
            }
            assert_installed_exactly(&dest, path);
        }
    });
}

/// Policy: a hot journal is part of the copy's committed state, so under
/// `Protect` it makes that state UNKNOWN - every protected install refuses with
/// the same typed signal an unreadable copy gets, and leaves the copy and its
/// journal exactly as they were.
#[test]
fn a_hot_journal_refuses_every_protected_install_and_survives_it() {
    crate::test_helpers::with_isolated_config(|| {
        let dir = tempfile::tempdir().expect("temp");
        let dest = crashed_legacy_copy(dir.path());
        let journal = sidecar_path(&dest, "-journal");
        let before = (
            std::fs::read(&dest).expect("copy"),
            std::fs::read(&journal).expect("journal"),
        );
        let source = server_copy(dir.path(), "download.stalx");
        let state = AppState::new().expect("state");

        let refusals = [
            (
                "session-bound",
                install(&state, &source, &dest, LocalEditsPolicy::Protect),
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
    });
}

/// Sidecars that hold nothing (a journal SQLite never armed, a WAL index with no
/// WAL) neither block a protected install nor survive it.
#[test]
fn inert_sidecars_do_not_block_a_protected_install_and_do_not_survive_it() {
    crate::test_helpers::with_isolated_config(|| {
        let dir = tempfile::tempdir().expect("temp");
        let dest = dir.path().join("cache").join("project.stalx");
        write_v2(
            &dest,
            LOCAL_NAME,
            Some(&fixture_link("https://example.com", false)),
        );
        std::fs::write(sidecar_path(&dest, "-journal"), [0u8; 512]).expect("unarmed journal");
        std::fs::write(sidecar_path(&dest, "-shm"), vec![0; 32768]).expect("lone shm");
        assert!(!has_hot_journal(&dest), "precondition: nothing is armed");
        let source = server_copy(dir.path(), "download.stalx");
        let state = AppState::new().expect("state");
        install(&state, &source, &dest, LocalEditsPolicy::Protect)
            .expect("a clean copy with inert sidecars installs");
        assert_installed_exactly(&dest, "inert");
    });
}

/// A WAL can carry COMMITTED frames, and SQLite applies them to whatever file
/// now bears the name. A download never inherits the old copy's WAL.
#[test]
fn a_stale_wal_is_not_left_beside_the_installed_copy() {
    crate::test_helpers::with_isolated_config(|| {
        let dir = tempfile::tempdir().expect("temp");
        let dest = dir.path().join("cache").join("project.stalx");
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("cache dir");
        std::fs::write(&dest, b"SQLite format 3\0legacy").expect("legacy copy");
        std::fs::write(sidecar_path(&dest, "-wal"), b"committed frames").expect("wal");
        std::fs::write(sidecar_path(&dest, "-shm"), [0u8; 64]).expect("shm");
        let source = server_copy(dir.path(), "download.stalx");
        let state = AppState::new().expect("state");
        install(&state, &source, &dest, LocalEditsPolicy::Discard).expect("install");
        assert_installed_exactly(&dest, "wal");
    });
}

/// A failed install puts the old file's sidecars back beside the old bytes, so
/// a crashed copy stays exactly as recoverable as it was.
#[test]
fn a_failed_install_puts_the_journal_back_beside_the_original() {
    crate::test_helpers::with_isolated_config(|| {
        let dir = tempfile::tempdir().expect("temp");
        let dest = crashed_legacy_copy(dir.path());
        let journal = sidecar_path(&dest, "-journal");
        let before = (
            std::fs::read(&dest).expect("copy"),
            std::fs::read(&journal).expect("journal"),
        );
        // The write is refused after the sidecars moved: a directory holds the
        // name of the install's temp.
        std::fs::create_dir(temp_path(&dest)).expect("block the temp");
        std::fs::write(temp_path(&dest).join("keep"), b"x").expect("non-empty");
        let source = server_copy(dir.path(), "download.stalx");
        let state = AppState::new().expect("state");
        assert!(install(&state, &source, &dest, LocalEditsPolicy::Discard).is_err());
        assert_eq!(
            (
                std::fs::read(&dest).expect("copy"),
                std::fs::read(&journal).expect("journal")
            ),
            before,
            "the original bytes and their journal are back"
        );
        assert_eq!(
            leftovers(&dest),
            vec![
                "project.stalx-journal".to_owned(),
                "project.stalx.tmp".to_owned()
            ],
            "only the journal (and the obstacle this test planted) remain"
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
    let mut armed = ARMED.to_vec();
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

/// A legacy revision the pull migrates raises the conversion notice a local
/// upgrade raises, on the open of the cache that follows (review finding
/// G-speleodb-1): the cache holds version 2 by then, so the open alone has
/// nothing to compare. A clean revision moves nothing and every line keeps
/// the names it showed (I-3), so it raises none.
#[test]
fn a_pulled_legacy_revision_that_moved_stations_raises_the_conversion_notice() {
    crate::test_helpers::with_isolated_config(|| {
        crate::app_dirs::init_for_test();
        for drift in [true, false] {
            let dir = tempfile::tempdir().expect("temp");
            let source = dir.path().join("download.stalx");
            super::project_save_tests::write_legacy_drift(&source, drift);
            let dest = dir.path().join("cache").join("project.stalx");
            std::fs::create_dir_all(dest.parent().expect("parent")).expect("cache dir");
            let state = AppState::new().expect("state");
            install(&state, &source, &dest, LocalEditsPolicy::Discard).expect("install");
            let opened = state.execute(crate::core::Command::OpenProject { path: dest.clone() });
            assert!(opened.success, "{:?}", opened.error);
            let notice = state.take_conversion_notice();
            // RS 8.3: C shows its start by the kept junction's name and its
            // own 1 takes the letter. RS-F: the 8.3 notice.
            let names = [
                "Line 3 / 0 is now shown as 1",
                "Line 3 / 1 is now shown as 1a",
            ];
            let notice = notice.expect("a notice");
            if !drift {
                assert_eq!(notice.changes, names, "a clean revision: the names");
                continue;
            }
            assert_eq!(
                notice.changes,
                ["2 stations moved by up to 5.00 m", names[0], names[1]],
                "the moves and the names: {notice:?}"
            );
            // The cache is version 2 already and nothing local keeps the
            // original: the notice points at the server's revision, never at a
            // local backup the pull did not make.
            assert_eq!(notice.file, super::ConversionFile::Downloaded, "{notice:?}");
            assert_eq!(state.take_conversion_notice(), None, "offered once");
            // Other tests share the backups folder: look for this path's own.
            let backups = super::project_io::upgrade_backup_dir().expect("backups dir");
            let prefix = super::project_io::upgrade_backup_prefix(&dest);
            let backed_up = std::fs::read_dir(&backups).map_or(0, |entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                    .count()
            });
            assert_eq!(backed_up, 0, "a pull makes no local backup to restore");
        }
    });
}

/// The pull's notice is for the copy it installed: an open that reads other
/// bytes at the cache path (here a later install of a version 2 revision there
/// before the copy was opened, as a fast-forward, a revision swap or a reload
/// writes) never shows it. Before the fix, any open of the path took it and
/// showed it, so the migration's notice reached a version 2 revision's open.
#[test]
fn a_pulled_notice_never_reaches_other_bytes_at_its_path() {
    crate::test_helpers::with_isolated_config(|| {
        crate::app_dirs::init_for_test();
        let dir = tempfile::tempdir().expect("temp");
        let source = dir.path().join("download.stalx");
        super::project_save_tests::write_legacy_drift(&source, true);
        let dest = dir.path().join("cache").join("project.stalx");
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("cache dir");
        let state = AppState::new().expect("state");
        install(&state, &source, &dest, LocalEditsPolicy::Discard).expect("legacy pull");
        // Another revision lands at the path before the copy is opened.
        let newer = server_copy(dir.path(), "newer.stalx");
        install_cache_bytes(&dest, &std::fs::read(&newer).expect("read")).expect("install v2");
        let opened = state.execute(crate::core::Command::OpenProject { path: dest.clone() });
        assert!(opened.success, "{:?}", opened.error);
        assert_eq!(
            name_at(&dest),
            SERVER_NAME,
            "the version 2 revision is open"
        );
        assert_eq!(
            state.take_conversion_notice(),
            None,
            "the migration's notice describes other content"
        );
    });
}

/// An open of another project between the pull and the copy's own open leaves
/// the notice for that open. Before the fix, any open dropped it.
#[test]
fn a_pulled_notice_waits_for_its_own_copy() {
    crate::test_helpers::with_isolated_config(|| {
        crate::app_dirs::init_for_test();
        let dir = tempfile::tempdir().expect("temp");
        let source = dir.path().join("download.stalx");
        super::project_save_tests::write_legacy_drift(&source, true);
        let dest = dir.path().join("cache").join("project.stalx");
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("cache dir");
        let state = AppState::new().expect("state");
        install(&state, &source, &dest, LocalEditsPolicy::Discard).expect("legacy pull");
        let other = server_copy(dir.path(), "other.stalx");
        let opened = state.execute(crate::core::Command::OpenProject { path: other });
        assert!(opened.success, "{:?}", opened.error);
        assert_eq!(
            state.take_conversion_notice(),
            None,
            "the other project has none"
        );
        let opened = state.execute(crate::core::Command::OpenProject { path: dest });
        assert!(opened.success, "{:?}", opened.error);
        let notice = state.take_conversion_notice().expect("the pull's notice");
        assert_eq!(notice.file, super::ConversionFile::Downloaded, "{notice:?}");
        assert_eq!(state.take_conversion_notice(), None, "offered once");
    });
}
