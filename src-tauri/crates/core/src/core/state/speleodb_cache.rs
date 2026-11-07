//! Replacing the local SpeleoDB cache `.stalx` with a freshly-downloaded copy.
//!
//! Three flows do this - the pull (`speleodb_open_project`), the fast-forward
//! (`speleodb_start_editing` / `speleodb_update`), and the revision swap
//! (`speleodb_restore_revision`) - and all three must handle the same two hazards
//! around clobbering a file the app may still hold open:
//!
//! 1. the per-machine `display_settings` in the OLD file have to survive the
//!    replace (the server copy has them stripped, #197), and
//! 2. any live SQLite handle to the destination has to be closed BEFORE the
//!    replace, and the replace itself has to be atomic.
//!
//! [`AppState::replace_cache_stalx`] is the single chokepoint that does all of
//! it, so no call site can forget half of it.
//!
//! It also carries the DATA-SAFETY half: the copy being replaced may hold edits
//! the server has never seen, and those exist in no git anywhere - overwriting
//! them is unrecoverable. So the plain verb REFUSES that replace
//! ([`SpeleoDbError::WouldDiscardLocalEdits`], which crosses IPC as a typed
//! routing signal for the keep-local / take-remote choice), and a caller that
//! genuinely means to discard has to say so by name through
//! [`AppState::replace_cache_stalx_abandoning`]. Same shape as the lock release
//! (`speleodb_release_held_lock_best_effort` vs `_abandoning`): the safe verb is
//! the one you reach for by default, and abandoning is spelled out at the call
//! site. Every caller entitled to it arrives from a frontend that has already had
//! the loss confirmed by name: `speleodb_take_remote` (the divergence prompt's
//! destroy-confirmed "Take remote"), `speleodb_open_project_discarding_local` (the
//! same confirmed choice on a refused open), and `speleodb_restore_revision` under
//! its `discard_local` waiver (a Restore only, never a View - a restore stamps the
//! copy un-pushed itself, so without the waiver Restore was single-use). Anything
//! else passes [`LocalEditsPolicy::Protect`], or calls
//! [`AppState::replace_cache_stalx`], and refuses instead.

use std::collections::HashMap;
use std::path::Path;

use super::AppState;
use crate::speleodb::dump::Dump;
use crate::speleodb::SpeleoDbError;

/// Snapshot the local per-machine `display_settings` from an existing cache file
/// BEFORE a download-replace overwrites it. View toggles are stripped from the
/// server copy on push (#197), so without re-applying them a fast-forward / re-open
/// - which replaces the whole local `.stalx` with the server's - would reset the
/// user's view to defaults. Empty when the file is absent or unreadable (a first
/// open → defaults, which is correct). Pairs with [`restore_display_settings`].
///
/// Opens READ-ONLY via `open_for_metadata`: this is a handful of `SELECT`s, so it
/// needs neither the integrity check / migration chain nor `open_existing`'s
/// read-write connection with its 256 MB mmap - and it must not add another
/// mmap'd handle to a file that is about to be replaced.
pub(super) fn preserved_display_settings(dest: &Path) -> HashMap<String, String> {
    if !dest.exists() {
        return HashMap::new();
    }
    crate::persistence::Database::open_for_metadata(dest)
        .and_then(|db| db.load_display_settings())
        .unwrap_or_default()
}

/// [#303] The `speleo_link.undo_mode` already recorded in the `.stalx` at `dest`.
///
/// Every stamp site rewrites the WHOLE link row, so without this a pull / fast-forward /
/// revision swap would reset the project's explicit undo policy, silently discarding a
/// `Disabled` that the project owner set precisely because the
/// project is too big to ship undo snapshots. The value rides in the pushed snapshot
/// (it is project state, not machine state), so the value in the file we just extracted
/// IS the authoritative answer and the stamp has to carry it forward.
///
/// `None` when the file carries no link or a legacy NULL. The caller materializes the
/// application default before stamping the shared row. Takes the handle each stamp site
/// has ALREADY opened on the file it is about to stamp, rather than a path: a second
/// connection to a file mid-replace is exactly what the rest of this module avoids.
pub(super) fn preserved_undo_mode(
    db: &crate::persistence::Database,
) -> Option<crate::models::UndoMode> {
    db.load_speleo_link()
        .ok()
        .flatten()
        .and_then(|l| l.undo_mode)
}

/// [#303] Empty the undo/redo tables in a freshly-downloaded cache file.
///
/// The file arrives carrying whatever the last pusher's session held (a Full-mode push
/// ships its undo history on purpose), and inheriting that implicitly is the #194 b3
/// replay hazard. Clearing it HERE - in the bytes on disk, before anything opens them -
/// rather than in memory after the load is deliberate: the load path reads these tables
/// into the live stack, and a clear that only touched memory would leave the history in
/// the file for the next open to pick up again.
///
/// Fatal while the file is still staged. A downloaded database is never installed until
/// all inherited history metadata has been removed successfully - bar a file a newer
/// build saved, which is never written ([`saved_by_a_newer_build`]).
fn drop_inherited_undo(dest: &Path) -> Result<(), SpeleoDbError> {
    crate::persistence::Database::clear_undo_history_at(dest).map_err(|e| {
        SpeleoDbError::Io(format!(
            "could not clear inherited history from {}: {e}",
            dest.display()
        ))
    })
}

/// [#576 NF-W10BE-3] Whether the file at `path` was saved by a NEWER build, read off
/// its SQLite header bytes alone - opening a connection to ask is not a read-only
/// act. `false` when the header cannot be read: the clear that follows then reports
/// what the file is.
fn saved_by_a_newer_build(path: &Path) -> bool {
    use std::io::Read as _;
    let mut head = [0u8; crate::persistence::Database::HEADER_LEN];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok_and(|()| {
            crate::persistence::Database::newer_schema_version_in_header(&head).is_some()
        })
}

fn sanitized_download_path(dest: &Path) -> std::path::PathBuf {
    let mut name = dest
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("project.stalx"))
        .to_os_string();
    name.push(".sanitize.tmp");
    dest.with_file_name(name)
}

/// [#576 RV19a-3] Remove the staged download and every SQLite sidecar beside
/// it. The history cleanup writes into the staged file (`drop_inherited_undo`),
/// so a crash there leaves a hot `-journal` under the staged name; the next
/// download is extracted under the same name, and any open that may write
/// would play that journal back into the fresh bytes - the old file's pages
/// became the new cache. Removing the file alone left the journal behind.
fn remove_staged_with_sidecars(staged: &Path) {
    let _ = std::fs::remove_file(staged);
    remove_sidecars(staged);
}

/// Put the finished file at `staged` in place as `dest` - a same-directory
/// rename, so `dest` is either the old file or the new one - and drop the SQLite
/// sidecars the old file left beside it. The ONE install of a file over the
/// cache `.stalx`, for the download replace and the Ariane adoption alike, so
/// the two cannot drift (#576 RV20-2).
///
/// [#576 W52-2] Sidecars beside `dest` are the OLD file's - a crash while it
/// was open leaves a hot `-journal` (or a `-wal`/`-shm`) - and a rename keeps
/// them beside the new file, whose next read-write open would play the old
/// file's pages back into it.
///
/// [#576 RV20-3] So they are settled BEFORE the rename ([`settle_sidecars`]),
/// and the install is refused, `dest` left as it was, when that cannot be done.
/// Removing them only after the rename left two windows in which the old
/// journal stayed beside the new file: a crash between the rename and the
/// removal, and a removal that failed (a Windows sharing violation) and was
/// only logged. The removal after the rename stays, as a backstop for a sidecar
/// that appeared since (another process opening `dest`).
pub(super) fn install_over(staged: &Path, dest: &Path) -> std::io::Result<()> {
    settle_sidecars(dest)?;
    std::fs::rename(staged, dest)?;
    #[cfg(test)]
    if SKIP_REMOVAL_AFTER_RENAME.with(std::cell::Cell::get) {
        return Ok(());
    }
    remove_sidecars(dest);
    Ok(())
}

