//! [#576 C15, NF-AN2-7, B27(f)] The recovery dialog's backend: which files it
//! offers, how it checks them, how a restore installs one, and how Repair
//! rewrites a file.
//!
//! All files are synthetic projects this build writes, some frozen mid-write
//! with raw SQL to produce a hot rollback journal. No fixture is read.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

use super::{BackupInfo, BackupKind, Database, RefusedSchemaObject};
use crate::models::Project;

/// A valid project file named `name` at `path`.
fn project_file(path: &Path, name: &str) {
    let mut db = Database::open(path).unwrap();
    db.save_project(&Project::new(name.to_string(), None))
        .unwrap();
}

/// The project name in `path` through the ordinary open path, or why it failed.
fn project_name(path: &Path) -> String {
    Database::open_existing(path)
        .and_then(|db| db.load_project())
        .map_or_else(|e| format!("<unreadable: {e:#}>"), |p| p.name)
}

fn journal_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push("-journal");
    path.with_file_name(name)
}

/// Copy `src` to `dest` frozen in the middle of a write: the main file already
/// holds pages a transaction spilled (the project renamed to "Torn", plus
/// filler), and `dest`'s hot rollback journal holds what undoes them. Opened
/// normally the copy rolls back to `src`'s committed state; read without the
/// journal it is torn (renamed, or outright malformed).
fn torn_copy(src: &Path, dest: &Path) {
    let conn = Connection::open(src).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = DELETE;
         PRAGMA cache_size = 10;
         BEGIN;
         UPDATE project SET name = 'Torn';
         CREATE TABLE filler (x BLOB);",
    )
    .unwrap();
    for _ in 0..300 {
        conn.execute("INSERT INTO filler VALUES (randomblob(4000))", [])
            .unwrap();
    }
    std::fs::copy(src, dest).unwrap();
    std::fs::copy(journal_of(src), journal_of(dest)).unwrap();
    conn.execute_batch("ROLLBACK;").unwrap();
}

/// What `path`'s main file says WITHOUT its journal (SQLite's `immutable` mode
/// ignores it): the torn state an interrupted write left on disk.
fn name_ignoring_the_journal(path: &Path) -> String {
    let uri = format!("file:{}?immutable=1", path.display());
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    conn.query_row("SELECT name FROM project LIMIT 1", [], |r| r.get(0))
        .unwrap_or_else(|e| format!("<unreadable: {e}>"))
}

fn listed<'a>(list: &'a [BackupInfo], file: &str) -> Option<&'a BackupInfo> {
    list.iter()
        .find(|b| Path::new(&b.path).file_name().unwrap().to_string_lossy() == file)
}

/// [#576 C15, NF-AN2-7] An interrupted save's `.tmp` is restored WITH its
/// rollback journal - checked and installed as its last committed state - and
/// both stay exactly as they were. Copying the `.tmp` alone installed the torn
/// pages the interrupted write had already spilled into it.
#[test]
fn an_interrupted_save_is_restored_with_its_journal_as_committed() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "Original");
    let committed = temp.path().join("work.stalx");
    project_file(&committed, "Committed");
    let interrupted = temp.path().join("CaveA.stalx.tmp");
    torn_copy(&committed, &interrupted);
    let torn = name_ignoring_the_journal(&interrupted);
    println!("the .tmp without its journal reads: {torn}");
    assert_ne!(
        torn, "Committed",
        "premise: without its journal the .tmp is torn"
    );
    let tmp_before = std::fs::read(&interrupted).unwrap();
    let journal_before = std::fs::read(journal_of(&interrupted)).unwrap();

    let outcome = Database::restore_from_backup(&project, &interrupted);
    println!(
        "restore -> {:?}",
        outcome.as_ref().map_err(|e| format!("{e:#}"))
    );
    outcome.unwrap();
    let restored = project_name(&project);
    println!("project now holds: {restored}");
    assert_eq!(
        restored, "Committed",
        "the committed state, not the torn pages"
    );
    assert!(
        std::fs::read(&interrupted).unwrap() == tmp_before
            && std::fs::read(journal_of(&interrupted)).unwrap() == journal_before,
        "the candidate and its journal are left as they were"
    );
}

