use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

use super::schema::PreUpgradeCopy;
use super::{Database, UpgradeRefusal, SCHEMA_VERSION};

/// Whether `path` names a `.stalx` project file, in any letter case.
///
/// Case-insensitive because everything else that decides "is this a project
/// file" is: the frontend's save dialogs accept `CaveA.STALX` as already having
/// the extension, the open router lowercases it, and a project copied from a
/// case-insensitive filesystem can arrive with any spelling. Comparing
/// case-sensitively made the save pipeline rewrite `CaveA.STALX` to
/// `CaveA.stalx` - on a case-sensitive filesystem a DIFFERENT file, which the
/// rename then replaced without the overwrite prompt that only ever saw
/// `CaveA.STALX` (#576 B33).
pub fn has_stalx_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("stalx"))
}

/// Every file type Stalagmight reads or writes. A trailing extension naming one
/// of them is a mistyped format and is REPLACED; any other is part of the name.
/// Mirrors `KNOWN_FILE_EXTENSIONS` in `src/utils/fileDialogs.ts` (#576 D35).
const KNOWN_FILE_EXTENSIONS: &[&str] = &[
    "stalx", "plnx", "gpx", "tml", "tmlu", "mak", "dat", "wpj", "srv", "dmp", "csv", "json", "png",
    "svg", "pdf", "xml",
];

/// `path` with a `.stalx` extension: returned as given when it already has one
/// in any letter case (see [`has_stalx_extension`]). Otherwise an extension that
/// names a known file type (`CaveA.tml`) - or a bare trailing dot - is replaced,
/// and anything else is appended to: `CaveA 1.5` becomes `CaveA 1.5.stalx`, not
/// `CaveA 1.stalx`, a different file no overwrite prompt was shown for
/// (#576 D35-D1, the frontend's `ensureExtension` rule).
pub fn with_stalx_extension(path: PathBuf) -> PathBuf {
    if has_stalx_extension(&path) {
        return path;
    }
    let replace = path.extension().is_some_and(|ext| {
        ext.is_empty()
            || KNOWN_FILE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
    });
    if replace {
        path.with_extension("stalx")
    } else {
        let mut appended = path.into_os_string();
        appended.push(".stalx");
        PathBuf::from(appended)
    }
}

/// [#576 W35-NF-1] A connection to the existing file at `path` for an opener
/// that only reads it, leaving nothing beside it.
///
/// READ_ONLY, as before, for every file but one: a file in WAL journal mode
/// with neither a `-wal` nor a `-journal` beside it. A read-only connection to
/// that creates the `-wal` and `-shm` and cannot remove them when it closes
/// (#576 RV8a-3, RV18-3), so it is opened READ_WRITE without CREATE instead,
/// with `query_only` on: it refuses INSERT, UPDATE, DELETE and schema changes,
/// so no statement the openers run writes through it (`query_only` does not
/// stop `PRAGMA journal_mode`, which rewrites the header; no caller runs one,
/// #576 RV19a-7). With no `-wal` nothing is committed outside the file, so
/// its last close has nothing to checkpoint into the file and removes the
/// sidecars the open made. Every other file keeps the read-only open: a
/// rollback-journal file's creates nothing, a hot journal still fails it with
/// `SQLITE_READONLY_ROLLBACK` (which callers of
/// [`Database::open_for_metadata`] rely on), and a WAL-mode file whose `-wal`
/// is there - a live writer's, or one a crash left - is read as it was, not
/// checkpointed by a reader. The `-wal` is looked for beside the file a
/// symlink points at, where SQLite keeps it (#576 RV19a-7).
///
/// One window stays open (#576 RV19a-7): a writer that starts on the file
/// between the check and this connection's close commits into a `-wal`, and
/// this connection, closing last, checkpoints those frames into the file. That
/// is the writer's committed data, written where it would have gone anyway -
/// not a corruption - but it is a write made by a reader's close.
///
/// Every caller runs the schema guard on it, so the authorizer goes on before
/// any statement (#619).
pub(super) fn connect_for_reading(path: &Path) -> rusqlite::Result<Connection> {
    use rusqlite::OpenFlags;
    if !wal_mode_with_nothing_beside(path) {
        let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        super::deny_stripped_sqlite_features(&mut conn)?;
        return Ok(conn);
    }
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    super::deny_stripped_sqlite_features(&mut conn)?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(conn)
}

