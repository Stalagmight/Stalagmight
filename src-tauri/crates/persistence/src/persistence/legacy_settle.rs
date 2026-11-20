//! Folding what SQLite left unfinished beside a legacy (SQLite) `.stalx` into
//! the file itself, on a copy the caller owns.
//!
//! A SQLite database is not always one file. An old build killed mid-save
//! leaves its file and a HOT `-journal`: the journal holds the before-images of
//! the pages the interrupted transaction had already written into the file, so
//! the file alone is that half-written transaction, and only the pair is the
//! survey 0.6.0 had committed. A file some other tool left in WAL mode keeps
//! its newest committed transactions in its `-wal` until a checkpoint, so the
//! file alone is an older survey. SQLite finds both by NAME and finishes them on
//! the next read-write open: it rolls a hot journal back into the file, and it
//! reads through a WAL (and checkpoints it when the last connection closes).
//!
//! [`settle`] makes SQLite do exactly that and nothing else (no migration, no
//! other write), so afterwards the file ALONE is what the pair was. It is the
//! frozen reader's own rule (`stalagmight_legacy::open_path` copies the file
//! with its [`SIDECARS`] and opens the copy read-write), so a settled copy is
//! the survey the app's open showed. Run it only on a copy: SQLite writes into
//! the file it settles.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags};

/// The sidecars that can hold part of a database's committed state, as the
/// frozen reader copies them: a `-journal` (rolled back) and a `-wal` (read
/// through). The `-shm` is an index SQLite rebuilds; it is never copied.
pub const SIDECARS: [&str; 2] = ["-journal", "-wal"];

/// `db`'s file name with `suffix` appended, beside it (where SQLite looks).
pub fn sidecar_of(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Fold the hot journal or the WAL beside the SQLite file `copy` into it,
/// then remove every sidecar SQLite left beside it (`-journal`, `-wal`,
/// `-shm`). A clean file is read and left as it was.
///
/// An error leaves `copy` and its sidecars in a state SQLite can still finish
/// (a rollback and a checkpoint are crash safe and can run again), but the
/// caller should treat the copy as unusable.
pub fn settle(copy: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(
        copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("could not open the copy read-write")?;
    // Nothing here runs SQL the file chose; the hardening is for good measure.
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    // The first read takes a shared lock, and SQLite rolls a hot journal back
    // into the file before it lets the read see anything.
    conn.query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
        .context("could not roll the unfinished write back")?;
    // A WAL's committed frames go into the file; TRUNCATE also empties the log.
    // On a file that is not in WAL mode this is a no-op that answers (0, -1, -1).
    let blocked: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .context("could not checkpoint the write-ahead log")?;
    if blocked != 0 {
        anyhow::bail!("the write-ahead log could not be checkpointed completely");
    }
    conn.close()
        .map_err(|(_, e)| e)
        .context("could not close the copy")?;
    for suffix in ["-journal", "-wal", "-shm"] {
        match std::fs::remove_file(sidecar_of(copy, suffix)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!(
                    "could not remove the copy's {suffix} after settling it"
                )))
            }
        }
    }
    Ok(())
}
