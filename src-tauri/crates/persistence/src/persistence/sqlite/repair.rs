use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, Transaction};
use std::path::Path;

use super::open::{connect_for_vacuum_into, is_link_refusal, scratch_spelling, VacuumTarget};
use super::{BackupInfo, BackupKind, Database, RestoreOutcome, SalvagedData, SalvagedSection};

/// [#576 C15] Where a restore stages the candidate before installing it: beside
/// the project, so the install is a same-filesystem rename. Not a name the
/// backup listing offers.
const RESTORE_STAGING_SUFFIX: &str = ".restore-staging";
/// [#576 C15] Where the backup listing checks a candidate that has a rollback
/// journal beside it (rolling the journal back is a write, so it happens on a
/// copy). Not a name the backup listing offers.
const CHECK_STAGING_SUFFIX: &str = ".restore-check";
/// [#576 B27(f)] Where Repair writes its compacted copy before renaming it over
/// the project. Not `.tmp`: that is what the listing offers as an interrupted
/// save.
pub(super) const REPAIR_STAGING_SUFFIX: &str = ".repair-staging";

#[cfg(test)]
thread_local! {
    /// [#576 W25-NF1] Test seam: run on this thread with every scratch name
    /// [`clear_scratch_name`] has just cleared, before the caller writes there, so
    /// a test can plant a link in exactly that window. Kept until a test clears it.
    pub(super) static AFTER_SCRATCH_CLEARED: std::cell::RefCell<Option<Box<dyn FnMut(&Path)>>> =
        const { std::cell::RefCell::new(None) };
}

/// `path` with `suffix` appended to its file name (`CaveA.stalx` + `.x` ->
/// `CaveA.stalx.x`).
pub(super) fn sibling(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("project"))
        .to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// SQLite's rollback journal for `path`.
fn journal_of(path: &Path) -> std::path::PathBuf {
    sibling(path, "-journal")
}

/// A restore's step 3: rename the staged file over the project (behind the
/// test-only fault hook, since a failing rename has no organic trigger here).
fn install_staged(staging: &Path, project: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    fault::check_install()?;
    std::fs::rename(staging, project)
}

/// Test-only fault injection for a restore's install rename (#576 RV3-R-6): on
/// Windows it fails when another process holds the project open, which a test
/// cannot arrange portably.
#[cfg(test)]
pub(super) mod fault {
    use std::cell::Cell;

    thread_local! {
        static FAIL_INSTALL: Cell<bool> = const { Cell::new(false) };
    }

    /// Every restore install on this thread fails until the guard drops.
    pub(in crate::persistence::sqlite) fn fail_restore_install() -> FailingInstall {
        FAIL_INSTALL.with(|fail| fail.set(true));
        FailingInstall
    }

    /// Clears the injection on drop, so a failed assertion cannot leak it into
    /// the next test on the same thread.
    pub(in crate::persistence::sqlite) struct FailingInstall;

    impl Drop for FailingInstall {
        fn drop(&mut self) {
            FAIL_INSTALL.with(|fail| fail.set(false));
        }
    }

    pub(super) fn check_install() -> std::io::Result<()> {
        if FAIL_INSTALL.with(Cell::get) {
            return Err(std::io::Error::other("injected install failure"));
        }
        Ok(())
    }
}

/// The safety copy a restore keeps of the file it replaces.
struct PreRestoreCopy {
    path: std::path::PathBuf,
    /// The project's rollback journal moved to `<path>-journal` with it.
    took_journal: bool,
}

/// Remove `path` and its journal, ignoring "not there".
fn remove_with_journal(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(journal_of(path));
}

/// Copy `from` (and its rollback journal, when there is one) to `to`, replacing
/// whatever was at `to`.
///
/// [#576 RVF-b-1] Never through a link left at `to` or at its journal's name:
/// both are cleared first, and a leftover that cannot be removed fails the copy
/// (see [`copy_to_new_file`]).
fn copy_with_journal(from: &Path, to: &Path) -> std::io::Result<()> {
    clear_scratch_name(&journal_of(to))?;
    copy_to_new_file(from, to)?;
    let journal = journal_of(from);
    if journal.exists() {
        copy_to_new_file(&journal, &journal_of(to))?;
    }
    Ok(())
}

/// [#576 RVF-b-1] Remove whatever is at `path` - a leftover of an interrupted
/// run, or a link, which `remove_file` removes itself and never what it leads to
/// (even one that leads nowhere yet, which `exists()` says is not there) - so
/// the write that follows creates `path` afresh. An entry that cannot be removed
/// (a folder the user cannot change, a link someone else owns in a shared one)
/// is an error: written to, a link at the name would take the write to wherever
/// it points, a place no write policy judged.
pub(super) fn clear_scratch_name(path: &Path) -> std::io::Result<()> {
    if path.symlink_metadata().is_ok() {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    #[cfg(test)]
    AFTER_SCRATCH_CLEARED.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(path);
        }
    });
    Ok(())
}

/// [#576 RVF-b-1] Copy `from` to `to` as a NEW file, which `fs::copy` does not
/// do: it opens `to` following a link there, so a link left at the name
/// received the copy wherever it pointed - creating a file a dangling one named,
/// overwriting a file it led to. Here the name is cleared first
/// ([`clear_scratch_name`]) and then created with `create_new` (O_EXCL, which
/// refuses any entry at the name, a link included). The copy gets `from`'s
/// permission bits (narrowed by the umask), as `fs::copy` gave it, so a private
/// project never gets a world-readable copy. Returns the bytes copied.
///
/// [#576 RVF-d-11] The trade-off on macOS: `fs::copy` there first tries an
/// APFS clone (`fclonefileat`: no new data blocks, instant), and this is a
/// byte-for-byte copy (`io::copy` has a kernel fast path on Linux only), so
/// each copy now costs the file's size in time and disk space, and on a nearly
/// full disk a large project's upgrade, repair or restore can be refused for
/// space a clone would not have needed. `fs::copy` into the `create_new` file
/// would not get the clone back: `fclonefileat` refuses a destination that
/// exists, and `fs::copy` then reopens it by name, following a link there. A
/// clone needs `fclonefileat` called here directly (it refuses an existing
/// name, a link included), which is untested on macOS.
pub(super) fn copy_to_new_file(from: &Path, to: &Path) -> std::io::Result<u64> {
    clear_scratch_name(to)?;
    let mut source = std::fs::File::open(from)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        options.mode(source.metadata()?.permissions().mode() & 0o777);
    }
    let mut target = options.open(to)?;
    std::io::copy(&mut source, &mut target)
}