/// [#576 W25-NF1] How a scratch name - a file this app creates afresh beside
/// another: a save's or a merge's temp, a push snapshot, a compaction or repair
/// staging copy - is spelled to SQLite, so that a link at it is refused, never
/// followed.
///
/// The name is cleared first, but a link planted between the clear and the open
/// was followed: SQLite resolves links itself on its walk to the file, the final
/// name included, and opens wherever they lead (its API has no exclusive create
/// for a database file). Opened with `SQLITE_OPEN_NOFOLLOW` ([`scratch_flags`]),
/// SQLite refuses a link anywhere on that walk (`SQLITE_CANTOPEN_SYMLINK`), and
/// its open of the file carries `O_NOFOLLOW`. A folder reached through a link is
/// still a place a save may go, so the folder is resolved first
/// ([`std::fs::canonicalize`]) and only the file's own name is left to walk: a
/// link SQLite meets after that was put there after the folder was resolved.
///
/// Unix only. Elsewhere SQLite gets the path as given, as before.
pub(super) fn scratch_spelling(path: &Path) -> std::io::Result<PathBuf> {
    if !cfg!(unix) {
        return Ok(path.to_path_buf());
    }
    let Some(name) = path.file_name() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} names no file", path.display()),
        ));
    };
    let folder = match path.parent() {
        Some(folder) if !folder.as_os_str().is_empty() => folder,
        _ => Path::new("."),
    };
    Ok(std::fs::canonicalize(folder)?.join(name))
}

/// [#576 RVF-d-10] A scratch name, spelled by [`scratch_spelling`], as the
/// target parameter of `VACUUM INTO`: TEXT holding the path's own bytes on
/// Unix. That spelling resolves the folder, so through a link to a folder whose
/// real name is not UTF-8 (a legacy Latin-1 name, some USB or SMB mounts) it
/// carries bytes a `&str` cannot hold; `to_string_lossy` turned them into
/// U+FFFD and sent SQLite to a folder that is not there. SQLite passes the text
/// to the file system as it is, and `VACUUM INTO` takes TEXT only, never a
/// BLOB. Elsewhere the path goes as it did, lossily (Windows file names are
/// UTF-16, and these were never resolved there).
///
/// [#619 x RVF-d-10] A path that is not UTF-8 goes as a `file:` URI instead,
/// each byte outside the URI's unreserved set percent-encoded. `VACUUM INTO`
/// ATTACHes its target, and SQLite hands the attached file name to the
/// authorizer ([`super::deny_stripped_sqlite_features`], on Repair's
/// connection), whose safe rusqlite wrapper panics on a name that is not UTF-8
/// and so refuses the statement: Repair in such a folder failed with
/// "authorization denied". The URI is ASCII, and SQLite decodes it back to the
/// same bytes when it opens the file (rusqlite's default flags, which
/// [`scratch_flags`] keeps, include `SQLITE_OPEN_URI`; `SQLITE_OPEN_NOFOLLOW`
/// still applies). A UTF-8 path goes as before, as plain text.
pub(super) struct VacuumTarget<'a>(pub(super) &'a Path);

/// `bytes`, an absolute (or relative) Unix path, as a `file:` URI SQLite reads
/// back to the same bytes: unreserved ASCII and `/` as they are, every other
/// byte as `%XX`.
#[cfg(unix)]
fn file_uri(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut uri = String::from(if bytes.first() == Some(&b'/') {
        "file://"
    } else {
        "file:"
    });
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

impl rusqlite::ToSql for VacuumTarget<'_> {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::ToSqlOutput;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            let bytes = self.0.as_os_str().as_bytes();
            if std::str::from_utf8(bytes).is_err() {
                return Ok(ToSqlOutput::from(file_uri(bytes)));
            }
            Ok(ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Text(
                bytes,
            )))
        }
        #[cfg(not(unix))]
        {
            Ok(ToSqlOutput::from(self.0.to_string_lossy().into_owned()))
        }
    }
}

/// [#576 W25-NF1] The flags a connection that creates or writes a scratch name
/// opens with: rusqlite's defaults (what [`Connection::open`] uses), plus
/// `SQLITE_OPEN_NOFOLLOW` on Unix (see [`scratch_spelling`]).
pub(super) fn scratch_flags() -> rusqlite::OpenFlags {
    let flags = rusqlite::OpenFlags::default();
    if cfg!(unix) {
        flags | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW
    } else {
        flags
    }
}

