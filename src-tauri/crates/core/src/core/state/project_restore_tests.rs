//! [#720] `AppState::restore_project_from_backup`: what it writes, what it
//! refuses, and that a refusal or a failure leaves every file as it was; and
//! `AppState::list_project_backups`, what the dialog offers.
//!
//! Projects and backups are version 2 files unless a test is about SQLite
//! itself (sidecars a SQLite file carries, a damaged page, WAL mode), which
//! uses a legacy SQLite file: those are still restorable backups (an old
//! `.backup`, the pre-upgrade original the first version 2 save keeps).
//!
//! All data is synthetic: projects built in memory and saved by this build. No
//! fixture is read or written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use super::legacy_fixture_kit::{read_legacy_file, write_legacy_file, Legacy};
use super::project_restore::pre_restore_path;
use super::AppState;
use crate::core::events::EventSink;
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::persistence::stalx_v2::{self, Extras, Format, Mode};
use crate::persistence::BackupKind;

/// The load needs a sink for its progress events; nothing listens here.
struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

/// The first eight bytes of an armed rollback journal (SQLite's `aJournalMagic`).
const JOURNAL_MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

fn state() -> AppState {
    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(NullSink));
    state
}

/// A one-line survey named `label`, valid (its piece is placed).
fn survey(label: &str) -> Project {
    let mut project = Project::new(label, None);
    let line = project
        .add_line(
            LineMeta::default(),
            StartAt::New,
            vec![crate::models::ShotData {
                length: 4.0,
                azimuth_in: 30.0,
                azimuth_out: 30.0,
                ..Default::default()
            }],
        )
        .unwrap();
    let start = project.line(line).unwrap().start;
    project.set_placement(start, 0.0, 0.0).unwrap();
    project
}

/// The version 2 bytes of [`survey`]`(label)`.
fn v2_bytes(label: &str) -> Vec<u8> {
    stalx_v2::encode(&survey(label), &Extras::default(), Mode::Normal).unwrap()
}

/// A saved version 2 project named `label` at `dir/file`.
fn project_file(dir: &Path, file: &str, label: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, v2_bytes(label)).unwrap();
    path
}

/// A saved legacy (SQLite) project named `label` at `dir/file`.
fn legacy_file(dir: &Path, file: &str, label: &str) -> PathBuf {
    let path = dir.join(file);
    write_legacy_file(&path, &Legacy::new(label.to_string(), None));
    path
}

/// The project name stored in the file (either format), read without writing
/// to it.
fn label(path: &Path) -> String {
    let head = std::fs::read(path).unwrap();
    match stalx_v2::sniff(&head) {
        Format::V2 => stalx_v2::probe_bytes(&head).project.unwrap().name,
        Format::Legacy => {
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap()
                .query_row("SELECT name FROM project", [], |row| row.get(0))
                .unwrap()
        }
        Format::Unknown => unreachable!("{} is not a project", path.display()),
    }
}

/// Every file in `dir` with its length and a hash of its bytes: what a refusal
/// must leave unchanged, and where a stray staged copy would show up.
fn snapshot(dir: &Path) -> BTreeMap<String, (usize, u64)> {
    use std::hash::{Hash as _, Hasher as _};
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let bytes = std::fs::read(&path).unwrap();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            (name, (bytes.len(), hasher.finish()))
        })
        .collect()
}

fn names(dir: &Path) -> Vec<String> {
    snapshot(dir).into_keys().collect()
}

/// `cave.stalx` ("Current") and `cave.stalx.backup` ("Older") in a fresh dir.
fn cave(dir: &TempDir) -> (PathBuf, PathBuf) {
    (
        project_file(dir.path(), "cave.stalx", "Current"),
        project_file(dir.path(), "cave.stalx.backup", "Older"),
    )
}

#[test]
fn a_restore_puts_the_backup_in_place_and_keeps_the_project_beside_it() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let (project_bytes, backup_bytes) = (
        std::fs::read(&project).unwrap(),
        std::fs::read(&backup).unwrap(),
    );

    let message = state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    assert_eq!(std::fs::read(&project).unwrap(), backup_bytes);
    assert_eq!(
        std::fs::read(pre_restore_path(&project)).unwrap(),
        project_bytes
    );
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        backup_bytes,
        "the backup is only read"
    );
    assert_eq!(
        message,
        "Restored from cave.stalx.backup. The file it replaced is kept as \
         cave.stalx.pre-restore and is listed with the earlier copies, so you can switch back."
    );
    assert_eq!(
        names(dir.path()),
        ["cave.stalx", "cave.stalx.backup", "cave.stalx.pre-restore"],
        "no staged copy is left behind"
    );
}

/// The restored file opens: it is the backup's project.
#[test]
fn the_restored_project_opens_as_the_backup() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    let opened = state();
    opened.do_load_project_full(project).unwrap();
    assert_eq!(opened.get_project_state().unwrap().name, "Older");
}

