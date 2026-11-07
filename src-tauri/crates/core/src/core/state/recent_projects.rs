use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::{CommandResult, ValidationWarning, WarningSeverity};
use crate::models::{Project, ProjectMetadata};
use crate::persistence::Database;

use super::AppState;

impl AppState {
    /// [#576 A9] The open project as an O(1) snapshot: what it is now, readable
    /// with no lock held, never changed by a later edit (see `ProjectSlot`). The
    /// read every command that only looks at the project uses. `None` when no
    /// project is open.
    pub fn project_snapshot(&self) -> Option<Arc<Project>> {
        self.project.lock().snapshot()
    }

    /// [#576 C2] The open project as it stands BETWEEN commands: a snapshot
    /// taken under `command_lock`.
    ///
    /// The commands run on the blocking pool, concurrently. A command mutates
    /// the project and then settles it (orphan conversion, frame renormalisation,
    /// the geometry reconcile, the undo push) under ONE `command_lock` span but
    /// several `project` acquisitions, so a plain [`Self::project_snapshot`] taken
    /// mid-command can be a project that was mutated and not yet reconciled - a
    /// state no user ever sees. Taking the snapshot under `command_lock` makes it
    /// either before or after every command. The lock is held only for the O(1)
    /// `Arc` clone; the caller reads the snapshot with no lock held.
    ///
    /// Every IPC read of the project goes through this or
    /// [`Self::settled_read`] (docs/architecture.md, "Where commands run").
    /// Never call it while holding `command_lock` - from inside `execute`, undo,
    /// redo, a load or an import - it is not reentrant; code there reads
    /// [`Self::project_snapshot`].
    pub fn settled_snapshot(&self) -> Option<Arc<Project>> {
        let _between_commands = self.hold_commands();
        self.project_snapshot()
    }