/// [#576 W42-NF1] Whether `e` is SQLite refusing a link on its walk to a name
/// opened with [`scratch_flags`] (`SQLITE_CANTOPEN_SYMLINK`), told from the
/// error's extended code - not a file SQLite could not create or write.
pub(super) fn is_link_refusal(e: &anyhow::Error) -> bool {
    e.downcast_ref::<rusqlite::Error>()
        .and_then(rusqlite::Error::sqlite_error)
        .is_some_and(|e| e.extended_code == rusqlite::ffi::SQLITE_CANTOPEN_SYMLINK)
}

/// [#576 W25-NF1] A connection on the existing database `path` for a
/// `VACUUM INTO` a scratch name. `VACUUM INTO` opens its target with the source
/// connection's flags, so the source is opened with [`scratch_flags`] too, which
/// is what makes a link at the target (spelled by [`scratch_spelling`]) refused.
/// It is opened at its canonical path (on Unix), every link on the way to it -
/// the file's own name included, a project opened through a link - resolved
/// first, so it is the same file SQLite opened before.
pub(super) fn connect_for_vacuum_into(path: &Path) -> Result<Connection> {
    let canonical = if cfg!(unix) {
        std::fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    Ok(Connection::open_with_flags(canonical, scratch_flags())?)
}

/// Whether `path` is a SQLite database in WAL journal mode - the header's
/// write and read format bytes (offsets 18 and 19) are 2 - with no `-wal` and
/// no `-journal` beside it. Unreadable or not a database: no.
///
/// On unix "beside it" means beside the file a symlink resolves to: SQLite's
/// unix VFS resolves the database path before it names the `-wal`, so looking
/// beside the link missed a writer's `-wal` (#576 RV19a-7).
fn wal_mode_with_nothing_beside(path: &Path) -> bool {
    use std::io::Read as _;
    let mut head = [0u8; 20];
    let read = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut head));
    let wal_mode = read.is_ok()
        && head.starts_with(b"SQLite format 3\0")
        && head.get(18..20) == Some(&[2u8, 2][..]);
    #[cfg(unix)]
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(not(unix))]
    let resolved = path.to_path_buf();
    wal_mode
        && !super::repair::sibling(&resolved, "-wal").exists()
        && !super::repair::sibling(&resolved, "-journal").exists()
}

