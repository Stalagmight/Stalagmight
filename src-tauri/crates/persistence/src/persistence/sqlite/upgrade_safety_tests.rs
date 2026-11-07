//! [#576 B6] Upgrading an older file is safe: one pre-upgrade copy is kept
//! beside it, the chain and its stamp commit together, and a crash-shaped v9
//! link still gets its backfill.
//!
//! Moved out of `tests.rs` unchanged (#576 E8: the monolith is frozen).

use std::path::Path;

use rusqlite::Connection;
use tempfile::TempDir;

use super::{Database, SCHEMA_VERSION};
use crate::models::{Project, SectionData, ShotData};

/// A current file holding one section of two shots, then reshaped by `surgery`.
pub(super) fn b6_build_file(dir: &Path, name: &str, surgery: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        surveyor: None,
        date: None,
        color: "#123456".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![
            ShotData {
                length: 5.0,
                ..Default::default()
            },
            ShotData {
                length: 7.0,
                ..Default::default()
            },
        ],
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    Connection::open(&path)
        .unwrap()
        .execute_batch(surgery)
        .unwrap();
    path
}

fn b6_user_version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn b6_has_column(path: &Path, table: &str, column: &str) -> bool {
    Connection::open(path)
        .unwrap()
        .query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

fn b6_dir_listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// [#576 B6-D2] Opening an older file keeps ONE copy of it, as it was, before the
/// upgrade changes anything: `<name>.stalx.pre-v<old>.backup` beside it, which the
/// recovery dialog lists. Upgrades are one-way (several drop undo history, v12
/// rewrote every pitch), so without it there was no way back. A current file
/// gets no copy, and an existing copy is never overwritten.
#[test]
fn an_upgrade_keeps_one_copy_of_the_file_as_it_was() {
    let temp = TempDir::new().unwrap();
    let path = b6_build_file(
        temp.path(),
        "CaveA.stalx",
        "UPDATE project SET schema_version = 17; PRAGMA user_version = 17;",
    );
    let original = std::fs::read(&path).unwrap();
    let copy = temp.path().join("CaveA.stalx.pre-v17.backup");

    drop(Database::open_existing(&path).unwrap());
    println!("after the upgrade: {:?}", b6_dir_listing(temp.path()));
    assert_eq!(b6_user_version(&path), i64::from(SCHEMA_VERSION));
    let kept = std::fs::read(&copy).unwrap_or_default();
    assert!(
        kept == original,
        "the copy must be the file exactly as it was before the upgrade ({} bytes kept, {} \
         original)",
        kept.len(),
        original.len()
    );

    // A current file takes no copy.
    let listing = b6_dir_listing(temp.path());
    drop(Database::open_existing(&path).unwrap());
    assert_eq!(
        b6_dir_listing(temp.path()),
        listing,
        "no copy of a current file"
    );

    // The same upgrade again (a different file content) never replaces the copy.
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "UPDATE project SET name = 'CaveB', schema_version = 17; PRAGMA user_version = 17;",
        )
        .unwrap();
    drop(Database::open_existing(&path).unwrap());
    assert!(
        std::fs::read(&copy).unwrap() == original,
        "an existing pre-upgrade copy must never be overwritten"
    );
}

