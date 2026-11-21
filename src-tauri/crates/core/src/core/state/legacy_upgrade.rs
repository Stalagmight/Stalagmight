//! The first version 2 write over a legacy (SQLite) `.stalx` (plan 7.2.10):
//! the original is made whole, kept in the app-data backups folder, and only
//! then replaced. Every write over a legacy file goes through [`upgrade`]: the
//! first save of an opened one, a Recents rename and a SpeleoDB link strip of
//! a closed one, and a Save As or Save a Copy onto one (the 0.6.0 file it
//! replaces is kept like any other original).
//!
//! A legacy file is not always one file (`persistence::legacy_settle`). Beside
//! a file an old build was killed writing sits a HOT `-journal`, and only the
//! pair is the survey 0.6.0 committed; beside a file in WAL mode a `-wal` may
//! hold its newest commits. SQLite finds them by NAME, so the JSON file must
//! never appear beside one (an old build opening it would roll the journal's
//! pages into it), and the original must never lose one while it still needs
//! it (the file alone opens as another survey). Hence the order, chosen so that
//! a kill between any two steps leaves the project's path holding either the
//! survey the user had or the complete version 2 file:
//!
//! 1. Read the original and check it against the guard: a file that is not
//!    what the open read refuses the upgrade before anything is written.
//! 2. When a hot journal or a WAL with frames is beside it, settle it: fold
//!    them into a private copy (`legacy_settle`, the frozen reader's own rule,
//!    so the copy is the survey the open showed), stage that copy beside the
//!    original under a hidden name, sync it, check the guard again and rename
//!    it over the original. Until the next step the settled file sits beside
//!    the old sidecar, which is SQLite's own state after a rollback or a
//!    checkpoint: a replay leaves the file as it is.
//! 3. Remove every sidecar beside the original: none of them holds anything
//!    the file does not hold now.
//! 4. Keep the original: write its bytes to the backups folder under a hidden
//!    name, sync, rename to `<stem>.<path key>.<UTC stamp>.pre-v2.stalx`, sync
//!    the folder. The Recovery dialog only ever sees a complete copy that
//!    needs no sidecar. When a copy of this project with the same bytes is
//!    already kept (the original restored from Recovery, then saved again),
//!    nothing is written: Recovery lists that one.
//! 5. Write version 2 (`write_atomic`), guarded by what the original holds now.
//!
//! A step that fails leaves the original whole (settled, or as it was), and a
//! write that fails removes the copy step 4 kept (never one an earlier upgrade
//! kept) while the original still holds the same bytes, so a save retried on
//! a read-only folder does not fill the backups folder. Settling replaces the file by a rename, so it needs what
//! the version 2 write needs (a writable folder) and nothing more: on Unix a
//! write-protected legacy file upgrades as an ordinary one does and keeps its
//! permission bits. On Windows a rename cannot replace a read-only file, so
//! the upgrade of one is refused like its version 2 write, and nothing is
//! left beside it (the staged copy is never made read-only).

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::persistence::legacy_settle;
use crate::persistence::stalx_v2::{
    self, check_unchanged, write_atomic, Fingerprint, Format, Guard, Stage, WriteError,
    MAX_PROJECT_FILE_BYTES,
};

use super::project_restore::sync_dir;
use super::speleodb_sidecars::{has_hot_journal, sidecar_path};

/// Every sidecar SQLite reads or writes beside a database.
const SIDECARS: [&str; 3] = ["-journal", "-wal", "-shm"];

