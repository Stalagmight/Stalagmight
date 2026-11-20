use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, Transaction};
use std::path::Path;

use super::{BackupInfo, Database, SalvagedData, SalvagedLine};

impl Database {
    /// Write a self-contained snapshot of an on-disk `.stalx` to `dest` using
    /// `VACUUM INTO` (#195).
    ///
    /// `VACUUM INTO` produces a single, fully-checkpointed database file in
    /// rollback (DELETE) journal mode with NO `-wal`/`-journal`/`-shm` sidecar,
    /// so the snapshot can be uploaded as one blob and reopened with
    /// [`Database::open_existing`]. It reads the source on its own connection and
    /// NEVER mutates it (the live project file is left untouched), and it folds in
    /// any committed WAL frames so the snapshot reflects the source's current
    /// committed state under either journal mode.
    ///
    /// The SpeleoDB push pipeline saves the live project first, then snapshots
    /// here; the caller clears `has_unpushed_edits` in the snapshot copy before
    /// upload so the dirty bit never rides to the server.
    ///
    /// `dest`'s parent directory is created if missing, and any stale file at
    /// `dest` is removed first (`VACUUM INTO` refuses to overwrite an existing
    /// target).
    pub fn vacuum_snapshot(src: &Path, dest: &Path) -> Result<()> {
        if !src.exists() {
            anyhow::bail!("source .stalx does not exist: {}", src.display());
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create snapshot dir {}", parent.display()))?;
        }
        // VACUUM INTO fails outright if the target file already exists.
        if dest.exists() {
            std::fs::remove_file(dest)
                .with_context(|| format!("remove stale snapshot {}", dest.display()))?;
        }
        let conn = Connection::open(src).context("open source .stalx for snapshot")?;
        // The destination filename is a bound parameter (supported by VACUUM INTO
        // since SQLite 3.27); this avoids any path-quoting pitfalls.
        conn.execute("VACUUM INTO ?1", params![dest.to_string_lossy()])
            .context("VACUUM INTO snapshot failed")?;
        Ok(())
    }

    /// The per-session / per-machine LOCAL tables stripped from a push snapshot:
    /// none of these are shared project data, so none may ride to collaborators.
    ///
    /// - `display_settings` - per-machine VIEW toggles (grid, layer/feature
    ///   visibility, color mode, grid spacing); a collaborator pulls their own view.
    /// - `undo_stack` - the editor's edit-session history. Stripped UNLESS the project
    ///   is in [`crate::models::UndoMode::Full`] (#303), which is the whole point of the
    ///   hybrid stack: the commit carries the operations that produced it so undo can
    ///   chain backwards commit by commit. See [`Self::sanitize_push_snapshot`]'s
    ///   `keep_undo` for the conditional, and note that the #194 b3 hazard it relaxes -
    ///   an adopting collaborator pressing Ctrl-Z and replaying the pusher's command
    ///   (and its `undo_data_json` prior-state payload) against a now-different tree,
    ///   silently wiping pitch/LRUD/comment/marker/temperature - is contained
    ///   elsewhere: a pulled stack is cleared on EVERY implicit adopt path, and is only
    ///   ever handed back through the explicit, confirmed, lock-held Unwind.
    /// - `redo_stack` - stripped ALWAYS (clarification #1). A redo branch replays
    ///   FORWARD onto a state the server has now committed past, so it is the stale
    ///   replay hazard with none of the undo side's value.
    /// - `collab_state` - the LOCAL wiring of the since-removed Ariane collab mode
    ///   (sibling `.tml` filesystem path, baseline, per-machine autosave toggle).
    ///   Meaningless to a remote collaborator and never theirs to inherit. It stays
    ///   on this list even though schema v15 DROPs the table, because the drop only
    ///   holds going forward: open a v15 file with an OLDER build and that build's
    ///   unconditional `init_schema()` re-creates the table (and starts writing it);
    ///   come back to a current build and `ensure_schema_version` takes its
    ///   `current == SCHEMA_VERSION` fast path, so the v15 migration never runs
    ///   again - and `save_project` does not clear the table either any more. The
    ///   strip loop `continue`s on a table that is absent, so carrying the entry
    ///   costs one `sqlite_master` probe per push and keeps the fence honest for the
    ///   downgrade-and-return round trip.
    /// - `speleo_undo_chain` - which revision this machine has unwound to (#303). Local
    ///   progress through history, not a property of the project.
    /// - `speleo_undo_watermark` - how far THIS machine's stack was inherited from a
    ///   loaded revision (#303). It exists to decide what to cut from the upload; shipping
    ///   it would tell the next puller to cut their own history against our clock.
    const PUSH_SNAPSHOT_LOCAL_TABLES: &[&str] = &[
        "display_settings",
        "undo_stack",
        "redo_stack",
        "collab_state",
        "speleo_undo_chain",
        "speleo_undo_watermark",
    ];