    /// [#576 C2] Run `read` between commands: under `command_lock`, so whatever
    /// it reads (the project, the undo stacks, the database) is never caught in
    /// the middle of a command. For short reads that go through `AppState`'s own
    /// query methods; a long read takes [`Self::settled_snapshot`] and computes
    /// on that instead, so it does not hold up the next edit.
    ///
    /// `read` must not reach anything that takes `command_lock` itself (`execute`,
    /// undo/redo, the imports, save, close, load, repair, the adjusted solve):
    /// the lock is not reentrant.
    ///
    /// [#576 RV5-PI-4] A panic in `read` is caught, the guard released normally, and
    /// the panic then resumed: unwinding through the guard would poison
    /// `command_lock`, and `refuse_after_crash` would then refuse undo, redo and
    /// every SpeleoDB push for the rest of the session over a crash that mutated
    /// nothing. A cell `read` itself held when it panicked (the project, the
    /// database) is still poisoned and reported by its own lock, as it should be.
    pub fn settled_read<T>(&self, read: impl FnOnce() -> T) -> T {
        let between_commands = self.hold_commands();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read));
        drop(between_commands);
        match outcome {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// A deep copy of the open project, for a caller that has to own and change
    /// it. A caller that only reads takes [`Self::project_snapshot`] instead: this
    /// copies the whole survey.
    pub fn get_project_state(&self) -> Option<Project> {
        self.project_snapshot().map(Arc::unwrap_or_clone)
    }

    /// Get recent projects list.
    ///
    /// Hot path on every navigation to the global view. Uses SQL aggregates
    /// to compute metadata without deserializing the full project - the
    /// previous implementation called `db.load_project()` for every entry,
    /// which on a 75k-shot project parsed every shot row only to throw it
    /// all away. That caused a multi-second freeze every time the user hit
    /// "back" with a large project in the recent list.
    pub fn get_recent_projects(&self) -> Vec<ProjectMetadata> {
        let recent = {
            let r = self.recent_projects.lock();
            r.clone()
        };

        recent
            .iter()
            .filter_map(|path| {
                // open_for_metadata skips PRAGMA quick_check and migrations -
                // both are unnecessary when we're only reading aggregate
                // columns and the user hasn't asked to open the project.
                let db = Database::open_for_metadata(path).ok()?;
                match db.load_metadata(path.clone()) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        tracing::debug!(
                            path = ?path,
                            error = %e,
                            "Failed to read recent project metadata; skipping"
                        );
                        None
                    }
                }
            })
            .collect()
    }

    /// [#576 RV4-R-M2] Write `recent` into the live config and persist it, under
    /// `config_write_lock` like every other write of the live config. A settings
    /// save (`modify_config`) clones the config under that lock and writes the
    /// clone back; a recents write taking only the `config` mutex could land in
    /// between and be reverted, in the live config and in `config.toml`, while
    /// the in-memory recents list kept it. With `truncate`, `recent` is first cut
    /// to `recent_projects_max`. The caller holds the `recent_projects` mutex (and
    /// may hold `project`): nothing holding `config_write_lock` takes either.
    ///
    /// Returns the save's outcome.
    pub(super) fn persist_recent_projects(
        &self,
        recent: &mut Vec<PathBuf>,
        truncate: bool,
    ) -> anyhow::Result<()> {
        let _guard = self.config_write_lock.lock();
        let mut config = self.config.lock();
        if truncate {
            recent.truncate(config.general.recent_projects_max);
        }
        config.general.recent_projects.clone_from(recent);
        config.save()
    }

    /// Remove a project from the recent projects list
    pub fn remove_from_recent_projects(&self, path: PathBuf) -> CommandResult {
        let mut recent = self.recent_projects.lock();

        // Remove the path from the list
        let initial_len = recent.len();
        recent.retain(|p| p != &path);

        // Idempotent forget: "remove from recent" means "ensure this path is not in
        // the list", so an already-absent entry is SUCCESS, not a failure. The
        // backend's `config.general.recent_projects` and this in-memory list are kept
        // in lockstep, but the FRONTEND holds its own config snapshot that can lag them
        // (e.g. after the backend trimmed an entry past `recent_projects_max`). That
        // stale snapshot still renders the trimmed path as an unavailable "missing"
        // card. The frontend only refreshes recents on a success result, so returning a
        // failure here left that row un-removable forever. Nothing changed on the
        // backend, so skip the re-save and return success; the caller then re-reads the
        // (already-correct) backend config and the stale row drops out. (The normal
        // case - a missing file whose path is still in the list - takes the resave
        // branch below.)
        if recent.len() == initial_len {
            tracing::debug!(path = ?path, "remove_from_recent: path already absent; treating as success");
            return CommandResult::success();
        }

        // Update config and save
        if let Err(e) = self.persist_recent_projects(&mut recent, false) {
            tracing::warn!("Failed to save config after removing from recent: {}", e);
            return CommandResult::failure(format!("Failed to save config: {e}"));
        }

        tracing::info!(path = ?path, "Removed project from recent list");
        CommandResult::success()
    }

    /// Permanently delete a project from disk and drop it from the recent list.
    ///
    /// The destructive counterpart to
    /// [`remove_from_recent_projects`](Self::remove_from_recent_projects), which
    /// only forgets the entry. The project's `.stalx` is handed to the OS's own
    /// delete via [`send_to_trash`]: on macOS it lands in the Trash, on Linux in
    /// the freedesktop trash where one exists. The app keeps no undo trail of its
    /// own - recovery, if any, is whatever the OS provides. A same-stem `.tml`
    /// next to the project is never touched: it is an unrelated file as far as
    /// this app is concerned.
    ///
    /// A path whose FINAL component is a symlink deletes THE LINK, and only the
    /// link: the OS delete never follows that last hop, and neither does this
    /// function's notion of identity. The session is torn down, and recents pruned,
    /// only for that exact path - never for the project the link pointed at, which
    /// is still on disk, still openable, and was never named. A symlink in a PARENT
    /// directory is a different thing entirely: the final component is then a real
    /// file, the delete really does destroy it, and referent identity
    /// ([`same_file`](super::project_io::same_file)) applies as usual. The stat that
    /// distinguishes the two is the first thing the body does.
    ///
    /// Tolerant of a file that is already gone (it still drops the entry from
    /// recents). If the deleted project is the one currently open in memory, the
    /// whole session is torn down through
    /// [`close_open_project_locked`](Self::close_open_project_locked) BEFORE the
    /// file is handed to the OS - see the call site for why the full teardown (and
    /// not just clearing `self.project`), why it has to happen first, and why the
    /// check and the teardown share one lock acquisition.
    ///
    /// A teardown that happened is reported back as
    /// [`CommandResult::session_closed`], on the failure return as well as the
    /// success one, because the frontend can predict it from neither `success` nor
    /// the path it asked to delete.
    ///
    /// A SpeleoDB write lock this session holds on the deleted project is handed back
    /// to the server as well - the delete is treated as an explicit abandon, so the
    /// un-pushed-edits veto does not apply. See the two call sites below for why the
    /// snapshot and the network release are split across the `project` guard, and why
    /// the release is sequenced last.
    ///
    /// The `.stalx` is trashed before recents are touched: if *that* fails the
    /// call returns a failure with recents untouched, so the entry stays
    /// re-openable. Once it succeeds the project is gone, so a later problem -
    /// the recents persist failing - is reported as a non-fatal
    /// [`CommandResult::warnings`] entry while the call still returns success and
    /// drops the recents entry. Returning a failure after the file is already
    /// trashed would strand a "File not found" zombie row in the UI.
    pub fn delete_project(&self, path: PathBuf) -> CommandResult {
        // Does the FINAL component of `path` name a symlink? That one question decides
        // which notion of identity everything below uses, so it is answered once, up
        // front, while the file is still there to be stat'd.
        //
        // `symlink_metadata` is precisely the right stat: it resolves symlinks in the
        // PARENT directories but NOT the last component. That asymmetry is the whole
        // point, because it mirrors exactly what the removal below destroys:
        //
        // - `<dir>/link.stalx` -> `<real>/project.stalx`. The final component is the
        //   link, and the removal acts on that directory ENTRY rather than on what it
        //   points at: `fs::remove_file` (the trash fallback, and the deterministic path
        //   under `cfg(test)`) unlinks the LINK; `trash::delete` is believed to behave
        //   the same for a symlink, but no test config can reach that real arm BY
        //   DESIGN (see the `send_to_trash` doc comment below), so it is unverified
        //   from tests. The referent survives either way, so referent identity here
        //   would report a destruction that did not happen.
        // - `<linkdir>/project.stalx` where `linkdir` -> `<realdir>`. The final
        //   component is an ordinary file and the removal really does destroy
        //   `<realdir>/project.stalx`, so referent identity is not just safe but
        //   REQUIRED: this is only a second spelling of that file's path, and the
        //   Windows delete depends on recognizing it (see the teardown comment below).
        //
        // A path with nothing on it at all - which this function tolerates - reports no
        // symlink, and that is the consistent answer rather than an arbitrary fallback:
        // with the file gone `same_file` can canonicalize neither side and degrades to
        // the same plain path comparison the symlink branch performs, so both notions of
        // identity already agree there.
        //
        // The RESULT is kept, not just the boolean, because this one stat is also the
        // answer to "is there anything here to remove?" further down, and those are two
        // different questions when the stat FAILS. `is_ok_and` and `Path::exists` both
        // flatten a failure into `false`, which reads an access denial - the file is
        // there, we simply may not look at it (a lost macOS folder permission, a parent
        // directory whose `+x` went away) - as "already gone". Everything downstream
        // then behaved as though the delete had succeeded: the trash was skipped, the
        // recents row was pruned, and an open managed project had its captured server
        // lock handed back under the EXPLICIT-ABANDON policy, which exists precisely
        // because destroying the `.stalx` forfeits its un-pushed edits. Nothing had
        // been destroyed, so those edits - and the file - were still there, now behind
        // a released mutex any other diver could take.
        let entry = std::fs::symlink_metadata(&path);
        let deleting_the_symlink_itself = entry
            .as_ref()
            .is_ok_and(|meta| meta.file_type().is_symlink());

        // Only a definitive "there is nothing here" may be tolerated as absence.
        // Anything else is refused BEFORE a single thing is torn down or released -
        // the session stays open, the lock stays held, recents keeps its row, and the
        // user gets the access error to act on.
        if let Err(e) = &entry {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = ?path,
                    error = %e,
                    purpose = "delete a project",
                    "Refusing to delete: cannot determine whether the file is there"
                );
                return CommandResult::failure(format!("Failed to delete {}: {e}", path.display()));
            }
        }

        // "Does `candidate` name the file this delete is about to destroy?" - the single
        // predicate behind BOTH the open-project teardown and the recents prune, so the
        // two can never disagree about what was deleted.
        //
        // For a symlink the answer is EXACT-PATH identity, because the link is all we
        // remove. Applying referent identity there was destructive-by-report: deleting
        // `<dir>/link.stalx` removed only the link, yet tore down a session opened
        // through `<real>/project.stalx` and pruned THAT project's recents row - so the
        // user lost their open session and their way back to a project whose `.stalx`
        // was still sitting on disk, and the call reported success. The delete dialog
        // promises to remove the file at the path the user named: nothing else is
        // destroyed, and nothing else is treated as destroyed. The teardown still fires
        // when the open project was opened THROUGH the link, so we also never leave a
        // live session whose `file_path` is a link we just removed.
        //
        // Everything else keeps referent identity - see the two comments below for what
        // each of the callers needs it for.
        let names_the_deleted_file = |candidate: &Path| {
            if deleting_the_symlink_itself {
                candidate == path.as_path()
            } else {
                super::project_io::same_file(candidate, &path)
            }
        };

        // Deleting the file backing the OPEN project tears the whole session down
        // first, for the same two reasons `close_project_if_open_at_held` closes before
        // a download-replace: SQLite holds a live handle with a 256 MB mmap over
        // the file, and on Windows it opens WITHOUT `FILE_SHARE_DELETE`, so both
        // the Recycle-Bin move and the permanent-remove fallback fail with "Access
        // is denied" while that handle is alive. Closing after the trash would
        // therefore be unreachable on Windows - the delete would never get that
        // far.
        //
        // It has to be the FULL teardown, not just `*self.project.lock() = None`:
        // that left the live SQLite handle open on a file that no longer exists,
        // and left the entire undo/redo stack in place so an undo could replay
        // against the deleted project. The teardown additionally drops the database
        // handle, clears the undo stack, drops the display-settings and
        // adjusted-geometry caches, and resets the read-only edit gate plus the
        // force-edit flag so neither leaks into the next open.
        //
        // ONE acquisition of the `project` lock covers BOTH the identity check and
        // the teardown, and that is load-bearing rather than tidiness. Reading the
        // open path through `open_project_path` (which RELEASES the lock) and only
        // then calling `close_project` (which re-acquires it) is a TOCTOU: a load
        // that finished installing project B in the gap made the close tear down
        // B's SQLite handle and undo stack while this call went on to delete A's
        // file. The other half of the atom lives in the installers (round 5):
        // every path that installs a project WITH a file path - `open_project`,
        // `do_load_project_full`, `create_project_full` - commits the slot, the
        // gates, the database handle, the undo stack AND the recents row under
        // one unbroken acquisition of this same lock, so holding it here orders
        // this delete strictly before that whole finalization or strictly after
        // it. Before: we see the pre-load world, match nothing of the loader's,
        // and the load lands intact. After: the teardown finds every handle
        // installed and drops them all. When the slot commit sat in its OWN
        // span, a delete of the very path being loaded landed in between -
        // matching the freshly-installed project, tearing down the half-built
        // session, and leaving the loader to install handles into an empty
        // world and report success. (`create_project` commits a project with no
        // `file_path`, which this check can never match.) `close_project` would
        // self-deadlock under the guard, so this goes through
        // `close_open_project_locked` - the same teardown with the guard passed
        // in. (`close_project_if_open_at_held` still has the older
        // check-release-close shape; the same restructuring would suit it, but its
        // callers replace rather than delete, so it is left alone here.)
        //
        // The cost of closing first: if the trash below fails for an unrelated
        // reason (no write access to the parent directory), the project is now
        // closed and its undo history dropped even though the file survives.
        // Recents is untouched in that branch, so the user reopens it from there.
        // That branch is exactly why the verdict is REPORTED to the frontend
        // (`with_session_closed` on both returns): the teardown has happened there
        // and `success` is false, so a frontend that keyed the close off `success`
        // kept rendering a phantom project with no backend behind it.
        //
        // Matched on file identity, not `==`, for the same reason
        // `close_project_if_open_at_held` does: `same_file`'s canonicalization resolves
        // `.`/`..`, symlinked parent directories and (on Windows) case/short-name
        // differences, so a second spelling of the open project's path still tears
        // the session down. A plain comparison was survivable while this check only
        // kept an autosave from resurrecting the file, but the Windows delete now
        // DEPENDS on it firing - miss it and the trash fails with "Access is
        // denied". When neither side canonicalizes (the file is already gone, which
        // this function tolerates) `same_file` falls back to comparing the paths.
        // It canonicalizes under the lock, i.e. two `stat`-class syscalls inside
        // the critical section; that is the price of the atomicity above, and it is
        // bounded work on a path we are about to delete anyway. (Deleting a symlink
        // ITSELF is the one case that does not get referent identity - the removal
        // leaves the referent alive, so tearing down a session opened through it
        // would be a teardown for nothing. See `names_the_deleted_file` above.)
        let mut cleared_claim = None;
        let (session_closed, held_lock) = {
            // [#576 A1] `command_lock` before `project`: the teardown is a slot commit,
            // and one landing inside an edit or an undo left that command's entry on
            // the emptied session's stacks.
            let commands = self.hold_commands();
            let mut open = self.project.lock();
            // A never-saved project has no `file_path` and so can never be the file
            // being deleted - same predicate `open_project_path` encodes.
            let deleting_open_project = open
                .as_ref()
                .and_then(|p| p.file_path.as_deref())
                .is_some_and(names_the_deleted_file);
            let mut held_lock = None;
            if deleting_open_project {
                // Snapshot what releasing a held SpeleoDB write lock needs, HERE,
                // while the database handle is still alive. Three constraints pin
                // this shape:
                //
                // - It must precede the teardown. The snapshot reads the
                //   `speleo_link` row through the open database handle, and
                //   `close_open_project_locked` drops that handle, so a release
                //   attempted afterwards asks a `None` database for the link, gets
                //   `NoRemoteProject`, and silently does nothing. That was the bug:
                //   the `.stalx` and its link row were destroyed and the frontend
                //   cleared its link state, while the SERVER mutex stayed held and
                //   blocked every other diver until it timed out.
                //   the `close_project` command gets this right by releasing before it
                //   issues `Command::CloseProject`; this path bypasses that command
                //   entirely, so it has to do the same thing for itself.
                // - The NETWORK half must not happen under this guard - see the
                //   release below. What makes splitting the two halves possible is
                //   that the snapshot is deliberately local-only: one `speleo_link`
                //   read plus the session and config locks, no network at all (see
                //   `speleodb_held_lock_release_params`). Bounded work, and the same
                //   read every close already performs.
                // - It is the DELETE snapshot, not the silent one. Destroying the
                //   file forfeits any un-pushed edits by the user's own instruction,
                //   so the veto that protects them on close / quit / switch has
                //   nothing left to protect here and would only strand the server
                //   lock. Same reasoning as `speleodb_unlink`, which likewise drops
                //   the local link and so could never release from this project
                //   again. Unlike unlink it keeps the claim check (#576 RV5-F4-4):
                //   a session that only inherited the lock (a second window or
                //   computer on the same account, gate opened by the status poll)
                //   leaves it with the account instead of freeing the mutex the
                //   other session is editing under (decision RV2-R-13).
                held_lock = self.speleodb_delete_lock_release_params();
                // End OUR claim now, while the identity decision above is still
                // atomic under this guard. After the guard drops, a successor Start
                // Editing on the SAME remote project can mint a claim of its own in
                // this slot (a same-account re-acquire is a server-side no-op), and
                // the deferred release must be able to read "a claim exists" as
                // "the successor's, stand down" - see
                // `speleodb_release_unless_reclaimed`. Clearing here is also what
                // the old post-release clear did, just moved to where it cannot be
                // confused with someone else's record (F8).
                // Kept, not dropped: nothing has been destroyed yet, and the
                // trash below can still fail. A claim cleared over a file that
                // survived is a mutex left held with no local pointer to it -
                // the session is closed, so the poll's catch-alls never see it,
                // and the no-project janitor has nothing to retry. See the
                // trash-failure return, which puts it back.
                // [#576 A8] Under the guard by type: the clear takes its slot, and
                // [NF-A8R-GAPS] the release below takes the proof it returns.
                cleared_claim = held_lock
                    .as_ref()
                    .map(|p| self.speleodb_forget_lock_acquired_under(&open, &p.project_id));
                tracing::info!(
                    path = ?path,
                    purpose = "release the live handle before deleting the file",
                    "Closing the open project because its .stalx is being deleted"
                );
                self.close_open_project_locked(&commands, &mut open);
            }
            (deleting_open_project, held_lock)
        };

        // Which recents entries name the file we are about to trash - decided BEFORE
        // it is trashed, because the answer becomes unobtainable afterwards.
        //
        // The prune has to ask the same identity question the open-project check
        // above did, and it asks it through the same predicate. A delete spelled with
        // a `..` segment, through a symlinked parent directory, or (on Windows) in a
        // different case is still a delete of that row's project, and a `!=` on raw
        // paths trashed the file and left the row behind - the "File not found"
        // zombie this function's doc promises to avoid, and an unusually sticky one,
        // since the row's own file no longer exists for a second delete to remove.
        // But `same_file` CANONICALIZES, and canonicalization fails on a path whose
        // file is gone: run it after the trash and both sides fall back to a plain
        // comparison, i.e. back to the `!=` bug. So the identity comparison happens
        // while the file is still there, and the prune below only has to match the
        // paths it produced.
        //
        // Matching on file identity rather than spelling is also what keeps this from
        // over-pruning: two different projects can never canonicalize to one path. An
        // entry whose own file is missing still matches by exact spelling, through
        // `same_file`'s own fallback. (What that fallback cannot do is match a
        // DIFFERENT spelling of an ALREADY-missing file - there is nothing left to
        // canonicalize on either side - but that entry is re-openable-then-deletable
        // and "Remove from recent" still clears it.)
        //
        // Deleting a symlink ITSELF prunes only rows naming that exact path, because
        // that is the only file being destroyed: the referent's own row must survive,
        // or "delete this link" would silently cost the user their way back to a
        // project that is still on disk.
        let doomed: Vec<PathBuf> = self
            .recent_projects
            .lock()
            .iter()
            .filter(|entry| names_the_deleted_file(entry.as_path()))
            .cloned()
            .collect();

        // Hand the primary `.stalx` to the OS trash FIRST. If this fails, nothing
        // on disk has changed yet, so abort cleanly - recents stay untouched and
        // the caller can retry. (Tolerant of a file that's already gone.)
        //
        // The stat taken at the top is what decides this, and it is the only thing
        // allowed to: `Path::exists` FOLLOWS symlinks, so it answers `false` for a
        // DANGLING link (the row would leave recents while the broken link stayed on
        // disk) AND for a file it merely cannot reach (see the top of this function
        // for what that cost). `symlink_metadata` answered both questions honestly
        // there, an unreadable path has already been refused, so what is left here is
        // exactly "there is an entry" versus "there is provably nothing".
        if entry.is_ok() {
            if let Err(e) = send_to_trash(&path) {
                // Nothing was destroyed, so the lock is still ours and still
                // guarding a project that is still there. Put the claim back -
                // without it the mutex has no local pointer at all: this call
                // closed the session, so the poll's catch-alls will never run
                // over it, and the deferred release below is not reached from
                // here either.
                self.speleodb_restore_lock_claim(
                    cleared_claim
                        .and_then(super::speleodb_lock_release::ClaimClearedUnderSlot::into_claim),
                );
                return CommandResult::failure(format!("Failed to delete {}: {e}", path.display()))
                    .with_session_closed(session_closed);
            }
        }

        // From here on the project IS deleted as far as the user is concerned, so
        // the rest is best-effort and MUST still drop the recents entry - otherwise
        // a later failure (the config persist) would strand a "File not found"
        // zombie row, because the frontend skips its refresh on `!success`.
        // Collect non-fatal problems as warnings and still report overall success
        // so the UI prunes the deleted project from its list.
        let mut warnings: Vec<ValidationWarning> = Vec::new();

        // Drop it from the recent list, matching the entries identified as the same
        // file above rather than re-testing the (now deleted) path. Best-effort: a
        // project that was never in recents (or already removed) leaves `doomed`
        // empty and should still delete cleanly.
        {
            let mut recent = self.recent_projects.lock();
            let before = recent.len();
            recent.retain(|p| !doomed.contains(p));
            if recent.len() != before {
                if let Err(e) = self.persist_recent_projects(&mut recent, false) {
                    // The `.stalx` is already trashed and the in-memory recents
                    // are pruned; a failed persist must NOT report the whole
                    // delete as a failure (that strands the zombie row). Warn.
                    tracing::warn!("Failed to save config after deleting project: {}", e);
                    warnings.push(ValidationWarning {
                        code: "delete_project_persist_failed".to_string(),
                        message: format!(
                            "Project deleted, but the recent-projects list could not be saved: {e}"
                        ),
                        severity: WarningSeverity::Warning,
                    });
                }
            }
        }

        // Everything local is settled, so now hand back the server lock snapshotted
        // above. LAST, for two reasons:
        //
        // - AFTER the trash, so the release only happens if the abandon that licenses
        //   it actually did. The early return on a failed trash therefore skips this,
        //   which is the wanted behaviour: the file, its recents entry and its lock
        //   are all still the user's to come back to (only the session was torn
        //   down). Releasing FIRST and then failing to remove the file would hand the
        //   project's mutex back while un-pushed edits survive locally on a file the
        //   user still has - exactly the "work stranded behind a freed lock" outcome
        //   the silent-release veto exists to prevent, arrived at by a different
        //   route.
        // - AFTER the recents prune, because this is the only unbounded step in the
        //   function: an HTTP round trip to a server that may be slow or gone. Doing
        //   the local work first means a process killed mid-release still leaves a
        //   consistent recents list rather than a zombie row.
        //
        // Best-effort, like every other release: the `.stalx` is already trashed, so
        // a failure here cannot un-delete it and must not fail the call. It is
        // surfaced as a warning rather than only logged because the local end of that
        // lock no longer exists - nothing will retry it from this project, and the
        // user's only remaining route to the stranded mutex is the held-lock prompt.
        if let (Some(params), Some(cleared)) = (held_lock, &cleared_claim) {
            if let Err(e) = self.speleodb_release_captured_lock(cleared, &params) {
                tracing::warn!(
                    error = %e,
                    purpose = "surrender the write lock of a project being deleted",
                    "speleodb: could not release the deleted project's lock"
                );
                warnings.push(ValidationWarning {
                    code: "delete_project_lock_release_failed".to_string(),
                    message: format!(
                        "Project deleted, but its SpeleoDB write lock could not be released: {e}. \
                         Other divers stay locked out until it expires; you can release it from \
                         the held-lock prompt."
                    ),
                    severity: WarningSeverity::Warning,
                });
            }
        }

        tracing::info!(path = ?path, "Deleted project");
        CommandResult::success()
            .with_warnings(warnings)
            .with_session_closed(session_closed)
    }

    /// Rename a project's display name on disk by path, without requiring it to
    /// be the open project. Used by the recent-projects metadata panel: a
    /// targeted DB update, plus keeping the open project's in-memory name in
    /// sync if it happens to be the same file.
    ///
    /// [#576 RV5-5] Under `command_lock`, like every other mutation of the open
    /// project: the name is written through a second connection to the file, and a
    /// save running concurrently on the pool replaces that file with one written
    /// from the old in-memory name (the rename then reported success over a file
    /// that kept the old name), or replaced it before the write (which SQLite then
    /// refused). Held for a closed file too: deciding which one this is needs the
    /// `project` lock, which sits below `command_lock`, and the span is one small
    /// write.
    pub fn set_project_name_for_path(&self, path: PathBuf, name: String) -> CommandResult {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return CommandResult::failure("Project name cannot be empty".to_string());
        }

        let _commands = self.hold_commands();
        let db = match Database::open_existing(&path) {
            Ok(db) => db,
            Err(e) => return CommandResult::failure(format!("Failed to open project: {e}")),
        };

        // Read-only edit gate (#191, D-B6): this renames a `.stalx` BY PATH -
        // usually NOT the open AppState project
        // - so the transient `edit_enabled` flag (which describes the *open*
        // project) can only speak for it when the path IS the currently open
        // project. Compute the open-project identity FIRST, BEFORE the disk write,
        // and gate only on a match so `guard_mutation` aborts a rename of the
        // read-only open project before ANY mutation. The project name is remote
        // content (it is pushed to SpeleoDB), so a rename of a locked remote
        // project is a real edit. A *closed* remote file is not gated here
        // (deferred to the issue that owns closed-file remote state).
        let is_open_project = {
            let guard = self.project.lock();
            guard
                .as_ref()
                .is_some_and(|open| open.file_path.as_deref() == Some(path.as_path()))
        };
        // [#576 C5] On the read-only-workspace allowlist: a rename is allowed in
        // Plan/Export like the project-settings name save.
        if is_open_project {
            if let Err(f) = self.guard_metadata_mutation("Rename project") {
                return CommandResult::refused(f);
            }
        }
        // [#576 B6-D1] A file a NEWER build saved is read-only whether or not it
        // is the open project: renaming it is a write into a schema this build
        // does not know. The open one was refused just above; this is the closed
        // one the recent-projects panel can reach.
        if let Some(version) = db.newer_schema_version() {
            return CommandResult::refused(super::app_state::newer_file_edit_refusal(
                "Rename project",
                version,
            ));
        }

        if let Err(e) = db.save_project_name(trimmed) {
            return CommandResult::failure(format!("Failed to rename project: {e}"));
        }

        // Keep the open project's in-memory state consistent if it's the same file.
        if is_open_project {
            let mut guard = self.project.lock();
            if let Some(open) = guard.as_mut() {
                if open.file_path.as_deref() == Some(path.as_path()) {
                    open.name = trimmed.to_string();
                }
            }
            // Dirty-bit flip (#191, D-B16): the gated open project was mutated.
            // Set-only; no-op for local-only (no speleo_link).
            self.mark_remote_forward_edit();
        }

        tracing::info!(path = ?path, "Renamed project");
        CommandResult::success()
    }
}