/// F5: restoring the safety copy undoes the restore instead of overwriting the
/// copy with the project first.
#[test]
fn restoring_the_safety_copy_undoes_the_restore() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let original = std::fs::read(&project).unwrap();
    let state = state();
    state
        .restore_project_from_backup(&project, &backup)
        .unwrap();
    let restored = std::fs::read(&project).unwrap();
    let safety = pre_restore_path(&project);

    let message = state
        .restore_project_from_backup(&project, &safety)
        .unwrap();

    assert_eq!(
        message,
        "Restored cave.stalx.pre-restore, the copy the last restore kept. The two files \
         swapped places: cave.stalx.pre-restore now holds the file it replaced, so restoring \
         it again switches back."
    );

    assert_eq!(
        std::fs::read(&project).unwrap(),
        original,
        "the original is back"
    );
    assert_eq!(
        std::fs::read(&safety).unwrap(),
        restored,
        "and the undone restore is kept"
    );
}

/// F2: a backup that is the project itself is refused, under any spelling.
#[test]
fn the_project_is_never_restored_from_itself() {
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), "cave.backup.stalx", "Current");
    let mut spellings = vec![
        project.clone(),
        dir.path().join(".").join("cave.backup.stalx"),
    ];
    #[cfg(unix)]
    {
        let hard = dir.path().join("hard-link.stalx");
        std::fs::hard_link(&project, &hard).unwrap();
        let soft = dir.path().join("soft-link.stalx");
        std::os::unix::fs::symlink(&project, &soft).unwrap();
        spellings.extend([hard, soft]);
    }
    let before = snapshot(dir.path());

    for spelling in spellings {
        let err = state()
            .restore_project_from_backup(&project, &spelling)
            .unwrap_err();
        assert_eq!(
            err,
            "Not restored: the backup you picked is cave.backup.stalx itself.",
            "{}",
            spelling.display()
        );
    }
    assert_eq!(snapshot(dir.path()), before);
}

/// F1, F4: a restore touches no file the app has open, in any of the three
/// roles, and says which one is open.
#[test]
fn a_restore_never_touches_the_open_project() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let safety = project_file(dir.path(), "cave.stalx.pre-restore", "Kept");
    let cases = [
        (
            &project,
            "Not restored: cave.stalx is open in Stalagmight. Close it, then restore.",
        ),
        (
            &safety,
            "Not restored: cave.stalx.pre-restore is open in Stalagmight, and the restore keeps \
             its copy of cave.stalx there. Close it, then restore.",
        ),
        (
            &backup,
            "Not restored: the backup cave.stalx.backup is open in Stalagmight. Close it, then \
             restore.",
        ),
    ];
    for (open, expected) in cases {
        let state = state();
        state.do_load_project_full(open.clone()).unwrap();
        let before = snapshot(dir.path());

        let err = state
            .restore_project_from_backup(&project, &backup)
            .unwrap_err();

        assert_eq!(err, expected);
        assert_eq!(snapshot(dir.path()), before, "{} open", open.display());
        assert_eq!(
            state.open_project_path().as_deref(),
            Some(open.as_path()),
            "still open"
        );
    }
}

/// Give the root page of the `project` table an invalid page type: `PRAGMA
/// quick_check` fails, so a load reports the file as corrupt.
fn damage_a_page(path: &Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    let root: i64 = conn
        .query_row(
            "SELECT rootpage FROM sqlite_master WHERE name = 'project'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(conn);
    let mut bytes = std::fs::read(path).unwrap();
    let at = usize::try_from((root - 1) * page_size).unwrap();
    bytes.splice(at..=at, [0]);
    std::fs::write(path, bytes).unwrap();
}

/// The version 2 bytes of [`survey`] with its placement removed: the piece has
/// no anchor, so the survey breaks an invariant (the load refuses it).
fn invalid_v2_bytes() -> Vec<u8> {
    let text = String::from_utf8(v2_bytes("Damaged")).unwrap();
    let from = text.find("\"placements\":[").unwrap();
    let to = from + text[from..].find("],\n").unwrap() + 3;
    format!("{}{}", &text[..from], &text[to..]).into_bytes()
}

/// A backup that would not open as a project is refused before anything is
/// written: restoring it would only bring the recovery dialog back (F2's
/// empty file, a torn `.tmp`, a damaged copy, anything else).
#[test]
fn a_backup_that_does_not_open_is_refused() {
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), "cave.stalx", "Current");
    let backup = dir.path().join("cave.stalx.backup");
    let not_a_project = {
        let path = dir.path().join("other.db");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE notes(text TEXT); INSERT INTO notes VALUES ('hello');")
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        bytes
    };
    let damaged_sqlite = {
        let path = legacy_file(dir.path(), "damaged.stalx", "Damaged");
        damage_a_page(&path);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        bytes
    };
    let complete = v2_bytes("Torn");
    let torn = complete[..complete.len() / 2].to_vec();
    let cases = [
        (Vec::new(), "is empty"),
        (
            b"not a database at all".to_vec(),
            "is not a Stalagmight project",
        ),
        (not_a_project, "is not a Stalagmight project"),
        (
            damaged_sqlite,
            "is damaged too: it fails the same integrity check",
        ),
        (
            invalid_v2_bytes(),
            "is damaged too: it fails the same integrity check",
        ),
        (torn, "is incomplete: the save that wrote it never finished"),
    ];
    for (contents, what) in cases {
        std::fs::write(&backup, &contents).unwrap();
        let before = snapshot(dir.path());

        let err = state()
            .restore_project_from_backup(&project, &backup)
            .unwrap_err();

        assert_eq!(
            err,
            format!(
                "Not restored: cave.stalx.backup {what}. Nothing was changed. Pick another backup."
            )
        );
        assert_eq!(snapshot(dir.path()), before, "{what}");
    }
}