    /// The subset of [`Self::PUSH_SNAPSHOT_LOCAL_TABLES`] whose strip is FATAL to the
    /// push: these carry the pusher's edit history, which is both the privacy leak
    /// and the data-CORRUPTION vector on adopt (#194 b3). If they cannot be emptied
    /// the snapshot must not be uploaded at all.
    ///
    /// `undo_stack` is here only when the strip is actually REQUESTED - in
    /// [`crate::models::UndoMode::Full`] (#303) the undo history is meant to ride, so
    /// there is nothing to fail over. `redo_stack` is unconditional: it is never
    /// uploaded in any mode.
    ///
    /// The chain pointer and watermark are fatal too. They define how inherited rows are
    /// classified, so shipping one without the matching stack cleanup can make a later
    /// client cut or retain the wrong history. All four history tables are stripped in
    /// one transaction.
    ///
    /// "Subset" is part of the policy contract: the atomic helper iterates this list,
    /// while the best-effort loop skips it. The sibling test pins that every fatal table
    /// is also documented and classified as local-only.
    const PUSH_SNAPSHOT_FATAL_TABLES: &[&str] = &[
        "undo_stack",
        "redo_stack",
        "speleo_undo_chain",
        "speleo_undo_watermark",
    ];

    /// [#303] Delete the undo rows a chain step INHERITED, leaving the ones authored in
    /// this session. Snapshot-only: the live stack keeps everything, so the user can still
    /// undo back through the loaded history after the push.
    ///
    /// Timestamps decide it, not positions - see `save_speleo_undo_watermark` for why a
    /// position watermark would delete the user's own work once the stack shrinks. Rows are
    /// parsed and compared in Rust rather than in SQL because `executed_at` is written with
    /// variable fractional digits, so a lexicographic `<=` would be answering a slightly
    /// different question than it appears to.
    ///
    /// A missing timestamp is legacy and therefore inherited, but missing values must be
    /// one prefix of the stack. Malformed, non-monotonic, or later-missing timestamps make
    /// the cutoff ambiguous, so they refuse the push instead of being normalized.
    fn cut_inherited_undo_rows(tx: &Transaction<'_>, watermark: &str) -> Result<usize> {
        let Ok(cutoff) = chrono::DateTime::parse_from_rfc3339(watermark) else {
            // An unreadable watermark must not silently widen into "cut nothing": that is
            // the duplication bug. Fail the push instead - it is recoverable, a wrong
            // commit is not.
            anyhow::bail!("unparseable inherited-history watermark `{watermark}`");
        };
        let mut doomed: Vec<i64> = Vec::new();
        {
            let mut stmt =
                tx.prepare("SELECT id, executed_at FROM undo_stack ORDER BY sequence")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            let mut previous = None;
            let mut saw_timestamp = false;
            for row in rows {
                let (id, executed_at) = row?;
                let inherited = match executed_at.as_deref() {
                    None if saw_timestamp => anyhow::bail!(
                        "missing undo timestamp after a timestamped row; refusing an ambiguous inherited-history cut"
                    ),
                    None => true,
                    Some(raw) => {
                        let stamp = chrono::DateTime::parse_from_rfc3339(raw)
                            .with_context(|| format!("malformed undo timestamp `{raw}`"))?;
                        if previous.is_some_and(|prev| stamp < prev) {
                            anyhow::bail!(
                                "non-monotonic undo timestamps; refusing an ambiguous inherited-history cut"
                            );
                        }
                        previous = Some(stamp);
                        saw_timestamp = true;
                        stamp <= cutoff
                    }
                };
                if inherited {
                    doomed.push(id);
                }
            }
        }
        {
            let mut delete = tx.prepare("DELETE FROM undo_stack WHERE id = ?1")?;
            for id in &doomed {
                delete.execute([id])?;
            }
        }
        Ok(doomed.len())
    }