/// [#576 NF-AN2-7] A rollback journal left beside the PROJECT (the file was being
/// written when it broke) belongs to the file being replaced: it moves aside with
/// that file's safety copy, so it cannot be rolled back into the restored file,
/// and the safety copy reads back as the file was.
#[test]
fn a_journal_left_beside_the_project_moves_aside_with_its_safety_copy() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("old.stalx");
    project_file(&source, "Original");
    let project = temp.path().join("CaveA.stalx");
    torn_copy(&source, &project);
    let backup = temp.path().join("CaveA.stalx.backup");
    project_file(&backup, "Restored");

    let outcome = Database::restore_from_backup(&project, &backup).unwrap();
    let safety = outcome.safety_copy.unwrap();
    println!(
        "project now holds: {}; safety copy {:?} holds: {}; project journal left: {}",
        project_name(&project),
        safety.file_name().unwrap(),
        project_name(&safety),
        journal_of(&project).exists()
    );
    assert_eq!(project_name(&project), "Restored");
    assert!(
        !journal_of(&project).exists(),
        "no journal left beside the restored file"
    );
    assert_eq!(
        project_name(&safety),
        "Original",
        "the replaced file and its journal are kept together"
    );
}

/// [#576 RV3-R-6] When the final rename fails AFTER the project's rollback
/// journal moved aside with its safety copy (Windows refuses the rename while
/// another process holds the file), the journal goes back beside the project:
/// the error says the project file was not changed, and it must read exactly as
/// it did. Left with the copy, the project's pages opened torn.
#[test]
fn a_failed_install_puts_the_projects_journal_back() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("old.stalx");
    project_file(&source, "Original");
    let project = temp.path().join("CaveA.stalx");
    torn_copy(&source, &project);
    let backup = temp.path().join("CaveA.stalx.backup");
    project_file(&backup, "Restored");
    let project_before = std::fs::read(&project).unwrap();
    let journal_before = std::fs::read(journal_of(&project)).unwrap();

    let outcome = {
        let _failing = super::repair::fault::fail_restore_install();
        Database::restore_from_backup(&project, &backup)
    };
    let message = outcome.as_ref().err().map(|e| format!("{e:#}"));
    // Bytes first: opening the project below rolls its journal back.
    let journal_back = std::fs::read(journal_of(&project)).ok();
    let project_unchanged = std::fs::read(&project).unwrap() == project_before;
    let listing: Vec<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("CaveA.stalx"))
        .collect();
    println!("restore -> {message:?}");
    println!(
        "project journal back: {}; project bytes unchanged: {project_unchanged}; \
         project reads: {}; beside it: {listing:?}",
        journal_back.as_deref() == Some(journal_before.as_slice()),
        project_name(&project)
    );
    assert!(
        message
            .as_deref()
            .is_some_and(|m| m.contains("your project file was not changed")),
        "{message:?}"
    );
    assert!(
        journal_back.as_deref() == Some(journal_before.as_slice()),
        "the project's journal must be back beside it, unchanged"
    );
    assert!(project_unchanged);
    assert_eq!(project_name(&project), "Original");
    assert!(
        !listing.iter().any(|name| name.contains(".pre-restore.")),
        "no torn safety copy is left behind: {listing:?}"
    );
}

/// [#576 C15] Restores never overwrite an earlier safety copy: each one is
/// numbered, and the copy of the original survives any number of restores.
#[test]
fn every_restore_keeps_its_own_numbered_safety_copy() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "Original");
    let first = temp.path().join("CaveA.stalx.backup");
    project_file(&first, "BackupOne");
    let second = temp.path().join("CaveA.stalx.backup.1");
    project_file(&second, "BackupTwo");

    let one = Database::restore_from_backup(&project, &first).unwrap();
    let two = Database::restore_from_backup(&project, &second).unwrap();
    let one = one.safety_copy.unwrap();
    let two = two.safety_copy.unwrap();
    assert_eq!(one.file_name().unwrap(), "CaveA.stalx.pre-restore.1");
    assert_eq!(two.file_name().unwrap(), "CaveA.stalx.pre-restore.2");
    assert_eq!(project_name(&one), "Original");
    assert_eq!(project_name(&two), "BackupOne");
    assert_eq!(project_name(&project), "BackupTwo");
}