/// The SQLite sidecar suffixes, the rollback journal first.
const SIDECAR_SUFFIXES: [&str; 3] = ["-journal", "-wal", "-shm"];

/// `path` with an SQLite sidecar suffix.
fn sidecar_path(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

/// Whether anything is at `path`: anything but "not found" counts, so a file
/// another program still holds (a Windows delete-pending one) is there.
fn is_present(path: &Path) -> bool {
    !matches!(path.symlink_metadata(), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// Whether the rollback journal at `journal` still reads as hot to SQLite: there,
/// and its first byte not zero. An empty or zeroed journal is one SQLite ignores.
fn looks_hot(journal: &Path) -> std::io::Result<bool> {
    use std::io::Read as _;
    let mut file = match std::fs::File::open(journal) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        opened => opened?,
    };
    let mut first = [0u8; 1];
    Ok(file.read(&mut first)? == 1 && first[0] != 0)
}

/// [#576 RV20-3] Make sure no SQLite sidecar is left beside `dest` before a rename
/// puts another file there, without losing what one may still hold for `dest`.
///
/// 1. `dest` is opened read-write and read, once, then closed: what its next
///    ordinary open would do, done now. SQLite rolls a hot `-journal` back into
///    `dest` - the last save, intact - and folds a `-wal` into it, removing both;
///    a stale `-wal`/`-shm` that is not `dest`'s goes too. This cannot lose
///    anything the next open of `dest` would keep: it IS that open. What the read
///    returns does not matter (a copy damaged under its journal is still the one
///    being replaced); only what is left beside it does.
/// 2. A `-journal` still hot after that is one SQLite could not roll back - an
///    I/O error, a read-only file, a sharing violation, a writer elsewhere holding
///    the file - and the only thing that can still restore `dest`. It refuses the
///    install, `dest` and its journal left as they were.
/// 3. Whatever is left (a journal SQLite ignores, a `-wal`/`-shm` nobody needs)
///    is removed; a removal that fails, or a sidecar still there after it, refuses
///    the install - the case the removal after the rename only logged.
///
/// A sidecar beside a `dest` that is not there belongs to no file and is
/// removed. Nothing to do, and no open, when no sidecar is there - the common case.
///
/// "Beside" is beside `dest` itself, also when `dest` is a symlink, whose
/// file's journal SQLite's unix VFS names after the resolved path: the rename
/// replaces the link, not the file it points at, so the new cache's sidecars
/// are named after `dest`, and the old file keeps its own journal, with which
/// it is still its last save.
fn settle_sidecars(dest: &Path) -> std::io::Result<()> {
    let sidecars = SIDECAR_SUFFIXES.map(|suffix| sidecar_path(dest, suffix));
    if !sidecars.iter().any(|sidecar| is_present(sidecar)) {
        return Ok(());
    }
    if is_present(dest) {
        if let Ok(conn) = open_to_settle(dest) {
            let _ = conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()));
            drop(conn);
        }
        let [journal, ..] = &sidecars;
        if looks_hot(journal)? {
            tracing::warn!(
                path = %dest.display(),
                purpose = "never install a file beside a journal that belongs to the old one",
                "speleodb: the old file's hot journal could not be rolled back; not replacing it"
            );
            return Err(std::io::Error::other(
                "the journal of an interrupted save beside it could not be rolled back",
            ));
        }
    }
    for sidecar in &sidecars {
        match std::fs::remove_file(sidecar) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("could not remove {}: {e}", sidecar.display()),
                ));
            }
            _ => {}
        }
        if is_present(sidecar) {
            return Err(std::io::Error::other(format!(
                "{} is still there after its removal",
                sidecar.display()
            )));
        }
    }
    Ok(())
}

/// The read-write connection [`settle_sidecars`] reads `dest` through, with the
/// hardening every other open of an existing file gets: `dest` is a server
/// download, a file from outside the app. The #619 authorizer goes on before
/// any statement, then the schema guard turns on defensive mode, turns
/// `trusted_schema` off and refuses a schema the app never writes. The guard's
/// own read of `sqlite_master` is a read like any other: a hot journal is
/// rolled back by it, also when the guard then refuses the file.
fn open_to_settle(dest: &Path) -> anyhow::Result<rusqlite::Connection> {
    let mut conn =
        rusqlite::Connection::open_with_flags(dest, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    crate::persistence::deny_stripped_sqlite_features(&mut conn)?;
    crate::persistence::check_schema(&conn)?;
    Ok(conn)
}

#[cfg(test)]
thread_local! {
    /// Test seam (#576 RV20-3): [`install_over`] stops right after its rename, on
    /// the thread that set this, as a crash there - or a removal that failed and
    /// was only logged - leaves the install.
    static SKIP_REMOVAL_AFTER_RENAME: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Remove the SQLite sidecars (`-journal`, `-wal`, `-shm`) beside `path`. One
/// that is not there is the common case; any other failure is logged, as the
/// sidecar then stays beside the file.
fn remove_sidecars(path: &Path) {
    for suffix in SIDECAR_SUFFIXES {
        let sidecar = sidecar_path(path, suffix);
        match std::fs::remove_file(&sidecar) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => tracing::warn!(
                path = %sidecar.display(),
                error = %e,
                purpose = "remove a stale SQLite sidecar",
                "speleodb: could not remove a sidecar beside the cache file"
            ),
            _ => {}
        }
    }
}

/// Re-apply preserved local `display_settings` into a freshly-downloaded cache file
/// (companion to [`preserved_display_settings`]). Best-effort per key - a failed
/// restore only means that one view toggle falls back to its default.
pub(super) fn restore_display_settings(
    db: &crate::persistence::Database,
    preserved: &HashMap<String, String>,
) {
    for (key, value) in preserved {
        if let Err(e) = db.save_display_setting(key, value) {
            tracing::warn!(error = %e, key = %key, "failed to restore local display setting after download-replace");
        }
    }
}

/// Stamp a freshly downloaded cache `.stalx` (opened with `open_existing`): put the
/// preserved local display settings back and write the hand-off link, which
/// `link_for` builds from the undo policy the file itself carries (read before the
/// upsert overwrites the row). The one body the three pull paths share - open,
/// fast-forward and revision swap - so they cannot stamp differently.
///
/// [#576 NF-PNEWER-2] A file a NEWER build saved gets neither: the link upsert
/// writes this build's columns over the newer build's row (resetting any column it
/// added), and the newer file is otherwise never written by this build (#576
/// B6-D1).
///
/// That branch is the first-time Open's (#576 RV4-R-S7-D2): the only pull that
/// installs a newer file, because it has no linked copy to protect. The file opens
/// read-only with the newer-version notice, and without the link it is not treated
/// as a remote copy, which a build that can neither edit nor push it could not use
/// anyway. Every path that would replace a linked copy - a re-open over one, the
/// fast-forward, the revision View / Restore - refuses a newer download before the
/// swap (`speleodb_pull::refuse_a_newer_download`) and never gets here with one.
pub(super) fn stamp_pulled_file(
    db: &crate::persistence::Database,
    preserved_view: &HashMap<String, String>,
    link_for: impl FnOnce(Option<crate::models::UndoMode>) -> crate::models::SpeleoLink,
) -> anyhow::Result<()> {
    if let Some(version) = db.newer_schema_version() {
        tracing::warn!(
            version,
            purpose = "never write into a file a newer build saved (#576 NF-PNEWER-2)",
            "speleodb: the downloaded file was saved by a newer version; not stamping it"
        );
        return Ok(());
    }
    restore_display_settings(db, preserved_view);
    let link = link_for(preserved_undo_mode(db));
    db.save_speleo_link(Some(&link))?;
    Ok(())
}

/// Whether a re-pull may destroy un-pushed local edits at its destination.
///
/// For the orchestration paths that serve BOTH kinds of caller with the same
/// mechanics - `speleodb_fast_forward`, the shared body behind
/// `speleodb_open_project` / `speleodb_open_project_discarding_local`, and
/// `speleodb_restore_revision`'s View-vs-Restore split - so the intent is named at
/// every call site instead of riding on a bare `bool`, and so a diff that adds a
/// waiver says `Discard` in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalEditsPolicy {
    /// Refuse the replace when the destination holds un-pushed edits. The default
    /// for every path the user did not explicitly point at their own work.
    Protect,
    /// Discard them: the user confirmed it (take-remote's destroy-confirmation, or
    /// the "take remote and discard local" branch of the open-project prompt).
    Discard,
}