impl Database {
    /// Open or create a database at the given path
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, || Connection::open(path))
    }

    /// [`Self::open`] for a scratch name a caller has just cleared (a save's or
    /// a merge's temp): SQLite gets [`scratch_spelling`] and [`scratch_flags`],
    /// so a link planted at the name since the clear is refused, never followed
    /// (#576 W25-NF1).
    pub fn open_scratch(path: &Path) -> Result<Self> {
        let spelled = scratch_spelling(path).context("Failed to open database")?;
        Self::open_with(path, || {
            Connection::open_with_flags(&spelled, scratch_flags())
        })
    }

    fn open_with(
        path: &Path,
        connect: impl FnOnce() -> rusqlite::Result<Connection>,
    ) -> Result<Self> {
        // Per-open chatter is debug-only - `open` is called for every
        // project create / save-as / import / recent-list refresh.
        tracing::debug!("Opening database at: {:?}", path);

        // Whether the file already existed decides fresh-vs-migrate below, so
        // capture it BEFORE Connection::open creates it for a new path. This is a
        // more robust discriminator than user_version/row-count: an existing file
        // that predates the user_version pragma (stamped 0) or that has no project
        // row yet must still be MIGRATED, not treated as fresh.
        let file_existed = path.exists();

        let mut conn = connect().context("Failed to open database")?;
        if file_existed {
            // An existing file may come from anywhere; refuse a hostile schema
            // before init_schema or a migration touches it (#608), with the
            // authorizer on first (#619).
            super::deny_stripped_sqlite_features(&mut conn)?;
            super::check_schema(&conn)?;
        }

        let db = Self { conn };
        db.enable_foreign_keys()?;

        // What the file is is decided BEFORE anything writes to it (#576
        // NF-PNEWER-1): `init_schema` creates every table this build knows that
        // the file lacks, which is exactly what a newer file (whose schema may
        // have dropped one) and a refused pre-release file must not get.
        let prior_version = db.get_user_version().unwrap_or(0);
        if prior_version > SCHEMA_VERSION {
            tracing::warn!(
                current = prior_version,
                expected = SCHEMA_VERSION,
                "Database schema version is newer than this app; leaving the file untouched"
            );
            return Ok(db);
        }
        if file_existed {
            db.refuse_pre_release_file()?;
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
        // An existing file already at SCHEMA_VERSION gets nothing more; a newer
        // one returned above, before `init_schema`.

        tracing::debug!("Schema initialized");

        Ok(db)
    }

    /// Open an existing database (does not create if missing)
    pub fn open_existing(path: &Path) -> Result<Self> {
        Self::open_existing_with(path, None)
    }

    /// [#576 RV4-R-M3-OPEN] [`Self::open_existing`] for a file in a place nothing
    /// may be written (the app's own folders, a system folder): a file this build
    /// can read as it is opens exactly as it would anywhere, but one the open would
    /// UPGRADE in place - the migration chain, and the pre-upgrade copy it keeps
    /// beside the file - is refused before the first write, with
    /// an [`UpgradeRefused`](super::UpgradeRefused) carrying the `refusal` message
    /// that fits the file, and left as it was.
    pub fn open_existing_refusing_upgrade(path: &Path, refusal: &UpgradeRefusal) -> Result<Self> {
        Self::open_existing_with(path, Some(refusal))
    }

    /// [#576 RV6b-R1-D1] Open an existing project only to READ it: nothing is
    /// written to the file or beside it, wherever it is.
    ///
    /// A file this build reads as it is opens exactly as [`Self::open_existing`]
    /// opens it. One the open would upgrade in place - the migration chain, and
    /// the `.pre-v<N>.backup` copy it keeps beside the file first - is instead
    /// copied into memory (`VACUUM INTO`, which writes only the copy) and the
    /// upgrade runs on that copy; the handle reads the upgraded copy and the file
    /// is left byte-for-byte as it was. For a caller that consumes a project and
    /// never writes it back (a merge source). The usual checks - quick_check, a
    /// project inside, foreign keys - run either way, and a file from before the
    /// first public release is refused as by every other open.
    ///
    /// The one write that can still happen is SQLite rolling back a hot journal
    /// left by a crashed writer, which every open of such a file does to read it.
    pub fn open_existing_to_read(path: &Path) -> Result<Self> {
        // Never shown: the refusal only routes the file to the in-memory copy.
        let read_only_here = UpgradeRefusal {
            older_format: "opened only to be read (an older file)".to_owned(),
            unstamped: "opened only to be read (an unstamped file)".to_owned(),
        };
        match Self::open_existing_with(path, Some(&read_only_here)) {
            Err(e) if e.downcast_ref::<super::UpgradeRefused>().is_some() => {
                #[cfg(test)]
                between_opens::run(path);
                tracing::info!(
                    path = ?path,
                    purpose = "read an older project without upgrading it in place (#576 RV6b-R1)",
                    "Reading an older-format project through an upgraded copy in memory"
                );
                Self::upgraded_copy_in_memory(path)
            }
            opened => opened,
        }
    }

    /// The file at `path`, copied into a private in-memory database and upgraded
    /// there. See [`Self::open_existing_to_read`].
    fn upgraded_copy_in_memory(path: &Path) -> Result<Self> {
        use rusqlite::OpenFlags;
        use std::sync::atomic::{AtomicU64, Ordering};
        static COPIES: AtomicU64 = AtomicU64::new(0);
        // A shared-cache in-memory database under a name no other open uses: the
        // handle below keeps it alive, and the source connection's `VACUUM INTO`
        // reaches the same database by that name. Both connections parse URIs,
        // which is what makes the name a memory database and not a file; the
        // source path is made absolute so it can never be read as a URI itself.
        let name = format!(
            "file:stalagmight-read-{}-{}?mode=memory&cache=shared",
            std::process::id(),
            COPIES.fetch_add(1, Ordering::Relaxed)
        );
        let uri = OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut conn = Connection::open_with_flags(
            &name,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | uri,
        )
        .context("Failed to create an in-memory copy of the project")?;
        // The copy is the file's content, and the upgrade runs on it (#619).
        super::deny_stripped_sqlite_features(&mut conn)?;
        let source_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .context("Failed to resolve the project path")?
                .join(path)
        };
        // [#576 RV8a-3] READ_WRITE, though it writes nothing: `VACUUM INTO` writes
        // only its target. A read-only connection to a file in WAL journal mode
        // creates the `-wal` / `-shm` beside it and cannot remove them when it
        // closes; a read-write one removes them as every other open's last close
        // does, so the source's folder is left as it was.
        let mut source =
            Connection::open_with_flags(&source_path, OpenFlags::SQLITE_OPEN_READ_WRITE | uri)
                .context("Failed to open the project to copy it")?;
        // Another open of an existing file: the schema guard first (#608). The
        // refusing first open already refused a hostile schema, so this catches a
        // file swapped in between (#576 RV18-2, tested through `between_opens`).
        super::deny_stripped_sqlite_features(&mut source)?;
        super::check_schema(&source)?;
        source
            .execute("VACUUM INTO ?1", [&name])
            .context("Failed to copy the project into memory")?;
        drop(source);
        // The upgrade's pre-upgrade copy is keyed off the database's own file name,
        // which an in-memory database does not have; refuse rather than let a
        // copy land anywhere if that ever stops holding.
        if conn.path().is_some_and(|p| !p.is_empty()) {
            anyhow::bail!("The in-memory copy of the project unexpectedly has a file name");
        }

        let db = Self { conn };
        db.enable_foreign_keys()?;
        db.validate_integrity()
            .context("Database file may be corrupted")?;
        if !db.has_project_data()? {
            anyhow::bail!("File does not contain a valid Stalagmight project");
        }
        db.ensure_schema_version()?;
        db.validate_foreign_keys()?;
        Ok(db)
    }

    fn open_existing_with(path: &Path, refuse_upgrade: Option<&UpgradeRefusal>) -> Result<Self> {
        // Verbose open-time logging is debug-only - it fires on every
        // project load and every recent-projects metadata read, which
        // adds up.  The error paths still log at error/warn.
        tracing::debug!("Opening existing database at: {:?}", path);

        if !path.exists() {
            tracing::error!("File does not exist: {:?}", path);
            anyhow::bail!("File does not exist");
        }

        // Open in read-only mode first to check if it's valid
        let mut conn =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
                .context("Failed to open database - file may not be a valid SQLite database")?;
        super::deny_stripped_sqlite_features(&mut conn)?;
        Self::finish_open_existing(conn, false, refuse_upgrade)
    }

    /// [`Self::open_existing`] for a file from an untrusted source, e.g. a server
    /// upload. Callers must pass a private COPY: this runs the same forward
    /// migrations, which write to the file. Because it is a copy, an upgrade keeps
    /// no `.pre-v<N>.backup` beside it, unlike every other open.
    ///
    /// The file can carry its own schema, so before anything reads it the
    /// connection is put in defensive mode with `trusted_schema` off (no
    /// non-innocuous functions from schema code), SQLite's authorizer refuses
    /// virtual tables, DBSTAT and the functions of FTS, R-tree and extension
    /// loading whether or not the linked SQLite has them (#619), every string
    /// or blob SQLite produces is capped at `max_value_bytes`, and any statement
    /// still running once `deadline` has passed since the open is interrupted.
    /// SQLite checks the deadline every 1,000 VM steps within a statement, so it
    /// bounds long statements; many short ones are bounded by the input size
    /// instead.
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
        super::deny_stripped_sqlite_features(&mut conn)?;
        conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, max_value_bytes)?;
        let expires = std::time::Instant::now() + deadline;
        conn.progress_handler(1_000, Some(move || std::time::Instant::now() > expires))?;
        // SQLite's guidance for untrusted files: no memory-mapping (a crafted
        // page is read through the checked path), validate cell sizes as pages
        // are read, and keep temporary sort data on disk, not in RAM.
        conn.pragma_update(None, "mmap_size", 0i64)?;
        conn.pragma_update(None, "cell_size_check", true)?;
        conn.pragma_update(None, "temp_store", "FILE")?;
        Self::finish_open_existing(conn, true, None)
    }

    /// The checks and migrations every existing-file open runs on `conn`.
    /// `untrusted` keeps the caller's hardened pragmas instead of the app's
    /// performance ones, and skips the pre-upgrade copy; `refuse_upgrade` is
    /// [`Self::open_existing_with`]'s.
    fn finish_open_existing(
        conn: Connection,
        untrusted: bool,
        refuse_upgrade: Option<&UpgradeRefusal>,
    ) -> Result<Self> {
        // Before the integrity check, which would connect any virtual table (#608).
        super::check_schema(&conn)?;
        let db = Self { conn };
        db.enable_foreign_keys()?;
        if !untrusted {
            db.apply_load_performance_pragmas();
        }
        // An untrusted open is always of a private copy the caller throws away
        // after the read (`open_existing_untrusted`), so an upgrade keeps no
        // pre-upgrade copy beside it.
        let pre_upgrade_copy = if untrusted {
            PreUpgradeCopy::Skip
        } else {
            PreUpgradeCopy::Keep
        };
        db.run_existing_open_checks(refuse_upgrade, pre_upgrade_copy)
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
        // mmap. 32 MiB holds a large project's shot tables with headroom
        // for the indexes; SQLite caps this against its own limits.
        let _ = self.conn.pragma_update(None, "cache_size", -32_000i64); // negative = KiB
                                                                         // Skip syncing on read-only-equivalent operations.
                                                                         // (load path doesn't write - this is harmless on writes too;
                                                                         // we already use a single transaction in save_project.)
        let _ = self.conn.pragma_update(None, "temp_store", "MEMORY");
    }

    fn run_existing_open_checks(
        self,
        refuse_upgrade: Option<&UpgradeRefusal>,
        pre_upgrade_copy: PreUpgradeCopy,
    ) -> Result<Self> {
        let db = &self;
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
        db.ensure_schema_version_with(refuse_upgrade, pre_upgrade_copy)?;
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
    ///
    /// Opened through `connect_for_reading` (#576 RV19a-3), so the check
    /// leaves nothing beside the file (#576 RV18-3: no open may follow it to
    /// remove a `-wal` / `-shm` a read-only connection to a WAL-mode file
    /// would make) and does not replay anything into it. A missing file is an
    /// error, not a new empty one. A journal found beside the file is NOT
    /// rolled back into
    /// it: the file was just written by the caller, so such a journal can only
    /// be a stale one left by a crash under the same name, and replaying it put
    /// the old file's pages into the new one. The read-only open fails on it
    /// (`SQLITE_READONLY_ROLLBACK`) and the file is refused instead.
    pub fn check_schema_at(path: &Path) -> Result<()> {
        let conn = connect_for_reading(path)
            .with_context(|| format!("open {} to check its schema", path.display()))?;
        super::check_schema(&conn)
    }

    /// [#576 B6-D1] The schema version stamped on this file when it is ABOVE the
    /// one this build knows - the file was saved by a newer Stalagmight - else
    /// `None`.
    ///
    /// Read off `PRAGMA user_version`, the same stamp `ensure_schema_version`
    /// keys its "newer than this app" branch on, so the two can never disagree
    /// about a file: every build stamps it on every file it writes. The project
    /// row's `schema_version` is deliberately not consulted - on an unstamped
    /// file it is an unverified claim (#485), and that shape is not one any
    /// build writes.
    pub fn newer_schema_version(&self) -> Option<i32> {
        self.get_user_version()
            .ok()
            .filter(|version| *version > SCHEMA_VERSION)
    }

    /// [#576 RV4-R-S7] How many leading bytes of a file
    /// [`Self::newer_schema_version_in_header`] reads: the SQLite database header.
    pub const HEADER_LEN: usize = 100;

    /// [#576 RV4-R-S7] [`Self::newer_schema_version`] for a project file that is
    /// not on disk yet - a SpeleoDB download still inside its dump - read off its
    /// first [`Self::HEADER_LEN`] bytes, `head`.
    ///
    /// The same stamp, not an estimate of it: SQLite keeps `PRAGMA user_version`
    /// in the database header, as the big-endian 32-bit integer at offset 60
    /// (sqlite.org/fileformat2.html, "The Database Header"), so the two answers
    /// cannot disagree about a file. `None` when `head` is not a SQLite database
    /// header at all: the open that follows reports what the file is instead.
    pub fn newer_schema_version_in_header(head: &[u8]) -> Option<i32> {
        const MAGIC: &[u8] = b"SQLite format 3\0";
        if !head.starts_with(MAGIC) {
            return None;
        }
        let stamp: [u8; 4] = head.get(60..64)?.try_into().ok()?;
        Some(i32::from_be_bytes(stamp)).filter(|version| *version > SCHEMA_VERSION)
    }

    /// [#576 B6-D1] Re-open `path` - the file this handle was opened from by
    /// [`Self::open_existing`] - through a READ-ONLY connection, consuming the
    /// read-write one so a session cannot keep both.
    ///
    /// For a file a newer build wrote ([`Self::newer_schema_version`]): the
    /// project is viewed, never written, and a read-only connection is what
    /// makes that hold for every path that reaches the handle - including the
    /// ones that are deliberately ungated (display settings) - instead of only
    /// for the ones that remember to check. A write through it fails with
    /// `SQLITE_READONLY` and leaves the file as it was.
    ///
    /// The checks `open_existing` ran (quick_check, project data, foreign keys)
    /// are not repeated: they described this same file a moment ago. The read
    /// tuning pragmas are, because they are per connection.
    pub fn into_read_only(self, path: &Path) -> Result<Self> {
        drop(self);
        #[cfg(test)]
        between_opens::run(path);
        let mut conn =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .context("Failed to reopen the database read-only")?;
        // Another open of an existing file: the schema guard first (#608). The
        // open that made this handle already refused a hostile schema, so this
        // catches a file swapped in between (#576 RV18-2, tested through
        // `between_opens`).
        super::deny_stripped_sqlite_features(&mut conn)?;
        super::check_schema(&conn)?;
        let db = Self { conn };
        db.enable_foreign_keys()?;
        let _ = db
            .conn
            .pragma_update(None, "mmap_size", 256i64 * 1024 * 1024);
        let _ = db.conn.pragma_update(None, "cache_size", -32_000i64);
        let _ = db.conn.pragma_update(None, "temp_store", "MEMORY");
        Ok(db)
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
        // [#576 W35-NF-1] Read-only, leaving nothing beside a WAL-mode file.
        let conn =
            connect_for_reading(path).context("Failed to open database read-only for metadata")?;
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
        let mut conn =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
                .context("Failed to open database read-write for metadata")?;
        super::deny_stripped_sqlite_features(&mut conn)?;
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
        if !has_stalx_extension(path) {
            anyhow::bail!("Not a Stalagmight project file: {}", path.display());
        }

        // Open for reading - read-only, or `query_only` for a WAL-mode file
        // with nothing beside it, which this path runs nothing past - leaving
        // nothing beside a WAL-mode file (#576 W35-NF-1; the limits are on
        // `connect_for_reading`, #576 RV19a-7).
        let conn = connect_for_reading(path).context("Failed to open database read-only")?;
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

/// [#576 FZ-PERSIST] TEST HOOK: cap this connection's database at `pages` pages
/// (`PRAGMA max_page_count`), so a write that would grow the file past it fails
/// with `SQLITE_FULL` - a disk that fills up part-way through a save, at a
/// chosen point, without a real full disk. Returns the cap SQLite settled on (it
/// never goes below the file's current size). Compiled for tests alone.
#[cfg(any(test, feature = "test-support"))]
impl Database {
    pub fn cap_page_count_for_test(&self, pages: u32) -> Result<u32> {
        Ok(self
            .conn
            .query_row(&format!("PRAGMA max_page_count = {pages}"), [], |row| {
                row.get(0)
            })?)
    }
}

/// Test-only seam (#576 RV18-2): a hook that runs between the two opens of one
/// existing file - the refusing first open of [`Database::open_existing_to_read`]
/// and its copy into memory, the handle [`Database::into_read_only`] drops
/// and its read-only reopen, and the rename of [`Database::attempt_repair`]'s
/// repaired copy and its verifying reopen (#576 RV19a-8) - with no connection
/// to the file held. A test swaps
/// the file there: the only way a file reaches the second open's schema guard,
/// because the first open refuses a hostile one itself.
#[cfg(test)]
pub(super) mod between_opens {
    use std::cell::Cell;
    use std::path::Path;

    thread_local! {
        static HOOK: Cell<Option<fn(&Path)>> = const { Cell::new(None) };
    }

    /// The next open sequence on this thread runs `hook` between its opens, once.
    pub(in crate::persistence::sqlite) fn set(hook: fn(&Path)) {
        HOOK.with(|slot| slot.set(Some(hook)));
    }

    pub(in crate::persistence::sqlite) fn run(path: &Path) {
        if let Some(hook) = HOOK.with(Cell::take) {
            hook(path);
        }
    }
}