    fn clear_push_history_metadata(
        conn: &Connection,
        keep_undo: bool,
        inherited_through: Option<&str>,
        chain_pointer_present: bool,
    ) -> Result<()> {
        let tx = conn
            .unchecked_transaction()
            .context("begin push history cleanup transaction")?;

        let delete_whole_undo =
            !keep_undo || (inherited_through.is_none() && chain_pointer_present);
        if keep_undo {
            if let Some(watermark) = inherited_through {
                if Self::table_exists_on(&tx, "undo_stack")? {
                    let cut = Self::cut_inherited_undo_rows(&tx, watermark)?;
                    tracing::debug!(
                        cut,
                        watermark = %watermark,
                        "sanitize_push_snapshot: dropped inherited undo rows from the upload"
                    );
                }
            } else if chain_pointer_present {
                tracing::warn!(
                    purpose = "fail closed on an unknown inherited-history extent (#303)",
                    "a chain step is recorded but its watermark is missing; stripping the \
                     whole undo history from this push rather than risk duplicating a \
                     loaded revision's operations"
                );
            }
        }

        for table in Self::PUSH_SNAPSHOT_FATAL_TABLES {
            if *table == "undo_stack" && !delete_whole_undo {
                continue;
            }
            if !Self::table_exists_on(&tx, table)? {
                continue;
            }
            tx.execute(&format!("DELETE FROM {table}"), [])
                .with_context(|| {
                    format!(
                        "could not strip `{table}` from the push snapshot; refusing to upload it"
                    )
                })?;
        }

        tx.commit().context("commit push history cleanup")?;
        Ok(())
    }

