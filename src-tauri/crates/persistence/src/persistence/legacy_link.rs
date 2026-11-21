//! The SpeleoDB link row of a legacy (SQLite) `.stalx`, read without opening
//! the file as a project (plan 6.1, Deviation F13).
//!
//! Every link reader in the app (the foreign-copy probe, logout's
//! forget-recents, the cached-copies and local-state listing, the access
//! probe, the revision undo-mode probe) reads a version 2 file through
//! [`super::stalx_v2::probe_path`]. That probe answers only `Format::Legacy`
//! for an old SQLite file, and the full legacy open (`migrate`) is far too
//! slow to paint a list with: it copies the file, checks its integrity, runs
//! the migration chain, loads and heals the survey and lays it out. This
//! reads the one row those readers need, the way 0.6.0's metadata open did:
//! - no migration, no integrity check, no project tables: one `SELECT` on
//!   `speleo_link`, whose narrower pre-v10 / pre-v11 / pre-#303 shapes are
//!   read with the missing columns as NULL;
//! - the frozen 0.6.0 schema guard ([`stalagmight_legacy::persistence::check_schema`])
//!   before anything else, so a crafted file cannot make the read run SQL it
//!   chose (views, triggers, virtual tables, expression defaults);
//! - read-only unless [`ReadOptions::recover`] asks for the one write SQLite
//!   makes on its own: rolling back a hot journal left by a kill or a power
//!   loss (a read-only connection fails with `SQLITE_READONLY_ROLLBACK`);
//! - optional caps for an untrusted download (a server revision): the longest
//!   value SQLite may produce and a deadline after which any statement is
//!   interrupted.
//!
//!
//! [`read_cached_copy`] reads the view toggles beside the link, for the
//! SpeleoDB cache's own reader (`read_cache_file`), on a copy at the frozen
//! reader's schema version only.
//!
//! Built on rusqlite and the frozen legacy crate only, so it outlives the
//! app's own SQLite layer (`persistence::sqlite`).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use rusqlite::config::DbConfig;
use rusqlite::limits::Limit;
use rusqlite::{Connection, OpenFlags};

use crate::models::{SpeleoLink, UndoMode};

/// How [`read_link`] opens the file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadOptions {
    /// Open read-write, so SQLite rolls a hot rollback journal back before the
    /// read. That rollback is the only write: no migration, no other change.
    pub recover: bool,
    /// The longest string or blob SQLite may produce (`SQLITE_LIMIT_LENGTH`),
    /// for a file from outside the app. `None` keeps SQLite's default.
    pub max_value_bytes: Option<i32>,
    /// Interrupt any statement still running this long after the open.
    pub deadline: Option<Duration>,
}

/// The `speleo_link` row of the legacy `.stalx` at `path`: `Ok(None)` when the
/// file has no link table or no row (an unmanaged project, or one older than
/// schema v9). An error means the file could not be read at all (not a SQLite
/// database, a refused schema, a hot journal on a read-only open, a lock, a
/// cap or the deadline): nothing is known about its link, and in particular
/// NOT that it has none.
pub fn read_link(path: &Path, options: &ReadOptions) -> Result<Option<SpeleoLink>> {
    let (conn, started) = open_guarded(path, options)?;
    let link = load_link(&conn)?;
    within_deadline(started, options)?;
    Ok(link)
}

/// What a legacy SpeleoDB cache copy holds for this machine: its link row and
/// its per-machine view toggles (the `display_settings` table).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CachedCopy {
    pub link: Option<SpeleoLink>,
    pub display: BTreeMap<String, String>,
}

/// The link row and the view toggles of a legacy cache copy written at the
/// frozen reader's own schema version (a copy 0.6.0 left), read like
/// [`read_link`]: the schema guard first, then the two tables, nothing copied,
/// healed, migrated or solved (review G-speleodb-6: the full open, run just to
/// read a few rows, stalled the first pull of a large project).
///
/// A file at any other schema version is refused: the frozen open would run the
/// migration chain over it, which renames and adds view-toggle keys on the way
/// (v12, v13), so only that open answers for it. The caller falls back to it on
/// any error, a hot journal on a read-only read included.
pub fn read_cached_copy(path: &Path, options: &ReadOptions) -> Result<CachedCopy> {
    let (conn, started) = open_guarded(path, options)?;
    let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != stalagmight_legacy::SUPPORTED_SCHEMA_VERSION {
        anyhow::bail!(
            "schema version {version} is migrated on open; only the full open reads its view toggles"
        );
    }
    let copy = CachedCopy {
        link: load_link(&conn)?,
        display: load_display(&conn)?,
    };
    within_deadline(started, options)?;
    Ok(copy)
}