/// [#576 C15] A candidate that does not pass the checks is refused before the
/// project is touched, and no safety copy or staging file is left behind.
#[test]
fn an_unusable_candidate_is_refused_before_anything_is_written() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "Original");
    let before = std::fs::read(&project).unwrap();
    let empty = temp.path().join("CaveA.stalx.backup");
    // A real database, but holding no project.
    drop(
        Connection::open(&empty)
            .unwrap()
            .execute_batch("CREATE TABLE t (x);"),
    );

    let refused = Database::restore_from_backup(&project, &empty);
    let message = refused.err().map(|e| format!("{e:#}")).unwrap_or_default();
    println!("restore -> {message}");
    assert!(message.contains("holds no project"), "{message}");
    assert_eq!(
        std::fs::read(&project).unwrap(),
        before,
        "the project is untouched"
    );
    let names: Vec<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names
            .iter()
            .any(|n| n.contains("pre-restore") || n.contains("staging")),
        "nothing left behind: {names:?}"
    );
}

/// [#576 C15] The Restore list offers every kind of candidate, labelled and
/// checked: backups (repair copies and pre-upgrade copies), an interrupted save's
/// `.tmp`, and the files earlier restores replaced (numbered). Unusable ones are
/// listed but marked, with the reason. Journals, half-written copies and the
/// project itself are never offered. Checking a candidate that has a journal
/// must not change it.
#[test]
fn the_restore_list_labels_and_checks_every_candidate() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path();
    let project = dir.join("CaveA.stalx");
    project_file(&project, "Original");
    project_file(&dir.join("CaveA.stalx.backup"), "Repair");
    project_file(&dir.join("CaveA.stalx.pre-v16.backup"), "Upgrade");
    project_file(&dir.join("CaveA.stalx.pre-restore.2"), "Second");
    project_file(&dir.join("CaveA.stalx.pre-restore"), "Legacy");
    std::fs::write(dir.join("CaveA.stalx.backup.1"), b"not a database").unwrap();
    let committed = dir.join("work.stalx");
    project_file(&committed, "Committed");
    let interrupted = dir.join("CaveA.stalx.tmp");
    torn_copy(&committed, &interrupted);
    let tmp_before = std::fs::read(&interrupted).unwrap();
    // Never offered: a stray journal of a backup, a half-written pre-upgrade copy.
    std::fs::write(dir.join("CaveA.stalx.backup-journal"), b"x").unwrap();
    std::fs::write(dir.join("CaveA.stalx.pre-v15.partial"), b"x").unwrap();

    let list = Database::list_backups(&project);
    for entry in &list {
        println!(
            "{:<28} kind={:?} restore_number={:?} usable={} problem={:?}",
            Path::new(&entry.path)
                .file_name()
                .unwrap()
                .to_string_lossy(),
            entry.kind,
            entry.restore_number,
            entry.usable,
            entry.problem
        );
    }
    assert_eq!(list.len(), 6, "exactly the six candidates");
    let backup = listed(&list, "CaveA.stalx.backup").unwrap();
    assert_eq!((backup.kind, backup.usable), (BackupKind::Backup, true));
    let upgrade = listed(&list, "CaveA.stalx.pre-v16.backup").unwrap();
    assert_eq!((upgrade.kind, upgrade.usable), (BackupKind::Backup, true));
    let second = listed(&list, "CaveA.stalx.pre-restore.2").unwrap();
    assert_eq!(
        (second.kind, second.restore_number, second.usable),
        (BackupKind::PreRestore, Some(2), true)
    );
    let legacy = listed(&list, "CaveA.stalx.pre-restore").unwrap();
    assert_eq!(
        (legacy.kind, legacy.restore_number),
        (BackupKind::PreRestore, None)
    );
    let broken = listed(&list, "CaveA.stalx.backup.1").unwrap();
    assert!(!broken.usable && broken.problem.is_some());
    let tmp = listed(&list, "CaveA.stalx.tmp").unwrap();
    assert_eq!((tmp.kind, tmp.usable), (BackupKind::InterruptedSave, true));
    assert!(
        std::fs::read(&interrupted).unwrap() == tmp_before && journal_of(&interrupted).exists(),
        "checking a candidate with a journal leaves both untouched"
    );
}

