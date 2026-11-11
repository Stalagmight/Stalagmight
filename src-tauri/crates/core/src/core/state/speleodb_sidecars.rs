//! SQLite's sidecar files beside a cached `.stalx`, and the rule every cache
//! install follows for them.
//!
//! A SQLite database is not one file. Beside `project.stalx` the library keeps a
//! rollback journal (`project.stalx-journal`) for the length of every write
//! transaction, and in WAL mode (which no `.stalx` this app writes uses, but a
//! downloaded file may) a write-ahead log and its index (`-wal`, `-shm`). SQLite
//! finds them BY NAME and never checks that they belong to the file beside them.
//! So a sidecar left by a kill or a power loss mid-save outlives the database it
//! was written for: rename a fresh download over `project.stalx` and the next
//! read-write open rolls the old file's pages into the new one. The result is a
//! corrupt cache ("Page 35: never used") or, worse, a silently mixed file that
//! is linked and pushable.
//!
//! Hence the rule, applied at every rename into the cache (the session-bound
//! install, the unbound replace and the prepared reload): with no connection of
//! ours left on the destination, move every sidecar out of SQLite's sight and
//! make that durable BEFORE the new file appears under the name; delete them
//! once it has; put them back if it never does ([`SetAsideSidecars`]).
//!
//! What a sidecar may HOLD decides who may throw it away:
//!
//! - A HOT rollback journal holds the before-images of the pages an unfinished
//!   transaction had already overwritten, which makes it part of the copy's last
//!   COMMITTED state: without it the file is torn, with it a read-write open
//!   recovers exactly what was committed, un-pushed edits included. So a hot
//!   journal makes the local state UNKNOWN ([`has_hot_journal`], read by
//!   `speleodb_local_state`), the `Protect` guard refuses over it, and only a
//!   confirmed `Discard` (the user chose the server copy over this one) reaches
//!   an install that removes it.
//! - Everything else goes on every install: a journal that was never armed
//!   (empty, or a zero header), a WAL index, and a WAL. A WAL can hold committed
//!   frames too, but SQLite reads a WAL beside any non-empty database, so the
//!   guard's own read-only open of the copy either reads through it or fails
//!   closed: the guard has already decided on what it holds by the time an
//!   install moves it. And it must move: SQLite would apply its frames to the
//!   NEW file just as readily, whatever journal mode that file says it uses.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::speleodb::SpeleoDbError;

/// The first eight bytes of an armed rollback journal (`aJournalMagic` in
/// SQLite's `pager.c`). SQLite writes the header with this magic only once the
/// transaction is about to modify the database file itself; until then the
/// journal holds nothing a rollback would need.
const JOURNAL_MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];

/// Every sidecar SQLite looks for beside a database, by suffix.
const SIDECARS: [&str; 3] = ["-journal", "-wal", "-shm"];

/// Appended to a sidecar for the length of one install: a name SQLite never
/// opens, so the new file cannot pick the old file's sidecar up.
const SET_ASIDE: &str = ".set-aside";

/// `db`'s own file name with `suffix` appended, beside it.
pub(super) fn sidecar_path(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    db.with_file_name(name)
}

/// Whether the rollback journal beside `db` is armed, i.e. may hold committed
/// pages that a read-write open would roll back into `db`.
///
/// Mirrors SQLite's own test rather than relying on a read-only open to trip
/// over it (`SQLITE_READONLY_ROLLBACK`), so the guard's answer does not depend
/// on which connection flags a reader happens to use. Fails closed: a journal
/// that is there but cannot be read counts as armed.
pub(super) fn has_hot_journal(db: &Path) -> bool {
    let mut header = [0u8; JOURNAL_MAGIC.len()];
    match std::fs::File::open(sidecar_path(db, "-journal")) {
        Ok(mut journal) => match journal.read_exact(&mut header) {
            Ok(()) => header == JOURNAL_MAGIC,
            // Shorter than a header: nothing was ever armed in it.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => false,
            Err(_) => true,
        },
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// The sidecars of one cache file, moved out of SQLite's sight for the length of
/// one install. Exactly one of [`Self::discard`] (the new file is in place) or
/// [`Self::restore`] (the original file is still in place) must follow.
///
/// No `Drop` either way, deliberately: restoring by default would put an old
/// journal back beside a NEW file the moment a caller forgot to discard, which
/// is the very corruption this exists to prevent, and discarding by default
/// would throw away the only recovery data a failed install leaves.
#[must_use = "discard after a successful install, restore after a failed one"]
pub(super) struct SetAsideSidecars {
    /// (name SQLite reads, name it is parked under), in the order they moved.
    moved: Vec<(PathBuf, PathBuf)>,
}

impl SetAsideSidecars {
    /// Move every sidecar of `db` aside and make the move durable.
    ///
    /// The caller must hold NO connection to `db`: a live connection's journal is
    /// mid-transaction state, not debris. Every caller either closed the open
    /// project's handle (under the `project` guard) or established that `db` is
    /// not the open project.
    ///
    /// A sidecar that cannot be moved fails the install before anything is
    /// replaced (and puts back what had already moved): installing next to it is
    /// exactly the replay this prevents.
    pub(super) fn take(db: &Path) -> Result<Self, SpeleoDbError> {
        let mut set_aside = Self { moved: Vec::new() };
        for suffix in SIDECARS {
            let original = sidecar_path(db, suffix);
            let parked = sidecar_path(db, &format!("{suffix}{SET_ASIDE}"));
            match std::fs::rename(&original, &parked) {
                Ok(()) => set_aside.moved.push((original, parked)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    set_aside.restore();
                    return Err(SpeleoDbError::Io(format!(
                        "could not move {} aside before replacing the project: {e}",
                        original.display()
                    )));
                }
            }
        }
        if !set_aside.moved.is_empty() {
            tracing::info!(
                path = %db.display(),
                count = set_aside.moved.len(),
                purpose = "keep a stale SQLite sidecar from replaying into the installed copy",
                "speleodb: set aside the outgoing cache file's sidecars"
            );
            // Durable BEFORE the new file takes the name: after a crash the
            // directory must not show the new file beside the old journal.
            super::speleodb_cache_install::sync_cache_directory(db);
        }
        Ok(set_aside)
    }

    /// The original file is still at its name (the install failed before or
    /// while replacing it, or put it back): return its sidecars to it, so a
    /// later open recovers exactly what it would have before.
    pub(super) fn restore(self) {
        for (original, parked) in self.moved.iter().rev() {
            if let Err(error) = std::fs::rename(parked, original) {
                tracing::error!(
                    %error,
                    path = %original.display(),
                    parked = %parked.display(),
                    "speleodb: could not put a set-aside SQLite sidecar back after a failed install"
                );
            }
        }
        if let Some((original, _)) = self.moved.first() {
            // Any sidecar names a file in the cache directory, which is what syncs.
            super::speleodb_cache_install::sync_cache_directory(original);
        }
    }

    /// The new file holds the name: the old file's sidecars describe nothing that
    /// exists any more. Best-effort - a leftover under the set-aside name is inert.
    pub(super) fn discard(self) {
        for (_, parked) in &self.moved {
            if let Err(error) = std::fs::remove_file(parked) {
                tracing::warn!(
                    %error,
                    path = %parked.display(),
                    "speleodb: could not delete a set-aside SQLite sidecar; it is inert"
                );
            }
        }
    }
}
