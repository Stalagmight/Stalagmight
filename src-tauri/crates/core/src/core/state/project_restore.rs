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
//! project and renames them into place:
//!
//! 1. The backup's bytes are copied to a staged file before anything else is
//!    written, and the staged copy is opened the way a load opens a project. A
//!    backup that would not open is refused while nothing has changed.
//! 2. A copy of the project replaces `<project>.stalx.pre-restore`. The copy that
//!    was there is parked under a hidden name until step 3 is done. When it is
//!    the backup being restored, its bytes are staged as well, so undoing a
//!    restore swaps the two files and loses neither.
//! 3. The staged backup replaces the project. If that fails, the parked copy
//!    goes back to `<project>.stalx.pre-restore`: a restore that fails changes
//!    nothing.
//!
//! Steps 2 and 3 run with the project slot and the load token locked (in that
//! order, as the SpeleoDB cache install takes them), and not at all while a
//! load is in flight or the project, its safety copy or the backup is the open
//! project. A load opens its file before it takes the project slot, so one
//! already under way could otherwise map the old file and later save over the
//! restore; and an open project would be saved over it, while closing it here
//! would throw its unsaved edits away. A rename leaves every open handle on the
//! inode it had, so even a file the app has open under another name (renamed by
//! another process) keeps the content its live connection reads.
//!
//! SQLite finds a database's `-journal`, `-wal` and `-shm` by NAME
//! ([`super::speleodb_sidecars`] explains the hazard). An armed journal, or a
//! WAL with frames, holds committed changes that the file alone does not, so a
//! restore refuses rather than copy, replace or put back a file without them.
//! Any other sidecar beside a name that is about to be replaced is moved out of
//! SQLite's sight first, so the new file never picks up the old one's.

use std::path::{Path, PathBuf};

use super::speleodb_sidecars::{has_hot_journal, sidecar_path, SetAsideSidecars};
use super::AppState;
use crate::persistence::{same_file_on_disk, Database};

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
        let result = self.restore(project, backup);
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

    fn restore(&self, project: &Path, backup: &Path) -> Result<String, String> {
        let files = Files::new(project, backup)?;
        files.check()?;
        // 1. The backup's bytes, staged and opened before anything is written.
        let staged = stage_copy(&files.backup_real, &files.dir, &files.project)?;
        drop(Database::open_readonly(&staged).map_err(|e| unopenable_backup(backup, &staged, &e))?);

        let project_slot = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let loading = self
            .active_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if loading.is_some() {
            return Err(
                "Not restored: a project is being opened. Try again once it has opened."
                    .to_string(),
            );
        }
        if let Some(open) = project_slot.as_ref().and_then(|p| p.file_path.as_deref()) {
            files.refuse_open(open)?;
        }
        // Again under the locks: the files may have changed while the backup
        // was being copied.
        files.check()?;
        let result = files.replace(staged);
        drop(loading);
        drop(project_slot);
        result
    }
}

/// The files one restore touches.
struct Files<'a> {
    /// The project's file, after following a link.
    project: PathBuf,
    /// The backup as the caller named it, for messages.
    backup: &'a Path,
    /// The backup's file, after following a link: where its sidecars are.
    backup_real: PathBuf,
    /// `<project>.stalx.pre-restore`.
    safety: PathBuf,
    /// The project's folder, where everything is staged.
    dir: PathBuf,
    /// The backup is the safety copy: restoring it undoes the last restore.
    undo: bool,
}

impl<'a> Files<'a> {
    fn new(project: &Path, backup: &'a Path) -> Result<Self, String> {
        let project = follow_link(project)?;
        if !backup.exists() {
            return Err(format!("Backup file not found: {}", backup.display()));
        }
        let backup_real = follow_link(backup)?;
        let safety = pre_restore_path(&project);
        let dir = project
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let undo = same_file_on_disk(backup, &safety);
        Ok(Self {
            project,
            backup,
            backup_real,
            safety,
            dir,
            undo,
        })
    }