/// A legacy SQLite backup (an old `.backup`, or the pre-upgrade original) is
/// restored over a version 2 project: the next open converts it again.
#[test]
fn a_legacy_backup_restores_over_a_version_2_project() {
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), "cave.stalx", "Current");
    let backup = legacy_file(dir.path(), "cave.stalx.backup", "Older");
    let before = std::fs::read(&project).unwrap();

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    assert_eq!(
        stalx_v2::sniff(&std::fs::read(&project).unwrap()),
        Format::Legacy
    );
    assert_eq!(label(&project), "Older");
    assert_eq!(std::fs::read(pre_restore_path(&project)).unwrap(), before);
    let opened = state();
    opened.do_load_project_full(project).unwrap();
    assert_eq!(opened.get_project_state().unwrap().name, "Older");
}

/// The temp a version 2 save writes through, complete (the rename never
/// happened), is a backup like any other.
#[test]
fn a_complete_save_temp_is_restored() {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("cave.stalx");
    std::fs::write(&project, invalid_v2_bytes()).unwrap();
    let temp = project_file(dir.path(), "cave.stalx.tmp", "Saved");

    state()
        .restore_project_from_backup(&project, &temp)
        .unwrap();

    assert_eq!(label(&project), "Saved");
}

/// An armed journal or a WAL with frames holds committed changes the file
/// alone does not, beside the project or the safety copy a restore would
/// replace (and might have to put back): refused, nothing changed. (Beside the
/// backup they ride with it: see
/// `a_sqlite_backup_restores_with_its_unfinished_changes`.)
#[test]
fn unfinished_changes_beside_the_project_or_the_safety_copy_are_refused() {
    let dir = TempDir::new().unwrap();
    let project = legacy_file(dir.path(), "cave.stalx", "Current");
    let backup = legacy_file(dir.path(), "cave.stalx.backup", "Older");
    legacy_file(dir.path(), "cave.stalx.pre-restore", "Kept");
    let mut armed = JOURNAL_MAGIC.to_vec();
    armed.resize(512, 0);
    let cases = [
        (
            "cave.stalx-wal",
            vec![1; 32],
            "Not restored: the project cave.stalx has changes that were never finished, in \
             cave.stalx-wal beside it, and replacing the file would lose them. Open the \
             project again so Stalagmight can finish them, or move that file away to restore \
             without them.",
        ),
        (
            "cave.stalx-journal",
            armed.clone(),
            "Not restored: the project cave.stalx has changes that were never finished, in \
             cave.stalx-journal beside it, and replacing the file would lose them. Open the \
             project again so Stalagmight can finish them, or move that file away to restore \
             without them.",
        ),
        (
            "cave.stalx.pre-restore-journal",
            armed,
            "Not restored: cave.stalx.pre-restore, where a restore keeps its copy of \
             cave.stalx, has changes that were never finished, in \
             cave.stalx.pre-restore-journal beside it, and replacing the file would lose them. \
             Move those files away, then restore.",
        ),
    ];
    for (sidecar, contents, expected) in cases {
        let sidecar = dir.path().join(sidecar);
        std::fs::write(&sidecar, contents).unwrap();
        let before = snapshot(dir.path());

        let err = state()
            .restore_project_from_backup(&project, &backup)
            .unwrap_err();

        assert_eq!(err, expected);
        assert_eq!(snapshot(dir.path()), before);
        std::fs::remove_file(sidecar).unwrap();
    }
}

/// Beside a version 2 file a journal belongs to no database: it is debris,
/// moved away when the name is replaced, never a reason to refuse, and it
/// never ends up beside a restored SQLite file.
#[test]
fn a_journal_beside_a_version_2_file_is_debris() {
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), "cave.stalx", "Current");
    let backup = legacy_file(dir.path(), "cave.stalx.backup", "Older");
    let mut armed = JOURNAL_MAGIC.to_vec();
    armed.resize(512, 0);
    std::fs::write(dir.path().join("cave.stalx-journal"), &armed).unwrap();

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    assert_eq!(
        names(dir.path()),
        ["cave.stalx", "cave.stalx.backup", "cave.stalx.pre-restore"]
    );
    assert_eq!(label(&project), "Older");
}

/// A SQLite file at `db` caught mid-write, as a kill leaves it: the write
/// renamed the project to "Rolled back" and spilled pages into the file, and
/// the armed journal beside it holds their before-images. The pair is the
/// committed project; the file alone is not. Returns the committed bytes.
fn interrupt_a_write_on(db: &Path) -> Vec<u8> {
    let journal = db.with_file_name(format!(
        "{}-journal",
        db.file_name().unwrap().to_string_lossy()
    ));
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = DELETE;
         PRAGMA cache_size = 10;
         CREATE TABLE IF NOT EXISTS ballast(blob BLOB);",
    )
    .unwrap();
    let committed = std::fs::read(db).unwrap();
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         UPDATE project SET name = 'Rolled back';
         INSERT INTO ballast
             SELECT randomblob(2000) FROM (
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n LIMIT 400)
                 SELECT i FROM n
             );",
    )
    .unwrap();
    let (main, armed) = (std::fs::read(db).unwrap(), std::fs::read(&journal).unwrap());
    conn.execute_batch("ROLLBACK;").unwrap();
    drop(conn);
    std::fs::write(db, &main).unwrap();
    std::fs::write(&journal, &armed).unwrap();
    assert_eq!(
        armed.get(..8),
        Some(&JOURNAL_MAGIC[..]),
        "the journal is armed"
    );
    assert_ne!(main, committed, "the write reached the file");
    committed
}