/// What the listing calls a file beside the project named `stem`, or `None`
/// when it is not a restore candidate.
fn classify_candidate(stem: &str, name: &str) -> Option<(BackupKind, Option<u32>)> {
    let rest = name.strip_prefix(stem)?;
    // A journal, WAL or shared-memory file is part of another file, never a
    // candidate of its own; the staging names are this module's own scratch.
    if rest.is_empty()
        || [
            "-journal",
            "-wal",
            "-shm",
            RESTORE_STAGING_SUFFIX,
            CHECK_STAGING_SUFFIX,
            REPAIR_STAGING_SUFFIX,
        ]
        .iter()
        .any(|suffix| rest.ends_with(suffix))
    {
        return None;
    }
    if let Some(number) = rest.strip_prefix(".pre-restore") {
        return match number {
            "" => Some((BackupKind::PreRestore, None)),
            _ => number
                .strip_prefix('.')
                .and_then(|n| n.parse::<u32>().ok())
                .map(|n| (BackupKind::PreRestore, Some(n))),
        };
    }
    if rest.contains(".backup") {
        return Some((BackupKind::Backup, None));
    }
    if rest.to_ascii_lowercase().ends_with(".tmp") {
        return Some((BackupKind::InterruptedSave, None));
    }
    None
}

/// How long a restore check waits out a competing writer on the candidate: the
/// 5000 ms every other open path has (`busy_timeout_tests`).
const CANDIDATE_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl Database {
    /// [#576 NF-FIX5-3] Wrap a connection a restore check opened on a candidate,
    /// with the busy timeout set here rather than left to whatever the caller's
    /// open happened to configure (rusqlite's default today), so the wait the
    /// busy-timeout test pins on every other open path holds on this one too.
    ///
    /// [#576 RV18-1] A candidate is an existing file like any other, so the
    /// schema guard (#608) runs here, before quick_check evaluates the file's
    /// CHECKs or anything else reads it: a file every open would refuse is never
    /// offered as usable, nor installed over the project.
    pub(super) fn for_candidate_check(conn: Connection) -> Result<Self> {
        conn.busy_timeout(CANDIDATE_BUSY_TIMEOUT)
            .context("Failed to set the busy timeout on a restore candidate")?;
        super::check_schema(&conn)?;
        Ok(Self { conn })
    }
}

