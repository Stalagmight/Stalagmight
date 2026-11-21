//! Restoring a project file from a backup: the recovery dialog's "Restore from
//! Backup" (#720), and the list it picks from.
//!
//! Backups and projects come in both formats (plan 10.4): a version 2 JSON
//! file (a `.backup` copy, a `.tmp` a save left behind) is checked with the
//! full decode, a legacy SQLite file (an old `.backup`, or the pre-upgrade
//! original the first version 2 save kept in the app-data `backups/` folder,
//! C3) with the conversion the load runs. Either way the staged copy must open
//! the way a load would open it, or nothing is written.
//!
//! A restore used to `std::fs::copy` the backup over the project, and a copy of
//! the project over `<project>.stalx.pre-restore`, both IN PLACE. A copy opens
//! its destination with `O_TRUNC` before it reads the source, and a loaded
//! project was memory-mapped (up to 256 MB) by its live connection, so:
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
//! project (checked before step 1 too, so such a refusal writes nothing). A
//! load opens its file before it takes the project slot, so one already under
//! way could otherwise map the old file and later save over the restore; and
//! an open project would be saved over it, while closing it here would throw
//! its unsaved edits away. (`Command::OpenProject` and the SpeleoDB reopen open
//! a file without the load token. Racing one of them takes concurrent IPC
//! calls on the same file, and the rename still leaves that connection's file
//! intact: a later save could undo the restore, but nothing is lost.) A rename
//! leaves every open handle on the inode it had, so even a file the app has
//! open under another name (renamed by another process) keeps the content its
//! live connection reads.
//!
//! SQLite finds a database's `-journal`, `-wal` and `-shm` by NAME
//! ([`super::speleodb_sidecars`] explains the hazard). An armed journal, or a
//! WAL with frames, beside a SQLite file holds committed changes that the file
//! alone does not. A SQLite backup is restored WITH them: its staged copy is
//! the file with them folded in, worked out on a private copy as an open does
//! (`legacy_upgrade::settled_bytes`; the backup and its sidecars are only
//! read), so the restored file is the survey the pair opens as. The project
//! and the safety copy are only ever replaced, never copied with their
//! sidecars, so a restore refuses rather than replace or put back such a file
//! without them. Any other sidecar beside a name that is about to be replaced
//! (beside a version 2 file a sidecar belongs to no database) is moved out of
//! SQLite's sight first, so the new file never picks up the old one's.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::speleodb_sidecars::{sidecar_path, SetAsideSidecars};
use super::AppState;
use crate::models::NewerSchema;
use crate::persistence::migrate::{LegacyError, MigrateError};
use crate::persistence::stalx_v2::{
    self, DecodeError, DecodeOptions, Fingerprint, Format, MAX_PROJECT_FILE_BYTES,
};
use crate::persistence::{same_file_on_disk, BackupInfo, BackupKind};

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
        {
            let project_slot = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let loading = self
                .active_loading
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            files.refuse_busy(open_path(project_slot.as_ref()), loading.is_some())?;
        }
        // 1. The backup's bytes, staged and checked before anything is written.
        // Only the staged copy is read: the legacy conversion copies it to a
        // private folder first, and the backup itself is never opened. A SQLite
        // backup's unfinished changes are folded into the staged copy.
        let staged = if is_sqlite(&files.backup_real)
            && super::legacy_upgrade::unfinished(&files.backup_real).is_some()
        {
            stage_settled(&files.backup_real, backup, &files.dir, &files.project)?
        } else {
            stage_copy(&files.backup_real, &files.dir, &files.project)?
        };
        let checked = self.check_opens(&staged, backup);
        remove_sidecars_of(&staged);
        checked?;

        let project_slot = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let loading = self
            .active_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Again under the locks: a load may have started, and the files may have
        // changed, while the backup was being copied.
        files.refuse_busy(open_path(project_slot.as_ref()), loading.is_some())?;
        files.check()?;
        let result = files.replace(staged);
        drop(loading);
        drop(project_slot);
        result
    }

    /// Whether the staged copy of `backup` opens the way a load opens a
    /// project: the version 2 decode (format, version, conventions, limits,
    /// invariants), or, for a legacy file, the frozen 0.6.0 open and the
    /// conversion. A backup a newer Stalagmight saved would put a file this
    /// build cannot open in the project's place (#649).
    pub(super) fn check_opens(&self, staged: &Path, backup: &Path) -> Result<(), String> {
        // Bounded as an open is: the staging already refused a larger backup,
        // and nothing larger is read into memory here either.
        let (bytes, _) = Fingerprint::read(staged, MAX_PROJECT_FILE_BYTES).map_err(|e| {
            format!(
                "Not restored: could not read {} ({e}). Nothing was changed.",
                name(backup)
            )
        })?;
        match stalx_v2::sniff(&bytes) {
            Format::V2 => match stalx_v2::decode(&bytes, &DecodeOptions::default()) {
                Ok(_) => Ok(()),
                Err(DecodeError::NewerVersion { found, supported }) => {
                    Err(newer_refusal(backup, found, supported))
                }
                Err(e) => Err(unopenable_backup(backup, &Unopenable::of_v2(&e, &bytes))),
            },
            Format::Legacy => {
                drop(bytes);
                let adjustment = self.get_config().closure_adjustment;
                match crate::persistence::migrate::migrate_path_for_app(staged, &adjustment) {
                    Ok(_) => Ok(()),
                    Err(MigrateError::Legacy(LegacyError::NewerSchema { found, supported })) => {
                        Err(format!(
                            "Not restored: {}",
                            NewerSchema {
                                file: name(backup),
                                found,
                                supported,
                            }
                        ))
                    }
                    Err(e) => Err(unopenable_backup(backup, &Unopenable::of_legacy(&e))),
                }
            }
            Format::Unknown if bytes.is_empty() => {
                Err(unopenable_backup(backup, &Unopenable::Empty))
            }
            Format::Unknown => Err(unopenable_backup(backup, &Unopenable::NotStalagmight)),
        }
    }

    /// Every backup of the project at `path` the recovery dialog can offer,
    /// newest first, each with its [`BackupKind`]:
    ///
    /// - beside the project, a file named after it plus a suffix with
    ///   `.backup` in it ([`BackupKind::Backup`]), or ending in `.tmp`
    ///   ([`BackupKind::InterruptedSave`]): `cave.stalx.backup`,
    ///   `cave.stalx.backup.1`, `cave.stalx.tmp` (the temp a version 2 save
    ///   writes through, left behind by a save that never renamed it). Only
    ///   the suffix decides, so a project whose own name contains `.backup`
    ///   (`cave.backup.stalx`) is never offered as a backup of itself (#720);
    /// - the copy the last restore kept, `<project>.stalx.pre-restore`
    ///   ([`BackupKind::PreRestore`]), so a restore can be undone: restoring
    ///   it swaps the two files;
    /// - in the app-data `backups/` folder, the SQLite original the first
    ///   version 2 save of this project kept (`<stem>.<path key>.<UTC stamp>.pre-v2.stalx`,
    ///   C3, [`BackupKind::PreUpgrade`]).
    ///
    /// SQLite's `-journal`, `-wal` and `-shm` files are never offered, nor the
    /// project itself under another name (a hard link), and neither is a file
    /// a restore would only refuse: one that is empty, that is neither a SQLite
    /// file nor a version 2 file, or a version 2 file that does not end (a
    /// save torn before it finished).
    pub fn list_project_backups(path: &Path) -> Vec<BackupInfo> {
        let mut backups = Vec::new();
        // Where a restore keeps its copy: beside the file a link names.
        let safety = pre_restore_path(&follow_link(path).unwrap_or_else(|_| path.to_path_buf()));
        if !same_file_on_disk(&safety, path) {
            backups.extend(restorable(&safety, BackupKind::PreRestore));
        }
        let own_name = path.file_name().unwrap_or_default().to_string_lossy();
        let dir = path.parent().map(|p| {
            if p.as_os_str().is_empty() {
                Path::new(".")
            } else {
                p
            }
        });
        for entry in dir
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .flatten()
        {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Some(suffix) = file_name.strip_prefix(&*own_name) else {
                continue;
            };
            let suffix = suffix.to_ascii_lowercase();
            let is_sidecar = ["-journal", "-wal", "-shm"]
                .iter()
                .any(|sidecar| suffix.ends_with(sidecar));
            let kind = if suffix.is_empty() || is_sidecar {
                None
            } else if suffix.contains(".backup") {
                Some(BackupKind::Backup)
            } else if suffix
                .rsplit_once('.')
                .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("tmp"))
            {
                Some(BackupKind::InterruptedSave)
            } else {
                None
            };
            if let Some(kind) = kind.filter(|_| !same_file_on_disk(&entry.path(), path)) {
                backups.extend(restorable(&entry.path(), kind));
            }
        }
        if let Some(folder) = super::project_io::upgrade_backup_dir() {
            for kept in kept_originals_in(&folder, path) {
                backups.extend(restorable(&kept, BackupKind::PreUpgrade));
            }
        }
        // Newest first; the same time by name, so the order is stable.
        backups.sort_by(|(a_time, a), (b_time, b)| {
            b_time.cmp(a_time).then_with(|| a.path.cmp(&b.path))
        });
        backups.into_iter().map(|(_, info)| info).collect()
    }
}

