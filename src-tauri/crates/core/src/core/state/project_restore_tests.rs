//! [#720] `AppState::restore_project_from_backup`: what it writes, what it
//! refuses, and that a refusal or a failure leaves every file as it was.
//!
//! All data is synthetic: projects built in memory and saved by this build. No
//! fixture is read or written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use super::project_restore::pre_restore_path;
use super::AppState;
use crate::core::events::EventSink;
use crate::models::Project;
use crate::persistence::Database;

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

/// A saved project named `label` at `dir/file`.
fn project_file(dir: &Path, file: &str, label: &str) -> PathBuf {
    let path = dir.join(file);
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new(label.to_string(), None))
        .unwrap();
    path
}

/// The project name stored in the file, read without writing to it.
fn label(path: &Path) -> String {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("SELECT name FROM project", [], |row| row.get(0))
        .unwrap()
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
        format!(
            "Restored from {}. The file it replaced is kept as cave.stalx.pre-restore.",
            backup.display()
        )
    );
    assert_eq!(
        names(dir.path()),
        ["cave.stalx", "cave.stalx.backup", "cave.stalx.pre-restore"],
        "no staged copy is left behind"
    );
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
    let damaged = {
        let path = project_file(dir.path(), "damaged.stalx", "Damaged");
        damage_a_page(&path);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        bytes
    };
    let cases = [
        (Vec::new(), "is empty"),
        (
            b"not a database at all".to_vec(),
            "is not a Stalagmight project",
        ),
        (not_a_project, "is not a Stalagmight project"),
        (damaged, "is damaged too: it fails the same integrity check"),
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

/// An armed journal or a WAL with frames holds committed changes the file
/// alone does not, beside the backup or the project: refused, nothing changed.
#[test]
fn unfinished_changes_beside_the_backup_or_the_project_are_refused() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let mut armed = JOURNAL_MAGIC.to_vec();
    armed.resize(512, 0);
    let cases = [
        (
            "cave.stalx.backup-journal",
            armed.clone(),
            "Not restored: the backup cave.stalx.backup has changes that were never finished, \
             in cave.stalx.backup-journal beside it. Pick another backup.",
        ),
        (
            "cave.stalx.backup-wal",
            vec![1; 32],
            "Not restored: the backup cave.stalx.backup has changes that were never finished, \
             in cave.stalx.backup-wal beside it. Pick another backup.",
        ),
        (
            "cave.stalx-journal",
            armed,
            "Not restored: the project cave.stalx has changes that were never finished, in \
             cave.stalx-journal beside it. Open the project once so Stalagmight can finish \
             them, then restore.",
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
/// replaces would replay into the new file. The ones beside the project and
/// beside the old safety copy are moved away, and the new files open intact.
#[test]
fn stale_sidecars_never_reach_the_restored_files() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let safety = project_file(dir.path(), "cave.stalx.pre-restore", "Old safety copy");
    let stranger = project_file(dir.path(), "stranger.stalx", "Stranger");
    let journal = armed_journal_of(&stranger);
    std::fs::remove_file(&stranger).unwrap();

    // Control: this journal really does replay into a file it lands beside.
    let control = project_file(dir.path(), "control.stalx", "Control");
    std::fs::write(dir.path().join("control.stalx-journal"), &journal).unwrap();
    let replayed = Database::open_existing(&control).map(|_| label(&control));
    assert_ne!(
        replayed.ok().as_deref(),
        Some("Control"),
        "the stale journal replays"
    );
    for leftover in ["control.stalx", "control.stalx-journal"] {
        let _ = std::fs::remove_file(dir.path().join(leftover));
    }

    // An armed journal beside the old safety copy, debris beside the project.
    std::fs::write(dir.path().join("cave.stalx.pre-restore-journal"), &journal).unwrap();
    std::fs::write(dir.path().join("cave.stalx-journal"), [0u8; 512]).unwrap();
    std::fs::write(dir.path().join("cave.stalx-shm"), [0u8; 64]).unwrap();

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    assert_eq!(
        names(dir.path()),
        ["cave.stalx", "cave.stalx.backup", "cave.stalx.pre-restore"]
    );
    drop(Database::open_existing(&project).unwrap());
    drop(Database::open_existing(&safety).unwrap());
    assert_eq!(label(&project), "Older");
    assert_eq!(label(&safety), "Current");
}

/// The safety copy comes first: when it cannot be written, the project is not
/// replaced, and nothing staged is left behind.
#[test]
fn no_safety_copy_no_restore() {
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    let blocker = pre_restore_path(&project);
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(
        blocker.join("keep"),
        b"a folder where the safety copy would go",
    )
    .unwrap();
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

#[test]
fn a_missing_project_is_restored_without_a_safety_copy() {
    let dir = TempDir::new().unwrap();
    let backup = project_file(dir.path(), "cave.stalx.backup", "Older");
    let project = dir.path().join("cave.stalx");

    let message = state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    assert_eq!(message, format!("Restored from {}.", backup.display()));
    assert_eq!(label(&project), "Older");
    assert_eq!(names(dir.path()), ["cave.stalx", "cave.stalx.backup"]);
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

/// The staged copies are created owner-only; `std::fs::copy` then gives each
/// the permissions of the file it copies, as the in-place copy did.
#[cfg(unix)]
#[test]
fn the_restored_files_keep_the_permissions_they_were_copied_from() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = TempDir::new().unwrap();
    let (project, backup) = cave(&dir);
    std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o644)).unwrap();

    state()
        .restore_project_from_backup(&project, &backup)
        .unwrap();

    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&project), 0o644);
    assert_eq!(mode(&pre_restore_path(&project)), 0o640);
}