    /// Everything that refuses a restore before it writes anything, apart
    /// from the open project. Nothing here opens a file that is not a regular
    /// file, so a FIFO or a device cannot block it.
    fn check(&self) -> Result<(), String> {
        if !self.backup_real.is_file() {
            return Err(format!(
                "Not restored: {} is not a file.",
                name(self.backup)
            ));
        }
        if same_file_on_disk(&self.project, self.backup) {
            return Err(format!(
                "Not restored: the backup you picked is {} itself.",
                name(&self.project)
            ));
        }
        for path in [&self.project, &self.safety] {
            if std::fs::symlink_metadata(path).is_ok_and(|m| !m.is_file()) {
                return Err(format!("Not restored: {} is not a file.", name(path)));
            }
        }
        // Before SQLite opens a file: an open acts on the sidecars it finds.
        refuse_unfinished(
            &self.backup_real,
            &format!("the backup {}", name(self.backup)),
            "a copy of the file alone would lose them. Pick another backup.",
        )?;
        refuse_unfinished(
            &self.project,
            &format!("the project {}", name(&self.project)),
            "replacing the file would lose them. Open the project again so Stalagmight can \
             finish them, or move that file away to restore without them.",
        )?;
        if !self.undo {
            refuse_unfinished(
                &self.safety,
                &format!(
                    "{}, where a restore keeps its copy of {},",
                    name(&self.safety),
                    name(&self.project)
                ),
                "replacing the file would lose them. Move those files away, then restore.",
            )?;
        }
        // Copying a backup over a project a newer Stalagmight saved would put an
        // older file in its place, and a backup a newer Stalagmight saved would
        // put a file this build cannot open there (#649).
        for file in [self.project.as_path(), self.backup_real.as_path()] {
            Database::check_not_newer_at(file).map_err(|e| format!("Not restored: {e}"))?;
        }
        Ok(())
    }

    /// A restore never touches a file the app has open: see the module docs.
    fn refuse_open(&self, open: &Path) -> Result<(), String> {
        if same_file_on_disk(open, &self.project) {
            return Err(format!(
                "Not restored: {} is open in Stalagmight. Close it, then restore.",
                name(&self.project)
            ));
        }
        if same_file_on_disk(open, &self.safety) {
            return Err(format!(
                "Not restored: {} is open in Stalagmight, and the restore keeps its copy of {} \
                 there. Close it, then restore.",
                name(&self.safety),
                name(&self.project)
            ));
        }
        if same_file_on_disk(open, self.backup) {
            return Err(format!(
                "Not restored: the backup {} is open in Stalagmight. Close it, then restore.",
                name(self.backup)
            ));
        }
        Ok(())
    }

    /// Steps 2 and 3: the safety copy, then the backup takes the project's name.
    fn replace(&self, staged: tempfile::TempPath) -> Result<String, String> {
        let project_meta = match std::fs::metadata(&self.project) {
            Ok(meta) => Some(meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(format!(
                    "Not restored: could not read {} ({e}). Nothing was changed.",
                    name(&self.project)
                ))
            }
        };
        let kept = project_meta.is_some();
        let previous = if kept {
            Some(self.keep_safety_copy()?)
        } else {
            None
        };
        // The restored file takes the place of the project, permissions included.
        let permissions = project_meta.map(|meta| meta.permissions());
        let installed = permissions
            .map_or(Ok(()), |p| std::fs::set_permissions(&staged, p))
            .map_err(|e| e.to_string())
            .and_then(|()| install(staged, &self.project));
        if let Err(e) = installed {
            let put_back = previous.map_or_else(String::new, |p| self.put_back(p));
            return Err(format!(
                "Not restored: could not replace {} ({e}). Nothing was changed.{put_back}",
                name(&self.project)
            ));
        }
        // The previous safety copy (parked) is deleted when `previous` drops.
        drop(previous);

        if kept && self.undo {
            return Ok(format!(
                "Restored {0}, the copy the last restore kept. The two files swapped places: \
                 {0} now holds the file it replaced, so restoring it again switches back.",
                name(&self.safety)
            ));
        }
        Ok(if kept {
            format!(
                "Restored from {}. The file it replaced is kept as {}.",
                self.backup.display(),
                name(&self.safety)
            )
        } else {
            format!("Restored from {}.", self.backup.display())
        })
    }

    /// Step 2: a copy of the project replaces the safety copy, and the one that
    /// was there is parked until the restore is done.
    fn keep_safety_copy(&self) -> Result<Previous, String> {
        let copy = stage_copy(&self.project, &self.dir, &self.project)?;
        let previous = park(&self.safety, &self.dir)?;
        if let Err(e) = install(copy, &self.safety) {
            let put_back = self.put_back(previous);
            return Err(format!(
                "Not restored: could not keep a copy of {} as {} ({e}). Nothing was changed.\
                 {put_back}",
                name(&self.project),
                name(&self.safety)
            ));
        }
        Ok(previous)
    }