/// The pre-upgrade originals of the project at `project` in the app-data
/// backups `folder`: `<stem>.<path key>.<UTC stamp>.pre-v2.stalx`
/// ([`super::project_io::upgrade_backup_prefix`]). Hidden partial copies and
/// sidecars do not match.
pub(super) fn kept_originals_in(folder: &Path, project: &Path) -> Vec<PathBuf> {
    let prefix = super::project_io::upgrade_backup_prefix(project);
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".pre-v2.stalx"))
                .is_some_and(is_upgrade_stamp)
        })
        .map(|entry| entry.path())
        .collect()
}

/// Whether `backup` is a pre-upgrade original in the app-data backups folder
/// (`*.pre-v2.stalx`).
fn is_kept_original(backup: &Path) -> bool {
    let in_folder = super::project_io::upgrade_backup_dir().is_some_and(|folder| {
        backup
            .parent()
            .is_some_and(|parent| same_file_on_disk(parent, &folder))
    });
    in_folder
        && backup
            .file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(".pre-v2.stalx"))
}

/// The file the open project's slot names, if any.
fn open_path(slot: Option<&crate::models::survey::Project>) -> Option<&Path> {
    slot.and_then(|p| p.file_path.as_deref())
}

/// `Not restored: <file> was saved by a newer version ...` for a version 2
/// file of a newer major version.
fn newer_refusal(file: &Path, found: u64, supported: u64) -> String {
    format!(
        "Not restored: {}",
        NewerSchema {
            file: name(file),
            found: i32::try_from(found).unwrap_or(i32::MAX),
            supported: i32::try_from(supported).unwrap_or(i32::MAX),
        }
    )
}

