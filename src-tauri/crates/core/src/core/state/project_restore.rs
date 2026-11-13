//! Restoring a project file from a backup: the recovery dialog's "Restore from
//! Backup" (#720).
//!
//! A restore used to `std::fs::copy` the backup over the project, and a copy of
//! the project over `<project>.stalx.pre-restore`, both IN PLACE. A copy opens
//! its destination with `O_TRUNC` before it reads the source, and a loaded
//! project is memory-mapped (up to 256 MB) by its live connection, so:
//!
//! - over the open project, the app went on showing (and later saved) the old
//!   project, and an empty backup crashed the next read with SIGBUS (F1);
//! - a backup that was the project itself came out as 0 bytes (F2);
//! - the safety copy rewrote `<project>.stalx.pre-restore` under the app when
//!   that file was the one open (F4);
//! - restoring `<project>.stalx.pre-restore` (undoing a restore) overwrote it
//!   with the project before reading it, and reported success (F5).
//!
//! So a restore never writes INTO a file. It stages full copies beside the
//! project and renames them into place, in this order:
//!
//! 1. The backup's bytes are copied to a staged file before anything else is
//!    written, and the staged copy is opened the way a load opens a project. A
//!    backup that would not open is refused while nothing has changed.
//! 2. A copy of the project replaces `<project>.stalx.pre-restore`. When that is
//!    the backup being restored, its bytes are already staged, so undoing a
//!    restore swaps the two files and loses neither.
//! 3. The staged backup replaces the project.
//!
//! A rename leaves every open handle on the inode it had, so a file the app has
//! open under another name (renamed by another process) keeps the content its
//! live connection reads. A restore still refuses outright when the project,
//! the safety copy or the backup is the open project: the in-memory project
//! would otherwise be saved over the restore later, and closing it here would
//! throw its unsaved edits away.
//!
//! SQLite finds a database's `-journal`, `-wal` and `-shm` by NAME
//! ([`super::speleodb_sidecars`] explains the hazard). An armed journal, or a
//! WAL with frames, holds committed changes that the file alone does not, so a
//! restore refuses rather than copy the backup or replace the project without
//! them. Any other sidecar beside a name that is about to be replaced is moved
//! out of SQLite's sight first, so the new file never picks up the old one's.

use std::path::{Path, PathBuf};

use super::project_io::same_file;
use super::speleodb_sidecars::{has_hot_journal, sidecar_path, SetAsideSidecars};
use super::AppState;
use crate::persistence::Database;

/// Where a restore keeps a copy of the file it replaces.
pub(super) fn pre_restore_path(project: &Path) -> PathBuf {
    project.with_extension("stalx.pre-restore")
}

impl AppState {
    /// Restore `project` from `backup` (the recovery dialog's "Restore from
    /// Backup"). Returns the message the dialog shows; an `Err` means nothing
    /// was restored and says why.
    pub fn restore_project_from_backup(
        &self,
        project: &Path,
        backup: &Path,
    ) -> Result<String, String> {
        // Held for the whole restore, so no load can make one of these files
        // the open project half-way through.
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let open = guard.as_ref().and_then(|p| p.file_path.clone());
        let result = restore(project, backup, open.as_deref());
        drop(guard);
        match &result {
            Ok(message) => tracing::info!(
                project = %project.display(),
                backup = %backup.display(),
                purpose = "replace a damaged project with the backup the user picked",
                "{message}"
            ),
            Err(reason) => tracing::warn!(
                project = %project.display(),
                backup = %backup.display(),
                "Restore refused or failed: {reason}"
            ),
        }
        result
    }
}