    /// Undo step 2 after a failure: the parked copy goes back, or the new copy
    /// goes when there was none. Returns a note for the message when that
    /// itself fails, naming where the previous copy is.
    fn put_back(&self, previous: Previous) -> String {
        match previous {
            Previous::Nothing => match std::fs::remove_file(&self.safety) {
                Ok(()) => String::new(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    tracing::warn!(error = %e, path = %self.safety.display(), "could not remove the new safety copy");
                    format!(
                        " {} holds a copy of the unchanged project.",
                        name(&self.safety)
                    )
                }
            },
            Previous::File(parked) => match parked.persist(&self.safety) {
                Ok(()) => {
                    sync_dir(&self.safety);
                    String::new()
                }
                Err(e) => {
                    tracing::error!(error = %e.error, path = %self.safety.display(), "could not put the previous safety copy back");
                    match e.path.keep() {
                        Ok(kept) => format!(
                            " The previous {} could not be put back; it is in {}.",
                            name(&self.safety),
                            name(&kept)
                        ),
                        Err(_) => String::new(),
                    }
                }
            },
        }
    }
}

/// What was at the safety copy's name before this restore.
enum Previous {
    /// Nothing: a failed restore removes the new copy again.
    Nothing,
    /// The previous safety copy, under a hidden name. Deleted when dropped.
    File(tempfile::TempPath),
}

/// Move the safety copy out of the way, under a hidden name in `dir`.
fn park(safety: &Path, dir: &Path) -> Result<Previous, String> {
    if std::fs::symlink_metadata(safety).is_err() {
        return Ok(Previous::Nothing);
    }
    let failed = |e: &dyn std::fmt::Display| {
        format!(
            "Not restored: could not set {} aside ({e}). Nothing was changed.",
            name(safety)
        )
    };
    let parked = tempfile::Builder::new()
        .prefix(&format!(".{}.previous-", name(safety)))
        .tempfile_in(dir)
        .map_err(|e| failed(&e))?
        .into_temp_path();
    std::fs::rename(safety, &parked).map_err(|e| failed(&e))?;
    Ok(Previous::File(parked))
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

/// A path that is a symbolic link stands for the file it names: a project is
/// restored into that file, as the in-place copy used to (the link stays a
/// link), and a backup's sidecars are beside that file.
fn follow_link(path: &Path) -> Result<PathBuf, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(path).map_err(|e| {
            format!(
                "Not restored: {} is a link to a file that cannot be found ({e}).",
                name(path)
            )
        }),
        _ => Ok(path.to_path_buf()),
    }
}

/// Refuse a file whose committed state is partly in a sidecar, or whose
/// sidecar is not a regular file (which nothing here opens).
fn refuse_unfinished(db: &Path, what: &str, consequence: &str) -> Result<(), String> {
    let journal = sidecar_path(db, "-journal");
    let wal = sidecar_path(db, "-wal");
    for sidecar in [&journal, &wal] {
        if std::fs::symlink_metadata(sidecar).is_ok_and(|m| !m.is_file()) {
            return Err(format!(
                "Not restored: {} beside {} is not a file. Move it away, then restore.",
                name(sidecar),
                name(db)
            ));
        }
    }
    let wal_has_frames = std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0);
    let unfinished = if has_hot_journal(db) {
        journal
    } else if wal_has_frames {
        wal
    } else {
        return Ok(());
    };
    Err(format!(
        "Not restored: {what} has changes that were never finished, in {} beside it, and \
         {consequence}",
        name(&unfinished)
    ))
}

/// A full, durable copy of `source` beside the project, under a hidden name the
/// backup list never offers, with the permissions of `source`.
fn stage_copy(source: &Path, dir: &Path, project: &Path) -> Result<tempfile::TempPath, String> {
    let failed = |what: &str, e: &dyn std::fmt::Display| {
        format!(
            "Not restored: could not {what} {} ({e}). Nothing was changed.",
            name(source)
        )
    };
    let mut staged = tempfile::Builder::new()
        .prefix(&format!(".{}.restore-", name(project)))
        .suffix(".stalx")
        .tempfile_in(dir)
        .map_err(|e| {
            format!(
                "Not restored: could not create a file beside {} ({e}). Nothing was changed.",
                name(project)
            )
        })?;
    let mut reader = std::fs::File::open(source).map_err(|e| failed("read", &e))?;
    let permissions = reader
        .metadata()
        .map_err(|e| failed("read", &e))?
        .permissions();
    std::io::copy(&mut reader, staged.as_file_mut()).map_err(|e| failed("copy", &e))?;
    // Through the handle that wrote it, so a read-only source does not stop it.
    staged
        .as_file()
        .sync_all()
        .map_err(|e| failed("write a copy of", &e))?;
    let staged = staged.into_temp_path();
    std::fs::set_permissions(&staged, permissions).map_err(|e| failed("copy", &e))?;
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