/// The first bytes of the file at `path` (at most `n`).
fn head_of(path: &Path, n: u64) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(n)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

/// Whether the file at `path` is a SQLite file (a legacy `.stalx`).
fn is_sqlite(path: &Path) -> bool {
    head_of(path, 16).is_some_and(|head| stalx_v2::sniff(&head) == Format::Legacy)
}

/// The refusal for a file at `path` that a newer Stalagmight saved (#649):
/// a version 2 file of a higher major version, or a SQLite file stamped with a
/// schema above the one the legacy reader knows. `None` for anything else,
/// unreadable files included: it may be exactly the damaged file a restore is
/// for.
fn newer_on_disk(path: &Path) -> Option<NewerSchema> {
    let head = head_of(path, 4096)?;
    match stalx_v2::sniff(&head) {
        Format::V2 => {
            let found = stalx_v2::probe_bytes(&head).version?;
            (found > stalx_v2::VERSION).then(|| NewerSchema {
                file: name(path),
                found: i32::try_from(found).unwrap_or(i32::MAX),
                supported: i32::try_from(stalx_v2::VERSION).unwrap_or(i32::MAX),
            })
        }
        Format::Legacy => {
            // The SQLite header's user version (offset 60, big-endian): what
            // `PRAGMA user_version` stamps.
            let stamp: [u8; 4] = head.get(60..64)?.try_into().ok()?;
            let found = i32::from_be_bytes(stamp);
            (found > crate::persistence::SCHEMA_VERSION).then(|| NewerSchema {
                file: name(path),
                found,
                supported: crate::persistence::SCHEMA_VERSION,
            })
        }
        Format::Unknown => None,
    }
}

