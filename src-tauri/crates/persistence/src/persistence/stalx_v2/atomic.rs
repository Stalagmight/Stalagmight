//! The durable save (plan 6.5): write `<file>.tmp` beside the destination,
//! `sync_all` it, check the external-modification guard, rename it over the
//! destination, fsync the parent directory. One data fsync and one directory
//! fsync. No handle on the destination is open across the rename (Windows
//! refuses to rename over an open file).
//!
//! The guard remembers the size, modification time and SHA-256 of the bytes
//! last read or written ([`Fingerprint`]); before the rename, a destination
//! whose content differs refuses the write so the app can ask (reload,
//! overwrite, save as).

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest as _, Sha256};

/// What the guard knows about a file's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub len: u64,
    /// The modification time, where the filesystem gives one. Informative:
    /// the content comparison uses the length and the hash.
    pub modified: Option<SystemTime>,
    pub sha256: [u8; 32],
}

impl Fingerprint {
    /// The fingerprint of `bytes` (as last read or written).
    pub fn of(bytes: &[u8], modified: Option<SystemTime>) -> Self {
        Self {
            len: bytes.len() as u64,
            modified,
            sha256: Sha256::digest(bytes).into(),
        }
    }

    /// Read a file and fingerprint it in one go (what an open does).
    pub fn read(path: &Path) -> std::io::Result<(Vec<u8>, Self)> {
        let bytes = std::fs::read(path)?;
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let fingerprint = Self::of(&bytes, modified);
        Ok((bytes, fingerprint))
    }

    /// Same content (length and hash), whatever the modification time says.
    pub fn same_content(&self, other: &Self) -> bool {
        self.len == other.len && self.sha256 == other.sha256
    }
}

/// What the destination must hold just before the rename.
#[derive(Debug, Clone, Copy)]
pub enum Guard<'a> {
    /// Replace whatever is there (an explicit overwrite).
    Overwrite,
    /// The bytes last read or written: anything else is an external change.
    Expect(&'a Fingerprint),
    /// Nothing: a first save must not replace a file that appeared meanwhile.
    Absent,
}

/// Why a save did not happen. The destination is unchanged in every case.
#[derive(Debug)]
pub enum WriteError {
    Io {
        stage: Stage,
        path: PathBuf,
        error: std::io::Error,
    },
    /// The file on disk is not the one last read or written.
    ExternallyModified {
        path: PathBuf,
        found: Option<Fingerprint>,
    },
    /// [`Guard::Absent`], but a file is there.
    Exists { path: PathBuf },
}

/// The step a save failed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    CreateTemp,
    WriteTemp,
    SyncTemp,
    Guard,
    Rename,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { stage, path, error } => write!(
                f,
                "could not save {} ({}): {error}",
                path.display(),
                match stage {
                    Stage::CreateTemp => "creating the temporary file",
                    Stage::WriteTemp => "writing the temporary file",
                    Stage::SyncTemp => "flushing the temporary file to disk",
                    Stage::Guard => "checking the file on disk",
                    Stage::Rename => "moving the new file into place",
                }
            ),
            Self::ExternallyModified { path, found } => match found {
                Some(_) => write!(
                    f,
                    "{} was changed by another program since it was opened",
                    path.display()
                ),
                None => write!(
                    f,
                    "{} was moved or deleted by another program since it was opened",
                    path.display()
                ),
            },
            Self::Exists { path } => write!(f, "{} already exists", path.display()),
        }
    }
}

impl std::error::Error for WriteError {}

/// `<dest>.tmp` (`cave.stalx` saves through `cave.stalx.tmp`, the name the
/// recovery listing looks for).
pub fn temp_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".tmp");
    dest.with_file_name(name)
}

/// Whether `path` still holds the content `expected` describes.
pub fn check_unchanged(path: &Path, expected: &Fingerprint) -> Result<(), WriteError> {
    let found = match Fingerprint::read(path) {
        Ok((_, found)) => found,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(WriteError::ExternallyModified {
                path: path.to_path_buf(),
                found: None,
            })
        }
        Err(error) => {
            return Err(WriteError::Io {
                stage: Stage::Guard,
                path: path.to_path_buf(),
                error,
            })
        }
    };
    if found.same_content(expected) {
        Ok(())
    } else {
        Err(WriteError::ExternallyModified {
            path: path.to_path_buf(),
            found: Some(found),
        })
    }
}