/// [#576 B27(f)] Repair rewrites the file with `VACUUM INTO` a sibling and a
/// rename, not a plain `VACUUM` (which goes through SQLite's temp directory, a
/// small tmpfs on many Linux systems), keeps its numbered backup, and leaves no
/// staging file - also when a killed run left one behind.
#[test]
fn repair_rewrites_beside_the_file_and_survives_a_leftover_staging_file() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "Original");
    let staging = temp.path().join("CaveA.stalx.repair-staging");
    std::fs::write(&staging, b"left by a killed run").unwrap();

    let (backup, ok) = Database::attempt_repair(&project).unwrap();
    println!(
        "repair ok={ok} backup={:?} staging left={} project holds: {}",
        backup.file_name().unwrap(),
        staging.exists(),
        project_name(&project)
    );
    assert!(ok);
    assert_eq!(project_name(&backup), "Original");
    assert_eq!(project_name(&project), "Original");
    assert!(!staging.exists(), "no staging file left behind");
}

/// [#576 NF-W3-3] Repair's numbered backup of a project frozen mid-write keeps
/// the project's rollback journal with it, so the backup reads as the project's
/// last committed state. Copied alone, the backup held the torn pages.
#[test]
fn repair_keeps_its_backup_with_the_projects_journal() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("old.stalx");
    project_file(&source, "Original");
    let project = temp.path().join("CaveA.stalx");
    torn_copy(&source, &project);
    let journal_before = std::fs::read(journal_of(&project)).unwrap();

    let (backup, ok) = Database::attempt_repair(&project).unwrap();
    let backup_journal = std::fs::read(journal_of(&backup)).ok();
    println!(
        "repair ok={ok}; backup {:?} journal kept: {}; backup reads: {}; project reads: {}",
        backup.file_name().unwrap(),
        backup_journal.as_deref() == Some(journal_before.as_slice()),
        project_name(&backup),
        project_name(&project)
    );
    assert!(
        backup_journal
            .as_deref()
            .is_some_and(|journal| journal == journal_before.as_slice()),
        "the backup keeps the project's journal, unchanged"
    );
    assert_eq!(project_name(&backup), "Original");
    assert_eq!(project_name(&project), "Original");
}

/// [#576 RV3-R-NITS, B27(f)] Observed, not read from the source: Repair
/// rewrites BESIDE the project and renames the rewrite over it, so afterwards
/// the project is a different file (inode). A plain `VACUUM` - which goes
/// through SQLite's temp directory, a small tmpfs on many Linux systems -
/// rewrites it in place and leaves the inode as it was, so this fails on it.
#[cfg(unix)]
#[test]
fn repair_replaces_the_file_rather_than_vacuuming_it_in_place() {
    use std::os::unix::fs::MetadataExt as _;

    let temp = TempDir::new().unwrap();
    let project = temp.path().join("CaveA.stalx");
    project_file(&project, "Original");
    let inode_before = std::fs::metadata(&project).unwrap().ino();

    let (_, ok) = Database::attempt_repair(&project).unwrap();
    let inode_after = std::fs::metadata(&project).unwrap().ino();
    println!(
        "repair ok={ok}; project inode {inode_before} -> {inode_after}; project holds: {}",
        project_name(&project)
    );
    assert!(ok);
    assert_ne!(
        inode_before, inode_after,
        "the repaired project must be the rewrite renamed into place"
    );
    assert_eq!(project_name(&project), "Original");
}