/// [#303] What to do with the undo history riding IN the downloaded `.stalx`.
///
/// Now that a Full-mode push ships its undo stack, every server copy arrives carrying
/// the last pusher's edit history. Inheriting that IMPLICITLY is the #194 b3 hazard:
/// press Ctrl-Z on a freshly-pulled project and you replay a stranger's command, with
/// its prior-state payload, against a tree you have not looked at yet.
///
/// So the default is [`Self::Drop`] on every path that replaces the cache file, and the
/// history is only ever handed back through the ONE explicit, confirmed, lock-held route
/// that exists to ask for it: the Unwind. `undo_history_keep_is_unwind_only` pins that
/// there is exactly one `Keep` site, the same way the `LocalEditsPolicy::Discard`
/// tripwire pins the waivers above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InheritedUndo {
    /// Empty the downloaded file's undo/redo tables. Every implicit adopt path.
    Drop,
    /// Keep them: the caller is the Unwind, and that history is what it came for.
    Keep,
}

/// [#576 A8 NF-A8-BYPASS] Proof a caller hands [`AppState::replace_cache_stalx_with`]:
/// whatever had to run before the cache file is replaced did. The pull's
/// un-pushed-edits guard (`LocalEditsCleared`), the revision swap's child-policy
/// check (`ChildPolicyChecked`) and the fast-forward's pinned download
/// (`PinnedDump`) implement it.
///
/// A proof stops a reorder at its own call site, but a replace callable with
/// strings alone could still be written above the step it must follow. Taking the
/// proof as an argument closes that: the replace has no other production entry
/// point (the guarded and abandoning bodies are private to this module).
/// Implementing this anywhere but on such a proof is the bypass it exists to stop.
pub(super) trait ReplaceLicence {}

impl AppState {
    /// Refuse when the copy of `project_id` at `dest` still holds edits the server
    /// has never seen. The gate in front of every download-replace, and the reason
    /// re-opening a project from the SpeleoDB list can no longer silently destroy a
    /// week of unpushed survey.
    ///
    /// Reads through [`AppState::local_copy_state`], so the OPEN project's live
    /// in-memory link counts - the case a naive on-disk-only check gets wrong, and
    /// the one where the user is most likely to be actively working.
    ///
    /// Public to the module so the ONE replace that does not go through
    /// [`AppState::replace_cache_stalx`] - Ariane adoption, which rebuilds the
    /// cache `.stalx` from an imported `.tml` rather than extracting it - can share
    /// the same gate.
    pub(super) fn guard_local_edits(
        &self,
        project_id: &str,
        dest: &Path,
    ) -> Result<(), SpeleoDbError> {
        // `None` = the file is there but unreadable, so we CANNOT rule out un-pushed
        // edits. A guard that stands in front of an overwrite has to fail closed on
        // that: the likely causes are a hot journal after a kill or power loss (which
        // a read-only connection cannot roll back - `SQLITE_READONLY_ROLLBACK`) and a
        // transient `SQLITE_BUSY`, and in both cases a read-write open would have
        // recovered work that reading the silence as "clean" would have overwritten.
        // Refusing is recoverable in a way overwriting is not: the frontend's prompt
        // offers keep-local (nothing is touched) or an explicit take-remote.
        //
        // [#576 W53] Refused as `state_unknown`, not as un-pushed edits: the same
        // routing, but the user is told the copy could not be read rather than about
        // edits that its link row, could it be read, may well say were never made.
        let Some(local) = self.local_copy_state(project_id, dest) else {
            tracing::warn!(
                project_id = %project_id,
                path = %dest.display(),
                purpose = "fail closed when the local copy cannot be read",
                "speleodb: refusing to replace a local copy whose state could not be determined"
            );
            return Err(SpeleoDbError::WouldDiscardLocalEdits {
                project_id: project_id.to_owned(),
                local_path: dest.to_string_lossy().into_owned(),
                last_edit_at: None,
                state_unknown: true,
            });
        };
        if !local.has_unpushed_edits {
            return Ok(());
        }
        tracing::info!(
            project_id = %project_id,
            path = %dest.display(),
            is_open = local.is_open,
            purpose = "avoid silently destroying un-pushed local work on a re-pull",
            "speleodb: refusing to replace a local copy that has un-pushed edits"
        );
        Err(SpeleoDbError::WouldDiscardLocalEdits {
            project_id: project_id.to_owned(),
            local_path: dest.to_string_lossy().into_owned(),
            last_edit_at: local.last_edit_at,
            state_unknown: false,
        })
    }

    /// Replace the cache `.stalx` at `dest` with `entry` extracted from `dump`,
    /// returning the local view toggles the caller must re-apply to the fresh file
    /// (see [`restore_display_settings`]).
    ///
    /// REFUSES (without touching `dest`) when the local copy of `project_id` holds
    /// un-pushed edits - see [`AppState::guard_local_edits`]. A caller that means
    /// to discard them calls [`AppState::replace_cache_stalx_abandoning`] instead.
    ///
    /// Order matters:
    ///
    /// 1. read the outgoing file's `display_settings` while it still exists;
    /// 2. close the open project IF it is this very file, so no live SQLite handle
    ///    (and no 256 MB mmap) survives into the replace - see
    ///    [`AppState::close_project_if_open_at_held`] for why that is required on
    ///    Windows and why it is conditional;
    /// 3. extract, which stages to a sibling temp file and RENAMES over `dest`, so
    ///    an interrupted extract leaves the old copy intact rather than a stub.
    ///
    /// Steps 2 and 3 cover different failure modes and neither replaces the other.
    ///
    /// [#576 A8 NF-A8-BYPASS] Test builds only: the orchestration replaces the cache
    /// file through [`AppState::replace_cache_stalx_with`] alone, which takes a
    /// [`ReplaceLicence`]. The body is the private `replace_cache_stalx_guarded`;
    /// this name exists for the tests elsewhere in `core::state` that drive the
    /// replace directly.
    #[cfg(test)]
    pub(super) fn replace_cache_stalx(
        &self,
        dump: &mut Dump<'_>,
        entry: &str,
        dest: &Path,
        project_id: &str,
        undo: InheritedUndo,
    ) -> Result<HashMap<String, String>, SpeleoDbError> {
        self.replace_cache_stalx_guarded(dump, entry, dest, project_id, undo)
    }

    /// The guarded replace's body (see [`AppState::replace_cache_stalx`]).
    fn replace_cache_stalx_guarded(
        &self,
        dump: &mut Dump<'_>,
        entry: &str,
        dest: &Path,
        project_id: &str,
        undo: InheritedUndo,
    ) -> Result<HashMap<String, String>, SpeleoDbError> {
        self.guard_local_edits(project_id, dest)?;
        self.replace_cache_stalx_unguarded(dump, entry, dest, undo)
    }