fn restore(project: &Path, backup: &Path, open: Option<&Path>) -> Result<String, String> {
    let project = follow_link(project)?;
    let safety = pre_restore_path(&project);

    if !backup.exists() {
        return Err(format!("Backup file not found: {}", backup.display()));
    }
    if !backup.is_file() {
        return Err(format!("Not restored: {} is not a file.", name(backup)));
    }
    if is_same_file(&project, backup) {
        return Err(format!(
            "Not restored: the backup you picked is {} itself.",
            name(&project)
        ));
    }
    if let Some(open) = open {
        refuse_open(open, &project, &safety, backup)?;
    }
    // Before SQLite opens either file: an open acts on the sidecars it finds.
    refuse_unfinished(backup, "the backup")?;
    refuse_unfinished(&project, "the project")?;
    // Copying a backup over a project a newer Stalagmight saved would put an
    // older file in its place, and a backup a newer Stalagmight saved would put
    // a file this build cannot open there (#649). Both before the safety copy.
    for file in [project.as_path(), backup] {
        Database::check_not_newer_at(file).map_err(|e| format!("Not restored: {e}"))?;
    }

    let dir = project
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // 1. The backup's bytes, staged before anything else is written.
    let staged = stage_copy(backup, dir, &project)?;
    drop(Database::open_readonly(&staged).map_err(|e| unopenable_backup(backup, &staged, &e))?);

    // 2. The safety copy of the project, when there is a project to copy.
    let kept = match std::fs::symlink_metadata(&project) {
        Ok(_) => {
            let copy = stage_copy(&project, dir, &project)?;
            install(copy, &safety).map_err(|e| {
                format!(
                    "Not restored: could not keep a copy of {} as {} ({e}). \
                     Nothing was changed.",
                    name(&project),
                    name(&safety)
                )
            })?;
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            return Err(format!(
                "Not restored: could not read {} ({e}). Nothing was changed.",
                name(&project)
            ))
        }
    };

    // 3. The backup takes the project's name.
    install(staged, &project).map_err(|e| {
        let kept_note = if kept {
            format!(" A copy of it is in {}.", name(&safety))
        } else {
            String::new()
        };
        format!(
            "Not restored: could not replace {} ({e}). The project is unchanged.{kept_note}",
            name(&project)
        )
    })?;

    // An undo: the backup was the safety copy, which now holds the replaced file.
    if kept && is_same_file(backup, &safety) {
        return Ok(format!(
            "Restored {0}, the copy the last restore kept. The two files swapped places: \
             {0} now holds the file it replaced, so restoring it again switches back.",
            name(&safety)
        ));
    }
    let mut message = format!("Restored from {}.", backup.display());
    if kept {
        message.push_str(&format!(
            " The file it replaced is kept as {}.",
            name(&safety)
        ));
    }
    Ok(message)
}

/// Why the backup cannot be restored, in the dialog's words. The full reason
/// goes to the log; the dialog says what it means for the user.
fn unopenable_backup(backup: &Path, staged: &Path, error: &anyhow::Error) -> String {
    let reason = format!("{error:#}");
    tracing::warn!(backup = %backup.display(), %reason, "The backup does not open as a project");
    let empty = std::fs::metadata(staged).is_ok_and(|m| m.len() == 0);
    let what = if empty {
        "is empty".to_string()
    } else if reason.contains("may be corrupted") {
        "is damaged too: it fails the same integrity check".to_string()
    } else if reason.contains("not a database")
        || reason.contains("does not contain a valid Stalagmight project")
    {
        "is not a Stalagmight project".to_string()
    } else {
        format!("cannot be opened as a Stalagmight project ({reason})")
    };
    format!(
        "Not restored: {} {what}. Nothing was changed. Pick another backup.",
        name(backup)
    )
}

/// A project path that is a symbolic link is restored into the file it names,
/// as the in-place copy used to, so the link stays a link.
fn follow_link(project: &Path) -> Result<PathBuf, String> {
    match std::fs::symlink_metadata(project) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(project).map_err(|e| {
            format!(
                "Not restored: {} is a link to a file that cannot be found ({e}).",
                name(project)
            )
        }),
        _ => Ok(project.to_path_buf()),
    }
}