/// [#576 RVF-b-1] A restore never stages the candidate through a link left at its
/// staging name (`<name>.restore-staging`), dangling or to an existing file: the
/// link is removed, the copy is made afresh, and the restore goes ahead.
#[cfg(unix)]
#[test]
fn a_restore_never_stages_through_a_link_at_its_staging_name() {
    use super::upgrade_safety_tests::PlantedLink;

    for planted in PlantedLink::BOTH {
        let temp = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let project = temp.path().join("CaveA.stalx");
        project_file(&project, "Original");
        let candidate = temp.path().join("CaveA.stalx.backup");
        project_file(&candidate, "Backup");
        let staging = temp.path().join("CaveA.stalx.restore-staging");
        let target = planted.plant(&staging, elsewhere.path());

        let restored = Database::restore_from_backup(&project, &candidate);
        let through = planted.written_through(&target);
        println!(
            "{planted:?}: restore -> {:?}; target: {through:?}; project holds: {}",
            restored.as_ref().map(|_| ()),
            project_name(&project)
        );
        assert_eq!(through, None, "{planted:?}: staged through the link");
        assert!(restored.is_ok(), "{planted:?}: the restore goes ahead");
        assert_eq!(project_name(&project), "Backup", "{planted:?}");
        assert!(
            staging.symlink_metadata().is_err(),
            "{planted:?}: nothing is left at the staging name"
        );
    }
}

/// [#576 RVF-b-1] A link at the restore's staging name that cannot be removed
/// (the project's folder is read-only) refuses the restore before anything is
/// copied, and the link's target - in a writable folder - is not written:
/// `fs::copy` followed the link there and staged the candidate in it, until
/// keeping the safety copy failed.
#[cfg(unix)]
#[test]
fn a_restore_is_refused_when_a_link_at_its_staging_name_cannot_be_removed() {
    use super::upgrade_safety_tests::PlantedLink;
    use std::os::unix::fs::PermissionsExt as _;

    for planted in PlantedLink::BOTH {
        let temp = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let folder = temp.path().join("CaveA");
        std::fs::create_dir(&folder).unwrap();
        let project = folder.join("CaveA.stalx");
        project_file(&project, "Original");
        let before = std::fs::read(&project).unwrap();
        let candidate = temp.path().join("Backup.stalx");
        project_file(&candidate, "Backup");
        let target = planted.plant(
            &folder.join("CaveA.stalx.restore-staging"),
            elsewhere.path(),
        );

        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        crate::models::test_support::assert_dir_refuses_new_files(&folder);
        let restored = Database::restore_from_backup(&project, &candidate);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();

        let through = planted.written_through(&target);
        let message = restored.err().map(|e| format!("{e:#}"));
        println!("{planted:?}: restore -> {message:?}; target: {through:?}");
        assert_eq!(through, None, "{planted:?}: staged through the link");
        assert!(message.is_some(), "{planted:?}: the restore is refused");
        assert_eq!(
            std::fs::read(&project).unwrap(),
            before,
            "{planted:?}: the project is untouched"
        );
    }
}