    /// [`AppState::replace_cache_stalx`] or its abandoning sibling, chosen by an
    /// explicit [`LocalEditsPolicy`]. For the orchestration verbs that serve both a
    /// protected and a confirmed-discard caller and therefore cannot pick one verb
    /// statically; their own callers name the policy.
    ///
    /// [#576 A8 NF-A8-BYPASS] The ONE way production replaces the cache file, and it
    /// takes the proof of whatever had to run first (`_licence`, see
    /// [`ReplaceLicence`]): a replace written without one does not compile.
    pub(super) fn replace_cache_stalx_with(
        &self,
        _licence: &impl ReplaceLicence,
        dump: &mut Dump<'_>,
        entry: &str,
        dest: &Path,
        project_id: &str,
        policy: LocalEditsPolicy,
        undo: InheritedUndo,
    ) -> Result<HashMap<String, String>, SpeleoDbError> {
        match policy {
            LocalEditsPolicy::Protect => {
                self.replace_cache_stalx_guarded(dump, entry, dest, project_id, undo)
            }
            LocalEditsPolicy::Discard => {
                self.replace_cache_stalx_unguarded(dump, entry, dest, undo)
            }
        }
    }

    /// [`AppState::replace_cache_stalx`] WITHOUT the un-pushed-edits guard, for a
    /// caller that is deliberately throwing the local edits away - today only
    /// `speleodb_take_remote` (and the open-project prompt's "take remote and
    /// discard local" branch, which routes through the same policy). Named so the
    /// destruction is visible at the call site rather than implied by a flag.
    ///
    /// [#576 A8 NF-A8-BYPASS] Test builds only, like [`AppState::replace_cache_stalx`]:
    /// production waives the guard solely by naming [`LocalEditsPolicy::Discard`] to
    /// [`AppState::replace_cache_stalx_with`].
    #[cfg(test)]
    pub(super) fn replace_cache_stalx_abandoning(
        &self,
        dump: &mut Dump<'_>,
        entry: &str,
        dest: &Path,
        undo: InheritedUndo,
    ) -> Result<HashMap<String, String>, SpeleoDbError> {
        self.replace_cache_stalx_unguarded(dump, entry, dest, undo)
    }

    /// The abandoning replace's body (see [`AppState::replace_cache_stalx_abandoning`]).
    fn replace_cache_stalx_unguarded(
        &self,
        dump: &mut Dump<'_>,
        entry: &str,
        dest: &Path,
        undo: InheritedUndo,
    ) -> Result<HashMap<String, String>, SpeleoDbError> {
        // [#576 RV6a-5] Read the view to carry over and close in ONE `command_lock`
        // span. A display toggle writes through the open handle under that lock
        // (`save_display_setting`), so it now lands wholly before the read or after
        // the close - never between them, into the file about to be replaced.
        let (preserved_view, closed) = {
            let commands = self.hold_commands();
            let view = preserved_display_settings(dest);
            (view, self.close_project_if_open_at_held(&commands, dest))
        };
        // Closing BEFORE the extract is the point (a live mmap must not survive into
        // the rename), so the failure path cannot simply reorder the two - it has to
        // undo the close. Left un-undone, a failed extract returned `Err` with the
        // project already closed backend-side while the frontend still displayed it:
        // every later command, auto-save included, then failed with "no project
        // open". The extract stages to a sibling temp and renames, so on failure
        // `dest` is still the file we just closed and re-opening restores the
        // pre-call state (bar the undo history, which the close spent either way).
        let staged = sanitized_download_path(dest);
        remove_staged_with_sidecars(&staged);
        match dump.extract_entry_to(entry, &staged) {
            Ok(()) => {
                // A server `.stalx` is input from outside: refuse a hostile schema
                // before the history cleanup below fires a trigger it carries, and
                // before it can replace the cache (#608).
                if let Err(e) = crate::persistence::Database::check_schema_at(&staged) {
                    remove_staged_with_sidecars(&staged);
                    if closed {
                        let _ = self.open_project(dest.to_path_buf());
                    }
                    return Err(SpeleoDbError::Parse(format!(
                        "the downloaded project file was refused: {e:#}"
                    )));
                }
                // [#303] THE chokepoint. Every server `.stalx` that becomes the local
                // cache file passes through here, so dropping the inherited history in
                // one place is what makes "a pulled stack is never silently adopted"
                // true of the pull, the fast-forward AND the revision swap at once,
                // without each of them having to remember.
                //
                // [#576 NF-W10BE-3] Except in a file a NEWER build saved: this build never
                // writes into one (#576 B6-D1), and the clear is a write. The first-time
                // Open is the only replace that installs such a file (every path over a
                // linked copy refuses it before the swap, `refuse_a_newer_download`), and
                // it opens read-only. Its history stays in the file as the newer build
                // wrote it, and none of it reaches the session: the load reads no history
                // from a newer file (`do_load_project_full`).
                if matches!(undo, InheritedUndo::Drop) && !saved_by_a_newer_build(&staged) {
                    if let Err(e) = drop_inherited_undo(&staged) {
                        remove_staged_with_sidecars(&staged);
                        if closed {
                            let _ = self.open_project(dest.to_path_buf());
                        }
                        return Err(e);
                    }
                }
                // [#576 W52-2] The install also drops the old cache's sidecars
                // (see `install_over`).
                if let Err(e) = install_over(&staged, dest) {
                    remove_staged_with_sidecars(&staged);
                    if closed {
                        let _ = self.open_project(dest.to_path_buf());
                    }
                    return Err(SpeleoDbError::Io(format!(
                        "could not install sanitized download at {}: {e}",
                        dest.display()
                    )));
                }
                Ok(preserved_view)
            }
            Err(e) => {
                if closed {
                    let reopened = self.open_project(dest.to_path_buf());
                    if reopened.success {
                        tracing::info!(
                            path = %dest.display(),
                            purpose = "undo the pre-replace close after a failed extract",
                            "speleodb: re-opened the project the failed replace had closed"
                        );
                    } else {
                        // Nothing left to try: report it loudly, because the shell is
                        // now showing a project the backend does not have open.
                        tracing::error!(
                            path = %dest.display(),
                            error = ?reopened.error,
                            purpose = "undo the pre-replace close after a failed extract",
                            "speleodb: could not re-open the project after a failed replace"
                        );
                    }
                }
                Err(e)
            }
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "unit test fixtures fail loudly on setup errors, and the printed outcome is the \
              evidence a run captures"
)]
mod tests {
    use super::*;
    use crate::core::state::tests_support::speleodb_prod_matches;
    use crate::models::{Project, SpeleoLink};
    use crate::persistence::Database;
    use crate::speleodb::dump::Dump;

    const ID: &str = "11111111-1111-1111-1111-111111111111";
    /// The `.stalx` entry inside the fixture dump below.
    const ENTRY: &str = "misc/project.stalx";

    /// The replace's licence for the tests that drive it directly: they test the
    /// replace, not the step that earns one.
    struct TestReplace;
    impl ReplaceLicence for TestReplace {}

    /// [#303] Spec item 15's structural half, and the reason `InheritedUndo` is a named
    /// enum rather than a bool: KEEPING a downloaded copy's undo history is a waiver of
    /// the #194 b3 guard, so there must be exactly ONE place in the whole orchestration
    /// that asks for it - the Unwind, which exists to fetch that history back under an
    /// explicit, confirmed, lock-held action.
    ///
    /// A site inventory (#576 A8-D2, docs/testing-recipes.md "Source scans"): a diff
    /// that adds a second `Keep` site fails here and has to justify itself. (Its old
    /// sibling - no production caller names the abandoning replace - is the module's
    /// visibility now: outside test builds that replace is private here, and the
    /// orchestration's one entry point takes a `ReplaceLicence`, #576 NF-A8-BYPASS.)
    #[test]
    fn undo_history_keep_is_unwind_only() {
        let keeps = speleodb_prod_matches("InheritedUndo::Keep", &["speleodb_cache.rs"]);
        assert_eq!(
            keeps, 1,
            "RULE (sanctioned scan, #576 A8-D2 - keeping inherited history is Unwind-only, \
             #194 b3): exactly one production site may keep a downloaded copy's undo \
             history (the Unwind); found {keeps}. Every other adopt path inherits a stranger's \
             history one Ctrl-Z away from the user (#194 b3)."
        );
    }