/// The same file on disk: the same canonical path, or (on Unix) the same inode,
/// which also catches a hard link under another name.
fn is_same_file(a: &Path, b: &Path) -> bool {
    same_file(a, b) || same_inode(a, b)
}

#[cfg(unix)]
fn same_inode(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_inode(_: &Path, _: &Path) -> bool {
    false
}

/// A restore never touches a file the app has open: see the module docs.
fn refuse_open(open: &Path, project: &Path, safety: &Path, backup: &Path) -> Result<(), String> {
    if is_same_file(open, project) {
        return Err(format!(
            "Not restored: {} is open in Stalagmight. Close it, then restore.",
            name(project)
        ));
    }
    if is_same_file(open, safety) {
        return Err(format!(
            "Not restored: {} is open in Stalagmight, and the restore keeps its copy of {} \
             there. Close it, then restore.",
            name(safety),
            name(project)
        ));
    }
    if is_same_file(open, backup) {
        return Err(format!(
            "Not restored: the backup {} is open in Stalagmight. Close it, then restore.",
            name(backup)
        ));
    }
    Ok(())
}

/// Refuse a file whose committed state is partly in a sidecar: copying or
/// replacing the file alone would tear it or drop those changes.
fn refuse_unfinished(db: &Path, role: &str) -> Result<(), String> {
    let journal = sidecar_path(db, "-journal");
    let wal = sidecar_path(db, "-wal");
    let wal_has_frames = std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0);
    let unfinished = if has_hot_journal(db) {
        journal
    } else if wal_has_frames {
        wal
    } else {
        return Ok(());
    };
    let advice = if role == "the project" {
        "Open the project once so Stalagmight can finish them, then restore."
    } else {
        "Pick another backup."
    };
    Err(format!(
        "Not restored: {role} {} has changes that were never finished, in {} beside it. {advice}",
        name(db),
        name(&unfinished)
    ))
}

/// A full, durable copy of `source` beside the project, under a hidden name the
/// backup list never offers.
fn stage_copy(source: &Path, dir: &Path, project: &Path) -> Result<tempfile::TempPath, String> {
    let staged = tempfile::Builder::new()
        .prefix(&format!(".{}.restore-", name(project)))
        .suffix(".stalx")
        .tempfile_in(dir)
        .map_err(|e| {
            format!(
                "Not restored: could not create a file beside {} ({e}). Nothing was changed.",
                name(project)
            )
        })?
        .into_temp_path();
    std::fs::copy(source, &staged).map_err(|e| {
        format!(
            "Not restored: could not copy {} ({e}). Nothing was changed.",
            name(source)
        )
    })?;
    sync_file(&staged).map_err(|e| {
        format!(
            "Not restored: could not write a copy of {} to disk ({e}). Nothing was changed.",
            name(source)
        )
    })?;
    Ok(staged)
}

/// Rename `staged` over `dest`, with `dest`'s sidecars moved out of SQLite's
/// sight while the name changes hands and deleted once it has.
fn install(staged: tempfile::TempPath, dest: &Path) -> Result<(), String> {
    let sidecars = SetAsideSidecars::take(dest).map_err(|e| e.to_string())?;
    match staged.persist(dest) {
        Ok(()) => {
            sync_dir(dest);
            sidecars.discard();
            Ok(())
        }
        Err(e) => {
            sidecars.restore();
            Err(e.error.to_string())
        }
    }
}

fn sync_file(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

/// Make a rename in `dest`'s directory durable (Unix; Windows has no directory
/// handle to sync and makes the rename durable itself).
fn sync_dir(dest: &Path) {
    #[cfg(unix)]
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(error) = std::fs::File::open(parent).and_then(|dir| dir.sync_all()) {
            tracing::warn!(%error, path = %parent.display(), "could not sync the restored project's folder");
        }
    }
    #[cfg(not(unix))]
    let _ = dest;
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