/// A file the dialog can offer, with its size and time (also returned as a
/// `SystemTime`, for the sort), or `None` when a restore would only refuse it
/// (see [`AppState::list_project_backups`]).
fn restorable(path: &Path, kind: BackupKind) -> Option<(std::time::SystemTime, BackupInfo)> {
    let meta = std::fs::metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)?;
    if meta.len() == 0 {
        return None;
    }
    let head = head_of(path, 64)?;
    let offered = match stalx_v2::sniff(&head) {
        Format::Legacy => true,
        Format::V2 => ends_like_a_complete_file(path, meta.len()),
        Format::Unknown => false,
    };
    let modified = meta.modified().ok();
    offered.then(|| {
        (
            modified.unwrap_or(std::time::UNIX_EPOCH),
            BackupInfo {
                path: path.display().to_string(),
                size_bytes: meta.len(),
                modified_at: modified
                    .map(|t| {
                        chrono::DateTime::<chrono::Utc>::from(t)
                            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    })
                    .unwrap_or_default(),
                kind,
            },
        )
    })
}

/// Whether a version 2 file of `len` bytes ends with its end marker (a torn
/// temp does not).
fn ends_like_a_complete_file(path: &Path, len: u64) -> bool {
    use std::io::{Seek as _, SeekFrom};
    const TAIL: u64 = 64;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if file
        .seek(SeekFrom::Start(len.saturating_sub(TAIL)))
        .is_err()
    {
        return false;
    }
    let mut tail = Vec::new();
    if file.take(TAIL).read_to_end(&mut tail).is_err() {
        return false;
    }
    ends_with_end_marker(&tail)
}

/// `"end":true}` and nothing but whitespace after it, at the end of `bytes`.
fn ends_with_end_marker(bytes: &[u8]) -> bool {
    let tail = bytes
        .get(bytes.len().saturating_sub(64)..)
        .unwrap_or_default();
    let compact: Vec<u8> = tail
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    compact.ends_with(b"\"end\":true}")
}