/// Why an upgrade did not happen. The project's path still holds the survey it
/// held (see [`UpgradeFailure::settled`] for its bytes).
#[derive(Debug)]
pub(super) enum UpgradeError {
    /// The write, or the settled copy's rename, failed or was refused by the
    /// guard (`ExternallyModified`, `Exists`), as `write_atomic` reports it.
    Write(WriteError),
    /// A sidecar beside the original could not be removed.
    Sidecar {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The unfinished changes beside the original could not be folded in.
    Unfinished(String),
    /// The original could not be kept in the backups folder.
    Backup(String),
}

impl fmt::Display for UpgradeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(e) => write!(f, "{e}"),
            Self::Sidecar { path, error } => write!(
                f,
                "could not remove {} before the upgrade: {error}",
                path.display()
            ),
            Self::Unfinished(e) => write!(
                f,
                "the original's unfinished changes could not be finished before the upgrade \
                 ({e})"
            ),
            Self::Backup(e) => write!(
                f,
                "the original could not be backed up before the upgrade ({e})"
            ),
        }
    }
}

/// A failed [`upgrade`].
#[derive(Debug)]
pub(super) struct UpgradeFailure {
    pub(super) error: UpgradeError,
    /// The original was settled before the failure: its bytes changed (its
    /// survey did not), and this is what it holds now, so the next write's
    /// guard must expect it.
    pub(super) settled: Option<Fingerprint>,
}

impl UpgradeFailure {
    const fn before_any_write(error: UpgradeError) -> Self {
        Self {
            error,
            settled: None,
        }
    }
}

/// Write `bytes` (a version 2 file) over the legacy original at `original`,
/// keeping the original in the backups folder first; see the module docs for
/// the steps. `guard` is the write's: `Expect` the bytes the open read,
/// `Overwrite` what is there. Returns the fingerprint of what was written.
pub(super) fn upgrade(
    original: &Path,
    bytes: &[u8],
    guard: Guard<'_>,
) -> Result<Fingerprint, UpgradeFailure> {
    // 1. What is there, checked against the guard before anything is written.
    let current = match Fingerprint::read(original, MAX_PROJECT_FILE_BYTES) {
        Ok(read) => Some(read),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(UpgradeFailure::before_any_write(UpgradeError::Write(
                WriteError::Io {
                    stage: Stage::Guard,
                    path: original.to_path_buf(),
                    error,
                },
            )))
        }
    };
    let refused = match (guard, &current) {
        (Guard::Expect(expected), Some((_, found))) if found.same_content(expected) => None,
        (Guard::Expect(_), found) => Some(WriteError::ExternallyModified {
            path: original.to_path_buf(),
            found_len: found.as_ref().map(|(_, fp)| fp.len),
        }),
        (Guard::Absent, Some(_)) => Some(WriteError::Exists {
            path: original.to_path_buf(),
        }),
        (Guard::Absent, None) | (Guard::Overwrite, _) => None,
    };
    if let Some(refused) = refused {
        return Err(UpgradeFailure::before_any_write(UpgradeError::Write(
            refused,
        )));
    }
    remove_leftovers(original);

    // 2. Make the original whole.
    let mut settled = None;
    let held = match current {
        Some((main, found))
            if stalx_v2::sniff(&main) == Format::Legacy && unfinished(original).is_some() =>
        {
            let whole = settled_bytes(&main, original)
                .map_err(|e| UpgradeFailure::before_any_write(UpgradeError::Unfinished(e)))?;
            drop(main);
            kill_point(Step::Settled);
            let now = replace_original(original, &whole, &found)
                .map_err(|e| UpgradeFailure::before_any_write(UpgradeError::Write(e)))?;
            settled = Some(now.clone());
            Some((whole, now))
        }
        other => other,
    };
    let failed = |error| UpgradeFailure {
        error,
        settled: settled.clone(),
    };

    // 3. Nothing beside the name the JSON file takes.
    remove_sidecars(original).map_err(failed)?;

    // 4. Keep the original.
    let kept = match &held {
        Some((original_bytes, _)) => {
            Some(keep(original, original_bytes).map_err(|e| failed(UpgradeError::Backup(e)))?)
        }
        None => None,
    };

    // 5. Write.
    let expected = held.as_ref().map(|(_, fp)| fp);
    let write_guard = match (guard, expected) {
        (Guard::Overwrite, _) => Guard::Overwrite,
        (_, Some(fp)) => Guard::Expect(fp),
        (_, None) => Guard::Absent,
    };
    match write_atomic(original, bytes, write_guard) {
        Ok(written) => {
            tracing::info!(
                path = ?original,
                backup = ?kept.as_ref().map(Kept::path),
                settled = settled.is_some(),
                purpose = "upgrade a legacy file to version 2, keeping the original",
                "Upgraded a legacy project file to version 2"
            );
            Ok(written)
        }
        Err(e) => {
            if let (Some(Kept::New(kept)), Some(expected)) = (kept, expected) {
                discard_backup(&kept, original, expected);
            }
            Err(failed(UpgradeError::Write(e)))
        }
    }
}