/// A SQLite file at `db` in WAL mode whose last commit (the project renamed to
/// `name`) is still in its WAL, as a kill leaves it.
fn commit_into_a_wal(db: &Path, name: &str) {
    let sidecar = |suffix: &str| {
        db.with_file_name(format!(
            "{}{suffix}",
            db.file_name().unwrap().to_string_lossy()
        ))
    };
    let conn = rusqlite::Connection::open(db).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    conn.execute_batch("PRAGMA wal_autocheckpoint = 0;")
        .unwrap();
    conn.execute("UPDATE project SET name = ?1", [name])
        .unwrap();
    let files: Vec<(PathBuf, Vec<u8>)> = [db.to_path_buf(), sidecar("-wal"), sidecar("-shm")]
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    drop(conn);
    for (path, bytes) in files {
        std::fs::write(path, bytes).unwrap();
    }
}

/// The label the file at `path` holds ALONE (read on a copy without any
/// sidecar).
fn label_alone(path: &Path) -> String {
    let scratch = TempDir::new().unwrap();
    let copy = scratch.path().join("alone.stalx");
    std::fs::copy(path, &copy).unwrap();
    label(&copy)
}

/// A SQLite backup whose committed state is partly in a sidecar restores as
/// the pair opens: an old build's hot journal rolled back, a WAL read through.
/// The backup and its sidecars are only read. A journal or WAL whose header
/// SQLite would never act on is no change.
#[test]
fn a_sqlite_backup_restores_with_its_unfinished_changes() {
    let mut armed = JOURNAL_MAGIC.to_vec();
    armed.resize(512, 0);
    for case in [
        "hot journal",
        "wal",
        "unsynced journal header",
        "foreign wal",
    ] {
        let dir = TempDir::new().unwrap();
        let project = project_file(dir.path(), "cave.stalx", "Current");
        let backup = legacy_file(dir.path(), "cave.stalx.backup", "Older");
        let restored_as = match case {
            "hot journal" => {
                interrupt_a_write_on(&backup);
                "Older"
            }
            "wal" => {
                commit_into_a_wal(&backup, "Newer in the WAL");
                assert_eq!(label_alone(&backup), "Older");
                "Newer in the WAL"
            }
            "unsynced journal header" => {
                std::fs::write(dir.path().join("cave.stalx.backup-journal"), &armed).unwrap();
                "Older"
            }
            _ => {
                std::fs::write(dir.path().join("cave.stalx.backup-wal"), vec![1; 32]).unwrap();
                "Older"
            }
        };
        let backup_files: BTreeMap<String, (usize, u64)> = snapshot(dir.path())
            .into_iter()
            .filter(|(name, _)| name.starts_with("cave.stalx.backup"))
            .collect();

        let restored = state().restore_project_from_backup(&project, &backup);
        assert!(restored.is_ok(), "{case}: {restored:?}");
        let message = restored.unwrap();

        assert!(message.starts_with("Restored from "), "{case}: {message}");
        // Before anything reads the restored files (a read of a WAL-mode file
        // makes its own sidecars).
        assert_eq!(
            names(dir.path())
                .into_iter()
                .filter(|n| !n.starts_with("cave.stalx.backup"))
                .collect::<Vec<_>>(),
            ["cave.stalx", "cave.stalx.pre-restore"],
            "{case}: nothing staged is left, and no sidecar beside the restored file"
        );
        let after: BTreeMap<String, (usize, u64)> = snapshot(dir.path())
            .into_iter()
            .filter(|(name, _)| name.starts_with("cave.stalx.backup"))
            .collect();
        assert_eq!(
            after, backup_files,
            "{case}: the backup and its sidecars are only read"
        );
        assert_eq!(label(&project), restored_as, "{case}");
        assert_eq!(label(&pre_restore_path(&project)), "Current", "{case}");
    }
}

/// A backup reached through a link has its sidecars beside the file the link
/// names, and those are the ones folded into the restored file.
#[cfg(unix)]
#[test]
fn a_linked_backup_restores_with_the_sidecars_beside_the_file_it_names() {
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), "cave.stalx", "Current");
    std::fs::create_dir(dir.path().join("elsewhere")).unwrap();
    let real = legacy_file(&dir.path().join("elsewhere"), "old.stalx", "Older");
    interrupt_a_write_on(&real);
    let link = dir.path().join("cave.stalx.backup");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let elsewhere = snapshot(&dir.path().join("elsewhere"));

    state()
        .restore_project_from_backup(&project, &link)
        .unwrap();

    assert_eq!(
        label(&project),
        "Older",
        "the pair beside the file the link names"
    );
    assert_eq!(snapshot(&dir.path().join("elsewhere")), elsewhere);
}

/// A sidecar that is not a regular file (a FIFO would block any open of it)
/// is refused without being opened, and promptly.
#[cfg(unix)]
#[test]
fn a_sidecar_that_is_not_a_file_is_never_opened() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let fifo = dir.path().join("cave.stalx-journal");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success(), "mkfifo");
    let (done, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(state().restore_project_from_backup(&project, &backup));
    });

    let err = result
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the restore must not block on a FIFO")
        .unwrap_err();

    assert_eq!(
        err,
        "Not restored: cave.stalx-journal beside cave.stalx is not a file. Move it away, then \
         restore."
    );
}