/// The checks a restore candidate has to pass, on a connection that already
/// sees its committed contents: the schema guard, quick_check, and a project
/// inside.
///
/// The reasons are worded for the dialog; SQLite's own error goes to the log.
fn check_candidate_connection(conn: Connection) -> std::result::Result<(), String> {
    let unreadable = |e: &anyhow::Error| {
        tracing::info!(error = %e, purpose = "check a restore candidate", "Candidate refused");
        let refused = e
            .chain()
            .find_map(|cause| cause.downcast_ref::<super::RefusedSchemaObject>());
        if let Some(refused) = refused {
            format!(
                "it contains {} named \"{}\", which Stalagmight never creates",
                refused.what,
                refused.display_name()
            )
        } else if format!("{e:#}").contains("integrity check failed") {
            "it fails the database integrity check".to_string()
        } else {
            "it is not a readable project file".to_string()
        }
    };
    let db = Database::for_candidate_check(conn).map_err(|e| unreadable(&e))?;
    db.validate_integrity().map_err(|e| unreadable(&e))?;
    match db.has_project_data() {
        Ok(true) => Ok(()),
        Ok(false) => Err("it holds no project".to_string()),
        Err(e) => Err(unreadable(&e)),
    }
}

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
        // VACUUM INTO fails outright if the target file already exists. [#576
        // RVF-b-1] A link there is removed too, even one that leads nowhere yet
        // (`exists()` follows it and says no): SQLite resolves a link at the
        // target name and would create the snapshot where it points.
        clear_scratch_name(dest)
            .with_context(|| format!("remove stale snapshot {}", dest.display()))?;
        // [#576 W25-NF1] A link planted at `dest` since the clear is refused, not
        // followed (`connect_for_vacuum_into`, `scratch_spelling`).
        let conn = connect_for_vacuum_into(src).context("open source .stalx for snapshot")?;
        let dest = scratch_spelling(dest)
            .with_context(|| format!("resolve the snapshot's folder {}", dest.display()))?;
        // The destination filename is a bound parameter (supported by VACUUM INTO
        // since SQLite 3.27); this avoids any path-quoting pitfalls.
        conn.execute("VACUUM INTO ?1", params![VacuumTarget(&dest)])
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
    /// project-data tables `imported_file_hashes` / `imported_section_hashes`
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
        // [#576 W25-NF1] Opened for the `VACUUM INTO` below: see `connect_for_vacuum_into`.
        let conn =
            connect_for_vacuum_into(snapshot).context("open snapshot to sanitize for push")?;
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
        // killed run must not wedge every later push. [#576 RVF-b-1] One that
        // cannot be removed refuses the push: a link left there would take the
        // compacted copy wherever it points (SQLite resolves it).
        if let Err(e) = clear_scratch_name(&compacted) {
            return Err(anyhow::Error::from(e).context(format!(
                "could not clear {} for the compacted push snapshot; refusing to upload it",
                compacted.display()
            )));
        }
        // [#576 W25-NF1] A link planted there since the clear is refused, not followed.
        let vacuumed = scratch_spelling(&compacted)
            .map_err(anyhow::Error::from)
            .and_then(|into| Ok(conn.execute("VACUUM INTO ?1", params![VacuumTarget(&into)])?));
        if let Err(e) = vacuumed {
            let _ = std::fs::remove_file(&compacted);
            // [#576 W42-NF1] A link refused there is no full disk: it is said as
            // the write-path policy words a refusal. Any other failure keeps the guess.
            if is_link_refusal(&e) {
                let name_of = |path: &Path| {
                    path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    )
                };
                return Err(e.context(format!(
                    "Refused to change {}: {} beside it, the name its compacted copy is \
                     written under first, is a link; refusing to upload it",
                    name_of(snapshot),
                    name_of(&compacted)
                )));
            }
            return Err(e.context(format!(
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
    ///
    /// [#576 RV18-1] The schema guard (#608) is not skipped: it reads only the
    /// schema, which SQLite has to parse before it can read any table of a
    /// damaged file anyway, and the salvage that follows must not run SQL the
    /// file chose. It is the salvage's lenient form (#576 RV19a-12): a type
    /// cell that lost its case ('Table') is checked, not refused.
    pub fn open_for_recovery(path: &Path) -> Result<Self> {
        tracing::info!("Opening database for recovery at: {:?}", path);

        if !path.exists() {
            anyhow::bail!("File does not exist");
        }

        // Read-only, leaving nothing beside a WAL-mode file (#576 W35-NF-1).
        let conn = super::open::connect_for_reading(path)
            .context("Failed to open database for recovery")?;
        super::schema_guard::check_schema_for_salvage(&conn)?;

        let db = Self { conn };
        // Skip integrity check intentionally - we know the file may be corrupt
        Ok(db)
    }

    /// Attempt to repair a corrupt database using REINDEX + a rewrite of every
    /// readable page.
    ///
    /// Always creates a backup first (`path.stalx.backup`, with numeric
    /// suffixes to avoid overwriting existing backups).
    ///
    /// Returns `(backup_path, integrity_ok)`.
    pub fn attempt_repair(path: &Path) -> Result<(std::path::PathBuf, bool)> {
        tracing::info!("Attempting repair of: {:?}", path);

        // 1. Create backup (never overwrite existing backups)
        let backup_path = Self::create_numbered_backup(path)?;
        tracing::info!("Created backup at: {:?}", backup_path);

        // 2. Open read-write (skip our integrity check wrapper). [#576 RV18-1] The
        // schema guard still runs (#608), before REINDEX evaluates an expression
        // index or the final quick_check a CHECK: a file every open refuses is
        // left as it is, beside the plain copy step 1 took of it.
        // [#576 W25-NF1] Opened for the `VACUUM INTO` below: see `connect_for_vacuum_into`.
        let conn = connect_for_vacuum_into(path).context("Failed to open database for repair")?;
        super::check_schema(&conn)?;

        // 3. Rebuild indexes
        if let Err(e) = conn.execute_batch("REINDEX;") {
            tracing::warn!(error = %e, "REINDEX failed, continuing with VACUUM");
        }

        // 4. Rewrite the database from its readable pages. [#576 B27(f)] `VACUUM
        // INTO` a sibling + rename, NOT a plain `VACUUM`: that one rewrites
        // through SQLite's temp directory (`SQLITE_TMPDIR` / `TMPDIR` / `/tmp`),
        // a small tmpfs on many Linux systems, so a large project could not be
        // repaired at all. The sibling sits where the project already fits - the
        // same reasoning as `sanitize_push_snapshot` - and a leftover from a
        // killed run is removed first (`VACUUM INTO` refuses an existing target).
        let staging = sibling(path, REPAIR_STAGING_SUFFIX);
        // [#576 RVF-b-1] A leftover that cannot be removed stops the repair: a
        // link left there would take the rewrite wherever it points.
        if let Err(e) =
            clear_scratch_name(&staging).and_then(|()| clear_scratch_name(&journal_of(&staging)))
        {
            return Err(anyhow::Error::from(e)
                .context("Could not clear the name the repaired copy is written under"));
        }
        // [#576 W25-NF1] A link planted there since the clear is refused, not followed.
        let vacuumed = scratch_spelling(&staging)
            .map_err(anyhow::Error::from)
            .and_then(|into| Ok(conn.execute("VACUUM INTO ?1", params![VacuumTarget(&into)])?));
        if let Err(e) = vacuumed {
            remove_with_journal(&staging);
            return Err(e.context("VACUUM failed - database may be too damaged to repair"));
        }
        // Closed before the rename: Windows cannot replace a file SQLite holds.
        drop(conn);
        if let Err(e) = std::fs::rename(&staging, path) {
            remove_with_journal(&staging);
            return Err(anyhow::Error::from(e)
                .context("Could not replace the project with its repaired copy"));
        }

        // 5. Verify result
        #[cfg(test)]
        super::open::between_opens::run(path);
        let conn = Connection::open(path).context("Failed to reopen the repaired database")?;
        // Another open of the file: guarded like the first (#608, #576 RV19a-8).
        super::check_schema(&conn)?;
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

        // Sections
        match db.try_read_sections() {
            Ok(sections) => {
                salvaged.section_count = sections.len();
                salvaged.sections_json = serde_json::to_string(&sections).ok();
            }
            Err(e) => salvaged
                .errors
                .push(format!("Failed to read sections: {e}")),
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

    /// List the files the recovery dialog offers to restore a project from.
    ///
    /// [#576 C15] Every candidate is labelled and checked: backups (Repair's
    /// numbered copies and the pre-upgrade copy), an interrupted save's `.tmp`,
    /// and the files earlier restores replaced (`.pre-restore.N`, which used to
    /// be left out - so a restore could not be undone from the dialog). A
    /// candidate that does not open, has a schema every open refuses (#608),
    /// fails the quick integrity check or holds no project is still listed - so
    /// the user sees it exists - but marked
    /// unusable with the reason. Journals, staging files and the project itself
    /// are never offered.
    pub fn list_backups(path: &Path) -> Vec<BackupInfo> {
        let mut backups = Vec::new();
        let Some(parent) = path.parent() else {
            return backups;
        };
        let stem = path.file_name().unwrap_or_default().to_string_lossy();

        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Some((kind, restore_number)) = classify_candidate(&stem, &name) else {
                    continue;
                };
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let problem = Self::check_restore_candidate(&entry.path()).err();
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
                    kind,
                    restore_number,
                    usable: problem.is_none(),
                    problem,
                });
            }
        }

        // Sort by modification time, newest first
        backups.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));
        backups
    }

    /// [#576 C15] Can `candidate` be restored? It must open, pass the schema
    /// guard (#608) and the quick integrity check and hold a project - as a
    /// restore would install it, i.e.
    /// together with the rollback journal beside it if there is one - and the
    /// check must not change it.
    ///
    /// Without a journal the file is self-contained and is opened read-only
    /// (leaving nothing beside a WAL-mode file, #576 W35-NF-1). With one, its
    /// committed contents only appear once the journal is rolled back, which is
    /// a write, so that happens on a copy beside it that is removed again.
    fn check_restore_candidate(candidate: &Path) -> std::result::Result<(), String> {
        if !journal_of(candidate).exists() {
            let conn = super::open::connect_for_reading(candidate)
                .map_err(|e| format!("it cannot be opened ({e})"))?;
            return check_candidate_connection(conn);
        }
        let staging = sibling(candidate, CHECK_STAGING_SUFFIX);
        let checked = copy_with_journal(candidate, &staging)
            .map_err(|e| format!("it could not be checked ({e})"))
            .and_then(|()| {
                let conn =
                    Connection::open(&staging).map_err(|e| format!("it cannot be opened ({e})"))?;
                check_candidate_connection(conn)
            });
        remove_with_journal(&staging);
        checked
    }

    /// [#576 C15, NF-AN2-7] Replace the project at `project` with the restore
    /// candidate `candidate`, safely:
    ///
    /// 1. The candidate (with its rollback journal, if one is beside it - an
    ///    interrupted save's `.tmp` can have one) is copied to a staging file
    ///    beside the project and checked there: opening it read-write rolls the
    ///    journal back to the last committed state, then the schema guard
    ///    (#608), quick_check and a project inside. A candidate that fails is
    ///    refused here, with nothing
    ///    else touched. The candidate itself is never changed.
    /// 2. The current project file is kept as a NUMBERED safety copy
    ///    (`.pre-restore.N`, the first free N), never overwriting an earlier one,
    ///    and a rollback journal left beside it moves aside WITH it: it belongs to
    ///    that file, and left in place it would be rolled back into the restored
    ///    one on the next open. If the copy cannot be made, nothing is restored.
    /// 3. The staged file is renamed over the project - one atomic step, so an
    ///    interruption leaves either the old file or the restored one, never a
    ///    half-copied mix. If the rename fails, step 2 is undone: the journal
    ///    goes back beside the project and the safety copy is removed.
    pub fn restore_from_backup(project: &Path, candidate: &Path) -> Result<RestoreOutcome> {
        if !candidate.exists() {
            anyhow::bail!("Backup file not found: {}", candidate.display());
        }
        if candidate == project {
            anyhow::bail!("A project cannot be restored from itself");
        }

        // 1. Stage and check.
        let staging = sibling(project, RESTORE_STAGING_SUFFIX);
        copy_with_journal(candidate, &staging).with_context(|| {
            format!(
                "Could not copy {} beside the project to check it",
                candidate.display()
            )
        })?;
        let checked = Connection::open(&staging)
            .map_err(|e| format!("it cannot be opened ({e})"))
            .and_then(check_candidate_connection);
        if let Err(problem) = checked {
            remove_with_journal(&staging);
            anyhow::bail!(
                "This file cannot be restored: {problem}. Your project file was not changed."
            );
        }
        // The check's read rolled any copied journal back and deleted it; one still
        // there would be applied to the project after the rename.
        if journal_of(&staging).exists() {
            remove_with_journal(&staging);
            anyhow::bail!(
                "The copy of {} still had an unfinished write after checking; nothing \
                 was restored and your project file was not changed",
                candidate.display()
            );
        }

        // 2. Keep the file being replaced, with its journal.
        let safety_copy = if project.exists() {
            match Self::keep_pre_restore_copy(project) {
                Ok(copy) => Some(copy),
                Err(e) => {
                    remove_with_journal(&staging);
                    return Err(e.context(
                        "Could not keep a copy of your current project file, so nothing was \
                         restored",
                    ));
                }
            }
        } else {
            None
        };

        // 3. Install.
        if let Err(e) = install_staged(&staging, project) {
            remove_with_journal(&staging);
            let message = Self::undo_pre_restore_copy(project, safety_copy.as_ref());
            return Err(anyhow::Error::from(e).context(message));
        }
        let safety_copy = safety_copy.map(|copy| copy.path);
        tracing::info!(
            project = ?project,
            candidate = ?candidate,
            safety_copy = ?safety_copy,
            purpose = "restore a project from the recovery dialog",
            "Restored the project from a backup"
        );
        Ok(RestoreOutcome { safety_copy })
    }

    /// [#576 RV3-R-6] Undo step 2 of a restore whose install failed, and say what
    /// the user is left with. The project's rollback journal goes back beside it
    /// (left with the safety copy, the project's pages opened torn), and the
    /// safety copy - a duplicate of an unchanged file, and torn itself once its
    /// journal is gone - is removed.
    fn undo_pre_restore_copy(project: &Path, copy: Option<&PreRestoreCopy>) -> String {
        const UNCHANGED: &str =
            "Could not put the restored file in place; your project file was not changed";
        let Some(copy) = copy else {
            return UNCHANGED.to_string();
        };
        if copy.took_journal {
            if let Err(e) = std::fs::rename(journal_of(&copy.path), journal_of(project)) {
                tracing::error!(
                    project = ?project,
                    safety_copy = ?copy.path,
                    error = %e,
                    purpose = "restore a project from the recovery dialog",
                    "Could not move the project's journal back after a failed restore"
                );
                return format!(
                    "Could not put the restored file in place, and the record of an unfinished \
                     write that belongs to your project could not be moved back beside it ({e}). \
                     It is kept with the safety copy {}: restore that copy to get your project \
                     back as it was",
                    copy.path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
        if project.exists() {
            let _ = std::fs::remove_file(&copy.path);
        }
        UNCHANGED.to_string()
    }

    /// [#576 C15] Copy `project` to the first free `<name>.pre-restore.N` (from
    /// 1), never overwriting, and move a rollback journal left beside `project`
    /// to `<name>.pre-restore.N-journal`, so the copy and its journal stay a pair.
    fn keep_pre_restore_copy(project: &Path) -> Result<PreRestoreCopy> {
        for n in 1..=10_000u32 {
            let copy = sibling(project, &format!(".pre-restore.{n}"));
            if copy.symlink_metadata().is_ok() {
                continue;
            }
            let mut target = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&copy)
                .with_context(|| format!("create {}", copy.display()))?;
            let written = std::fs::File::open(project)
                .and_then(|mut source| std::io::copy(&mut source, &mut target))
                .and_then(|_| target.sync_all());
            drop(target);
            if let Err(e) = written {
                let _ = std::fs::remove_file(&copy);
                return Err(anyhow::Error::from(e).context(format!("write {}", copy.display())));
            }
            let journal = journal_of(project);
            let took_journal = journal.exists();
            if took_journal {
                if let Err(e) = std::fs::rename(&journal, journal_of(&copy)) {
                    let _ = std::fs::remove_file(&copy);
                    return Err(anyhow::Error::from(e)
                        .context("move the project's unfinished-write journal aside"));
                }
            }
            return Ok(PreRestoreCopy {
                path: copy,
                took_journal,
            });
        }
        anyhow::bail!("Too many earlier pre-restore copies beside the project")
    }

    /// Create a numbered backup: file.stalx.backup, .backup.1, .backup.2, etc.
    ///
    /// [#576 NF-W3-3] With the project's rollback journal, when it has one: a
    /// project frozen mid-write holds pages its journal undoes, so the main
    /// file copied alone is torn, while the pair opens as the last committed
    /// state (the restore list never offers the journal on its own).
    pub(super) fn create_numbered_backup(path: &Path) -> Result<std::path::PathBuf> {
        let base = path.with_extension("stalx.backup");
        if !base.exists() {
            copy_with_journal(path, &base)?;
            return Ok(base);
        }
        for i in 1..100 {
            let numbered = path.with_extension(format!("stalx.backup.{i}"));
            if !numbered.exists() {
                copy_with_journal(path, &numbered)?;
                return Ok(numbered);
            }
        }
        anyhow::bail!("Too many existing backups (max 100)")
    }

    /// Whether the salvage can read the table `name`: a `sqlite_master` row of
    /// type `table` in any ASCII case. [#576 W52-6] The salvage's schema guard
    /// checks a type cell that lost its case ('Table') as the table it is
    /// (`check_schema_for_salvage`, RV19a-12), and SQLite reads it, so the
    /// summary below reads it too: under [`Self::table_exists`]'s exact match a
    /// 'Table' project table was reported as "Project table is empty", beside
    /// the full project the salvage read back from it.
    fn salvage_table_exists(&self, name: &str) -> Result<bool> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE lower(type) = 'table' AND name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(exists > 0)
    }

    pub(super) fn try_read_project_metadata(&self) -> Result<Option<(String, Option<String>)>> {
        if !self.salvage_table_exists("project")? {
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

    pub(super) fn try_read_sections(&self) -> Result<Vec<SalvagedSection>> {
        if !self.salvage_table_exists("sections")? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, surveyor, date, color FROM sections ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok(SalvagedSection {
                id: row.get(0)?,
                name: row.get(1)?,
                surveyor: row.get(2)?,
                date: row.get(3)?,
                color: row.get(4)?,
            })
        })?;
        let mut sections = Vec::new();
        for row in rows {
            sections.push(row?);
        }
        Ok(sections)
    }

    pub(super) fn try_read_shot_count(&self) -> Result<usize> {
        if !self.salvage_table_exists("shots")? {
            return Ok(0);
        }
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM shots", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub(super) fn try_read_merge_count(&self) -> Result<usize> {
        if !self.salvage_table_exists("station_merges")? {
            return Ok(0);
        }
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM station_merges", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::{Database, Path};

    #[test]
    fn fatal_tables_are_a_subset_of_local_tables() {
        // Every atomic/fatal history table is also part of the documented local-only
        // policy set. The sanitizer's best-effort loop skips exactly this subset.
        for table in Database::PUSH_SNAPSHOT_FATAL_TABLES {
            assert!(
                Database::PUSH_SNAPSHOT_LOCAL_TABLES.contains(table),
                "`{table}` is marked FATAL but is not in PUSH_SNAPSHOT_LOCAL_TABLES, so \
                 the strip loop never visits it and its fatality can never fire"
            );
        }
    }

    #[test]
    fn history_cleanup_is_fatal_and_rolls_back_as_one_unit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let snapshot = dir.path().join("snap.stalx");
        let mut db = Database::open(&snapshot).expect("open snapshot");
        db.save_project(&crate::models::Project::new("Snapshot".to_owned(), None))
            .expect("seed project");
        db.save_undo_stack(vec![(r#"{"c":1}"#.to_owned(), r#"{"u":1}"#.to_owned())])
            .expect("seed undo");
        db.save_redo_stack_with_timestamps(vec![(
            r#"{"c":2}"#.to_owned(),
            r#"{"u":2}"#.to_owned(),
            None,
        )])
        .expect("seed redo");
        db.save_speleo_undo_chain(Some("older-tip"))
            .expect("seed chain pointer");
        db.save_speleo_undo_watermark(Some("2026-08-01T00:00:00+00:00"))
            .expect("seed watermark");
        db.conn
            .execute_batch(
                "CREATE TRIGGER block_chain_delete BEFORE DELETE ON speleo_undo_chain \
                 BEGIN SELECT RAISE(ABORT, 'chain delete blocked'); END;",
            )
            .expect("install blocker");
        drop(db);

        let err = Database::sanitize_push_snapshot(&snapshot, false, None)
            .expect_err("a history-metadata cleanup failure must refuse the push");
        assert!(format!("{err:#}").contains("speleo_undo_chain"));

        // The inducer is a trigger, which the app's own open refuses (#608); drop
        // it before reopening to inspect what the refused push left behind.
        rusqlite::Connection::open(&snapshot)
            .expect("open to drop the inducer")
            .execute_batch("DROP TRIGGER block_chain_delete")
            .expect("drop the inducer");
        let db = Database::open_existing(&snapshot).expect("reopen refused snapshot");
        assert_eq!(db.load_undo_stack().expect("load undo").len(), 1);
        assert_eq!(
            db.load_redo_stack_with_timestamps()
                .expect("load redo")
                .len(),
            1
        );
        assert_eq!(
            db.load_speleo_undo_chain().expect("load chain pointer"),
            Some("older-tip".to_owned())
        );
        assert_eq!(
            db.load_speleo_undo_watermark().expect("load watermark"),
            Some("2026-08-01T00:00:00+00:00".to_owned())
        );
    }

    #[test]
    fn inherited_history_cut_rejects_invalid_timestamp_shapes() {
        fn rejected(stamps: [Option<&str>; 2], expected: &str) {
            let dir = tempfile::tempdir().expect("tempdir");
            let snapshot = dir.path().join("snap.stalx");
            let mut db = Database::open(&snapshot).expect("open snapshot");
            db.save_project(&crate::models::Project::new("Snapshot".to_owned(), None))
                .expect("seed project");
            if stamps.iter().any(Option::is_none) {
                // Legacy schemas allowed NULL timestamps. Recreate only this fixture
                // table so the invalid legacy ordering remains representable.
                db.conn
                    .execute_batch(
                        "DROP TABLE undo_stack; \
                         CREATE TABLE undo_stack ( \
                           id INTEGER PRIMARY KEY, sequence INTEGER NOT NULL, \
                           command_json BLOB NOT NULL, undo_data_json BLOB NOT NULL, \
                           executed_at TEXT \
                         );",
                    )
                    .expect("make legacy nullable timestamp table");
            }
            let row = |name: &str| {
                (
                    format!(r#"{{"type":"{name}"}}"#),
                    r#"{"undo":true}"#.to_owned(),
                    Some("2026-08-01T00:00:00+00:00".to_owned()),
                )
            };
            db.save_undo_stack_with_timestamps(vec![row("First"), row("Second")])
                .expect("seed undo");
            for (sequence, stamp) in stamps.into_iter().enumerate() {
                db.conn
                    .execute(
                        "UPDATE undo_stack SET executed_at = ?1 WHERE sequence = ?2",
                        rusqlite::params![stamp, sequence as i64],
                    )
                    .expect("shape timestamp fixture");
            }
            drop(db);

            let err = Database::sanitize_push_snapshot(
                &snapshot,
                true,
                Some("2026-08-01T00:00:01+00:00"),
            )
            .expect_err("an ambiguous cutoff must refuse the push");
            assert!(format!("{err:#}").contains(expected));
            assert_eq!(
                Database::open_existing(&snapshot)
                    .expect("reopen refused snapshot")
                    .load_undo_stack()
                    .expect("load undo")
                    .len(),
                2,
                "a refused cutoff must leave the history untouched"
            );
        }

        rejected(
            [Some("not-a-timestamp"), Some("2026-08-01T00:00:00+00:00")],
            "malformed undo timestamp",
        );
        rejected(
            [Some("2026-08-01T00:00:00+00:00"), None],
            "missing undo timestamp after a timestamped row",
        );
    }

    #[test]
    fn sanitize_compacts_beside_the_snapshot_and_leaves_no_staging_file() {
        // The compaction is a `VACUUM INTO` a sibling + rename, not a plain `VACUUM`.
        // A plain VACUUM rewrites the whole database through SQLite's temp directory
        // (`SQLITE_TMPDIR` / `TMPDIR` / `/tmp`), which is nowhere near the snapshot, so
        // on a box with a small tmpfs `/tmp` a large project ran out of space there -
        // and, now that this failure is fatal to the push, that became a push the user
        // could never complete. `VACUUM INTO` writes into the snapshot's own directory,
        // which the caller already sized for a full copy of the project.
        //
        // Behaviourally: the sanitize succeeds, the staging file is consumed by the
        // rename rather than littering the staging dir, and the result is still a
        // readable database with the local tables emptied.
        let dir = tempfile::tempdir().expect("tempdir");
        let snapshot = dir.path().join("snap.stalx");
        let mut db = Database::open(&snapshot).expect("open snapshot");
        db.save_project(&crate::models::Project::new("Snap".to_owned(), None))
            .expect("save project");
        db.save_display_setting("grid", "on").expect("seed a local");
        drop(db);

        Database::sanitize_push_snapshot(&snapshot, false, None).expect("sanitize");

        assert!(
            !Database::vacuum_temp_path(&snapshot).exists(),
            "the staging copy must be renamed away, not left beside the snapshot"
        );
        let db = Database::open_for_metadata(&snapshot).expect("compacted snapshot reopens");
        assert!(
            db.load_display_settings()
                .expect("read display settings")
                .is_empty(),
            "the local-only tables are still stripped after the compaction"
        );
    }

    #[test]
    fn a_leftover_staging_file_does_not_wedge_every_later_push() {
        // `VACUUM INTO` refuses to overwrite an existing target, so a staging copy
        // left behind by a killed run would make every subsequent sanitize fail -
        // and since that failure is fatal to the push, the user could never complete
        // one again without knowing to delete a file they have never heard of. The
        // code removes the leftover first; nothing pinned that it does.
        let dir = tempfile::tempdir().expect("tempdir");
        let snapshot = dir.path().join("snap.stalx");
        let mut db = Database::open(&snapshot).expect("open snapshot");
        db.save_project(&crate::models::Project::new("Snap".to_owned(), None))
            .expect("save project");
        drop(db);

        // Exactly what a killed run leaves: a stale file at the staging path. Its
        // contents are irrelevant - VACUUM INTO objects to its mere existence.
        let stale = Database::vacuum_temp_path(&snapshot);
        std::fs::write(&stale, b"leftover from a killed push").expect("plant a leftover");

        Database::sanitize_push_snapshot(&snapshot, false, None)
            .expect("sanitize past the leftover");

        assert!(
            !stale.exists(),
            "the leftover must be consumed, not left to wedge the next push too"
        );
        Database::open_for_metadata(&snapshot).expect("sanitized snapshot still reopens");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_compaction_is_fatal_and_leaves_the_snapshot_intact() {
        // The compaction failure the fatal path exists for is "no room for a second
        // copy beside the snapshot". Out-of-space is impractical to stage, but an
        // unwritable staging directory reaches the same `VACUUM INTO` error arm. Two
        // things must hold: the push is REFUSED (silently uploading an uncompacted
        // snapshot would ship the freed pages this whole function exists to reclaim),
        // and the caller's snapshot is left byte-intact so a retry has something to
        // retry with.
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let staging = dir.path().join("staging");
        std::fs::create_dir(&staging).expect("staging dir");
        let snapshot = staging.join("snap.stalx");
        let mut db = Database::open(&snapshot).expect("open snapshot");
        db.save_project(&crate::models::Project::new("Snap".to_owned(), None))
            .expect("save project");
        drop(db);
        let before = std::fs::read(&snapshot).expect("read snapshot bytes");

        // Read+execute only: the existing snapshot stays readable, but no NEW file
        // (the `VACUUM INTO` target) can be created beside it.
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o555))
            .expect("make the staging dir unwritable");
        // ...unless we are root, for whom the mode bits are advisory. Probe the
        // property this test depends on rather than the uid behind it, and fail
        // when the bits do not bite: a skip would pass on nothing (#576
        // NF-ROOT-PERM; CI runs the Rust tests unprivileged).
        crate::models::test_support::assert_dir_refuses_new_files(&staging);
        let result = Database::sanitize_push_snapshot(&snapshot, false, None);
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
            .expect("restore permissions so tempdir can clean up");

        let err = result.expect_err("an uncompactable snapshot must refuse the push");
        let chain = format!("{err:#}");
        assert!(
            chain.contains("refusing to upload it"),
            "the error must say the push was refused, got: {chain}"
        );
        assert_eq!(
            std::fs::read(&snapshot).expect("snapshot still readable"),
            before,
            "a refused sanitize must leave the caller's snapshot byte-identical"
        );
        assert!(
            !Database::vacuum_temp_path(&snapshot).exists(),
            "a failed compaction must not leave a staging file behind"
        );
    }

    #[test]
    fn sanitize_never_falls_back_to_a_plain_vacuum() {
        // Which SQL a function issues is not observable from its result, so the
        // tmp-directory hazard is guarded at the source: a future edit that reaches
        // back for `VACUUM;` silently reinstates "a small /tmp blocks every push on a
        // large project".
        // A sanctioned source scan (#576 A8-D2, docs/testing-recipes.md "Source scans").
        use crate::models::source_scan::{fn_body, prod_region};
        let src = prod_region(include_str!("repair.rs"), "pub fn sanitize_push_snapshot");
        let body = fn_body(src, "pub fn sanitize_push_snapshot(");
        assert!(
            body.contains("VACUUM INTO ?1"),
            "RULE (no plain VACUUM on the push path): sanitize must compact via VACUUM \
             INTO a sibling"
        );
        assert!(
            !body.contains("\"VACUUM;\"") && !body.contains("execute_batch(\"VACUUM"),
            "RULE (no plain VACUUM on the push path): sanitize must NOT run a plain VACUUM \
             (it rewrites through SQLITE_TMPDIR)"
        );
    }

    #[test]
    fn vacuum_temp_path_is_a_sibling_of_the_snapshot() {
        // The rename that follows the VACUUM INTO must stay on one filesystem (a
        // cross-device rename fails with EXDEV), which only holds while the staging
        // file is a sibling.
        let path = Path::new("/some/dir/snap.stalx");
        let temp = Database::vacuum_temp_path(path);
        assert_eq!(temp.parent(), path.parent());
        assert_eq!(
            temp.file_name().and_then(|n| n.to_str()),
            Some("snap.stalx.vacuum.tmp")
        );
    }

    /// [#576 RVF-b-1] A push snapshot is never written through a link left at its
    /// name, dangling or to an existing file: the link is removed and the snapshot
    /// is a real file there. `exists()` follows a link and says no for a dangling
    /// one, so it stayed, and SQLite created the snapshot where it pointed.
    #[cfg(unix)]
    #[test]
    fn a_snapshot_is_never_written_through_a_link_at_its_name() {
        use super::super::upgrade_safety_tests::PlantedLink;

        for planted in PlantedLink::BOTH {
            let dir = tempfile::tempdir().expect("tempdir");
            let elsewhere = tempfile::tempdir().expect("tempdir");
            let live = dir.path().join("live.stalx");
            let mut db = Database::open(&live).expect("open live");
            db.save_project(&crate::models::Project::new("Live".to_owned(), None))
                .expect("save project");
            drop(db);
            let dest = dir.path().join("snapshot.stalx");
            let target = planted.plant(&dest, elsewhere.path());

            let snapshot = Database::vacuum_snapshot(&live, &dest);

            assert_eq!(
                planted.written_through(&target),
                None,
                "{planted:?}: the snapshot was written through the link at its name"
            );
            assert!(snapshot.is_ok(), "{planted:?}: {snapshot:?}");
            assert!(
                dest.symlink_metadata().is_ok_and(|m| m.is_file()),
                "{planted:?}: the snapshot is a real file at its name"
            );
            Database::open_for_metadata(&dest).expect("the snapshot opens");
        }
    }

    /// [#576 RVF-b-1] A link at the compaction's staging name that cannot be
    /// removed (the staging folder is read-only) refuses the push, and the link's
    /// target - in a writable folder - is not written: the leftover's removal was
    /// best-effort, and SQLite resolved the dangling link and created the compacted
    /// copy where it pointed. (A link to an existing, non-empty file SQLite
    /// refuses as an existing target.) The snapshot holds none of the tables the
    /// sanitize strips, so nothing before the compaction writes to it (in a
    /// read-only folder a strip could not create its journal).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_compaction_is_refused_when_a_link_at_its_staging_name_cannot_be_removed() {
        use super::super::upgrade_safety_tests::PlantedLink;
        use std::os::unix::fs::PermissionsExt as _;

        for planted in PlantedLink::BOTH {
            let dir = tempfile::tempdir().expect("tempdir");
            let elsewhere = tempfile::tempdir().expect("tempdir");
            let staging = dir.path().join("staging");
            std::fs::create_dir(&staging).expect("staging dir");
            let snapshot = staging.join("snap.stalx");
            super::Connection::open(&snapshot)
                .expect("open snapshot")
                .execute_batch("CREATE TABLE survey (x); INSERT INTO survey VALUES (1);")
                .expect("write snapshot");
            let before = std::fs::read(&snapshot).expect("read snapshot bytes");
            let target = planted.plant(&Database::vacuum_temp_path(&snapshot), elsewhere.path());

            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o555))
                .expect("make the staging dir unwritable");
            crate::models::test_support::assert_dir_refuses_new_files(&staging);
            let result = Database::sanitize_push_snapshot(&snapshot, false, None);
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
                .expect("restore permissions so tempdir can clean up");

            assert_eq!(
                planted.written_through(&target),
                None,
                "{planted:?}: the compacted copy was written through the link"
            );
            let err = result.expect_err("the push is refused");
            assert!(
                format!("{err:#}").contains("refusing to upload it"),
                "{planted:?}: {err:#}"
            );
            assert_eq!(
                std::fs::read(&snapshot).expect("snapshot still readable"),
                before,
                "{planted:?}: the snapshot is left byte-identical"
            );
        }
    }

    /// [#576 RVF-b-1] The journal half of `copy_with_journal` (a restore's or a
    /// candidate check's staging, a numbered backup) is never written through a
    /// link at `<to>-journal` that cannot be removed (the folder is read-only; a
    /// writable leftover at `<to>` itself lets the copy reach the journal): the
    /// copy fails and the link's target is not written. The removal's error was
    /// ignored and `fs::copy` followed the link.
    #[cfg(unix)]
    #[test]
    fn copying_with_a_journal_never_writes_through_a_link_at_the_journal_name() {
        use super::super::upgrade_safety_tests::PlantedLink;
        use super::{copy_with_journal, journal_of};
        use std::os::unix::fs::PermissionsExt as _;

        for planted in PlantedLink::BOTH {
            let dir = tempfile::tempdir().expect("tempdir");
            let elsewhere = tempfile::tempdir().expect("tempdir");
            let from = dir.path().join("CaveA.stalx.backup");
            std::fs::write(&from, b"the candidate").expect("write the candidate");
            std::fs::write(journal_of(&from), b"its journal").expect("write its journal");
            let locked = dir.path().join("locked");
            std::fs::create_dir(&locked).expect("locked dir");
            let to = locked.join("CaveA.stalx.restore-staging");
            std::fs::write(&to, b"a leftover").expect("write a leftover");
            let target = planted.plant(&journal_of(&to), elsewhere.path());

            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555))
                .expect("make the folder read-only");
            crate::models::test_support::assert_dir_refuses_new_files(&locked);
            let copied = copy_with_journal(&from, &to);
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
                .expect("restore permissions so tempdir can clean up");

            assert_eq!(
                planted.written_through(&target),
                None,
                "{planted:?}: the journal was copied through the link at its name"
            );
            assert!(copied.is_err(), "{planted:?}: the copy fails: {copied:?}");
        }
    }

    /// [#576 RVF-b-1] What every scratch writer clears its name with: a link there
    /// is removed and its target never touched; nothing there is fine; and an
    /// entry that cannot be removed (a read-only folder here) is an error, not a
    /// silent leftover the write would then follow.
    #[cfg(unix)]
    #[test]
    fn clearing_a_scratch_name_removes_a_link_and_reports_one_it_cannot() {
        use super::super::upgrade_safety_tests::PlantedLink;
        use super::clear_scratch_name;
        use std::os::unix::fs::PermissionsExt as _;

        for planted in PlantedLink::BOTH {
            let dir = tempfile::tempdir().expect("tempdir");
            let elsewhere = tempfile::tempdir().expect("tempdir");
            let name = dir.path().join("scratch");
            let target = planted.plant(&name, elsewhere.path());
            clear_scratch_name(&name).expect("a link in a writable folder is removed");
            assert!(
                name.symlink_metadata().is_err(),
                "{planted:?}: link removed"
            );
            assert_eq!(planted.written_through(&target), None, "{planted:?}");
            clear_scratch_name(&name).expect("nothing at the name is fine");

            let locked = dir.path().join("locked");
            std::fs::create_dir(&locked).expect("locked dir");
            let name = locked.join("scratch");
            let other = tempfile::tempdir().expect("tempdir");
            let target = planted.plant(&name, other.path());
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555))
                .expect("make the folder read-only");
            crate::models::test_support::assert_dir_refuses_new_files(&locked);
            let cleared = clear_scratch_name(&name);
            let nothing_there = clear_scratch_name(&locked.join("absent"));
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
                .expect("restore permissions so tempdir can clean up");
            assert!(
                cleared.is_err(),
                "{planted:?}: a link it cannot remove is an error"
            );
            assert!(nothing_there.is_ok(), "{planted:?}: {nothing_there:?}");
            assert_eq!(planted.written_through(&target), None, "{planted:?}");
        }
    }
}