/// The sidecar that holds part of `db`'s committed state, if any: an armed
/// rollback journal, or a WAL with frames (SQLite reads a WAL beside any
/// non-empty database, whatever its header says).
pub(super) fn unfinished(db: &Path) -> Option<PathBuf> {
    if has_hot_journal(db) {
        return Some(sidecar_path(db, "-journal"));
    }
    let wal = sidecar_path(db, "-wal");
    std::fs::metadata(&wal)
        .is_ok_and(|m| m.len() > 0)
        .then_some(wal)
}

/// What the SQLite file `main` (read from `beside`) holds once the unfinished
/// changes in the sidecars beside `beside` are folded in, worked out on a
/// private copy as the frozen reader's open does. `beside` and its sidecars
/// are only read.
pub(super) fn settled_bytes(main: &[u8], beside: &Path) -> Result<Vec<u8>, String> {
    let dir = tempfile::Builder::new()
        .prefix("stalx-settle-")
        .tempdir()
        .map_err(|e| format!("could not create a private folder: {e}"))?;
    let copy = dir.path().join("project.stalx");
    std::fs::write(&copy, main).map_err(|e| format!("could not copy the file: {e}"))?;
    for suffix in legacy_settle::SIDECARS {
        let from = sidecar_path(beside, suffix);
        if from.is_file() {
            std::fs::copy(&from, sidecar_path(&copy, suffix))
                .map_err(|e| format!("could not copy {}: {e}", from.display()))?;
        }
    }
    legacy_settle::settle(&copy).map_err(|e| format!("{e:#}"))?;
    std::fs::read(&copy).map_err(|e| format!("could not read the settled copy: {e}"))
}