    /// Sanitize a push SNAPSHOT in place (#197/#194) before upload:
    ///
    /// - clears `speleo_link.has_unpushed_edits` / `last_edit_at` - the dirty bit is
    ///   local hand-off state, never server state (the next puller re-stamps it);
    /// - empties every table in [`Self::PUSH_SNAPSHOT_LOCAL_TABLES`] - per-machine /
    ///   per-session local state that must not proliferate.
    ///
    /// `keep_undo` (#303) exempts `undo_stack` from that strip, for a project resolved to
    /// [`crate::models::UndoMode::Full`]: there the history since the last commit is the
    /// payload, and shipping it is what lets undo chain backwards commit by commit.
    /// `redo_stack` is stripped either way. The caller resolves the mode - this function
    /// must not, because it runs on a snapshot COPY whose link row is about to be
    /// scrubbed and is in no position to arbitrate policy.
    ///
    /// Everything else PROLIFERATES with the project, by design: survey data,
    /// `project_settings` (shared validation config #57), AND the durable
    /// project-data tables `imported_file_hashes` / `imported_line_hashes`
    /// (project-wide import-dedup - collaborators must not re-import the same file)
    /// and `resolved_warnings` (a resolved warning stays resolved for everyone, like
    /// a comment). These are NOT stripped - only a writer holding the lock can push,
    /// so anything in the snapshot is a legitimate editor's change.
    ///
    /// Operates on a SNAPSHOT copy via its own connection; the live project file is
    /// never touched.
    ///
    /// FAIL-CLOSED for history. A table that is ABSENT is skipped (an older snapshot
    /// shape legitimately lacks it, and what does not exist cannot leak). The fatal
    /// history tables are cleaned in one transaction; local preferences remain
    /// best-effort. A snapshot still holding history metadata or a dirty bit never
    /// reaches the upload.
    ///
    /// Finishes with a `VACUUM`, which is load-bearing rather than cosmetic: `DELETE`
    /// only marks pages free, so without it every deleted undo row is still
    /// physically present in the bytes that get uploaded. `VACUUM` rewrites the file
    /// from the live pages, dropping the freed ones for real.
    /// [#303] Is a chain pointer recorded in this snapshot?
    ///
    /// Read from the SNAPSHOT rather than the live database because that is the copy whose
    /// undo rows are about to be judged, and because `sanitize_push_snapshot` takes a bare
    /// path, not an `AppState`. The caller captures this before any local table is stripped,
    /// so the privacy decision cannot depend on `PUSH_SNAPSHOT_LOCAL_TABLES` ordering.
    ///
    /// An absent table answers false - a file that never had a chain step.
    fn chain_pointer_present(conn: &Connection) -> Result<bool> {
        if !Self::table_exists_on(conn, "speleo_undo_chain")? {
            return Ok(false);
        }
        let rows: i64 =
            conn.query_row("SELECT COUNT(*) FROM speleo_undo_chain", [], |r| r.get(0))?;
        Ok(rows > 0)
    }