/// Open `path` as [`ReadOptions`] says, with the caps applied and the frozen
/// schema guard passed. Returns the connection and when the read started.
fn open_guarded(path: &Path, options: &ReadOptions) -> Result<(Connection, Instant)> {
    if !path.exists() {
        anyhow::bail!("File does not exist: {}", path.display());
    }
    let flags = if options.recover {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let started = Instant::now();
    let mut conn = Connection::open_with_flags(path, flags)
        .context("Failed to open the legacy project to read its SpeleoDB link")?;
    harden(&mut conn, started, options)?;
    stalagmight_legacy::persistence::check_schema(&conn)?;
    Ok((conn, started))
}

/// The progress handler interrupts a long statement; a read made of short ones
/// is checked once at the end, as 0.6.0's untrusted open was.
fn within_deadline(started: Instant, options: &ReadOptions) -> Result<()> {
    if options
        .deadline
        .is_some_and(|deadline| started.elapsed() >= deadline)
    {
        anyhow::bail!("The SpeleoDB link read ran past its deadline");
    }
    Ok(())
}

/// Apply the caps an untrusted file needs before the first read.
fn harden(conn: &mut Connection, started: Instant, options: &ReadOptions) -> Result<()> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    if let Some(max) = options.max_value_bytes {
        conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, max)?;
    }
    if let Some(deadline) = options.deadline {
        let expires = started + deadline;
        conn.progress_handler(1_000, Some(move || Instant::now() > expires))?;
        // A crafted page is read through the checked path, not a mapping.
        conn.pragma_update(None, "mmap_size", 0i64)?;
        conn.pragma_update(None, "cell_size_check", true)?;
    }
    Ok(())
}

/// The `display_settings` rows, as 0.6.0's `load_display_settings` reads them;
/// none when the table is absent.
fn load_display(conn: &Connection) -> Result<BTreeMap<String, String>> {
    let table_exists = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'display_settings'",
        [],
        |row| row.get::<_, i64>(0),
    )? > 0;
    if !table_exists {
        return Ok(BTreeMap::new());
    }
    let mut statement = conn.prepare("SELECT key, value FROM display_settings")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    Ok(rows)
}

/// The row itself. The table can be narrower than 0.6.0's (a file last written
/// at schema v9 or v10 has no `last_edit_at` / `last_verify_at`, one older
/// than #303 no `undo_mode`): the missing columns read as NULL, the value a
/// migrated-but-never-stamped row carries.
fn load_link(conn: &Connection) -> Result<Option<SpeleoLink>> {
    let table_exists = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'speleo_link'",
        [],
        |row| row.get::<_, i64>(0),
    )? > 0;
    if !table_exists {
        return Ok(None);
    }
    let mut statement = conn.prepare("SELECT name FROM pragma_table_info('speleo_link')")?;
    let columns: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let column = |name: &'static str| {
        if columns.iter().any(|c| c == name) {
            name
        } else {
            "NULL"
        }
    };
    let sql = format!(
        "SELECT endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits, \
         {}, {}, {} FROM speleo_link WHERE id = 1",
        column("last_edit_at"),
        column("last_verify_at"),
        column("undo_mode"),
    );
    let row = conn.query_row(&sql, [], |row| {
        Ok(SpeleoLink {
            endpoint: row.get(0)?,
            project_id: row.get(1)?,
            project_name: row.get(2)?,
            base_commit_sha: row.get(3)?,
            has_unpushed_edits: row.get::<_, i64>(4)? != 0,
            last_edit_at: row.get(5)?,
            last_verify_at: row.get(6)?,
            // An unknown spelling reads as 0.6.0 read it (`from_db_str`).
            undo_mode: row
                .get::<_, Option<String>>(7)?
                .as_deref()
                .map(UndoMode::from_db_str),
        })
    });
    match row {
        Ok(link) => Ok(Some(link)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
