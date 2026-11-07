//! [#576 W35-NF-1] The openers that only read a file leave nothing beside it.
//!
//! A READ_ONLY connection to a file in WAL journal mode creates its `-wal` and
//! `-shm` and cannot remove them when it closes (#576 RV8a-3, RV18-3), so the
//! recent-projects listing (`open_for_metadata`), the Global View preview
//! (`open_readonly`), the salvage (`open_for_recovery`) and the restore
//! listing's check of a candidate left both beside a WAL-mode file. Only
//! `check_schema_at` had been fixed (RV18-3).
//!
//! All data is synthetic: a project this build saves, switched to WAL mode
//! with plain SQL. No fixture is read or written.

use std::path::{Path, PathBuf};

use super::*;
use tempfile::TempDir;

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A project file named `name` in `dir`, in WAL journal mode, with nothing
/// beside it (the switch's own connection removes its sidecars on close).
fn wal_mode_project(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    Database::open(&path)
        .unwrap()
        .save_project(&Project::new("CaveA".to_string(), None))
        .unwrap();
    let mode: String = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal", "fixture: the file is in WAL mode");
    path
}

/// Each opener against a fresh WAL-mode file: open, read what its caller
/// reads, close. Returns what the folder held before and after, whether the
/// file's bytes are unchanged, and the read's outcome.
fn open_and_read(
    opener: &str,
    open: impl Fn(&Path) -> Result<String>,
) -> (Vec<String>, Vec<String>, bool, String) {
    let dir = TempDir::new().unwrap();
    let path = wal_mode_project(dir.path(), "CaveA.stalx");
    let before = files_in(dir.path());
    let bytes = std::fs::read(&path).unwrap();
    let outcome = match open(&path) {
        Ok(read) => format!("read {read}"),
        Err(e) => format!("failed: {e:#}"),
    };
    let after = files_in(dir.path());
    let unchanged = std::fs::read(&path).unwrap() == bytes;
    println!(
        "{opener}: {outcome}; folder before {before:?}, after {after:?}; file bytes unchanged: \
         {unchanged}"
    );
    (before, after, unchanged, outcome)
}

#[test]
fn the_read_only_openers_leave_nothing_beside_a_wal_mode_file() {
    let runs = [
        open_and_read("open_for_metadata", |path| {
            let db = Database::open_for_metadata(path)?;
            Ok(format!("{:?}", db.try_read_project_metadata()?))
        }),
        open_and_read("open_readonly", |path| {
            let db = Database::open_readonly(path)?;
            Ok(format!("project {:?}", db.load_project()?.name))
        }),
        open_and_read("open_for_recovery", |path| {
            let db = Database::open_for_recovery(path)?;
            Ok(format!("{} sections", db.try_read_sections()?.len()))
        }),
    ];
    for (before, after, unchanged, outcome) in runs {
        assert!(
            outcome.starts_with("read"),
            "the read goes through: {outcome}"
        );
        assert_eq!(before, ["CaveA.stalx"], "fixture: nothing beside it");
        assert_eq!(after, before, "and still nothing beside it");
        assert!(unchanged, "the file itself is not written");
    }
}

/// The restore listing checks a candidate with no journal beside it by
/// opening it directly; a WAL-mode candidate must be left as it was found.
#[test]
fn the_restore_listing_leaves_nothing_beside_a_wal_mode_candidate() {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("CaveA.stalx");
    Database::open(&project)
        .unwrap()
        .save_project(&Project::new("CaveA".to_string(), None))
        .unwrap();
    let candidate = wal_mode_project(dir.path(), "CaveA.stalx.backup");
    let before = files_in(dir.path());
    let bytes = std::fs::read(&candidate).unwrap();

    let listed = Database::list_backups(&project);
    let after = files_in(dir.path());
    let unchanged = std::fs::read(&candidate).unwrap() == bytes;
    let verdicts: Vec<(String, bool, Option<String>)> = listed
        .iter()
        .map(|b| (b.path.clone(), b.usable, b.problem.clone()))
        .collect();
    println!(
        "list_backups: {verdicts:?}; folder before {before:?}, after {after:?}; candidate bytes \
         unchanged: {unchanged}"
    );

    assert_eq!(listed.len(), 1, "the one candidate is listed");
    assert!(listed.iter().all(|b| b.usable), "and usable: {verdicts:?}");
    assert_eq!(before, ["CaveA.stalx", "CaveA.stalx.backup"], "fixture");
    assert_eq!(after, before, "nothing is left beside the candidate");
    assert!(unchanged, "the candidate itself is not written");
}