/// `20261120T101112.123456789Z`: the stamp `UpgradeBackup` names a
/// pre-upgrade original with, so a project named `cave` is not offered the
/// originals of `cave.old`.
pub(super) fn is_upgrade_stamp(stamp: &str) -> bool {
    let b = stamp.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        b.get(range)
            .is_some_and(|s| s.iter().all(u8::is_ascii_digit))
    };
    b.len() == 26
        && digits(0..8)
        && b.get(8) == Some(&b'T')
        && digits(9..15)
        && b.get(15) == Some(&b'.')
        && digits(16..25)
        && b.get(25) == Some(&b'Z')
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
        // Before SQLite opens a file: an open acts on the sidecars it finds. The
        // backup's unfinished changes ride with it (step 1).
        refuse_odd_sidecars(&self.backup_real)?;
        refuse_unfinished(
            &self.project,
            &format!("the project {}", name(&self.project)),
            "replacing the file would lose them. Open the project again so Stalagmight can \
             finish them, or move that file away to restore without them.",
        )?;
        refuse_unfinished(
            &self.safety,
            &format!(
                "{}, where a restore keeps its copy of {},",
                name(&self.safety),
                name(&self.project)
            ),
            "replacing the file would lose them. Move those files away, then restore.",
        )?;
        // Copying a backup over a project a newer Stalagmight saved would put an
        // older file in its place (#649). The backup's own check is on its
        // staged copy.
        if let Some(newer) = newer_on_disk(&self.project) {
            return Err(format!("Not restored: {newer}"));
        }
        Ok(())
    }

    /// A restore never runs while a project is being opened, and never touches
    /// the file the app has open: see the module docs.
    fn refuse_busy(&self, open_project: Option<&Path>, loading: bool) -> Result<(), String> {
        if loading {
            return Err(
                "Not restored: a project is being opened. Try again once it has opened."
                    .to_string(),
            );
        }
        let Some(open) = open_project else {
            return Ok(());
        };
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
            let outcome = previous.map_or_else(|| Ok(()), |p| self.put_back(p));
            return Err(format!(
                "Not restored: could not replace {} ({e}).{}",
                name(&self.project),
                after_failure(outcome)
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
        // Named the way the dialog lists it: a pre-upgrade original's file name
        // (a hash and a nanosecond stamp in the app-data folder) means nothing.
        let restored = if is_kept_original(self.backup) {
            "Restored the original from before the upgrade to the new format.".to_string()
        } else {
            format!("Restored from {}.", name(self.backup))
        };
        Ok(if kept {
            format!(
                "{restored} The file it replaced is kept as {} and is listed with the earlier \
                 copies, so you can switch back.",
                name(&self.safety)
            )
        } else {
            restored
        })
    }

    /// Step 2: a copy of the project replaces the safety copy, and the one that
    /// was there is parked until the restore is done.
    fn keep_safety_copy(&self) -> Result<Previous, String> {
        let copy = stage_copy(&self.project, &self.dir, &self.project)?;
        let previous = park(&self.safety, &self.dir)?;
        if let Err(e) = install(copy, &self.safety) {
            let outcome = self.put_back(previous);
            return Err(format!(
                "Not restored: could not keep a copy of {} as {} ({e}).{}",
                name(&self.project),
                name(&self.safety),
                after_failure(outcome)
            ));
        }
        Ok(previous)
    }

    /// Undo step 2 after a failure: the parked copy goes back, or the new copy
    /// goes when there was none. An `Err` says, for the message, what could
    /// not be undone and where the previous copy is.
    fn put_back(&self, previous: Previous) -> Result<(), String> {
        match previous {
            Previous::Nothing => match std::fs::remove_file(&self.safety) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => {
                    tracing::warn!(error = %e, path = %self.safety.display(), "could not remove the new safety copy");
                    Err(format!(
                        "The project is unchanged, and {} now holds a copy of it.",
                        name(&self.safety)
                    ))
                }
            },
            Previous::File(parked) => match parked.persist(&self.safety) {
                Ok(()) => {
                    sync_dir(&self.safety);
                    Ok(())
                }
                Err(e) => {
                    tracing::error!(error = %e.error, path = %self.safety.display(), "could not put the previous safety copy back");
                    // Never delete the only copy of it.
                    let mut parked = e.path;
                    parked.disable_cleanup(true);
                    Err(format!(
                        "The project is unchanged, but the previous {} could not be put back: it \
                         is in {}.",
                        name(&self.safety),
                        name(&parked)
                    ))
                }
            },
        }
    }
}