/// [#576 B6] The migration chain and its stamp commit together: an arm that fails
/// part-way leaves the file exactly as unmigrated as it was - no column added by
/// an earlier step, no stamp - so the next open runs the whole chain again
/// instead of finding a half-migrated file. Here the v13 arm's `DROP COLUMN
/// is_reference` fails on an index over that column, after `migrate_schema` has
/// already re-added a missing `sections` column and the arm itself has added
/// `shots.kind`.
#[test]
fn a_migration_that_fails_part_way_leaves_the_file_unmigrated() {
    let temp = TempDir::new().unwrap();
    let path = b6_build_file(
        temp.path(),
        "CaveA.stalx",
        "ALTER TABLE shots ADD COLUMN is_reference INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE shots DROP COLUMN kind;
         CREATE INDEX idx_shots_is_reference ON shots(is_reference);
         ALTER TABLE sections DROP COLUMN azimuth_epoch_year;
         UPDATE project SET schema_version = 12;
         PRAGMA user_version = 12;",
    );

    let opened = Database::open_existing(&path);
    println!(
        "open_existing -> {}",
        opened
            .as_ref()
            .err()
            .map_or_else(|| "<opened>".to_string(), |e| format!("{e:#}"))
    );
    assert!(opened.is_err(), "premise: the v13 arm fails on this file");
    drop(opened);

    let kind_added = b6_has_column(&path, "shots", "kind");
    let azimuth_added = b6_has_column(&path, "sections", "azimuth_epoch_year");
    let stamp = b6_user_version(&path);
    println!(
        "after the failed open: shots.kind present={kind_added} \
         sections.azimuth_epoch_year present={azimuth_added} user_version={stamp}"
    );
    assert_eq!(stamp, 12, "no stamp for a chain that did not finish");
    assert!(
        !kind_added && !azimuth_added,
        "every step before the failure must be rolled back with it"
    );
}

/// [#576 B6] The v9 -> v10 `speleo_link.last_edit_at` backfill must not depend on
/// the column being absent. A crash between that ALTER and its backfill (they
/// committed separately) left the column present, the backfill unrun and the
/// stamp still 9 - and since the backfill was guarded on the column, no later open
/// ever ran it: the legacy dirty bit could then never be cleared by a push.
#[test]
fn a_crash_shaped_v9_link_still_gets_its_last_edit_at_backfill() {
    let temp = TempDir::new().unwrap();
    let path = b6_build_file(
        temp.path(),
        "CaveA.stalx",
        "INSERT INTO speleo_link (id, endpoint, project_id, project_name, base_commit_sha, \
             has_unpushed_edits, last_edit_at)
         VALUES (1, 'https://speleodb.invalid', '00000000-0000-4000-8000-000000000001', \
             'CaveA', NULL, 1, NULL);
         UPDATE project SET schema_version = 9;
         PRAGMA user_version = 9;",
    );

    let db = Database::open_existing(&path).unwrap();
    let link = db.load_speleo_link().unwrap().unwrap();
    println!(
        "last_edit_at after open = {:?} (has_unpushed_edits={})",
        link.last_edit_at, link.has_unpushed_edits
    );
    assert_eq!(
        link.last_edit_at.as_deref(),
        Some("1970-01-01T00:00:00+00:00"),
        "a dirty pre-v10 row must get the epoch stamp, whatever the column's presence"
    );
}

/// [#576 B6-D2-FAIL] When the pre-upgrade copy cannot be written, the upgrade is
/// REFUSED: the open fails with a message saying so, and the file is left
/// exactly as it was - not migrated, not stamped, no half-written copy beside
/// it. The decided behaviour; "warn and upgrade anyway" would destroy the only
/// way back. The project's folder is made read-only, so the copy (written beside
/// the file) cannot be created while the file itself stays writable.
#[cfg(unix)]
#[test]
fn an_upgrade_whose_copy_cannot_be_written_is_refused_and_leaves_the_file_as_it_was() {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("CaveA");
    std::fs::create_dir(&folder).unwrap();
    let path = b6_build_file(
        &folder,
        "CaveA.stalx",
        "UPDATE project SET schema_version = 17; PRAGMA user_version = 17;",
    );
    let original = std::fs::read(&path).unwrap();

    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let premise = std::fs::File::create(folder.join("premise-probe"));
    let opened = premise
        .is_err()
        .then(|| Database::open_existing(&path).map(drop));
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        premise.is_err(),
        "premise: a read-only folder refuses new files (run the tests as a non-root user)"
    );

    let message = format!("{:#}", opened.unwrap().unwrap_err());
    let listing = b6_dir_listing(&folder);
    let unchanged = std::fs::read(&path).unwrap() == original;
    let stamp = b6_user_version(&path);
    println!("open_existing -> {message}");
    println!("folder: {listing:?}; file unchanged: {unchanged}; user_version: {stamp}");
    assert!(
        message.contains("Could not keep a copy of this project before upgrading it"),
        "the refusal must say why: {message}"
    );
    assert_eq!(
        listing,
        vec!["CaveA.stalx".to_string()],
        "no copy, no leftovers"
    );
    assert!(
        unchanged,
        "a refused upgrade must leave the file byte-identical"
    );
    assert_eq!(stamp, 17, "not migrated");
}