/// Copy the journal of a write transaction on `db` that has already written
/// into the database file: a genuine armed journal, as a kill mid-save leaves.
fn armed_journal_of(db: &Path) -> Vec<u8> {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = DELETE;
         PRAGMA cache_size = 10;
         CREATE TABLE IF NOT EXISTS ballast(blob BLOB);
         BEGIN IMMEDIATE;
         UPDATE project SET name = 'Rolled back';
         INSERT INTO ballast
             SELECT randomblob(2000) FROM (
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n LIMIT 400)
                 SELECT i FROM n
             );",
    )
    .unwrap();
    let mut journal_name = db.file_name().unwrap().to_os_string();
    journal_name.push("-journal");
    let journal = std::fs::read(db.with_file_name(journal_name)).unwrap();
    conn.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(
        journal.get(..8),
        Some(&JOURNAL_MAGIC[..]),
        "the journal is armed"
    );
    journal
}

/// SQLite finds a sidecar by name, so a stale journal beside a name the restore
/// replaces would replay into the new file. An armed one beside a SQLite file
/// (here a genuine one, which the control shows does replay) is refused;
/// debris beside the project and the safety copy is moved away, and the new
/// files open intact.
#[test]
fn stale_sidecars_never_reach_the_restored_files() {
    let dir = TempDir::new().unwrap();
    let project = legacy_file(dir.path(), "cave.stalx", "Current");
    let backup = legacy_file(dir.path(), "cave.stalx.backup", "Older");
    let safety = legacy_file(dir.path(), "cave.stalx.pre-restore", "Old safety copy");
    let stranger = legacy_file(dir.path(), "stranger.stalx", "Stranger");
    let journal = armed_journal_of(&stranger);
    std::fs::remove_file(&stranger).unwrap();

    // Control: this journal really does replay into a file it lands beside.
    let control = legacy_file(dir.path(), "control.stalx", "Control");
    std::fs::write(dir.path().join("control.stalx-journal"), &journal).unwrap();
    let replayed = rusqlite::Connection::open(&control).and_then(|conn| {
        conn.query_row("SELECT name FROM project", [], |row| {
            row.get::<_, String>(0)
        })
    });
    assert_ne!(
        replayed.ok().as_deref(),
        Some("Control"),
        "the stale journal replays"
    );
    for leftover in ["control.stalx", "control.stalx-journal"] {
        let _ = std::fs::remove_file(dir.path().join(leftover));
    }

    // Armed beside the old safety copy: refused, nothing changed.
    let armed = dir.path().join("cave.stalx.pre-restore-journal");
    std::fs::write(&armed, &journal).unwrap();
    let before = snapshot(dir.path());
    let err = state()
        .restore_project_from_backup(&project, &backup)
        .unwrap_err();
    assert!(
        err.contains("has changes that were never finished"),
        "{err}"
    );
    assert_eq!(snapshot(dir.path()), before);
    std::fs::remove_file(&armed).unwrap();

    // Debris: an unarmed journal and WAL indexes.
    std::fs::write(dir.path().join("cave.stalx-journal"), [0u8; 512]).unwrap();
    std::fs::write(dir.path().join("cave.stalx-shm"), [0u8; 64]).unwrap();
    std::fs::write(dir.path().join("cave.stalx.pre-restore-shm"), [0u8; 64]).unwrap();

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    assert_eq!(
        names(dir.path()),
        ["cave.stalx", "cave.stalx.backup", "cave.stalx.pre-restore"]
    );
    drop(read_legacy_file(&project));
    drop(read_legacy_file(&safety));
    assert_eq!(label(&project), "Older");
    assert_eq!(label(&safety), "Current");
}

/// Make the rename onto `db` fail: its `-shm` is a folder, and the name it
/// would be set aside under is a folder that is not empty.
fn block_the_rename_onto(db: &Path) {
    let mut shm = db.file_name().unwrap().to_os_string();
    shm.push("-shm");
    let shm = db.with_file_name(shm);
    std::fs::create_dir(&shm).unwrap();
    let mut aside = shm.file_name().unwrap().to_os_string();
    aside.push(".set-aside");
    let aside = shm.with_file_name(aside);
    std::fs::create_dir(&aside).unwrap();
    std::fs::write(aside.join("keep"), b"in the way").unwrap();
}

/// The safety copy comes first: when it cannot be written, the project is not
/// replaced, the previous safety copy is back, and nothing staged is left.
#[test]
fn no_safety_copy_no_restore() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let safety = project_file(dir.path(), "cave.stalx.pre-restore", "Kept");
    block_the_rename_onto(&safety);
    let before = snapshot(dir.path());

    let err = state()
        .restore_project_from_backup(&project, &backup)
        .unwrap_err();

    assert!(
        err.starts_with(
            "Not restored: could not keep a copy of cave.stalx as cave.stalx.pre-restore ("
        ) && err.ends_with("). Nothing was changed."),
        "{err}"
    );
    assert_eq!(snapshot(dir.path()), before);
}