/// The end of a failure message: whether step 2 was undone.
fn after_failure(put_back: Result<(), String>) -> String {
    match put_back {
        Ok(()) => " Nothing was changed.".to_string(),
        Err(note) => format!(" {note}"),
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

/// Why a backup does not open, as the dialog says it.
enum Unopenable {
    Empty,
    NotStalagmight,
    /// A version 2 file cut short (a save that never finished writing it).
    Incomplete,
    /// It breaks the survey's invariants, or SQLite's integrity check.
    Damaged,
    Other(String),
}

impl Unopenable {
    /// `bytes` is the file: malformed text in a file that does not end with
    /// its end marker is a torn file (a save cut short mid-token), not a
    /// malformed one.
    fn of_v2(error: &DecodeError, bytes: &[u8]) -> Self {
        tracing::warn!(reason = %error, "The version 2 backup does not open as a project");
        match error {
            DecodeError::NotStalx | DecodeError::ByteOrderMark | DecodeError::Legacy => {
                Self::NotStalagmight
            }
            DecodeError::Truncated { .. } | DecodeError::MissingEnd => Self::Incomplete,
            DecodeError::Syntax { .. } if !ends_with_end_marker(bytes) => Self::Incomplete,
            DecodeError::Invalid { .. } | DecodeError::DuplicateStation { .. } => Self::Damaged,
            other => Self::Other(other.to_string()),
        }
    }

    fn of_legacy(error: &MigrateError) -> Self {
        let reason = format!("{error:#}");
        tracing::warn!(%reason, "The legacy backup does not open as a project");
        match error {
            MigrateError::Legacy(LegacyError::NotSqlite) => Self::NotStalagmight,
            MigrateError::Legacy(LegacyError::Corrupt(detail))
                if detail.contains("may be corrupted") =>
            {
                Self::Damaged
            }
            MigrateError::Legacy(LegacyError::Corrupt(detail))
                if detail.contains("not a database")
                    || detail.contains("does not contain a valid Stalagmight project") =>
            {
                Self::NotStalagmight
            }
            _ => Self::Other(reason),
        }
    }
}

/// Why the backup cannot be restored, in the dialog's words. The full reason
/// goes to the log; the dialog says what it means for the user.
fn unopenable_backup(backup: &Path, why: &Unopenable) -> String {
    let what = match why {
        Unopenable::Empty => "is empty".to_string(),
        Unopenable::NotStalagmight => "is not a Stalagmight project".to_string(),
        Unopenable::Incomplete => {
            "is incomplete: the save that wrote it never finished".to_string()
        }
        Unopenable::Damaged => "is damaged too: it fails the same integrity check".to_string(),
        Unopenable::Other(reason) => {
            format!("cannot be opened as a Stalagmight project ({reason})")
        }
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

/// Refuse a file whose sidecar is not a regular file (which nothing here
/// opens).
fn refuse_odd_sidecars(db: &Path) -> Result<(), String> {
    for sidecar in [sidecar_path(db, "-journal"), sidecar_path(db, "-wal")] {
        if std::fs::symlink_metadata(&sidecar).is_ok_and(|m| !m.is_file()) {
            return Err(format!(
                "Not restored: {} beside {} is not a file. Move it away, then restore.",
                name(&sidecar),
                name(db)
            ));
        }
    }
    Ok(())
}

/// Refuse a SQLite file whose committed state is partly in a sidecar, or a
/// file whose sidecar is not a regular file. Beside a version 2 file (or none)
/// a sidecar is debris, moved aside when a name is replaced.
fn refuse_unfinished(db: &Path, what: &str, consequence: &str) -> Result<(), String> {
    refuse_odd_sidecars(db)?;
    if !is_sqlite(db) {
        return Ok(());
    }
    let Some(unfinished) = super::legacy_upgrade::unfinished(db) else {
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
    let mut reader = std::fs::File::open(source).map_err(|e| failed("read", &e))?;
    let metadata = reader.metadata().map_err(|e| failed("read", &e))?;
    // A backup the restore could never open is refused before a byte of it
    // is copied (the user picks the file: it may be anything).
    if metadata.len() > MAX_PROJECT_FILE_BYTES {
        return Err(too_large(source));
    }
    let staged = stage_from(
        &mut std::io::Read::take(&mut reader, MAX_PROJECT_FILE_BYTES.saturating_add(1)),
        metadata.permissions(),
        source,
        dir,
        project,
    )?;
    // It grew while it was copied: the copy stopped one byte past the cap.
    if std::fs::metadata(&staged)
        .map_err(|e| failed("copy", &e))?
        .len()
        > MAX_PROJECT_FILE_BYTES
    {
        return Err(too_large(source));
    }
    Ok(staged)
}

/// The refusal of a backup larger than any project Stalagmight reads.
fn too_large(source: &Path) -> String {
    format!(
        "Not restored: {} is larger than {} MiB, the most Stalagmight reads. Nothing was changed.",
        name(source),
        MAX_PROJECT_FILE_BYTES / (1024 * 1024)
    )
}

/// [`stage_copy`] of a SQLite `backup` (`named` as the caller named it) with
/// the unfinished changes beside it folded in: the survey the pair opens as.
fn stage_settled(
    backup: &Path,
    named: &Path,
    dir: &Path,
    project: &Path,
) -> Result<tempfile::TempPath, String> {
    let read = Fingerprint::read(backup, MAX_PROJECT_FILE_BYTES)
        .and_then(|(bytes, _)| std::fs::metadata(backup).map(|meta| (bytes, meta.permissions())));
    let (bytes, permissions) = read.map_err(|e| {
        format!(
            "Not restored: could not read {} ({e}). Nothing was changed.",
            name(named)
        )
    })?;
    let settled = super::legacy_upgrade::settled_bytes(&bytes, backup).map_err(|e| {
        tracing::warn!(reason = %e, backup = %backup.display(), "Could not finish a backup's unfinished changes");
        format!(
            "Not restored: the changes that were never finished beside {} could not be \
             finished ({e}). Nothing was changed. Pick another backup.",
            name(named)
        )
    })?;
    stage_from(&mut settled.as_slice(), permissions, backup, dir, project)
}

/// Write everything `reader` yields to a staged file beside the project, sync
/// it and give it `permissions` (those of `source`, named in messages).
fn stage_from(
    reader: &mut dyn std::io::Read,
    permissions: std::fs::Permissions,
    source: &Path,
    dir: &Path,
    project: &Path,
) -> Result<tempfile::TempPath, String> {
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
    std::io::copy(reader, staged.as_file_mut()).map_err(|e| failed("copy", &e))?;
    // Through the handle that wrote it, so a read-only source does not stop it.
    staged
        .as_file()
        .sync_all()
        .map_err(|e| failed("write a copy of", &e))?;
    let staged = staged.into_temp_path();
    std::fs::set_permissions(&staged, permissions).map_err(|e| failed("copy", &e))?;
    Ok(staged)
}

/// Delete what SQLite left beside a staged copy it opened (never a user's file).
fn remove_sidecars_of(staged: &Path) {
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = sidecar_path(staged, suffix);
        if let Err(e) = std::fs::remove_file(&sidecar) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, path = %sidecar.display(), "could not remove a staged copy's sidecar");
            }
        }
    }
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
/// handle to sync and makes the rename durable itself). Best effort: the caller
/// has already renamed, so a failure is logged, not returned. Also used by
/// every save (`project_io.rs`), so a folder that never syncs (some network or
/// removable volumes) warns once per run and is a debug line after that.
pub(super) fn sync_dir(dest: &Path) {
    #[cfg(unix)]
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(error) = std::fs::File::open(parent).and_then(|dir| dir.sync_all()) {
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::debug!(%error, path = %parent.display(), "could not sync the project's folder after a rename");
            } else {
                tracing::warn!(%error, path = %parent.display(), "could not sync the project's folder after a rename");
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dest;
}

pub(super) fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