    pub fn sanitize_push_snapshot(
        snapshot: &Path,
        keep_undo: bool,
        inherited_through: Option<&str>,
    ) -> Result<()> {
        // No transaction spans the WHOLE sanitizer: VACUUM cannot run inside one.
        // The history tables use one short transaction that commits before compaction.
        let conn = Connection::open(snapshot).context("open snapshot to sanitize for push")?;
        // Capture this from the untouched snapshot. The missing-watermark branch below
        // must not change meaning when the local-table list is reordered.
        let chain_pointer_present =
            keep_undo && inherited_through.is_none() && Self::chain_pointer_present(&conn)?;
        // The history cleanup is one fatal transaction. A failed chain/watermark strip
        // must roll back the undo/redo deletes too, leaving no half-sanitized snapshot.
        Self::clear_push_history_metadata(
            &conn,
            keep_undo,
            inherited_through,
            chain_pointer_present,
        )?;

        // The remaining local tables are preferences or obsolete local wiring. Their
        // cleanup remains best-effort because a failed view-setting strip is not allowed
        // to block publishing survey data.
        for table in Self::PUSH_SNAPSHOT_LOCAL_TABLES {
            if Self::PUSH_SNAPSHOT_FATAL_TABLES.contains(table) {
                continue;
            }
            if !Self::table_exists_on(&conn, table)? {
                // An older snapshot shape simply has no such table - nothing local
                // can be riding in it, so this must not abort the push.
                continue;
            }
            if let Err(e) = conn.execute(&format!("DELETE FROM {table}"), []) {
                tracing::warn!(
                    error = %e,
                    table = %table,
                    "sanitize_push_snapshot: failed to strip local-only table from snapshot"
                );
            }
        }
        // The dirty bit / edit timestamp are local hand-off state, never server
        // state, and a stale `has_unpushed_edits = 1` riding to the server is a
        // correctness bug for whoever pulls it - so a failure here is fatal too.
        // A missing `speleo_link` (older snapshot shape) carries no dirty bit at
        // all, so there is nothing to clear and nothing to fail over.
        if Self::table_exists_on(&conn, "speleo_link")? {
            conn.execute(
                "UPDATE speleo_link SET has_unpushed_edits = 0, last_edit_at = NULL",
                [],
            )
            .context("could not clear the dirty bit in the push snapshot; refusing to upload it")?;
        }
        // Reclaim the freed pages: the rows deleted above are still byte-for-byte
        // present in the file until the database is rewritten, and this file is
        // about to be uploaded to collaborators.
        //
        // `VACUUM INTO` a sibling + rename, NOT a plain `VACUUM`. A plain VACUUM
        // rewrites through SQLite's TEMP directory (`SQLITE_TMPDIR` / `TMPDIR` /
        // `/tmp`), which is nowhere near the snapshot: on a box with a small tmpfs
        // `/tmp` - the common Linux default - a large project ran out of space there
        // and, now that the failure is fatal, that turned into a push the user can
        // never complete. `VACUUM INTO` writes the compacted copy straight into the
        // snapshot's own directory, which is the staging area the caller already
        // chose and sized for a full copy of the project. Same reasoning as
        // [`Self::vacuum_snapshot`], which has always done it this way.
        let compacted = Self::vacuum_temp_path(snapshot);
        // VACUUM INTO refuses to overwrite an existing target; a leftover from a
        // killed run must not wedge every later push.
        let _ = std::fs::remove_file(&compacted);
        if let Err(e) = conn.execute("VACUUM INTO ?1", params![compacted.to_string_lossy()]) {
            let _ = std::fs::remove_file(&compacted);
            return Err(anyhow::Error::from(e).context(format!(
                "could not compact the push snapshot into {} (most likely not enough \
                 free disk space there for a second copy of the project); refusing to \
                 upload it",
                compacted.display()
            )));
        }
        // Close the connection explicitly BEFORE the rename: on Windows the source
        // file cannot be replaced while SQLite holds it open, and the rewritten copy
        // (and its rollback journal) must be fully flushed before the caller reads it
        // for the upload.
        drop(conn);
        if let Err(e) = std::fs::rename(&compacted, snapshot) {
            let _ = std::fs::remove_file(&compacted);
            return Err(anyhow::Error::from(e).context(format!(
                "could not replace the push snapshot at {} with its compacted copy; \
                 refusing to upload it",
                snapshot.display()
            )));
        }
        Ok(())
    }