/// Delete a single file by handing it to the OS's native delete.
///
/// Production prefers the platform trash (`trash::delete`) - macOS Trash, Windows
/// Recycle Bin, freedesktop trash on most Linux - so deletes are recoverable where
/// the OS supports it. When there's no usable trash for the file's filesystem (a
/// NAS / removable mount, `HOME` unset, a read-only FS), the trash call fails and
/// we fall back to a permanent `fs::remove_file` so the delete still happens - the
/// user asked for a real delete, and the OS provides recovery only where it can.
///
/// Test builds skip all of that and take the deterministic `fs::remove_file`
/// directly, so `cargo test` neither pollutes the developer's real Trash nor
/// depends on a trash directory existing on a headless CI runner; the Android
/// `cdylib` target does the same in production, since it has no desktop trash
/// and excludes the `trash` crate. Test builds still COMPILE the `trash` crate -
/// only the call to it is skipped.
///
/// The gate is `test-support` as well as `cfg(test)`: a dependent crate's tests
/// (the app crate's `command_tests`) compile this crate WITHOUT `cfg(test)`, and
/// on macOS the `trash` crate's `delete` can raise a per-binary Automation
/// consent prompt (observed as focus-stealing TCC dialogs during test runs).
/// The gate is unconditional across all platforms, not just macOS: headless CI
/// runners and deterministic test output matter everywhere `trash::delete`
/// would otherwise touch a real, environment-dependent trash implementation.
///
/// `test-support` is further gated on `debug_assertions`: a release build
/// (`debug_assertions` off) that somehow still has `test-support` enabled has
/// NO arm here at all and fails to compile, rather than silently taking the
/// deterministic-delete stub in a shipped binary. That configuration -
/// release-profile with `test-support` - is deliberately uninhabited; the
/// only way to reach it is a mis-wired dependency edge that pulls a dev/test
/// feature into a release build, and that should be a compile error, not a
/// behavior change.
#[cfg(all(not(test), not(feature = "test-support"), not(target_os = "android")))]
fn send_to_trash(path: &Path) -> Result<(), String> {
    after_trash_attempt(trash::delete(path), path)
}