/// [#576 B6-D2-FAIL] The same refusal when only the COPY is blocked - here by a
/// directory squatting on the name it is written under - while the file and its
/// folder stay writable, so an upgrade that went ahead anyway would visibly
/// migrate the file. Every platform.
#[test]
fn an_upgrade_is_refused_when_only_its_copy_is_blocked() {
    let temp = TempDir::new().unwrap();
    let path = b6_build_file(
        temp.path(),
        "CaveA.stalx",
        "UPDATE project SET schema_version = 17; PRAGMA user_version = 17;",
    );
    let original = std::fs::read(&path).unwrap();
    std::fs::create_dir(temp.path().join("CaveA.stalx.pre-v17.partial")).unwrap();

    let opened = Database::open_existing(&path).map(drop);
    let unchanged = std::fs::read(&path).unwrap() == original;
    let stamp = b6_user_version(&path);
    println!("open_existing -> {opened:?}");
    println!(
        "folder: {:?}; file unchanged: {unchanged}; user_version: {stamp}",
        b6_dir_listing(temp.path())
    );
    let message = format!("{:#}", opened.unwrap_err());
    assert!(
        message.contains("Could not keep a copy of this project before upgrading it"),
        "the refusal must say why: {message}"
    );
    assert!(
        unchanged,
        "a refused upgrade must leave the file byte-identical"
    );
    assert_eq!(stamp, 17, "not migrated");
    assert!(
        !temp.path().join("CaveA.stalx.pre-v17.backup").exists(),
        "no copy was kept, so none may be listed"
    );
}

/// [#576 RV6b-R3] A refused upgrade carries the message for the write it refused:
/// an older file would be upgraded with a backup copy beside it; an unstamped file
/// whose row claims this version (or a newer one) would only have its tables
/// completed in place. Either way the file and its folder are left as they were.
#[test]
fn a_refused_upgrade_says_which_write_it_refused() {
    let refusal = super::UpgradeRefusal {
        older_format: "older: refused".to_owned(),
        unstamped: "unstamped: refused".to_owned(),
    };
    let future = SCHEMA_VERSION + 1;
    let cases = [
        (
            "older (stamped v16)",
            "UPDATE project SET schema_version = 16; PRAGMA user_version = 16;".to_owned(),
            "older: refused",
        ),
        (
            "unstamped, row claims this version",
            "PRAGMA user_version = 0;".to_owned(),
            "unstamped: refused",
        ),
        (
            "unstamped, row claims a newer version",
            format!("UPDATE project SET schema_version = {future}; PRAGMA user_version = 0;"),
            "unstamped: refused",
        ),
    ];
    for (what, surgery, expected) in cases {
        let dir = TempDir::new().unwrap();
        let path = b6_build_file(dir.path(), "CaveA.stalx", &surgery);
        let before = std::fs::read(&path).unwrap();
        let message = Database::open_existing_refusing_upgrade(&path, &refusal)
            .err()
            .and_then(|e| {
                e.downcast_ref::<super::UpgradeRefused>()
                    .map(ToString::to_string)
            });
        let unchanged = std::fs::read(&path).unwrap() == before;
        println!(
            "{what}: refusal {message:?}; file unchanged: {unchanged}; folder: {:?}",
            b6_dir_listing(dir.path())
        );
        assert_eq!(message.as_deref(), Some(expected), "{what}");
        assert!(unchanged, "{what}: the file is left as it was");
        assert_eq!(b6_dir_listing(dir.path()), ["CaveA.stalx"], "{what}");
    }
}

/// [#576 RVF-b-1] What a link planted at a scratch name leads to: a file that is
/// not there yet, or an existing one (a settings file), in a folder of its own.
#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
pub(super) enum PlantedLink {
    Dangling,
    ToExistingFile,
}

