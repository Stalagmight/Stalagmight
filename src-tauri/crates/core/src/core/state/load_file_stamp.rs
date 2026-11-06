//! [#576 FZ-FZ-INT-R2] Which file a load read, compared with the file at its path
//! when the load commits.
//!
//! A load opens its database handle long before it commits (the read and solve run
//! with no lock held), and a save renames a freshly written file over its path. A
//! save of the same file landing in between - another session of it saving, then
//! closing - left the load holding a handle on the DELETED file, and the commit
//! installed that handle: every later write of the session (a display setting, an
//! ignored warning) went into the deleted file (SQLite refuses some of them as
//! "readonly database moved"), and the session's next save rebuilt the file from
//! there, dropping what the file at the path held. Such a load is now refused at
//! its commit, under `command_lock` (every save holds it, so the file at the path
//! cannot change while the commit decides), with the retryable
//! `FILE_REPLACED_DURING_LOAD`; the frontend repeats the load once.
//!
//! What "the same file" means:
//! - unix: the device and inode the path names (`stat`). A rename over the path
//!   always changes them, and the replaced file cannot hand its inode number to the
//!   new one while the load's handle still holds it open.
//! - elsewhere (Windows): the save epoch, [`AppState::note_save_renamed`], bumped by
//!   every save that renamed a file into place this session. Windows has no stable
//!   std accessor for the file index (`GetFileInformationByHandle` needs `unsafe`,
//!   which the workspace denies, or a new dependency), and a save-generation row in
//!   the file would change the file format. The epoch is process-wide, so a save of
//!   ANY file while a load runs refuses that load once (the retry reads the file
//!   then at the path). Windows also refuses a rename over a file a load holds open
//!   (SQLite opens it without `FILE_SHARE_DELETE`), so there the save fails before
//!   the load can be left on a replaced file; the epoch covers what is left.
use std::path::Path;
use std::sync::atomic::Ordering;

use super::AppState;

/// The identity of the file at a path, as a load's commit compares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileStamp {
    /// `(dev, ino)` of the file the path names; `None` when it could not be read,
    /// which compares equal to itself (an unreadable path refuses nothing here; the
    /// load's own open fails on it).
    #[cfg(unix)]
    inode: Option<(u64, u64)>,
    /// [`AppState::saves_renamed`] when the stamp was taken.
    #[cfg(not(unix))]
    saves_renamed: u64,
}

impl AppState {
    /// The stamp of the file at `path` now. Takes no lock.
    pub(super) fn file_stamp(&self, path: &Path) -> FileStamp {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            FileStamp {
                inode: std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino())),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            FileStamp {
                saves_renamed: self.saves_renamed.load(Ordering::SeqCst),
            }
        }
    }

    /// A save renamed a file into place (over an open project's file, a Save As
    /// target or a detached copy's destination). Called right after the rename,
    /// still under the save's `command_lock`. Takes no lock.
    pub(super) fn note_save_renamed(&self) {
        self.saves_renamed.fetch_add(1, Ordering::SeqCst);
    }

    /// [`Self::note_save_renamed`]'s count so far.
    #[cfg(test)]
    pub(super) fn saves_renamed_so_far(&self) -> u64 {
        self.saves_renamed.load(Ordering::SeqCst)
    }
}