    /// Where [`Self::sanitize_push_snapshot`] stages its compacted copy: a SIBLING
    /// of the snapshot, so the follow-up rename stays on one filesystem (a
    /// cross-device rename fails with `EXDEV`) and so the space it needs is the same
    /// space the snapshot itself already occupies - never a small `/tmp`.
    fn vacuum_temp_path(snapshot: &Path) -> std::path::PathBuf {
        let mut name = snapshot
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("snapshot"))
            .to_os_string();
        name.push(".vacuum.tmp");
        snapshot.with_file_name(name)
    }

    /// `table_exists` against a bare [`Connection`] - the sanitize path holds one
    /// directly rather than a [`Database`].
    fn table_exists_on(conn: &Connection, name: &str) -> Result<bool> {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(exists > 0)
    }

    /// Open a database for recovery purposes, skipping integrity checks.
    /// This allows reading whatever data is still accessible from a corrupt file.
    pub fn open_for_recovery(path: &Path) -> Result<Self> {
        tracing::info!("Opening database for recovery at: {:?}", path);

        if !path.exists() {
            anyhow::bail!("File does not exist");
        }

        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("Failed to open database for recovery")?;

        let db = Self { conn };
        // Skip integrity check intentionally - we know the file may be corrupt
        Ok(db)
    }

    /// Attempt to repair a corrupt database using REINDEX + VACUUM.
    ///
    /// Always creates a backup first (`path.stalx.backup`, with numeric
    /// suffixes to avoid overwriting existing backups).
    ///
    /// Returns `(backup_path, integrity_ok)`.
    pub fn attempt_repair(path: &Path) -> Result<(std::path::PathBuf, bool)> {
        tracing::info!("Attempting repair of: {:?}", path);

        // 0. A newer schema's file is not this build's to rewrite (#649).
        Self::check_not_newer_at(path)?;

        // 1. Create backup (never overwrite existing backups)
        let backup_path = Self::create_numbered_backup(path)?;
        tracing::info!("Created backup at: {:?}", backup_path);

        // 2. Open read-write (skip our integrity check wrapper)
        let conn = Connection::open(path).context("Failed to open database for repair")?;

        // 3. Rebuild indexes
        if let Err(e) = conn.execute_batch("REINDEX;") {
            tracing::warn!(error = %e, "REINDEX failed, continuing with VACUUM");
        }

        // 4. VACUUM rebuilds the entire database from readable pages
        conn.execute_batch("VACUUM;")
            .context("VACUUM failed - database may be too damaged to repair")?;

        // 5. Verify result
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .unwrap_or_else(|_| "error".to_string());
        let ok = check == "ok";

        if ok {
            tracing::info!("Repair succeeded - integrity check passed");
        } else {
            tracing::error!(result = %check, "Repair completed but integrity check still fails");
        }

        Ok((backup_path, ok))
    }

    /// Export whatever data can be read from a (possibly corrupt) database.
    ///
    /// Each table is read independently so a failure in one table doesn't
    /// prevent salvaging data from others.
    pub fn export_salvageable(path: &Path) -> Result<SalvagedData> {
        let db = Self::open_for_recovery(path)?;
        let mut salvaged = SalvagedData::default();

        // Project metadata
        match db.try_read_project_metadata() {
            Ok(Some((name, location))) => {
                salvaged.project_name = Some(name);
                salvaged.project_location = location;
            }
            Ok(None) => salvaged.errors.push("Project table is empty".into()),
            Err(e) => salvaged
                .errors
                .push(format!("Failed to read project metadata: {e}")),
        }

        // Lines
        match db.try_read_lines() {
            Ok(lines) => {
                salvaged.line_count = lines.len();
                salvaged.lines_json = serde_json::to_string(&lines).ok();
            }
            Err(e) => salvaged.errors.push(format!("Failed to read lines: {e}")),
        }

        // Shots
        match db.try_read_shot_count() {
            Ok(count) => salvaged.shot_count = count,
            Err(e) => salvaged.errors.push(format!("Failed to read shots: {e}")),
        }

        // Station merges
        match db.try_read_merge_count() {
            Ok(count) => salvaged.merge_count = count,
            Err(e) => salvaged
                .errors
                .push(format!("Failed to read station merges: {e}")),
        }

        // Full project (best-effort)
        match db.load_project() {
            Ok(project) => {
                salvaged.full_project_json = serde_json::to_string(&project).ok();
            }
            Err(e) => salvaged
                .errors
                .push(format!("Failed to load full project: {e}")),
        }

        Ok(salvaged)
    }

    /// List backup files for a given project path.
    ///
    /// A backup is a file named after the project plus a suffix with `.backup`
    /// in it, or ending in `.tmp`: `cave.stalx.backup`, `cave.stalx.backup.1`,
    /// `cave.stalx.tmp`. Only the suffix decides, so a project whose own name
    /// contains `.backup` (`cave.backup.stalx`) is never offered as a backup of
    /// itself, and neither is its safety copy `cave.backup.stalx.pre-restore`
    /// (#720). SQLite's `-journal`, `-wal` and `-shm` files are not databases
    /// and are never offered, and neither is a file that is empty or does not
    /// start with SQLite's header: a restore would only refuse it.
    pub fn list_backups(path: &Path) -> Vec<BackupInfo> {
        let mut backups = Vec::new();
        let Some(parent) = path.parent() else {
            return backups;
        };
        let own_name = path.file_name().unwrap_or_default().to_string_lossy();

        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Some(suffix) = name.strip_prefix(&*own_name) else {
                    continue;
                };
                let suffix = suffix.to_ascii_lowercase();
                let is_sidecar = ["-journal", "-wal", "-shm"]
                    .iter()
                    .any(|sidecar| suffix.ends_with(sidecar));
                let is_backup = !suffix.is_empty()
                    && !is_sidecar
                    && (suffix.contains(".backup")
                        || suffix
                            .rsplit_once('.')
                            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("tmp")))
                    && !same_file_on_disk(&entry.path(), path);
                if is_backup {
                    let file_meta = std::fs::metadata(entry.path())
                        .ok()
                        .filter(std::fs::Metadata::is_file)
                        .filter(|_| starts_like_a_database(&entry.path()));
                    if let Some(meta) = file_meta {
                        backups.push(BackupInfo {
                            path: entry.path().display().to_string(),
                            size_bytes: meta.len(),
                            modified_at: meta
                                .modified()
                                .ok()
                                .map(|t| {
                                    let dt: chrono::DateTime<Utc> = t.into();
                                    dt.to_rfc3339()
                                })
                                .unwrap_or_default(),
                        });
                    }
                }
            }
        }

        // Sort by modification time, newest first
        backups.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
        backups
    }

    /// Create a numbered backup: file.stalx.backup, .backup.1, .backup.2, etc.
    pub(super) fn create_numbered_backup(path: &Path) -> Result<std::path::PathBuf> {
        let base = path.with_extension("stalx.backup");
        if !base.exists() {
            std::fs::copy(path, &base)?;
            return Ok(base);
        }
        for i in 1..100 {
            let numbered = path.with_extension(format!("stalx.backup.{i}"));
            if !numbered.exists() {
                std::fs::copy(path, &numbered)?;
                return Ok(numbered);
            }
        }
        anyhow::bail!("Too many existing backups (max 100)")
    }

    pub(super) fn try_read_project_metadata(&self) -> Result<Option<(String, Option<String>)>> {
        if !self.table_exists("project")? {
            return Ok(None);
        }
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))?;
        if count == 0 {
            return Ok(None);
        }
        let (name, location): (String, Option<String>) =
            self.conn
                .query_row("SELECT name, location FROM project LIMIT 1", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?;
        Ok(Some((name, location)))
    }

    pub(super) fn try_read_lines(&self) -> Result<Vec<SalvagedLine>> {
        if !self.table_exists("sections")? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, surveyor, date, color FROM sections ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok(SalvagedLine {
                id: row.get(0)?,
                name: row.get(1)?,
                surveyor: row.get(2)?,
                date: row.get(3)?,
                color: row.get(4)?,
            })
        })?;
        let mut lines = Vec::new();
        for row in rows {
            lines.push(row?);
        }
        Ok(lines)
    }

    pub(super) fn try_read_shot_count(&self) -> Result<usize> {
        if !self.table_exists("shots")? {
            return Ok(0);
        }
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM shots", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub(super) fn try_read_merge_count(&self) -> Result<usize> {
        if !self.table_exists("station_merges")? {
            return Ok(0);
        }
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM station_merges", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

/// The first 16 bytes of every SQLite database file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Whether the file at `path` starts with SQLite's header. A cheap filter for
/// the backup list, not a check that the file opens.
fn starts_like_a_database(path: &Path) -> bool {
    use std::io::Read as _;
    let mut header = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| &header == SQLITE_HEADER)
}

/// The same file on disk: the same canonical path, or (on Unix) the same inode,
/// which also catches a hard link under another name. What the backup list and
/// a restore (#720) mean by "the project itself".
pub fn same_file_on_disk(a: &Path, b: &Path) -> bool {
    if let (Ok(ca), Ok(cb)) = (a.canonicalize(), b.canonicalize()) {
        if ca == cb {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return ma.dev() == mb.dev() && ma.ino() == mb.ino();
        }
    }
    false
}