#[cfg(unix)]
impl PlantedLink {
    pub(super) const BOTH: [Self; 2] = [Self::Dangling, Self::ToExistingFile];
    /// What an existing target holds before anything runs.
    const EXISTING_BYTES: &'static [u8] = b"theme = \"dark\"\n";

    /// Plant the link at `name`, leading into `elsewhere`; returns its target.
    pub(super) fn plant(self, name: &Path, elsewhere: &Path) -> std::path::PathBuf {
        let target = match self {
            Self::Dangling => elsewhere.join("planted.toml"),
            Self::ToExistingFile => {
                let target = elsewhere.join("config.toml");
                std::fs::write(&target, Self::EXISTING_BYTES).unwrap();
                target
            }
        };
        std::os::unix::fs::symlink(&target, name).unwrap();
        target
    }

    /// How the target is no longer as it was (`None`: it is): a dangling link's
    /// target was created, an existing one's bytes changed.
    pub(super) fn written_through(self, target: &Path) -> Option<String> {
        match (self, std::fs::read(target)) {
            (Self::Dangling, Err(_)) => None,
            (Self::Dangling, Ok(bytes)) => Some(format!("created, {} bytes", bytes.len())),
            (Self::ToExistingFile, Ok(bytes)) if bytes == Self::EXISTING_BYTES => None,
            (Self::ToExistingFile, Ok(bytes)) => {
                Some(format!("overwritten, {} bytes", bytes.len()))
            }
            (Self::ToExistingFile, Err(e)) => Some(format!("gone: {e}")),
        }
    }
}

/// [#576 RVF-b-1] The pre-upgrade copy is never written through a link left at
/// the name it is written under first (`<name>.pre-v<N>.partial`), whether the
/// link leads nowhere yet or to an existing file: the link is removed, the copy
/// is created afresh, and the upgrade goes ahead with a real copy kept.
///
/// `fs::copy` opened that name following the link: a dangling one got its target
/// created holding the whole project, one to an existing file (a settings file)
/// got it overwritten, and the rename then moved the link itself to the
/// `.backup` name. The write policy judges only the project's own path.
#[cfg(unix)]
#[test]
fn an_upgrade_never_writes_its_copy_through_a_link_at_the_partial_name() {
    for planted in PlantedLink::BOTH {
        let temp = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let path = b6_build_file(
            temp.path(),
            "CaveA.stalx",
            "UPDATE project SET schema_version = 17; PRAGMA user_version = 17;",
        );
        let original = std::fs::read(&path).unwrap();
        let partial = temp.path().join("CaveA.stalx.pre-v17.partial");
        let target = planted.plant(&partial, elsewhere.path());

        let opened = Database::open_existing(&path).map(drop);
        let through = planted.written_through(&target);
        let copy = temp.path().join("CaveA.stalx.pre-v17.backup");
        let copy_is_a_file = copy.symlink_metadata().is_ok_and(|m| m.is_file());
        println!(
            "{planted:?}: open -> {opened:?}; target: {through:?}; folder: {:?}",
            b6_dir_listing(temp.path())
        );
        assert_eq!(
            through, None,
            "{planted:?}: the upgrade's copy was written through the link at .partial"
        );
        assert!(opened.is_ok(), "{planted:?}: the upgrade goes ahead");
        assert!(copy_is_a_file, "{planted:?}: the kept copy is a real file");
        assert!(
            std::fs::read(&copy).unwrap() == original,
            "{planted:?}: the copy is the file as it was"
        );
        assert_eq!(
            b6_dir_listing(temp.path()),
            ["CaveA.stalx", "CaveA.stalx.pre-v17.backup"],
            "{planted:?}: no link and no partial copy left"
        );
        assert_eq!(b6_user_version(&path), i64::from(SCHEMA_VERSION));
    }
}