    /// A `.stalx` at `path` linked to [`ID`], carrying the given un-pushed-edits bit.
    fn seed_linked_stalx(path: &Path, dirty: bool) {
        let mut db = Database::open(path).expect("open db");
        db.save_project(&Project::new("Local".to_owned(), None))
            .expect("save project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://x/".to_owned(),
            project_id: ID.to_owned(),
            project_name: "Local".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: dirty,
            last_edit_at: dirty.then(|| "2026-07-27T10:00:00Z".to_owned()),
            last_verify_at: None,
            undo_mode: None,
        }))
        .expect("save link");
    }

    /// A one-entry ZIP standing in for a downloaded dump.
    fn server_dump() -> Vec<u8> {
        use std::io::Write as _;
        let temp = tempfile::TempDir::new().expect("server project temp");
        let project = temp.path().join("server.stalx");
        {
            let mut db = Database::open(&project).expect("open server project");
            db.save_project(&Project::new("Server Copy".to_owned(), None))
                .expect("save server project");
        }
        let bytes = std::fs::read(project).expect("read server project");
        let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file(ENTRY, opts).expect("start_file");
        writer.write_all(&bytes).expect("write entry");
        writer.finish().expect("finish zip").into_inner()
    }

    fn replaced_project_name(path: &Path) -> String {
        Database::open_existing(path)
            .expect("open replaced project")
            .load_project()
            .expect("load replaced project")
            .name
    }

    #[test]
    fn only_a_named_discard_gets_a_replace_past_un_pushed_edits() {
        // The behaviour the split verb exists for, pinned from all three sides: the
        // default replace refuses over un-pushed work and leaves the file alone, the
        // policy-driven form agrees under `Protect`, and `Discard` is the ONLY thing
        // that gets through. A caller that wants the destruction has to name it.
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, true);
        let before = std::fs::read(&dest).expect("read dest");
        let state = AppState::new().expect("state");
        let zip = server_dump();

        let err = state
            .replace_cache_stalx(
                &mut Dump::open(&zip).expect("open dump"),
                ENTRY,
                &dest,
                ID,
                InheritedUndo::Drop,
            )
            .expect_err("un-pushed edits must refuse the plain replace");
        assert!(
            matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }),
            "and refuse it with the typed routing signal, got {err:?}"
        );
        assert_eq!(
            std::fs::read(&dest).expect("read dest"),
            before,
            "a refusal must leave the local copy byte-identical"
        );

        let err = state
            .replace_cache_stalx_with(
                &TestReplace,
                &mut Dump::open(&zip).expect("open dump"),
                ENTRY,
                &dest,
                ID,
                LocalEditsPolicy::Protect,
                InheritedUndo::Drop,
            )
            .expect_err("Protect is the same refusal");
        assert!(matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }));
        assert_eq!(
            std::fs::read(&dest).expect("read dest"),
            before,
            "still untouched"
        );

        state
            .replace_cache_stalx_with(
                &TestReplace,
                &mut Dump::open(&zip).expect("open dump"),
                ENTRY,
                &dest,
                ID,
                LocalEditsPolicy::Discard,
                InheritedUndo::Drop,
            )
            .expect("the confirmed-discard waiver replaces the copy");
        assert_eq!(
            replaced_project_name(&dest),
            "Server Copy",
            "and only that waiver actually overwrites it"
        );
    }

    /// [#303] Spec item 15's behavioural half: a replace that carries
    /// `InheritedUndo::Drop` leaves every downloaded history table EMPTY, and one that
    /// carries `Keep` leaves them intact.
    ///
    /// Asserted on the FILE rather than on the in-memory stack, because that is where it
    /// has to be true: the load path reads these tables into the live stack, so a clear
    /// that only touched memory would leave the history on disk for the next open.
    #[test]
    fn a_dropped_replace_empties_the_downloaded_undo_tables() {
        /// A real `.stalx` dump entry carrying two undo rows and one redo row, standing
        /// in for a Full-mode push's snapshot.
        fn dump_with_history() -> Vec<u8> {
            use std::io::Write as _;
            let temp = tempfile::TempDir::new().expect("temp");
            let seeded = temp.path().join("seeded.stalx");
            {
                let mut db = Database::open(&seeded).expect("open seed");
                db.save_project(&Project::new("Pushed".to_owned(), None))
                    .expect("save project");
                db.save_undo_stack(vec![
                    (r#"{"c":1}"#.to_owned(), r#"{"u":1}"#.to_owned()),
                    (r#"{"c":2}"#.to_owned(), r#"{"u":2}"#.to_owned()),
                ])
                .expect("seed undo");
                db.save_redo_stack_with_timestamps(vec![(
                    r#"{"c":3}"#.to_owned(),
                    r#"{"u":3}"#.to_owned(),
                    None,
                )])
                .expect("seed redo");
                db.save_speleo_undo_chain(Some("older-revision"))
                    .expect("seed chain pointer");
                db.save_speleo_undo_watermark(Some("2026-08-01T00:00:00+00:00"))
                    .expect("seed watermark");
            }
            let bytes = std::fs::read(&seeded).expect("read seed");
            let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file(ENTRY, opts).expect("start_file");
            writer.write_all(&bytes).expect("write entry");
            writer.finish().expect("finish zip").into_inner()
        }

        let history = |path: &Path| -> (usize, usize, Option<String>, Option<String>) {
            let db = Database::open_existing(path).expect("open replaced");
            (
                db.load_undo_stack_with_timestamps()
                    .expect("load undo")
                    .len(),
                db.load_redo_stack_with_timestamps()
                    .expect("load redo")
                    .len(),
                db.load_speleo_undo_chain().expect("load chain pointer"),
                db.load_speleo_undo_watermark().expect("load watermark"),
            )
        };

        let state = AppState::new().expect("state");
        let zip = dump_with_history();

        // Sanity: the fixture really does carry history, so the assertions below are not
        // passing vacuously.
        let temp = tempfile::TempDir::new().expect("temp");
        let kept = temp.path().join("kept.stalx");
        state
            .replace_cache_stalx_abandoning(
                &mut Dump::open(&zip).expect("open dump"),
                ENTRY,
                &kept,
                InheritedUndo::Keep,
            )
            .expect("replace with Keep");
        assert_eq!(
            history(&kept),
            (
                2,
                1,
                Some("older-revision".to_owned()),
                Some("2026-08-01T00:00:00+00:00".to_owned()),
            ),
            "Keep leaves the downloaded history in place - what the Unwind needs"
        );

        let dropped = temp.path().join("dropped.stalx");
        state
            .replace_cache_stalx_abandoning(
                &mut Dump::open(&zip).expect("open dump"),
                ENTRY,
                &dropped,
                InheritedUndo::Drop,
            )
            .expect("replace with Drop");
        assert_eq!(
            history(&dropped),
            (0, 0, None, None),
            "Drop empties all history metadata, so no adopt path can inherit it (#194 b3)"
        );
    }

    #[test]
    fn failed_history_cleanup_leaves_the_existing_cache_untouched() {
        use std::io::Write as _;

        let seed_dir = tempfile::TempDir::new().expect("seed temp");
        let seeded = seed_dir.path().join("seeded.stalx");
        {
            let mut db = Database::open(&seeded).expect("open seed");
            db.save_project(&Project::new("Pushed".to_owned(), None))
                .expect("save project");
            db.save_undo_stack(vec![(r#"{"c":1}"#.to_owned(), r#"{"u":1}"#.to_owned())])
                .expect("seed undo");
        }
        rusqlite::Connection::open(&seeded)
            .expect("reopen seed")
            .execute_batch(
                "CREATE TRIGGER block_undo_delete BEFORE DELETE ON undo_stack \
                 BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
            )
            .expect("install blocking trigger");
        let bytes = std::fs::read(&seeded).expect("read seed");
        let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
        writer
            .start_file(ENTRY, zip::write::SimpleFileOptions::default())
            .expect("start entry");
        writer.write_all(&bytes).expect("write entry");
        let zip = writer.finish().expect("finish zip").into_inner();

        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        std::fs::write(&dest, b"original cache").expect("seed destination");
        let before = std::fs::read(&dest).expect("read destination");
        let state = AppState::new().expect("state");

        let err = state
            .replace_cache_stalx_abandoning(
                &mut Dump::open(&zip).expect("open dump"),
                ENTRY,
                &dest,
                InheritedUndo::Drop,
            )
            .expect_err("a downloaded file carrying a trigger must not be adopted");
        // Refused by the schema check before the cleanup could fire the trigger.
        assert!(
            err.to_string()
                .contains("the downloaded project file was refused: the file defines a trigger"),
            "{err}"
        );
        assert_eq!(std::fs::read(&dest).expect("read destination"), before);
        assert!(
            !sanitized_download_path(&dest).exists(),
            "failed staged download must be cleaned up"
        );
    }

    #[test]
    fn a_clean_copy_is_replaced_without_any_waiver() {
        // The other half: `Protect` is not "always refuse". With nothing un-pushed
        // there is no work to lose, so an ordinary re-pull goes straight through.
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, false);
        let state = AppState::new().expect("state");
        let zip = server_dump();

        state
            .replace_cache_stalx(
                &mut Dump::open(&zip).expect("open dump"),
                ENTRY,
                &dest,
                ID,
                InheritedUndo::Drop,
            )
            .expect("a clean local copy needs no waiver");
        assert_eq!(
            replaced_project_name(&dest),
            "Server Copy",
            "the server copy landed"
        );
    }

    /// [#576 RV19a-3] A journal a crash left beside the staged download is
    /// removed before the next download is extracted there. The replace removed
    /// only the staged file, so the stale `-journal` (from a crash while the
    /// history cleanup wrote into the previous download) sat beside the fresh
    /// bytes; the read-write schema check then played it back, and the OLD
    /// file's pages became the new cache.
    #[test]
    fn a_stale_journal_beside_the_staged_download_never_reaches_the_cache() {
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, false);
        let staged = sanitized_download_path(&dest);
        let mut journal_name = staged.into_os_string();
        journal_name.push("-journal");
        let staged_journal = std::path::PathBuf::from(journal_name);

        // A hot journal of a different `.stalx`, caught mid-write after a cache
        // spill (header synced), as a crash leaves it.
        let old = temp.path().join("old.stalx");
        {
            let mut db = Database::open(&old).expect("open old");
            db.save_project(&Project::new("Old Copy".to_owned(), None))
                .expect("save old");
        }
        let conn = rusqlite::Connection::open(&old).expect("raw open old");
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE; CREATE TABLE junk(x);
             PRAGMA cache_size = 1; BEGIN;
             INSERT INTO junk SELECT hex(randomblob(2000)) FROM
               (WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200)
                SELECT i FROM n);",
        )
        .expect("write mid-transaction");
        let mut old_journal = old.into_os_string();
        old_journal.push("-journal");
        std::fs::copy(std::path::PathBuf::from(old_journal), &staged_journal)
            .expect("plant the stale journal");
        conn.execute_batch("ROLLBACK").expect("rollback");
        drop(conn);

        let state = AppState::new().expect("state");
        let zip = server_dump();
        let replaced = state.replace_cache_stalx(
            &mut Dump::open(&zip).expect("open dump"),
            ENTRY,
            &dest,
            ID,
            InheritedUndo::Drop,
        );
        let name = Database::open_existing(&dest)
            .and_then(|db| db.load_project())
            .map(|project| project.name);
        println!(
            "stale journal beside the staged download: replace -> {:?}; the cache holds {:?}; \
             journal left: {}",
            replaced.as_ref().map_err(ToString::to_string),
            name.as_ref().map_err(|e| format!("{e:#}")),
            staged_journal.exists()
        );

        replaced.expect("the download replaces the cache");
        assert_eq!(
            name.expect("the cache opens"),
            "Server Copy",
            "the cache is the download, not the stale journal's file"
        );
        assert!(!staged_journal.exists(), "the stale journal is gone");
    }

    /// `path` with an SQLite sidecar suffix (`-journal`, `-wal`, `-shm`).
    fn sidecar_of(path: &Path, suffix: &str) -> std::path::PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        std::path::PathBuf::from(name)
    }

    /// [#576 W52-2] The sidecars a crash left beside the CACHE file are removed
    /// with the file the replace renames over. They belong to the old cache:
    /// the replace renamed the download over it and left them, so the next
    /// read-write open played the stale `-journal` back into the new cache (the
    /// same playback RV19a-3 closed for the staged download), and a stale
    /// `-wal`/`-shm` waited beside it for the first open in WAL mode. The
    /// guarded replace refuses a cache whose link it cannot read past such a
    /// journal (as un-pushed edits), so the replace that installs the download
    /// is the user's "take remote and discard local" (`Discard`).
    #[test]
    fn the_old_caches_sidecars_never_reach_the_new_cache() {
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, false);

        // A hot journal of a different `.stalx`, caught mid-write after a cache
        // spill (header synced), as a crash leaves it, beside the cache file.
        let old = temp.path().join("old.stalx");
        {
            let mut db = Database::open(&old).expect("open old");
            db.save_project(&Project::new("Old Copy".to_owned(), None))
                .expect("save old");
        }
        let conn = rusqlite::Connection::open(&old).expect("raw open old");
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE; CREATE TABLE junk(x);
             PRAGMA cache_size = 1; BEGIN;
             INSERT INTO junk SELECT hex(randomblob(2000)) FROM
               (WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200)
                SELECT i FROM n);",
        )
        .expect("write mid-transaction");
        std::fs::copy(sidecar_of(&old, "-journal"), sidecar_of(&dest, "-journal"))
            .expect("plant the stale journal");
        conn.execute_batch("ROLLBACK").expect("rollback");
        drop(conn);
        for suffix in ["-wal", "-shm"] {
            std::fs::write(sidecar_of(&dest, suffix), b"stale").expect("plant a stale sidecar");
        }

        let state = AppState::new().expect("state");
        let zip = server_dump();
        let replaced = state.replace_cache_stalx_with(
            &TestReplace,
            &mut Dump::open(&zip).expect("open dump"),
            ENTRY,
            &dest,
            ID,
            LocalEditsPolicy::Discard,
            InheritedUndo::Drop,
        );
        let left: Vec<&str> = ["-journal", "-wal", "-shm"]
            .into_iter()
            .filter(|suffix| sidecar_of(&dest, suffix).exists())
            .collect();
        let name = Database::open_existing(&dest)
            .and_then(|db| db.load_project())
            .map(|project| project.name);
        println!(
            "stale sidecars beside the cache: replace -> {:?}; sidecars left beside it: {left:?}; \
             the cache then holds {:?}",
            replaced.as_ref().map_err(ToString::to_string),
            name.as_ref().map_err(|e| format!("{e:#}")),
        );

        replaced.expect("the download replaces the cache");
        assert!(
            left.is_empty(),
            "no stale sidecar is left beside the cache: {left:?}"
        );
        assert_eq!(
            name.expect("the cache opens"),
            "Server Copy",
            "the cache is the download, not the stale journal's file"
        );
    }

    /// A clean `.stalx` linked to [`ID`] at `dest`, with a hot rollback journal
    /// beside it as a kill or power loss mid-save leaves one: the file caught
    /// mid-transaction after a cache spill (header synced), copied together with
    /// its OWN `-journal`, so a read-write open rolls it back to the last save.
    fn seed_behind_its_own_hot_journal(dir: &Path, dest: &Path) {
        seed_behind_its_own_hot_journal_after(dir, dest, "");
    }

    /// [`seed_behind_its_own_hot_journal`], with `schema_sql` committed into
    /// the file first.
    fn seed_behind_its_own_hot_journal_after(dir: &Path, dest: &Path, schema_sql: &str) {
        let live = dir.join("live.stalx");
        seed_linked_stalx(&live, false);
        let conn = rusqlite::Connection::open(&live).expect("raw open live");
        conn.execute_batch(schema_sql).expect("commit the schema");
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE; CREATE TABLE junk(x);
             PRAGMA cache_size = 1; BEGIN;
             INSERT INTO junk SELECT hex(randomblob(2000)) FROM
               (WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200)
                SELECT i FROM n);",
        )
        .expect("write mid-transaction");
        std::fs::copy(&live, dest).expect("snapshot the file mid-save");
        std::fs::copy(sidecar_of(&live, "-journal"), sidecar_of(dest, "-journal"))
            .expect("snapshot its journal");
        conn.execute_batch("ROLLBACK").expect("rollback");
    }

    /// [#576 W53 (W52-NF1)] A cache copy the guard cannot read past a hot journal
    /// is refused as UNREADABLE, not as un-pushed edits. The guard fails closed on
    /// it (right: a read-write open may recover work), but it said "this project
    /// has local edits that were never pushed" about a copy whose link row says it
    /// is clean, so the user was told about edits they never made.
    #[test]
    fn a_cache_copy_behind_a_hot_journal_is_refused_as_unreadable_not_as_unpushed() {
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_behind_its_own_hot_journal(temp.path(), &dest);
        let before = std::fs::read(&dest).expect("read dest");

        let state = AppState::new().expect("state");
        let zip = server_dump();
        let err = state
            .replace_cache_stalx(
                &mut Dump::open(&zip).expect("open dump"),
                ENTRY,
                &dest,
                ID,
                InheritedUndo::Drop,
            )
            .expect_err("a copy that cannot be read must not be replaced");
        let message = err.to_string();
        println!(
            "clean cache copy behind a hot journal: replace -> Err({message:?}); \
             journal still beside it: {}",
            sidecar_of(&dest, "-journal").exists()
        );

        assert!(
            matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }),
            "still the routing signal that offers keep-local or take-remote, got {err:?}"
        );
        assert!(
            !message.contains("never pushed"),
            "the copy's link says it is clean, so the refusal must not claim un-pushed \
             edits: {message:?}"
        );
        assert!(
            message.contains("could not be read"),
            "the refusal names what went wrong: {message:?}"
        );
        assert_eq!(
            std::fs::read(&dest).expect("read dest"),
            before,
            "a refusal leaves the local copy byte-identical"
        );
        assert!(
            sidecar_of(&dest, "-journal").exists(),
            "and its journal in place"
        );
    }

    /// [#576 W53 (W52-NF1)] The recovery the unreadable refusal names holds: opening
    /// the local copy (the read-write open behind "keep the local copy") rolls its
    /// own hot journal back, and the guard then reads the copy as what it is.
    #[test]
    fn opening_a_cache_copy_behind_its_own_hot_journal_recovers_it() {
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_behind_its_own_hot_journal(temp.path(), &dest);
        let state = AppState::new().expect("state");

        let refused = state
            .guard_local_edits(ID, &dest)
            .expect_err("behind its journal the copy cannot be read")
            .to_string();
        assert!(
            refused.contains("could not be read"),
            "refused as unreadable: {refused:?}"
        );

        let name = Database::open_existing(&dest)
            .and_then(|db| db.load_project())
            .map(|project| project.name);
        let after = state.guard_local_edits(ID, &dest);
        println!(
            "open the local copy -> {:?}; journal left: {}; guard then -> {:?}",
            name.as_ref().map_err(|e| format!("{e:#}")),
            sidecar_of(&dest, "-journal").exists(),
            after.as_ref().map_err(ToString::to_string),
        );
        assert_eq!(
            name.expect("the read-write open recovers the copy"),
            "Local",
            "the copy as last saved"
        );
        assert!(
            !sidecar_of(&dest, "-journal").exists(),
            "the journal is rolled back"
        );
        after.expect("the recovered copy is clean, so the replace may go ahead");
    }

    /// The replace a confirmed "take the server copy" runs: `Discard`, with the
    /// downloaded history dropped.
    fn discard_replace(
        state: &AppState,
        dest: &Path,
    ) -> Result<HashMap<String, String>, SpeleoDbError> {
        let zip = server_dump();
        state.replace_cache_stalx_with(
            &TestReplace,
            &mut Dump::open(&zip)?,
            ENTRY,
            dest,
            ID,
            LocalEditsPolicy::Discard,
            InheritedUndo::Drop,
        )
    }

    /// The SQLite sidecars beside `path`.
    fn sidecars_left(path: &Path) -> Vec<&'static str> {
        ["-journal", "-wal", "-shm"]
            .into_iter()
            .filter(|suffix| sidecar_of(path, suffix).exists())
            .collect()
    }

    /// [#576 RV20-3] Nothing of the old cache is left for the new one even when
    /// the install stops right after its rename: a crash there (nothing after the
    /// rename has run), or a removal that failed and was only logged (a Windows
    /// sharing violation). The old cache's sidecars were removed only AFTER the
    /// rename, so in either window its hot journal stayed beside the new file and
    /// the next read-write open rolled it in. The seam stops the install right
    /// after the rename, on this thread.
    #[test]
    fn the_old_caches_sidecars_are_settled_before_the_rename() {
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_behind_its_own_hot_journal(temp.path(), &dest);
        for suffix in ["-wal", "-shm"] {
            std::fs::write(sidecar_of(&dest, suffix), b"stale").expect("plant a stale sidecar");
        }

        let state = AppState::new().expect("state");
        SKIP_REMOVAL_AFTER_RENAME.with(|skip| skip.set(true));
        let replaced = discard_replace(&state, &dest);
        SKIP_REMOVAL_AFTER_RENAME.with(|skip| skip.set(false));
        let left = sidecars_left(&dest);
        let name = Database::open_existing(&dest)
            .and_then(|db| db.load_project())
            .map(|project| project.name);
        println!(
            "install stopped right after its rename: replace -> {:?}; sidecars beside the new \
             cache: {left:?}; the cache then holds {:?}",
            replaced.as_ref().map_err(ToString::to_string),
            name.as_ref().map_err(|e| format!("{e:#}")),
        );

        replaced.expect("the download replaces the cache");
        assert!(
            left.is_empty(),
            "no sidecar of the old cache beside the new one: {left:?}"
        );
        assert_eq!(
            name.expect("the cache opens"),
            "Server Copy",
            "the cache is the download, not the old copy's journal played into it"
        );
    }

    /// [#576 RV20-3] A hot journal that SQLite cannot roll back refuses the
    /// replace before the rename, and the copy and its journal stay as they were:
    /// together they are still the last save. Here the copy is read-only, so the
    /// read-write open falls back to read-only and cannot play the journal back.
    /// The rename used to go ahead regardless, and a stop right after it left the
    /// journal beside the new file.
    #[cfg(unix)]
    #[test]
    fn a_journal_that_cannot_be_rolled_back_refuses_the_replace() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_behind_its_own_hot_journal(temp.path(), &dest);
        let journal = sidecar_of(&dest, "-journal");
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o444))
            .expect("make the copy read-only");
        if std::fs::OpenOptions::new().write(true).open(&dest).is_ok() {
            println!("file modes do not bind for this user; nothing to check");
            return;
        }
        let copy_before = std::fs::read(&dest).expect("read dest");
        let journal_before = std::fs::read(&journal).expect("read journal");

        let state = AppState::new().expect("state");
        SKIP_REMOVAL_AFTER_RENAME.with(|skip| skip.set(true));
        let replaced = discard_replace(&state, &dest);
        SKIP_REMOVAL_AFTER_RENAME.with(|skip| skip.set(false));
        let copy_kept = std::fs::read(&dest).is_ok_and(|bytes| bytes == copy_before);
        let journal_kept = std::fs::read(&journal).is_ok_and(|bytes| bytes == journal_before);
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644))
            .expect("make the file writable again");
        let name = Database::open_existing(&dest)
            .and_then(|db| db.load_project())
            .map(|project| project.name);
        println!(
            "read-only copy behind its hot journal: replace -> {:?}; copy kept: {copy_kept}; \
             journal kept: {journal_kept}; the file then holds {:?}",
            replaced.as_ref().map_err(ToString::to_string),
            name.as_ref().map_err(|e| format!("{e:#}")),
        );

        let refused = replaced
            .expect_err("a journal that cannot be rolled back refuses the replace")
            .to_string();
        assert!(
            refused.contains("could not install"),
            "refused as a failed install: {refused:?}"
        );
        assert!(
            copy_kept && journal_kept,
            "the copy and its journal stay as they were"
        );
        assert_eq!(
            name.expect("once writable, the copy opens"),
            "Local",
            "its journal rolls it back to the last save"
        );
    }

    /// [#576 PR #594 review] The connection the settle reads the old cache
    /// through gets the hardening every other open of an existing file does:
    /// the cache is a server download, a file from outside the app. It was
    /// opened raw, with no authorizer, no defensive mode and `trusted_schema`
    /// on. The probes are persistence's (`untrusted_sql_probes`); on the repo's
    /// build only the authorizer refuses a `CREATE VIRTUAL TABLE` with
    /// SQLITE_AUTH, so they fail without it.
    #[test]
    fn the_settle_opens_the_old_cache_with_the_untrusted_file_hardening() {
        use rusqlite::config::DbConfig;
        use stalagmight_persistence::persistence::sqlite::untrusted_sql_probes;
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, false);

        let conn = open_to_settle(&dest).expect("the settle opens a cache this build saved");
        let failures = untrusted_sql_probes::refusal_failures(&conn, "settle");
        let defensive = conn
            .db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE)
            .expect("read defensive mode");
        let trusted_schema = conn
            .db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA)
            .expect("read trusted_schema");
        println!(
            "settle connection: defensive {defensive}, trusted_schema {trusted_schema}, \
             probe failures {failures:#?}"
        );
        assert!(failures.is_empty(), "{failures:#?}");
        assert!(defensive, "defensive mode is on");
        assert!(!trusted_schema, "trusted_schema is off");
    }

    /// [#576 PR #594 review] The hardened settle still settles an old cache
    /// whose schema the guard refuses (a view, which the app never writes):
    /// the guard's read of `sqlite_master` rolls the hot journal back before
    /// it refuses, so nothing of the old cache is left beside the new one, even
    /// with the install stopped right after its rename.
    #[test]
    fn the_settle_rolls_back_the_journal_of_a_cache_the_schema_guard_refuses() {
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_behind_its_own_hot_journal_after(temp.path(), &dest, "CREATE VIEW v AS SELECT 1;");
        // The settle's open on a twin of the pair, so the replace below still
        // meets the journal.
        let twin = temp.path().join("twin.stalx");
        std::fs::copy(&dest, &twin).expect("copy the cache");
        std::fs::copy(sidecar_of(&dest, "-journal"), sidecar_of(&twin, "-journal"))
            .expect("copy its journal");
        let refused = open_to_settle(&twin)
            .map(drop)
            .map_err(|e| format!("{e:#}"));
        let journal_hot = looks_hot(&sidecar_of(&twin, "-journal")).expect("read the journal");

        let state = AppState::new().expect("state");
        SKIP_REMOVAL_AFTER_RENAME.with(|skip| skip.set(true));
        let replaced = discard_replace(&state, &dest);
        SKIP_REMOVAL_AFTER_RENAME.with(|skip| skip.set(false));
        let left = sidecars_left(&dest);
        let name = Database::open_existing(&dest)
            .and_then(|db| db.load_project())
            .map(|project| project.name);
        println!(
            "old cache with a view, behind its hot journal: the settle's open -> {refused:?}; \
             journal still hot after it: {journal_hot}; replace -> {:?}; sidecars beside the \
             new cache: {left:?}; the cache then holds {:?}",
            replaced.as_ref().map_err(ToString::to_string),
            name.as_ref().map_err(|e| format!("{e:#}")),
        );

        assert!(refused.is_err(), "the guard refuses the view");
        assert!(!journal_hot, "its read rolled the journal back first");
        replaced.expect("the download replaces the cache");
        assert!(left.is_empty(), "no sidecar beside the new cache: {left:?}");
        assert_eq!(name.expect("the cache opens"), "Server Copy");
    }

    /// [#576 PR #594 review] A cache that is a symlink: SQLite's unix VFS names
    /// the journal after the file the link resolves to, while the settle looks
    /// beside `dest` itself. That is the place that matters here: the rename
    /// replaces the link, not its target, so the new cache is a file at `dest`
    /// whose sidecars are named after `dest`. The old target and its hot
    /// journal stay where they were, together still its last save. Stopped
    /// right after the rename, as the RV20-3 tests do.
    #[cfg(unix)]
    #[test]
    fn a_hot_journal_beside_a_symlinked_caches_target_stays_with_the_target() {
        let temp = tempfile::TempDir::new().expect("temp");
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).expect("create the target's folder");
        let target = elsewhere.join("project.stalx");
        seed_behind_its_own_hot_journal(&elsewhere, &target);
        let dest = temp.path().join("project.stalx");
        std::os::unix::fs::symlink(&target, &dest).expect("link the cache to its target");

        let state = AppState::new().expect("state");
        SKIP_REMOVAL_AFTER_RENAME.with(|skip| skip.set(true));
        let replaced = discard_replace(&state, &dest);
        SKIP_REMOVAL_AFTER_RENAME.with(|skip| skip.set(false));
        let still_a_link = dest
            .symlink_metadata()
            .expect("dest is there")
            .file_type()
            .is_symlink();
        let left = sidecars_left(&dest);
        let target_journal_hot =
            looks_hot(&sidecar_of(&target, "-journal")).expect("read the target's journal");
        let cache = Database::open_existing(&dest)
            .and_then(|db| db.load_project())
            .map(|project| project.name);
        let old = Database::open_existing(&target)
            .and_then(|db| db.load_project())
            .map(|project| project.name);
        println!(
            "symlinked cache, hot journal beside its target: replace -> {:?}; dest still a \
             link: {still_a_link}; sidecars beside the new cache: {left:?}; the target's \
             journal still hot: {target_journal_hot}; the cache holds {:?}; the old target \
             holds {:?}",
            replaced.as_ref().map_err(ToString::to_string),
            cache.as_ref().map_err(|e| format!("{e:#}")),
            old.as_ref().map_err(|e| format!("{e:#}")),
        );

        replaced.expect("the download replaces the cache");
        assert!(!still_a_link, "the rename replaced the link");
        assert!(left.is_empty(), "no sidecar beside the new cache: {left:?}");
        assert!(target_journal_hot, "the old target keeps its journal");
        assert_eq!(cache.expect("the cache opens"), "Server Copy");
        assert_eq!(
            old.expect("the old target opens"),
            "Local",
            "its journal rolls it back to its last save"
        );
    }
}