/// [#576 RVF-b-1] Repair's numbered backup is never written through a link at
/// its name that cannot be removed (the folder is read-only): the repair is
/// refused before anything is written, and the link's target is not written.
/// `fs::copy` followed a dangling link at `.backup` there, and the rewrite then
/// went through one at `.repair-staging` too (SQLite resolves a link at a
/// `VACUUM INTO` target), until the final rename failed. A link to an existing
/// file at `.backup` counts as a backup already there and is never written.
#[cfg(unix)]
#[test]
fn repair_is_refused_when_a_link_at_its_backup_name_cannot_be_removed() {
    use super::upgrade_safety_tests::PlantedLink;
    use std::os::unix::fs::PermissionsExt as _;

    for planted in PlantedLink::BOTH {
        let temp = TempDir::new().unwrap();
        let backup_elsewhere = TempDir::new().unwrap();
        let staging_elsewhere = TempDir::new().unwrap();
        let folder = temp.path().join("CaveA");
        std::fs::create_dir(&folder).unwrap();
        let project = folder.join("CaveA.stalx");
        project_file(&project, "Original");
        let before = std::fs::read(&project).unwrap();
        let backup_target =
            planted.plant(&folder.join("CaveA.stalx.backup"), backup_elsewhere.path());
        let staging_target = planted.plant(
            &folder.join("CaveA.stalx.repair-staging"),
            staging_elsewhere.path(),
        );

        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        crate::models::test_support::assert_dir_refuses_new_files(&folder);
        let repaired = Database::attempt_repair(&project);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();

        let through = (
            planted.written_through(&backup_target),
            planted.written_through(&staging_target),
        );
        let message = repaired.err().map(|e| format!("{e:#}"));
        println!("{planted:?}: repair -> {message:?}; (backup, staging) targets: {through:?}");
        assert_eq!(
            through,
            (None, None),
            "{planted:?}: repair wrote through a link at its backup or staging name"
        );
        assert!(message.is_some(), "{planted:?}: the repair is refused");
        assert_eq!(
            std::fs::read(&project).unwrap(),
            before,
            "{planted:?}: the project is untouched"
        );
    }
}

/// [#576 B27(f)] A source-level tripwire, like the push snapshot's: Repair must
/// never fall back to a plain `VACUUM`.
#[test]
fn repair_never_runs_a_plain_vacuum() {
    let source = include_str!("repair.rs");
    let start = source.find("pub fn attempt_repair").unwrap();
    let body = &source[start..];
    let body = &body[..body.find("\n    }\n").unwrap()];
    assert!(
        !body.contains("\"VACUUM;\"") && body.contains("VACUUM INTO"),
        "attempt_repair must compact with VACUUM INTO a sibling, not a plain VACUUM"
    );
}

// === A hostile schema is refused on the recovery paths too (#608) ===

/// Schemas every open of an existing file refuses (#608), as SQL run on a saved
/// project: a view, a trigger, a CHECK the app never writes (which
/// `quick_check` evaluates on every row) and an expression index (which
/// `REINDEX` evaluates on every row).
const HOSTILE_SCHEMAS: [(&str, &str); 4] = [
    ("CREATE VIEW v AS SELECT 1;", "a view"),
    (
        "CREATE TRIGGER t AFTER UPDATE ON project BEGIN SELECT 1; END;",
        "a trigger",
    ),
    (
        "CREATE TABLE c (x CHECK (length(zeroblob(10)) > 0)); INSERT INTO c VALUES (1);",
        "a table with a CHECK constraint the app never writes",
    ),
    (
        "CREATE INDEX e ON shots (length(hex(zeroblob(10))) + length);",
        "an expression or partial index",
    ),
];

/// A valid project at `path` with `sql` run on it afterwards through a raw
/// connection.
fn hostile_project_file(path: &Path, sql: &str) {
    project_file(path, "Crafted");
    Connection::open(path).unwrap().execute_batch(sql).unwrap();
}