/// [#576 RVF-b-1] A stale `.partial` a crash left mid-copy (a plain file) never
/// blocks a later upgrade: it is replaced by a whole copy.
#[test]
fn a_stale_partial_copy_does_not_block_the_upgrade() {
    let temp = TempDir::new().unwrap();
    let path = b6_build_file(
        temp.path(),
        "CaveA.stalx",
        "UPDATE project SET schema_version = 17; PRAGMA user_version = 17;",
    );
    let original = std::fs::read(&path).unwrap();
    std::fs::write(
        temp.path().join("CaveA.stalx.pre-v17.partial"),
        b"half a copy",
    )
    .unwrap();

    let opened = Database::open_existing(&path).map(drop);
    println!(
        "open -> {opened:?}; folder: {:?}",
        b6_dir_listing(temp.path())
    );
    assert!(opened.is_ok(), "the upgrade goes ahead: {opened:?}");
    assert!(
        std::fs::read(temp.path().join("CaveA.stalx.pre-v17.backup")).unwrap() == original,
        "the copy is the whole file as it was, not the stale partial"
    );
    assert_eq!(
        b6_dir_listing(temp.path()),
        ["CaveA.stalx", "CaveA.stalx.pre-v17.backup"]
    );
    assert_eq!(b6_user_version(&path), i64::from(SCHEMA_VERSION));
}

/// [#576 RVF-b-1] A link at `.partial` that cannot be removed (the folder is
/// read-only, the project inside it writable) refuses the upgrade, and the link's
/// target - in a writable folder - is not written: `fs::copy` followed the link
/// there before the rename failed.
#[cfg(unix)]
#[test]
fn an_upgrade_is_refused_when_a_link_at_the_partial_name_cannot_be_removed() {
    use std::os::unix::fs::PermissionsExt as _;

    for planted in PlantedLink::BOTH {
        let temp = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let folder = temp.path().join("CaveA");
        std::fs::create_dir(&folder).unwrap();
        let path = b6_build_file(
            &folder,
            "CaveA.stalx",
            "UPDATE project SET schema_version = 17; PRAGMA user_version = 17;",
        );
        let original = std::fs::read(&path).unwrap();
        let target = planted.plant(
            &folder.join("CaveA.stalx.pre-v17.partial"),
            elsewhere.path(),
        );

        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        let premise = std::fs::File::create(folder.join("premise-probe"));
        let opened = premise
            .is_err()
            .then(|| Database::open_existing(&path).map(drop));
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            premise.is_err(),
            "premise: a read-only folder refuses new files (run the tests as a non-root user)"
        );

        let through = planted.written_through(&target);
        let message = opened.unwrap().err().map(|e| format!("{e:#}"));
        println!(
            "{planted:?}: open -> {message:?}; target: {through:?}; folder: {:?}",
            b6_dir_listing(&folder)
        );
        assert_eq!(
            through, None,
            "{planted:?}: the upgrade's copy was written through the link at .partial"
        );
        assert!(
            message
                .as_deref()
                .is_some_and(|m| m.contains("Could not keep a copy of this project")),
            "{planted:?}: the upgrade is refused, saying why: {message:?}"
        );
        assert!(
            std::fs::read(&path).unwrap() == original,
            "{planted:?}: a refused upgrade leaves the file byte-identical"
        );
        assert_eq!(b6_user_version(&path), 17, "{planted:?}: not migrated");
    }
}

/// What Windows asks of a `sync_all`: FlushFileBuffers needs a handle with
/// GENERIC_WRITE, and a `File::open` handle is GENERIC_READ only. Linux and
/// macOS fsync either, so a plain `sync_all` here could never fail the way it
/// did there. Truncating to the current length changes nothing, but needs a
/// handle opened for writing on every platform (ftruncate on a read-only
/// descriptor is refused, as SetFileInformationByHandle is without
/// GENERIC_WRITE), so it stands in for the Windows check.
fn rvfd1_flush_like_windows(file: &std::fs::File) -> std::io::Result<()> {
    let len = file.metadata()?.len();
    file.set_len(len)?;
    file.sync_all()
}