/// Step 2: put `whole` (the settled original) in place of `original`, which
/// must still hold `expected`. Returns what `original` holds now.
fn replace_original(
    original: &Path,
    whole: &[u8],
    expected: &Fingerprint,
) -> Result<Fingerprint, WriteError> {
    let staged = hidden_sibling(original, "settled");
    let io = |stage: Stage, path: &Path| {
        let path = path.to_path_buf();
        move |error: std::io::Error| WriteError::Io { stage, path, error }
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(io(Stage::CreateTemp, &staged))?;
    let installed = file
        .write_all(whole)
        .map_err(io(Stage::WriteTemp, &staged))
        .and_then(|()| {
            inherit_mode(&file, original);
            file.sync_all().map_err(io(Stage::SyncTemp, &staged))
        })
        .and_then(|()| {
            drop(file);
            kill_point(Step::Staged);
            check_unchanged(original, expected)
        })
        .and_then(|()| std::fs::rename(&staged, original).map_err(io(Stage::Rename, original)));
    if let Err(e) = installed {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    kill_point(Step::Replaced);
    sync_dir(original);
    let modified = std::fs::metadata(original).and_then(|m| m.modified()).ok();
    Ok(Fingerprint::of(whole, modified))
}

/// Give the staged settled copy the original's Unix permission bits, set
/// through the handle that writes, as `write_atomic` does for its temp. Unix
/// only: on Windows the only bit is read-only, a rename cannot replace a
/// read-only file anyway, and a read-only staged copy could not be removed
/// after that refused rename (review finding D-migration-upgrade-12).
#[cfg(unix)]
fn inherit_mode(staged: &std::fs::File, original: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(metadata) = std::fs::metadata(original) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o777;
    if let Err(e) = staged.set_permissions(std::fs::Permissions::from_mode(mode)) {
        tracing::warn!(error = %e, "Could not carry the original's permissions onto its settled copy");
    }
}

#[cfg(not(unix))]
const fn inherit_mode(_staged: &std::fs::File, _original: &Path) {}

/// Step 3: remove every sidecar beside `original`, durably.
fn remove_sidecars(original: &Path) -> Result<(), UpgradeError> {
    let mut removed = false;
    for suffix in SIDECARS {
        let sidecar = sidecar_path(original, suffix);
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {
                removed = true;
                kill_point(Step::SidecarRemoved);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(UpgradeError::Sidecar {
                    path: sidecar,
                    error,
                })
            }
        }
    }
    if removed {
        sync_dir(original);
    }
    Ok(())
}

/// Step 4: keep `bytes` (the original, whole) in the backups folder as
/// `<stem>.<path key>.<UTC stamp>.pre-v2.stalx`
/// ([`super::project_io::upgrade_backup_prefix`]). Returns where.
///
/// When a copy of this project with the same bytes is already kept (an
/// original restored from Recovery and saved again), nothing is written: a
/// second identical copy would only be a second identical row in Recovery.
/// That copy is returned as [`Kept::Already`], which a failed write never
/// removes.
fn keep(original: &Path, bytes: &[u8]) -> Result<Kept, String> {
    let dir = super::project_io::upgrade_backup_dir()
        .ok_or_else(|| "the app data folder is not available".to_owned())?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    if let Some(same) = already_kept(&dir, original, bytes) {
        tracing::info!(
            backup = ?same,
            purpose = "keep the original of a legacy file its first version 2 write replaces",
            "The legacy original is already kept with the same bytes; no second copy"
        );
        return Ok(Kept::Already(same));
    }
    let prefix = super::project_io::upgrade_backup_prefix(original);
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ");
    let name = format!("{prefix}{stamp}.pre-v2.stalx");
    let kept = dir.join(&name);
    // Hidden, and not ending in `.pre-v2.stalx`: never listed by Recovery.
    let staged = dir.join(format!(".{name}.partial"));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .map_err(|e| format!("could not write {}: {e}", staged.display()))
        .and_then(|()| {
            kill_point(Step::BackupStaged);
            if std::fs::symlink_metadata(&kept).is_ok() {
                return Err(format!("{} already exists", kept.display()));
            }
            std::fs::rename(&staged, &kept)
                .map_err(|e| format!("could not name {}: {e}", kept.display()))
        });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    sync_dir(&kept);
    kill_point(Step::BackupKept);
    tracing::info!(
        backup = ?kept,
        purpose = "keep the original of a legacy file its first version 2 write replaces",
        "Kept the legacy original before writing version 2"
    );
    Ok(Kept::New(kept))
}

/// Where step 4 left the original.
#[derive(Debug)]
enum Kept {
    /// A copy this upgrade wrote: redundant, and removed, if the write fails.
    New(PathBuf),
    /// A copy with the same bytes an earlier upgrade kept: never removed here.
    Already(PathBuf),
}

impl Kept {
    fn path(&self) -> &Path {
        match self {
            Self::New(path) | Self::Already(path) => path,
        }
    }
}

/// A kept original of the project at `original` in `dir` whose bytes are
/// `bytes`, if any. Only files of the same length are read.
fn already_kept(dir: &Path, original: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let len = u64::try_from(bytes.len()).ok()?;
    super::project_restore::kept_originals_in(dir, original)
        .into_iter()
        .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.len() == len))
        .find(|path| std::fs::read(path).is_ok_and(|kept| kept == bytes))
}