// Android has no desktop trash and the `trash` crate isn't built there (the
// dependency is gated off `target_os = "android"` in Cargo.toml), so fall straight
// through to a permanent remove. The app ships desktop-only; this just keeps the
// Android `cdylib` target compiling.
#[cfg(all(not(test), not(feature = "test-support"), target_os = "android"))]
fn send_to_trash(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

#[cfg(any(test, all(feature = "test-support", debug_assertions)))]
fn send_to_trash(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

/// Turns the result of a platform trash attempt into this module's `Result`,
/// falling back to a permanent `fs::remove_file` when the trash could not take
/// the file. Split out of [`send_to_trash`]'s real (desktop, non-test) arm so
/// the fallback branch and its message format stay covered by a unit test even
/// though that arm itself never runs in a test build.
///
/// Compiled in test builds (for the unit tests below) and in the real desktop
/// production arm (where it is actually called); not compiled anywhere else, so
/// it is never dead code.
#[cfg(any(test, all(not(feature = "test-support"), not(target_os = "android"))))]
fn after_trash_attempt(result: Result<(), trash::Error>, path: &Path) -> Result<(), String> {
    match result {
        Ok(()) => Ok(()),
        Err(trash_err) => std::fs::remove_file(path).map_err(|fs_err| {
            format!("trash failed ({trash_err}); permanent delete also failed ({fs_err})")
        }),
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::{after_trash_attempt, AppState};

    /// The stub arm covers the happy path everywhere; this re-covers
    /// `after_trash_attempt`'s pass-through of a successful trash call, since
    /// the real `send_to_trash` arm that calls it never runs in a test build.
    #[test]
    fn after_trash_attempt_passes_through_success() {
        let temp = tempfile::TempDir::new().expect("temp");
        let path = temp.path().join("unused.stalx");
        assert_eq!(after_trash_attempt(Ok(()), &path), Ok(()));
    }

    /// When the platform trash fails, the fallback permanently removes the
    /// file and still reports success - this is the branch the production
    /// `send_to_trash` arm exercises but no test config compiles it (see the
    /// arm's doc comment), so it is covered here directly.
    #[test]
    fn after_trash_attempt_falls_back_to_remove_file_on_trash_error() {
        let temp = tempfile::TempDir::new().expect("temp");
        let path = temp.path().join("project.stalx");
        std::fs::write(&path, b"fallback should remove this").expect("seed the file");

        let trash_err = trash::Error::Unknown {
            description: "synthetic trash failure".to_string(),
        };
        let result = after_trash_attempt(Err(trash_err), &path);

        assert_eq!(result, Ok(()));
        assert!(
            !path.exists(),
            "the fallback must have permanently removed the file"
        );
    }

    /// When the platform trash AND the fallback both fail, the error message
    /// pins the "trash failed" prefix and includes both underlying errors -
    /// the format the frontend surfaces to the user.
    #[test]
    fn after_trash_attempt_reports_both_errors_when_fallback_also_fails() {
        let temp = tempfile::TempDir::new().expect("temp");
        let path = temp.path().join("missing.stalx");
        assert!(!path.exists(), "precondition: nothing to remove");

        let trash_err = trash::Error::Unknown {
            description: "synthetic trash failure".to_string(),
        };
        let result = after_trash_attempt(Err(trash_err), &path);

        let err = result.expect_err("both the trash call and the fallback failed");
        assert!(
            err.starts_with("trash failed ("),
            "message must be prefixed with 'trash failed (': {err}"
        );
        assert!(
            err.contains("synthetic trash failure"),
            "message must include the trash error text: {err}"
        );
        assert!(
            err.contains("permanent delete also failed"),
            "message must include the fs::remove_file error: {err}"
        );
    }

    /// A path the app cannot STAT is not an absent path, and reading it as one
    /// was destructive-by-report. `Path::exists` and `is_ok_and` both flatten an
    /// access denial into `false`, so a project behind a lost folder permission
    /// looked exactly like a project already gone: the trash was skipped, the
    /// recents row was pruned, and an open managed project had its captured
    /// server lock released under the EXPLICIT-ABANDON policy - the policy that
    /// exists because destroying the `.stalx` forfeits its un-pushed edits. The
    /// file and those edits were still sitting there, now behind a mutex any
    /// other diver could take.
    #[cfg(unix)]
    #[test]
    fn a_path_that_cannot_be_stat_ed_is_refused_rather_than_read_as_already_gone() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::TempDir::new().expect("temp");
        let dir = temp.path().join("locked");
        std::fs::create_dir(&dir).expect("mkdir");
        let path = dir.join("project.stalx");
        std::fs::write(&path, b"still here, and never read").expect("seed the file");

        // Deny everything on the PARENT, which is what a lost folder permission
        // looks like from here: the file is still on disk, we simply cannot look
        // at it.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000))
            .expect("lock the directory");
        // Root ignores the bits, and then there is no unreadable path to test
        // with: fail rather than pass on nothing (#576 NF-ROOT-PERM).
        crate::models::test_support::assert_lookup_refused(&path);
        let state = AppState::new().expect("state");
        let result = state.delete_project(path.clone());
        // Unlocked before any assertion, so a failing assert cannot leave a
        // directory the temp-dir cleanup is unable to remove.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("unlock the directory");

        assert!(
            !result.success,
            "a path we cannot read must not report a successful delete"
        );
        assert!(
            path.exists(),
            "and nothing may have been removed on the strength of a failed stat"
        );
    }
}
