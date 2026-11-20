use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use super::Database;
#[cfg(feature = "writer")]
use super::SCHEMA_VERSION;

impl Database {
    /// Open or create a database at the given path
    #[cfg(feature = "writer")]
    pub fn open(path: &Path) -> Result<Self> {
        // Per-open chatter is debug-only - `open` is called for every
        // project create / save-as / import / recent-list refresh.
        tracing::debug!("Opening database at: {:?}", path);

        // Whether the file already existed decides fresh-vs-migrate below, so
        // capture it BEFORE Connection::open creates it for a new path. This is a
        // more robust discriminator than user_version/row-count: an existing file
        // that predates the user_version pragma (stamped 0) or that has no project
        // row yet must still be MIGRATED, not treated as fresh.
        let file_existed = path.exists();

        let conn = Connection::open(path).context("Failed to open database")?;
        if file_existed {
            // An existing file may come from anywhere; refuse a hostile schema
            // before init_schema or a migration touches it (#608), and a newer
            // one before init_schema writes this build's tables into it (#649).
            super::check_schema(&conn)?;
            super::newer_schema::check_not_newer(&conn)?;
        }

        let mut db = Self { conn };
        db.enable_foreign_keys()?;

        let prior_version = db.get_user_version().unwrap_or(0);
        db.init_schema()?;

        // A brand-new file (did not exist until Connection::open created it) was
        // just built at the CURRENT schema by init_schema (CREATE-IF-NOT-EXISTS),
        // so stamp it directly -- cheap, and the hot path for every save/import
        // temp file. An EXISTING file may predate a column-adding migration:
        // init_schema cannot ALTER an existing table, so it must run the real
        // migration chain so e.g. the v9->v10 speleo_link.last_edit_at ALTER
        // actually happens BEFORE the version is stamped -- otherwise
        // ensure_schema_version()'s fast path (user_version == SCHEMA_VERSION)
        // skips the migration forever and a later `SELECT last_edit_at` fails.
        // ensure_schema_version() also drops the unsafe pre-v7 undo/redo payloads
        // it used to be open()'s job to strip. (#241)
        if !file_existed {
            db.set_user_version(SCHEMA_VERSION)?;
        } else if prior_version < SCHEMA_VERSION {
            db.ensure_schema_version()?;
        }
        // An existing file already at SCHEMA_VERSION is left untouched; a newer
        // one was refused above.

        tracing::debug!("Schema initialized");

        Ok(db)
    }