/// The write failed: the copy step 4 kept is redundant while the original
/// still holds the same bytes (a failed write leaves it untouched). Kept when
/// the original changed meanwhile: then it is the only copy of what was there.
fn discard_backup(kept: &Path, original: &Path, expected: &Fingerprint) {
    if check_unchanged(original, expected).is_err() {
        tracing::warn!(backup = ?kept, "The upgrade failed and the original changed meanwhile; its backup is kept");
        return;
    }
    if let Err(e) = std::fs::remove_file(kept) {
        tracing::warn!(error = %e, backup = ?kept, "Could not remove the backup of a failed upgrade");
    }
}

/// Remove what an upgrade of `original` killed earlier left behind: a staged
/// settled copy beside it ([`hidden_sibling`]) and a partial backup in the
/// backups folder ([`keep`]). Neither is ever needed: at every step the
/// original under its own name is whole. Best effort.
fn remove_leftovers(original: &Path) {
    let Some(name) = original.file_name() else {
        return;
    };
    let staged_prefix = format!(".{}.settled-", name.to_string_lossy());
    let is_ours = |rest: &str| {
        rest.split_once('-').is_some_and(|(pid, n)| {
            [pid, n]
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
    };
    let partial_prefix = format!(".{}", super::project_io::upgrade_backup_prefix(original));
    let folder = original
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staged = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix(&staged_prefix)
                .is_some_and(is_ours)
        });
    let partial = super::project_io::upgrade_backup_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix(&partial_prefix)
                .and_then(|rest| rest.strip_suffix(".pre-v2.stalx.partial"))
                .is_some_and(super::project_restore::is_upgrade_stamp)
        });
    for entry in staged.chain(partial) {
        let path = entry.path();
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(path = ?path, "Removed what an interrupted upgrade left behind");
            }
            Err(e) => {
                tracing::debug!(error = %e, path = ?path, "Could not remove what an interrupted upgrade left behind");
            }
        }
    }
}

/// A name beside `path` that no other file uses, hidden, and that the
/// Recovery listing never offers (it does not start with the project's name).
fn hidden_sibling(path: &Path, what: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path
        .file_name()
        .map_or_else(|| "project".into(), |n| n.to_string_lossy().into_owned());
    path.with_file_name(format!(
        ".{name}.{what}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

/// The points between the upgrade's steps where a test kills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    /// The settled copy is in the private folder; nothing beside the project changed.
    Settled,
    /// The settled copy is staged and synced beside the original.
    Staged,
    /// The settled copy replaced the original; the old sidecars are still there.
    Replaced,
    /// One sidecar beside the original was removed.
    SidecarRemoved,
    /// The backup is written and synced under its hidden name.
    BackupStaged,
    /// The backup is under its listed name.
    BackupKept,
}

#[cfg(test)]
impl Step {
    pub(super) const ALL: [Self; 6] = [
        Self::Settled,
        Self::Staged,
        Self::Replaced,
        Self::SidecarRemoved,
        Self::BackupStaged,
        Self::BackupKept,
    ];
}

#[cfg(test)]
thread_local! {
    /// The step after which the next upgrade on this thread is killed (tests).
    pub(super) static KILL_AT: std::cell::Cell<Option<Step>> = const { std::cell::Cell::new(None) };
}

/// A kill injected by a test: the unwind skips every cleanup this module would
/// run on an error, so the files are left exactly as a kill leaves them.
#[cfg(test)]
#[expect(
    clippy::panic,
    clippy::manual_assert,
    reason = "the injected kill: unwinding past every cleanup is what a kill does"
)]
fn kill_point(step: Step) {
    if KILL_AT.with(std::cell::Cell::get) == Some(step) {
        panic!("injected kill after {step:?}");
    }
}

#[cfg(not(test))]
const fn kill_point(_: Step) {}