/// What the schema refusal inside `err` names, when that is why it failed.
fn refused_what(err: &anyhow::Error) -> Option<&'static str> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<RefusedSchemaObject>())
        .map(|refused| refused.what)
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// [#576 RV18-1] A restore candidate is an existing file like any other: one
/// whose schema every open refuses (#608) is listed as not usable, with the
/// reason, and a restore refuses it and leaves the project as it was - whichever
/// way the check reaches the candidate (read-only, or through a copy beside it
/// whose rollback journal is rolled back). The checks used to skip the guard
/// and run with the file's schema trusted, so such a file was listed as usable,
/// installed over the project, and then refused by the next open.
#[test]
fn a_restore_candidate_with_a_hostile_schema_is_listed_unusable_and_refused() {
    let mut wrong = Vec::new();
    for (sql, what) in HOSTILE_SCHEMAS {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        let project = dir.join("CaveA.stalx");
        project_file(&project, "Original");
        let before = std::fs::read(&project).unwrap();
        let backup = dir.join("CaveA.stalx.backup");
        hostile_project_file(&backup, sql);
        // The same schema as the committed state behind an interrupted save's
        // rollback journal.
        let committed = dir.join("work.stalx");
        hostile_project_file(&committed, sql);
        let interrupted = dir.join("CaveA.stalx.tmp");
        torn_copy(&committed, &interrupted);

        let list = Database::list_backups(&project);
        for file in ["CaveA.stalx.backup", "CaveA.stalx.tmp"] {
            let entry = listed(&list, file).unwrap();
            println!(
                "{what}: {file} listed usable={} problem={:?}",
                entry.usable, entry.problem
            );
            let problem = entry.problem.as_deref().unwrap_or_default();
            if entry.usable || !problem.contains(what) {
                wrong.push(format!("{what}: {file} listed usable={}", entry.usable));
            }
        }
        for candidate in [&backup, &interrupted] {
            let restored = Database::restore_from_backup(&project, candidate)
                .map_or_else(|e| format!("{e:#}"), |_| "restored".to_owned());
            let file = candidate.file_name().unwrap().to_string_lossy();
            println!(
                "{what}: restore from {file} -> {restored}; the project then opens as: {}",
                project_name(&project)
            );
            if !(restored.contains("cannot be restored") && restored.contains(what)) {
                wrong.push(format!("{what}: restore from {file} -> {restored}"));
            }
        }
        if std::fs::read(&project).unwrap() != before {
            wrong.push(format!("{what}: the project was replaced"));
        }
        let names = files_in(dir);
        if names.iter().any(|n| {
            n.contains("pre-restore") || n.contains("staging") || n.contains("restore-check")
        }) {
            wrong.push(format!("{what}: left behind: {names:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// [#576 RV18-1] Salvage reads a file nothing else has checked, so the schema
/// guard (#608) is its first step: a hostile schema is refused before any table
/// is read. It used to read on with the file's schema trusted.
#[test]
fn salvage_refuses_a_file_with_a_hostile_schema() {
    let mut wrong = Vec::new();
    for (sql, what) in HOSTILE_SCHEMAS {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("CaveA.stalx");
        hostile_project_file(&project, sql);

        let salvaged = Database::export_salvageable(&project);
        println!(
            "{what}: salvage -> {}",
            salvaged.as_ref().map_or_else(
                |e| format!("{e:#}"),
                |s| format!("salvaged project {:?}", s.project_name)
            )
        );
        if salvaged.err().as_ref().and_then(refused_what) != Some(what) {
            wrong.push(format!("{what}: salvage did not refuse the file"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// [#576 RV18-1] Repair opens the file read-write and rebuilds its indexes,
/// which evaluates an expression index on every row, and its last step runs
/// `quick_check`, which evaluates every CHECK. The schema guard (#608) now runs
/// before both, so a hostile file is refused and left as it was (the backup
/// Repair always takes first is a plain copy of it). It used to be rewritten.
#[test]
fn repair_refuses_a_file_with_a_hostile_schema() {
    let mut wrong = Vec::new();
    for (sql, what) in HOSTILE_SCHEMAS {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("CaveA.stalx");
        hostile_project_file(&project, sql);
        let before = std::fs::read(&project).unwrap();

        let repaired = Database::attempt_repair(&project);
        let unchanged = std::fs::read(&project).unwrap() == before;
        println!(
            "{what}: repair -> {}; file unchanged: {unchanged}",
            repaired
                .as_ref()
                .map_or_else(|e| format!("{e:#}"), |(_, ok)| format!("repaired, ok={ok}"))
        );
        if repaired.err().as_ref().and_then(refused_what) != Some(what) || !unchanged {
            wrong.push(format!("{what}: not refused, or the file changed"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