    /// [`Self::open_existing`] for a file from an untrusted source, e.g. a server
    /// upload. Callers must pass a private COPY: this runs the same forward
    /// migrations, which write to the file.
    ///
    /// The file can carry its own schema, so before anything reads it the
    /// connection is put in defensive mode with `trusted_schema` off (no
    /// non-innocuous functions from schema code), every string or blob SQLite
    /// produces is capped at `max_value_bytes`, and any statement still running
    /// once `deadline` has passed since the open is interrupted. SQLite checks the
    /// deadline every 1,000 VM steps within a statement, so it bounds long
    /// statements; many short ones are bounded by the input size instead.
    pub fn open_existing_untrusted(
        path: &Path,
        max_value_bytes: i32,
        deadline: std::time::Duration,
    ) -> Result<Self> {
        use rusqlite::config::DbConfig;
        use rusqlite::limits::Limit;

        if !path.exists() {
            anyhow::bail!("File does not exist");
        }
        let mut conn =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
                .context("Failed to open database - file may not be a valid SQLite database")?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
        conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, max_value_bytes)?;
        let expires = std::time::Instant::now() + deadline;
        conn.progress_handler(1_000, Some(move || std::time::Instant::now() > expires))?;
        // SQLite's guidance for untrusted files: no memory-mapping (a crafted
        // page is read through the checked path), validate cell sizes as pages
        // are read, and keep temporary sort data on disk, not in RAM.
        conn.pragma_update(None, "mmap_size", 0i64)?;
        conn.pragma_update(None, "cell_size_check", true)?;
        conn.pragma_update(None, "temp_store", "FILE")?;
        let db = Self::finish_open_existing(conn, true)?;
        // Quick-check validates page structure without materializing stored values.
        // Walk every permitted table while the connection limits are active so a
        // large dormant blob cannot enter the cache and be read later without them.
        let tables: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for table in tables {
            let quoted = table.replace('"', "\"\"");
            let mut statement = db.conn.prepare(&format!("SELECT * FROM \"{quoted}\""))?;
            let mut rows = statement.query([])?;
            while rows.next()?.is_some() {
                if std::time::Instant::now() > expires {
                    anyhow::bail!("Downloaded SQLite processing deadline exceeded");
                }
            }
        }
        if std::time::Instant::now() > expires {
            anyhow::bail!("Downloaded SQLite processing deadline exceeded");
        }
        Ok(db)
    }

    /// The checks and migrations every existing-file open runs on `conn`.
    /// `untrusted` keeps the caller's hardened pragmas instead of the app's
    /// performance ones.
    fn finish_open_existing(conn: Connection, untrusted: bool) -> Result<Self> {
        // Before the integrity check, which would connect any virtual table (#608).
        super::check_schema(&conn)?;
        // Before the migrations, whose history cleanup writes into the file (#649).
        super::newer_schema::check_not_newer(&conn)?;
        let db = Self { conn };
        db.enable_foreign_keys()?;
        if !untrusted {
            db.apply_load_performance_pragmas();
        }
        db.run_existing_open_checks()
    }

    fn apply_load_performance_pragmas(&self) {
        // Let SQLite mmap the whole file (up to 256 MB).  Sequential
        // scans during integrity check / shot load go through the OS
        // page cache without a per-row copy into SQLite's buffer.
        // Cheap to enable; bounded by SQLite's own mmap_size cap.
        let _ = self
            .conn
            .pragma_update(None, "mmap_size", 256i64 * 1024 * 1024);
        // Default cache_size is 2 000 pages (~2 MB). Large projects
        // benefit from holding more pages resident so the per-line
        // shot SELECT and the integrity check don't re-fault from
        // mmap. 32 MiB covers the 11 MB 92.stalx with headroom for
        // the indexes; SQLite caps this against its own limits.
        let _ = self.conn.pragma_update(None, "cache_size", -32_000i64); // negative = KiB
                                                                         // Skip syncing on read-only-equivalent operations.
                                                                         // (load path doesn't write - this is harmless on writes too;
                                                                         // we already use a single transaction in save_project.)
        let _ = self.conn.pragma_update(None, "temp_store", "MEMORY");
    }

    fn run_existing_open_checks(mut self) -> Result<Self> {
        let db = &mut self;
        // Run quick integrity check before reading any data (SPEC 14.4)
        let t = std::time::Instant::now();
        db.validate_integrity()
            .context("Database file may be corrupted")?;
        tracing::debug!(
            ms = t.elapsed().as_millis() as u64,
            "open_existing: validate_integrity"
        );

        // Check if the project table exists and has data
        let t = std::time::Instant::now();
        let has_data = db.has_project_data()?;
        tracing::debug!(
            ms = t.elapsed().as_millis() as u64,
            "open_existing: has_project_data"
        );

        if !has_data {
            tracing::error!("File does not contain valid Stalagmight project data");
            anyhow::bail!("File does not contain a valid Stalagmight project");
        }

        // Run lightweight migrations to ensure newer tables exist in legacy projects
        let t = std::time::Instant::now();
        db.ensure_schema_version()?;
        tracing::debug!(
            ms = t.elapsed().as_millis() as u64,
            "open_existing: ensure_schema_version"
        );

        let t = std::time::Instant::now();
        db.validate_foreign_keys()?;
        tracing::debug!(
            ms = t.elapsed().as_millis() as u64,
            "open_existing: validate_foreign_keys"
        );

        Ok(self)
    }

    /// Check if the database has valid project data
    pub fn has_project_data(&self) -> Result<bool> {
        // First check if project table exists
        let table_exists: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='project'",
            [],
            |row| row.get(0),
        )?;

        if table_exists == 0 {
            tracing::warn!("project table does not exist in database");
            return Ok(false);
        }

        // Check if project table has any rows
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))?;

        Ok(count > 0)
    }
}