/// Write `bytes` to `dest` durably and atomically: afterwards `dest` holds
/// either its old content or `bytes`, never a mix. Returns the new
/// fingerprint for the next save's guard.
pub fn write_atomic(
    dest: &Path,
    bytes: &[u8],
    guard: Guard<'_>,
) -> Result<Fingerprint, WriteError> {
    let temp = temp_path(dest);
    let io = |stage: Stage, path: &Path| {
        let path = path.to_path_buf();
        move |error: std::io::Error| WriteError::Io { stage, path, error }
    };
    // A leftover from an interrupted save (or anything planted there): the
    // name is ours. `remove_file` removes a symlink itself, never its target.
    if std::fs::symlink_metadata(&temp).is_ok() {
        std::fs::remove_file(&temp).map_err(io(Stage::CreateTemp, &temp))?;
    }
    failpoint(Stage::CreateTemp).map_err(io(Stage::CreateTemp, &temp))?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(io(Stage::CreateTemp, &temp))?;
    #[cfg(unix)]
    inherit_mode(&file, dest);
    let staged = fill(file, bytes).and_then(|()| check_guard(dest, guard));
    if let Err(error) = staged {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = failpoint(Stage::Rename).and_then(|()| std::fs::rename(&temp, dest)) {
        let _ = std::fs::remove_file(&temp);
        return Err(WriteError::Io {
            stage: Stage::Rename,
            path: dest.to_path_buf(),
            error,
        });
    }
    sync_parent(dest);
    let modified = std::fs::metadata(dest).and_then(|m| m.modified()).ok();
    Ok(Fingerprint::of(bytes, modified))
}

fn fill(mut file: File, bytes: &[u8]) -> Result<(), WriteError> {
    let temp = PathBuf::new();
    failpoint(Stage::WriteTemp)
        .and_then(|()| file.write_all(bytes))
        .map_err(|error| WriteError::Io {
            stage: Stage::WriteTemp,
            path: temp.clone(),
            error,
        })?;
    failpoint(Stage::SyncTemp)
        .and_then(|()| file.sync_all())
        .map_err(|error| WriteError::Io {
            stage: Stage::SyncTemp,
            path: temp,
            error,
        })
    // `file` is closed here, before the rename (Windows).
}

fn check_guard(dest: &Path, guard: Guard<'_>) -> Result<(), WriteError> {
    failpoint(Stage::Guard).map_err(|error| WriteError::Io {
        stage: Stage::Guard,
        path: dest.to_path_buf(),
        error,
    })?;
    match guard {
        Guard::Overwrite => Ok(()),
        Guard::Expect(expected) => check_unchanged(dest, expected),
        Guard::Absent => {
            if std::fs::symlink_metadata(dest).is_ok() {
                Err(WriteError::Exists {
                    path: dest.to_path_buf(),
                })
            } else {
                Ok(())
            }
        }
    }
}

/// Make the rename durable: fsync the directory that holds `dest`. Unix only
/// (a directory cannot be opened for syncing on Windows, where the rename is
/// journaled by NTFS). A failure is logged, not returned: the new file is
/// already in place.
fn sync_parent(dest: &Path) {
    #[cfg(unix)]
    {
        let parent = match dest.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        if let Err(error) = File::open(parent).and_then(|dir| dir.sync_all()) {
            tracing::warn!(
                error = %error,
                "Saved the project, but could not flush its folder to disk"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = dest;
}

/// Give the temp the permissions of the file it replaces (a fresh file is
/// `0o666 & !umask`; a user's `chmod 600` must survive a save).
#[cfg(unix)]
fn inherit_mode(temp: &File, dest: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(metadata) = std::fs::metadata(dest) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o777;
    if let Err(error) = temp.set_permissions(std::fs::Permissions::from_mode(mode)) {
        tracing::warn!(error = %error, "Could not carry the project file's permissions onto its replacement");
    }
}

#[cfg(test)]
thread_local! {
    /// The stage the next save on this thread fails at (tests only).
    pub(super) static FAIL_AT: std::cell::Cell<Option<Stage>> = const { std::cell::Cell::new(None) };
}

/// An injected I/O failure at `stage` (tests only; always `Ok` otherwise).
#[cfg_attr(
    not(test),
    expect(
        clippy::unnecessary_wraps,
        reason = "the test build injects failures through the same signature"
    )
)]
fn failpoint(stage: Stage) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_AT.with(std::cell::Cell::get) == Some(stage) {
        return Err(std::io::Error::other(format!(
            "injected failure at {stage:?}"
        )));
    }
    let _ = stage;
    Ok(())
}