/// When the backup cannot take the project's name, the safety copy the restore
/// just replaced goes back: a failed restore changes nothing. That includes an
/// undo, where the safety copy IS the file being restored.
#[test]
fn a_restore_that_fails_at_the_last_step_changes_nothing() {
    for undo in [false, true] {
        let dir = TempDir::new().unwrap();
        let (project, backup) = cave(&dir);
        let safety = project_file(dir.path(), "cave.stalx.pre-restore", "Kept");
        block_the_rename_onto(&project);
        let before = snapshot(dir.path());
        let source = if undo { &safety } else { &backup };

        let err = state()
            .restore_project_from_backup(&project, source)
            .unwrap_err();

        assert!(
            err.starts_with("Not restored: could not replace cave.stalx (")
                && err.ends_with("). Nothing was changed."),
            "undo={undo}: {err}"
        );
        assert_eq!(snapshot(dir.path()), before, "undo={undo}");
        assert_eq!(label(&safety), "Kept", "undo={undo}");
    }
}

/// The same with no safety copy before the restore: the new one goes again.
#[test]
fn a_failed_first_restore_leaves_no_safety_copy_behind() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    block_the_rename_onto(&project);
    let before = snapshot(dir.path());

    let err = state()
        .restore_project_from_backup(&project, &backup)
        .unwrap_err();

    assert!(err.ends_with("). Nothing was changed."), "{err}");
    assert_eq!(snapshot(dir.path()), before);
    assert!(!pre_restore_path(&project).exists());
}

/// A SQLite backup in WAL mode (as a file from elsewhere may be) restores, and
/// checking it leaves no `-wal`/`-shm` behind under the staged copy's name.
#[test]
fn a_wal_mode_backup_leaves_no_sidecars_behind() {
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), "cave.stalx", "Current");
    let backup = legacy_file(dir.path(), "cave.stalx.backup", "Older");
    {
        let conn = rusqlite::Connection::open(&backup).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }
    assert_eq!(names(dir.path()), ["cave.stalx", "cave.stalx.backup"]);

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    assert_eq!(
        names(dir.path()),
        ["cave.stalx", "cave.stalx.backup", "cave.stalx.pre-restore"]
    );
    assert_eq!(label(&project), "Older");
}

/// A load opens its file before it takes the project slot, so a restore while
/// one is in flight could be saved over later: refused, nothing changed.
#[test]
fn no_restore_while_a_project_is_being_opened() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let state = state();
    let loading = state.start_new_load();
    let before = snapshot(dir.path());

    let err = state
        .restore_project_from_backup(&project, &backup)
        .unwrap_err();

    assert_eq!(
        err,
        "Not restored: a project is being opened. Try again once it has opened."
    );
    assert_eq!(snapshot(dir.path()), before);
    state.clear_active_load(&loading);
    state
        .restore_project_from_backup(&project, &backup)
        .unwrap();
}

#[test]
fn a_missing_project_is_restored_without_a_safety_copy() {
    let dir = TempDir::new().unwrap();
    let backup = project_file(dir.path(), "cave.stalx.backup", "Older");
    let project = dir.path().join("cave.stalx");

    let message = state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    assert_eq!(message, "Restored from cave.stalx.backup.");
    assert_eq!(label(&project), "Older");
    assert_eq!(names(dir.path()), ["cave.stalx", "cave.stalx.backup"]);
}

/// A safety copy left behind by an earlier restore, restored into a project
/// that is gone: nothing is swapped, so the message does not say so.
#[test]
fn a_left_over_safety_copy_restored_into_a_missing_project_is_not_a_swap() {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("cave.stalx");
    let safety = project_file(dir.path(), "cave.stalx.pre-restore", "Kept");

    let message = state()
        .restore_project_from_backup(&project, &safety)
        .unwrap();

    assert_eq!(message, "Restored from cave.stalx.pre-restore.");
    assert_eq!(label(&project), "Kept");
    assert_eq!(label(&safety), "Kept", "the copy is only read");
}

#[test]
fn a_missing_backup_is_reported() {
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), "cave.stalx", "Current");
    let backup = dir.path().join("cave.stalx.backup");
    let before = snapshot(dir.path());

    let err = state()
        .restore_project_from_backup(&project, &backup)
        .unwrap_err();

    assert_eq!(err, format!("Backup file not found: {}", backup.display()));
    assert_eq!(snapshot(dir.path()), before);
}

/// A project path that is a link is restored into the file it names, as the
/// in-place copy used to: the link stays a link.
#[cfg(unix)]
#[test]
fn a_linked_project_is_restored_into_the_file_it_names() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    let real = project_file(&dir.path().join("real"), "cave.stalx", "Current");
    let link = dir.path().join("cave.stalx");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let backup = project_file(dir.path(), "cave.stalx.backup", "Older");

    state().restore_project_from_backup(&link, &backup).unwrap();

    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(label(&real), "Older");
    assert_eq!(label(&pre_restore_path(&real)), "Current");
}

/// The restored file takes the project's place, permissions included, and
/// the safety copy keeps them too. A read-only backup is copied, not opened
/// for writing.
#[cfg(unix)]
#[test]
fn the_restored_files_keep_the_projects_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    for backup_mode in [0o644, 0o444] {
        let dir = TempDir::new().unwrap();
        let (project, backup) = cave(&dir);
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(backup_mode)).unwrap();

        state()
            .restore_project_from_backup(&project, &backup)
            .unwrap();

        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&project), 0o640, "backup {backup_mode:o}");
        assert_eq!(mode(&pre_restore_path(&project)), 0o640);
        assert_eq!(mode(&backup), backup_mode, "the backup is only read");
        assert_eq!(label(&project), "Older");
    }
}

