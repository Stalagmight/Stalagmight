use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use super::{Database, SCHEMA_VERSION};

impl Database {
    /// Open or create a database at the given path
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
            // before init_schema or a migration touches it (#608).
            super::check_schema(&conn)?;
        }

        let mut db = Self { conn };
        db.enable_foreign_keys()?;

        let prior_version = db.get_user_version().unwrap_or(0);
        if prior_version > SCHEMA_VERSION {
            tracing::warn!(
                current = prior_version,
                expected = SCHEMA_VERSION,
                "Database schema version is newer than this app"
            );
        }
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
        // An existing file already at (or newer than) SCHEMA_VERSION is left
        // untouched; a newer version was warned about above.

        tracing::debug!("Schema initialized");

        Ok(db)
    }

    /// Open an existing database (does not create if missing)
    pub fn open_existing(path: &Path) -> Result<Self> {
        // Verbose open-time logging is debug-only - it fires on every
        // project load and every recent-projects metadata read, which
        // adds up.  The error paths still log at error/warn.
        tracing::debug!("Opening existing database at: {:?}", path);

        if !path.exists() {
            tracing::error!("File does not exist: {:?}", path);
            anyhow::bail!("File does not exist");
        }

        // Open in read-only mode first to check if it's valid
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .context("Failed to open database - file may not be a valid SQLite database")?;
        Self::finish_open_existing(conn, false)
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
        Self::finish_open_existing(conn, true)
    }

    /// The checks and migrations every existing-file open runs on `conn`.
    /// `untrusted` keeps the caller's hardened pragmas instead of the app's
    /// performance ones.
    fn finish_open_existing(conn: Connection, untrusted: bool) -> Result<Self> {
        // Before the integrity check, which would connect any virtual table (#608).
        super::check_schema(&conn)?;
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
        // benefit from holding more pages resident so the per-section
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

    /// Refuse the file at `path` if its schema could run SQL the file chose
    /// ([`super::check_schema`]), without opening it as a project. For files
    /// that arrive from elsewhere before anything else touches them, e.g. a
    /// SpeleoDB download about to replace the local cache.
    pub fn check_schema_at(path: &Path) -> Result<()> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("open {} to check its schema", path.display()))?;
        super::check_schema(&conn)
    }

    /// Open a database for read-only metadata extraction only.
    ///
    /// Skips the per-file integrity check, foreign-key validation, schema
    /// migration, and project-data presence check that `open_existing` /
    /// `open_readonly` run. Those are appropriate when actually loading
    /// project data; for the recent-projects listing they add hundreds of
    /// milliseconds per file (PRAGMA quick_check walks every page). The
    /// metadata reads below are pure SELECTs and will fail with a clear
    /// error if the schema is missing - that's enough validation for the
    /// purpose. If the user later opens this project, the full open path
    /// runs its checks then.
    pub fn open_for_metadata(path: &Path) -> Result<Self> {
        if !path.exists() {
            anyhow::bail!("File does not exist: {}", path.display());
        }
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("Failed to open database read-only for metadata")?;
        super::check_schema(&conn)?;
        Ok(Self { conn })
    }

    /// [`Self::open_for_metadata`] with WRITE access, for the one question a
    /// read-only connection cannot answer: what is in a file whose last write
    /// was interrupted.
    ///
    /// A hot rollback journal (a kill, a power loss) fails a read-only open with
    /// `SQLITE_READONLY_ROLLBACK` because rolling it back is a write. This opens
    /// read-write so SQLite performs that recovery on the first read - and does
    /// nothing else: no integrity check, no project-data check, no foreign-key
    /// validation and, above all, NO `ensure_schema_version`. That last one is
    /// the point. The caller (`core::state::speleodb_foreign`, in the root
    /// crate) uses this to decide whether a file may be opened AT ALL, and may well
    /// refuse it; running the load path's migrations there would rewrite - and
    /// in the v15 case discard undo/redo history from - a file the user is
    /// about to be told they cannot open, before they have answered the prompt.
    ///
    /// Rolling the journal back is itself a write, and an unavoidable one: it is
    /// the only way to see the committed contents, and it is exactly what the
    /// next ordinary open would do anyway. Everything beyond it is avoidable,
    /// so this does none of it.
    ///
    /// Do NOT reach for this to load project data. It is `open_for_metadata`'s
    /// sibling, with `open_for_metadata`'s caveats: the schema is whatever the
    /// file has, and a missing table surfaces as a failed SELECT.
    pub fn open_for_metadata_recovering(path: &Path) -> Result<Self> {
        if !path.exists() {
            anyhow::bail!("File does not exist: {}", path.display());
        }
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .context("Failed to open database read-write for metadata")?;
        super::check_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Open an existing database in read-only mode.
    /// Does not create if missing, does not run migrations.
    /// Used for lazy loading survey geometry without modifying files.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        tracing::debug!("Opening database read-only at: {:?}", path);

        if !path.exists() {
            anyhow::bail!("File does not exist: {}", path.display());
        }

        // Validate it's a Stalagmight project file by extension
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if extension != "stalx" {
            anyhow::bail!("Not a Stalagmight project file: {}", path.display());
        }

        // Open in read-only mode - no writes possible
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("Failed to open database read-only")?;
        super::check_schema(&conn)?;

        let db = Self { conn };
        db.enable_foreign_keys()?;

        // Run quick integrity check before reading any data (SPEC 14.4)
        db.validate_integrity()
            .context("Database file may be corrupted")?;

        if let Ok(version) = db.get_user_version() {
            if version > SCHEMA_VERSION {
                tracing::warn!(
                    current = version,
                    expected = SCHEMA_VERSION,
                    "Database schema version is newer than this app"
                );
            }
        }

        // Verify project data exists
        if !db.has_project_data()? {
            anyhow::bail!("File does not contain a valid Stalagmight project");
        }

        db.validate_foreign_keys()?;
        Ok(db)
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

    /// Create an in-memory database (for testing)
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to create in-memory database")?;
        let db = Self { conn };
        db.enable_foreign_keys()?;
        db.init_schema()?;
        db.set_user_version(SCHEMA_VERSION)?;
        Ok(db)
    }
}