/// What the WAL-mode open must not change: a statement that writes still
/// fails on each handle, and a file another connection is writing (its `-wal`
/// is there, holding a committed change) is read as before - the change is
/// seen, the writer's `-wal` and `-shm` stay, and the file is not
/// checkpointed under the writer.
#[test]
fn the_wal_mode_open_still_refuses_writes_and_leaves_a_live_writer_alone() {
    let dir = TempDir::new().unwrap();
    let path = wal_mode_project(dir.path(), "CaveA.stalx");
    let openers: [(&str, fn(&Path) -> Result<Database>); 3] = [
        ("open_for_metadata", Database::open_for_metadata),
        ("open_readonly", Database::open_readonly),
        ("open_for_recovery", Database::open_for_recovery),
    ];
    for (opener, open) in openers {
        let db = open(&path).unwrap();
        let write = db.conn.execute("UPDATE project SET name = 'Changed'", []);
        drop(db);
        println!("{opener}: a write -> {write:?}");
        assert!(write.is_err(), "{opener}: a write must fail");
    }

    let writer = Connection::open(&path).unwrap();
    writer
        .execute("UPDATE project SET name = 'Renamed by the writer'", [])
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let with_writer = files_in(dir.path());
    let name = Database::open_for_metadata(&path)
        .unwrap()
        .try_read_project_metadata()
        .unwrap()
        .map(|(name, _)| name);
    let after = files_in(dir.path());
    let unchanged = std::fs::read(&path).unwrap() == bytes;
    println!(
        "with a live writer: the read sees {name:?}; folder {with_writer:?} -> {after:?}; file \
         bytes unchanged: {unchanged}"
    );
    drop(writer);

    assert_eq!(name.as_deref(), Some("Renamed by the writer"));
    assert_eq!(
        with_writer,
        ["CaveA.stalx", "CaveA.stalx-shm", "CaveA.stalx-wal"],
        "fixture: the writer's sidecars"
    );
    assert_eq!(after, with_writer, "the writer's sidecars stay");
    assert!(unchanged, "and its change is not checkpointed under it");
}

/// [#576 RV19a-7] A reader handed a SYMLINK to a WAL-mode file looks for the
/// `-wal` where SQLite keeps it: beside the link's target. The check looked
/// beside the link, saw nothing, and took the read-write `query_only` open,
/// whose last close then checkpointed a crashed writer's committed frames into
/// the target - a reader writing the file.
#[cfg(unix)]
#[test]
fn a_reader_through_a_symlink_never_checkpoints_the_targets_wal() {
    use rusqlite::config::DbConfig;

    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    let target = wal_mode_project(&dir.path().join("real"), "CaveA.stalx");
    {
        // A writer that "crashes": its committed frames stay in the -wal.
        let writer = Connection::open(&target).unwrap();
        writer
            .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .unwrap();
        writer
            .execute_batch(
                "PRAGMA wal_autocheckpoint = 0;
                 UPDATE project SET name = 'Renamed by the writer';",
            )
            .unwrap();
    }
    let link = dir.path().join("Linked.stalx");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let bytes = std::fs::read(&target).unwrap();
    let beside_target = files_in(&dir.path().join("real"));

    let name = Database::open_for_metadata(&link)
        .unwrap()
        .try_read_project_metadata()
        .unwrap()
        .map(|(name, _)| name);
    let after = files_in(&dir.path().join("real"));
    let unchanged = std::fs::read(&target).unwrap() == bytes;
    println!(
        "through a symlink: the read sees {name:?}; beside the target {beside_target:?} -> \
         {after:?}; target bytes unchanged: {unchanged}"
    );

    assert_eq!(name.as_deref(), Some("Renamed by the writer"));
    assert_eq!(
        beside_target,
        ["CaveA.stalx", "CaveA.stalx-shm", "CaveA.stalx-wal"],
        "fixture: the crashed writer's sidecars beside the target"
    );
    assert!(
        unchanged,
        "the reader must not checkpoint the writer's frames into the target"
    );
    assert_eq!(after, beside_target, "and leaves the writer's -wal for it");
}