/// A project that does not exist yet takes the backup's permissions.
#[cfg(unix)]
#[test]
fn a_missing_project_takes_the_backups_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = TempDir::new().unwrap();
    let backup = project_file(dir.path(), "cave.stalx.backup", "Older");
    std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o604)).unwrap();
    let project = dir.path().join("cave.stalx");

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    let mode = std::fs::metadata(&project).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o604);
}

// --- list_project_backups -------------------------------------------------

fn offered(path: &Path) -> Vec<String> {
    let mut found: Vec<String> = AppState::list_project_backups(path)
        .into_iter()
        .map(|b| {
            Path::new(&b.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    found
}

/// Beside the project: both formats, a complete temp, the copy the last
/// restore kept (so a restore can be undone); never the project itself (under
/// a `.backup` name of its own), a sidecar, an empty or foreign file, or a
/// torn temp.
#[test]
fn the_dialog_offers_restorable_files_beside_the_project_only() {
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), "cave.backup.stalx", "Current");
    project_file(dir.path(), "cave.backup.stalx.backup", "Older");
    legacy_file(dir.path(), "cave.backup.stalx.backup.1", "Oldest");
    project_file(dir.path(), "cave.backup.stalx.tmp", "Saved");
    project_file(dir.path(), "cave.backup.stalx.pre-restore", "Kept");
    let complete = v2_bytes("Torn");
    std::fs::write(
        dir.path().join("cave.backup.stalx.backup.2"),
        &complete[..complete.len() - 8],
    )
    .unwrap();
    std::fs::write(dir.path().join("cave.backup.stalx.backup.3"), b"").unwrap();
    std::fs::write(dir.path().join("cave.backup.stalx.backup.4"), b"notes").unwrap();
    std::fs::write(dir.path().join("cave.backup.stalx-journal"), [0u8; 512]).unwrap();
    std::fs::write(dir.path().join("cave.backup.stalx.backup-wal"), [1u8; 64]).unwrap();

    assert_eq!(
        offered(&project),
        [
            "cave.backup.stalx.backup",
            "cave.backup.stalx.backup.1",
            "cave.backup.stalx.pre-restore",
            "cave.backup.stalx.tmp"
        ]
    );
}

/// The SQLite original the first version 2 save kept in the app-data backups
/// folder is offered for that project, and only for it: a project whose name
/// only starts the same (`<stem>.old`) is not offered it.
#[test]
fn the_dialog_offers_the_pre_upgrade_original_from_the_backups_folder() {
    crate::app_dirs::init_for_test();
    let folder = super::project_io::upgrade_backup_dir().unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    let stem = format!("restore-list-{}", uuid::Uuid::new_v4());
    let dir = TempDir::new().unwrap();
    let project = project_file(dir.path(), &format!("{stem}.stalx"), "Current");
    let other_project = project_file(dir.path(), &format!("{stem}.old.stalx"), "Other");
    let mine = super::project_io::upgrade_backup_prefix(&project);
    let theirs = super::project_io::upgrade_backup_prefix(&other_project);
    let original = legacy_file(
        &folder,
        &format!("{mine}20261120T101112.123456789Z.pre-v2.stalx"),
        "Original",
    );
    let other = legacy_file(
        &folder,
        &format!("{theirs}20261120T101112.123456789Z.pre-v2.stalx"),
        "Other",
    );
    let sidecar = folder.join(format!(
        "{mine}20261120T101112.123456789Z.pre-v2.stalx-journal"
    ));
    std::fs::write(&sidecar, [0u8; 512]).unwrap();

    let listed = offered(&project);
    let listed_other = offered(&other_project);
    for leftover in [&original, &other, &sidecar] {
        std::fs::remove_file(leftover).unwrap();
    }

    assert_eq!(
        listed,
        [original.file_name().unwrap().to_string_lossy().into_owned()]
    );
    assert_eq!(
        listed_other,
        [other.file_name().unwrap().to_string_lossy().into_owned()]
    );
}

/// The pre-upgrade original is restorable: the project goes back to the old
/// format (its next save upgrades it again).
#[test]
fn the_pre_upgrade_original_is_offered_and_restores() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let stem = format!("restore-upgrade-{}", uuid::Uuid::new_v4());
    let path = legacy_file(dir.path(), &format!("{stem}.stalx"), "Legacy");
    let original = std::fs::read(&path).unwrap();
    let state = state();
    state.do_load_project_full(path.clone()).unwrap();
    assert!(state.save_project(None).success);
    assert_eq!(stalx_v2::sniff(&std::fs::read(&path).unwrap()), Format::V2);
    assert!(state.close_project().success);

    let backups = AppState::list_project_backups(&path);
    let kept = backups
        .iter()
        .find(|b| std::fs::read(&b.path).ok().as_deref() == Some(original.as_slice()))
        .expect("the original is offered");
    assert_eq!(kept.kind, BackupKind::PreUpgrade);
    let message = state
        .restore_project_from_backup(&path, Path::new(&kept.path))
        .unwrap();

    // Named as the dialog lists it, never by its app-data path.
    assert_eq!(
        message,
        format!(
            "Restored the original from before the upgrade to the new format. The file it \
             replaced is kept as {stem}.stalx.pre-restore and is listed with the earlier copies, \
             so you can switch back."
        )
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    for backup in &backups {
        let _ = std::fs::remove_file(&backup.path);
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// Every offered copy says what it is (the dialog words it from the kind:
/// real-app demo I-2), newest first, with its time in one fixed-width UTC
/// form the dialog shows in local time.
#[test]
fn each_offered_copy_says_what_it_is_newest_first() {
    use std::time::{Duration, UNIX_EPOCH};
    crate::app_dirs::init_for_test();
    let folder = super::project_io::upgrade_backup_dir().unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    let dir = TempDir::new().unwrap();
    let stem = format!("restore-kinds-{}", uuid::Uuid::new_v4());
    let project = project_file(dir.path(), &format!("{stem}.stalx"), "Current");
    let prefix = super::project_io::upgrade_backup_prefix(&project);
    let original = legacy_file(
        &folder,
        &format!("{prefix}20261120T101112.123456789Z.pre-v2.stalx"),
        "Original",
    );
    let repair = project_file(dir.path(), &format!("{stem}.stalx.backup"), "Repaired");
    let temp = project_file(dir.path(), &format!("{stem}.stalx.tmp"), "Interrupted");
    let safety = project_file(dir.path(), &format!("{stem}.stalx.pre-restore"), "Kept");
    // Oldest to newest, a minute apart: the original, the repair's copy, the
    // interrupted save, the copy the last restore kept.
    let base = UNIX_EPOCH + Duration::from_hours(500_000);
    for (minutes, path) in [(0, &original), (1, &repair), (2, &temp), (3, &safety)] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(base + Duration::from_mins(minutes))
            .unwrap();
    }

    let listed = AppState::list_project_backups(&project);
    std::fs::remove_file(&original).unwrap();

    let kinds: Vec<(String, BackupKind)> = listed
        .iter()
        .map(|b| (file_name(&b.path), b.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            (format!("{stem}.stalx.pre-restore"), BackupKind::PreRestore),
            (format!("{stem}.stalx.tmp"), BackupKind::InterruptedSave),
            (format!("{stem}.stalx.backup"), BackupKind::Backup),
            (
                file_name(&original.display().to_string()),
                BackupKind::PreUpgrade
            ),
        ]
    );
    let times: Vec<&str> = listed.iter().map(|b| b.modified_at.as_str()).collect();
    assert_eq!(
        times,
        [
            "2027-01-15T08:03:00.000Z",
            "2027-01-15T08:02:00.000Z",
            "2027-01-15T08:01:00.000Z",
            "2027-01-15T08:00:00.000Z",
        ]
    );
}

/// The copy a restore keeps is offered next (real-app demo I-2: it sat beside
/// the project and the list never showed it), so a restore can be undone from
/// the dialog: restoring it swaps the two files back.
#[test]
fn the_copy_the_last_restore_kept_is_offered_and_undoes_it() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let kept_copies = |listed: &[crate::persistence::BackupInfo]| -> Vec<PathBuf> {
        listed
            .iter()
            .filter(|b| b.kind == BackupKind::PreRestore)
            .map(|b| PathBuf::from(&b.path))
            .collect()
    };
    assert!(kept_copies(&AppState::list_project_backups(&project)).is_empty());

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();
    let kept = kept_copies(&AppState::list_project_backups(&project));
    assert_eq!(kept, [pre_restore_path(&project)]);

    let message = state()
        .restore_project_from_backup(&project, &kept[0])
        .unwrap();
    assert!(message.contains("swapped places"), "{message}");
    assert_eq!(label(&project), "Current");
    assert_eq!(label(&pre_restore_path(&project)), "Older");
}

/// A project reached through a link keeps its restore copy beside the file the
/// link names, and that is the copy offered. A copy that is the project
/// itself (a hard link) is never offered.
#[cfg(unix)]
#[test]
fn the_offered_restore_copy_is_the_one_a_restore_keeps() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    let real = project_file(&dir.path().join("real"), "cave.stalx", "Current");
    let link = dir.path().join("cave.stalx");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let backup = project_file(dir.path(), "cave.stalx.backup", "Older");
    state().restore_project_from_backup(&link, &backup).unwrap();

    let offered: Vec<PathBuf> = AppState::list_project_backups(&link)
        .into_iter()
        .filter(|b| b.kind == BackupKind::PreRestore)
        .map(|b| PathBuf::from(b.path))
        .collect();
    assert_eq!(offered, [pre_restore_path(&real.canonicalize().unwrap())]);

    let other = TempDir::new().unwrap();
    let project = project_file(other.path(), "cave.stalx", "Current");
    std::fs::hard_link(&project, pre_restore_path(&project)).unwrap();
    assert!(AppState::list_project_backups(&project).is_empty());
}

/// The kind's wire names are the ones the generated TypeScript type lists
/// (`src/types/ipc/BackupKind.ts`), which the dialog words.
#[test]
fn backup_kinds_go_over_the_wire_as_the_dialog_reads_them() {
    let wire: Vec<serde_json::Value> = [
        BackupKind::PreUpgrade,
        BackupKind::PreRestore,
        BackupKind::Backup,
        BackupKind::InterruptedSave,
    ]
    .into_iter()
    .map(|kind| serde_json::to_value(kind).unwrap())
    .collect();
    assert_eq!(
        wire,
        ["pre_upgrade", "pre_restore", "backup", "interrupted_save"]
    );
}