/// [#576 RVF-d-1] The pre-upgrade copy is flushed through a handle that can
/// write. It was flushed through `File::open`, which on Windows is refused
/// ("Access is denied"), so every older-format project with data failed to
/// open there. The helper hands its flush the handle that wrote the copy.
#[test]
fn the_pre_upgrade_copy_is_flushed_through_a_handle_that_can_write() {
    let temp = TempDir::new().unwrap();
    let from = temp.path().join("CaveA.stalx");
    std::fs::write(&from, b"SQLite format 3\0 then the pages of a project").unwrap();
    let to = temp.path().join("CaveA.stalx.pre-v17.partial");

    // The stand-in tells the two handles apart: a read-only one, as the old
    // code flushed, is refused.
    std::fs::write(&to, b"x").unwrap();
    let read_only = rvfd1_flush_like_windows(&std::fs::File::open(&to).unwrap());
    println!("flush through File::open: {read_only:?}");
    assert!(
        read_only.is_err(),
        "the stand-in must refuse a read-only handle"
    );

    let flushed = super::schema::write_synced_copy(&from, &to, rvfd1_flush_like_windows);
    println!("flush through write_synced_copy: {flushed:?}");
    flushed.unwrap();
    assert!(
        std::fs::read(&to).unwrap() == std::fs::read(&from).unwrap(),
        "the copy holds the file's bytes"
    );
}

/// [#576 RVF-d-1] The upgrade path end to end with the copy written by
/// `write_synced_copy`: a partial copy left by an earlier attempt that died is
/// replaced, not appended to, the kept copy is the file as it was, nothing is
/// left under the partial name, and (on Unix) the copy keeps the file's mode,
/// as `std::fs::copy` did.
#[test]
fn an_upgrade_replaces_a_partial_copy_left_by_an_earlier_attempt() {
    let temp = TempDir::new().unwrap();
    let path = b6_build_file(
        temp.path(),
        "CaveA.stalx",
        "UPDATE project SET schema_version = 17; PRAGMA user_version = 17;",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let original = std::fs::read(&path).unwrap();
    let partial = temp.path().join("CaveA.stalx.pre-v17.partial");
    std::fs::write(&partial, vec![b'x'; original.len() * 2]).unwrap();

    drop(Database::open_existing(&path).unwrap());
    println!("after the upgrade: {:?}", b6_dir_listing(temp.path()));
    assert_eq!(b6_user_version(&path), i64::from(SCHEMA_VERSION));
    let copy = temp.path().join("CaveA.stalx.pre-v17.backup");
    assert!(
        std::fs::read(&copy).unwrap() == original,
        "the kept copy is the file exactly as it was"
    );
    assert!(!partial.exists(), "nothing is left under the partial name");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the copy keeps the file's mode");
    }
}

/// [#576 RVF-d-1] A read-only older file was never upgradable: the copy was
/// kept and the upgrade then failed on its first write ("attempt to write a
/// readonly database"). Its copy is read-only too, so now the flush cannot open
/// it for writing: the open is still refused, with the copy's reason, and no
/// copy is left behind. Skipped where file modes do not bind (running as root).
#[cfg(unix)]
#[test]
fn a_read_only_older_file_is_refused_and_leaves_no_copy() {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = TempDir::new().unwrap();
    let path = b6_build_file(
        temp.path(),
        "CaveA.stalx",
        "UPDATE project SET schema_version = 17; PRAGMA user_version = 17;",
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    if std::fs::OpenOptions::new().write(true).open(&path).is_ok() {
        println!("file modes do not bind for this user; nothing to check");
        return;
    }
    let original = std::fs::read(&path).unwrap();

    let opened = Database::open_existing(&path).map(drop);
    println!(
        "open_existing -> {opened:?}; folder {:?}",
        b6_dir_listing(temp.path())
    );
    let message = format!("{:#}", opened.unwrap_err());
    assert!(
        message.contains("Could not keep a copy of this project before upgrading it"),
        "{message}"
    );
    assert!(std::fs::read(&path).unwrap() == original, "left as it was");
    assert_eq!(b6_dir_listing(temp.path()), ["CaveA.stalx"], "no copy kept");
}
