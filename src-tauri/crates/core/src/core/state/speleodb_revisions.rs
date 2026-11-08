//! [#218] SpeleoDB revision history and the View / Restore swap.
//!
//! Split out of `core/state/speleodb.rs` (which is frozen at the file-size ratchet
//! warn line) into a sibling `impl AppState` block.
//!
//! The two modes share one command and differ in what they promise. A **Restore**
//! rides FORWARD on HEAD: the chosen tree lands in the working copy as an unpushed
//! edit with BASE stamped at the CURRENT server tip, so history is never rewritten
//! and the next push is an ordinary push. A **View** is a read-only peek that pins
//! BASE to the viewed hexsha and stays clean, so the next Update fast-forwards it
//! away. Both replace the cache `.stalx` and reload the project - a View is not
//! ephemeral, only reversible.

use super::speleodb::{local_dto, validate_project_id, DumpAt};
use super::speleodb_cache::{stamp_pulled_file, InheritedUndo, LocalEditsPolicy};
use super::speleodb_pull::{ArianeRoute, StalxAmbiguity};
use super::AppState;
use crate::models::UndoMode;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::models::{SpeleoRevision, SpeleoRewindTarget, SpeleoUnwindTarget};
use crate::speleodb::{client, dump, sync};
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto, SpeleoOpenResult};

/// Read one downloaded revision's effective undo mode without installing it over the
/// working copy. Revision metadata does not carry this policy, so a forward chain step
/// has to inspect the child `.stalx` it would load.
fn downloaded_revision_undo_mode(
    dump: &mut dump::Dump<'_>,
    entry: &str,
    scratch_base: &std::path::Path,
) -> Result<UndoMode, SpeleoDbError> {
    let mut name = scratch_base
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("project.stalx"))
        .to_os_string();
    name.push(format!(
        ".undo-mode.{}.stalx",
        uuid::Uuid::new_v4().simple()
    ));
    let probe = scratch_base.with_file_name(name);

    let result = (|| {
        dump.extract_entry_to(entry, &probe)?;
        let db = crate::persistence::Database::open_for_metadata(&probe).map_err(|e| {
            SpeleoDbError::Io(format!(
                "could not open the downloaded revision to read its undo policy: {e}"
            ))
        })?;
        let stored_mode = db
            .load_speleo_link()
            .map_err(|e| {
                SpeleoDbError::Io(format!(
                    "could not read the downloaded revision's undo policy: {e}"
                ))
            })?
            .and_then(|link| link.undo_mode);
        // NULL revisions predate shared undo history. They cannot prove that a stack
        // was intentionally published, so never reinterpret them using this machine's
        // current global default.
        Ok(stored_mode.map_or(UndoMode::Disabled, UndoMode::resolved))
    })();

    if let Err(e) = std::fs::remove_file(&probe) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                error = %e,
                path = %probe.display(),
                purpose = "clean up a downloaded revision policy probe",
                "could not remove the temporary revision policy probe"
            );
        }
    }
    result
}

struct RewindCandidate {
    target: SpeleoRewindTarget,
    endpoint: String,
    token: String,
    insecure_http: bool,
    project_id: String,
}

/// [#218] Validate a git commit SHA supplied over IPC (revision restore) before it
/// is interpolated into the download-at-hash URL. A git object id is 7..=40 lowercase
/// or uppercase hex digits (the `/revisions/` endpoint returns full 40-hex ids; a
/// short prefix is also accepted). Rejecting anything else closes the route-traversal
/// vector at the trust boundary, mirroring [`validate_project_id`].
pub(super) fn validate_hexsha(hexsha: &str) -> Result<(), SpeleoDbError> {
    let len = hexsha.len();
    if (7..=40).contains(&len) && hexsha.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(SpeleoDbError::InvalidRevision)
    }
}

/// [#218] The BASE / dirty stamp for a revision swap, keyed on the mode. Pure (no
/// I/O, no clock) so both branches are unit-testable in isolation from the download.
///
/// - **Restore** (`restore == true`): the old tree rides as an unpushed edit ON TOP
///   of the current server HEAD, so BASE = `server_tip` and the dirty bit is set. A
///   push lands it as a forward commit (SpeleoDB exposes no server-side revert). The
///   one exception is restoring the tip itself, which changes nothing and so leaves
///   the copy clean (#303 - see the branch).
/// - **View** (`restore == false`): BASE = the viewed `hexsha` itself and the copy is
///   clean, so the working tree IS that revision (behind tip) and a later Update
///   fast-forwards straight back to the latest.
///
/// `server_tip` is `None` only when the HEAD fetch failed with no fallback; the caller
/// prefers the open link's last-known BASE over `None` so the next fence can't read a
/// spurious `Diverged`.
fn revision_swap_base(
    restore: bool,
    hexsha: &str,
    server_tip: Option<String>,
) -> (Option<String>, bool) {
    if restore {
        // [#303] ...EXCEPT when the revision restored IS the tip, where there is
        // nothing to push: the copy was just downloaded at the commit the server
        // already holds, so its content equals the server's by construction.
        //
        // This is the Unwind's first step (`unwind_target` returns the current commit
        // so its history can be loaded back), and it is why a project could get stuck
        // reporting "the upload could not be confirmed": the stamp made Done push a
        // commit with no change in it, and what the server does with that is
        // deployment-specific - one answers 304, another closes the connection
        // mid-request, which is indistinguishable from a lost upload and so keeps the
        // lock and asks the user to retry. Retrying re-sent the same empty commit.
        //
        // Nothing is at risk from not stamping it: undo/redo call `mark_remote_dirty`
        // like any other mutation, so the moment the user actually steps back through
        // the loaded history the copy is dirty again and Done pushes it. Done over a
        // copy nobody has touched now just releases the lock.
        // Case-insensitive like the six SHA comparisons in `sync.rs`: git object ids come
        // back from the server and either case is legitimate, and a false "not the tip"
        // here stamps the copy dirty - which is exactly the empty-commit push the block
        // above exists to prevent.
        let restored_the_tip = server_tip
            .as_deref()
            .is_some_and(|tip| tip.eq_ignore_ascii_case(hexsha));
        (server_tip, !restored_the_tip)
    } else {
        (Some(hexsha.to_owned()), false)
    }
}

/// [#303] Would an Unwind here throw away work that was never pushed?
///
/// True when the copy is dirty and no chain pointer accounts for it. A restore stamps the
/// copy un-pushed itself, so from an Unwind's second step onwards the dirt IS an Unwind's
/// own doing and the pointer proves it - that case discards nothing the user did not
/// already agree to. Dirt with NO pointer is different: real un-pushed work whose history
/// the undo stack no longer covers (edits the user undid, so they sit on the redo stack; a
/// byte-budget trim; a legacy file), and the swap replaces all of it.
///
/// One predicate, two readers - `speleodb_unwind_target` puts the answer on the DTO so the
/// dialog can warn BEFORE the user commits, and `speleodb_guard_unwind` decides with it.
/// Keeping them on the same function is what stops the warning and the refusal from
/// drifting apart, which is how this surfaced: the entry offered an action that only ever
/// failed.
fn unwind_discards_unpushed(has_unpushed_edits: bool, unwound_from: Option<&str>) -> bool {
    has_unpushed_edits && unwound_from.is_none()
}

impl AppState {
    /// [#303] The preconditions for an Unwind - the one action allowed to keep a
    /// downloaded copy's undo history, and therefore the one that has to earn it.
    ///
    /// Two of them, and the second is the interesting one:
    ///
    /// 1. **The stack in the direction of travel must be EMPTY.** That is what makes "the
    ///    working copy equals a commit" true, and which stack it is mirrors the step.
    ///    Going BACK, the undo stack: it is cleared at a lock or a confirmed push, so
    ///    undoing everything since then returns the tree to exactly that commit's state.
    ///    Going FORWARD, the redo stack: every operation of the commit being sat on has
    ///    been re-applied, so the tree really is that commit's and the child's history
    ///    stacks onto it cleanly. Either way, entries left on that stack mean the copy sits
    ///    somewhere BETWEEN commits, and checking out a revision would silently discard the
    ///    work that has not been walked through yet.
    /// 2. **Dirt that no Unwind put there must be acknowledged.** See
    ///    [`unwind_discards_unpushed`] for which dirt that is. It used to be a flat
    ///    refusal, on the grounds that un-pushed work "must not be discarded on the
    ///    strength of a waiver the user was never shown" - but the affordance is offered
    ///    exactly when the undo stack is empty, which is also what undoing everything
    ///    produces, so the button existed only to fail. The waiver is now SHOWN: the DTO
    ///    carries the same predicate, the dialog raises a red warning on it, and
    ///    `discard_unpushed` is that dialog reporting it did. Absent the acknowledgement
    ///    this still refuses, so no other caller can discard un-pushed work by passing
    ///    `keep_undo_history` alone.
    /// [#303] Is the revision being asked for actually the step the claimed direction
    /// leads to?
    ///
    /// `chain_forward` arrives raw over IPC and decides WHICH stack `speleodb_guard_unwind`
    /// requires to be empty - undo going back, redo going forward. So a backward step
    /// declared forward is measured against the redo stack, which an ordinary session
    /// leaves empty, and the guard waves through exactly the case it exists to refuse: a
    /// copy sitting between commits, whose un-walked undo entries the swap then discards.
    /// The structural tripwire only greps for `if chain_forward`, so it cannot see this.
    ///
    /// Rather than trust the flag, ask the resolver it names and require the requested
    /// `hexsha` to be what that resolver returns. A lie about direction fails because the
    /// SHA will not match. Costs the same `GET /revisions/` the affordance already spent to
    /// offer the step, and it is spent only on the `keep_undo_history` path - the one that
    /// gets to keep a downloaded copy's history, and so the one that has to earn it.
    ///
    /// Case-insensitive, like every other SHA comparison in this feature.
    fn speleodb_verify_chain_target(
        &self,
        hexsha: &str,
        chain_forward: bool,
    ) -> Result<(), SpeleoErrorDto> {
        let expected = if chain_forward {
            self.speleodb_rewind_candidate()?
                .map(|candidate| candidate.target.hexsha)
        } else {
            self.speleodb_unwind_target()?.map(|t| t.hexsha)
        };
        if expected
            .as_deref()
            .is_some_and(|sha| sha.eq_ignore_ascii_case(hexsha))
        {
            return Ok(());
        }
        Err(local_dto(SpeleoDbError::refused(
            "that revision is not the next step in the direction requested; reopen the \
             undo history to pick it up again"
                .to_owned(),
        )))
    }

    fn speleodb_guard_unwind(
        &self,
        link: &crate::models::SpeleoLink,
        discard_unpushed: bool,
        chain_forward: bool,
    ) -> Result<(), SpeleoErrorDto> {
        // A poisoned stack is REFUSED: the guard exists to stop a chain step over
        // un-walked work, so it has to fail closed on not knowing - and say why
        // (#576 A11, the second refusal site in `refuse_after_crash`).
        self.refuse_after_crash("Loading another version's history")
            .map_err(|refusal| local_dto(super::speleodb::refused_by(refusal)))?;
        let stack_empty = {
            let s = self.undo_stack.lock();
            if chain_forward {
                s.redo_len() == 0
            } else {
                s.is_empty()
            }
        };
        if !stack_empty {
            return Err(local_dto(SpeleoDbError::refused(
                if chain_forward {
                    "redo the remaining steps before loading the next version's history"
                } else {
                    "undo the remaining steps before loading earlier history"
                }
                .to_owned(),
            )));
        }
        if !discard_unpushed
            && unwind_discards_unpushed(
                link.has_unpushed_edits,
                self.speleodb_unwound_from().as_deref(),
            )
        {
            return Err(local_dto(SpeleoDbError::refused(
                "this copy holds un-pushed edits that the undo history no longer covers; \
                 push or discard them before loading earlier history"
                    .to_owned(),
            )));
        }
        Ok(())
    }

    /// [#303] The revision the working copy has been unwound to, if any.
    pub(super) fn speleodb_unwound_from(&self) -> Option<String> {
        self.database
            .lock()
            .as_ref()
            .map(crate::persistence::Database::load_speleo_undo_chain)?
            .ok()
            .flatten()
    }

    /// [#303] Record (or clear) that watermark.
    ///
    /// Best-effort on the way in, like the chain pointer - but the two failures are not
    /// symmetric. Losing the chain pointer costs a walk its position; losing this lets a
    /// loaded revision's history ride into the next commit as though it were authored
    /// here, which is the duplication this exists to stop. Hence the louder log.
    pub(super) fn speleodb_set_inherited_through(&self, instant: Option<&str>) {
        {
            let guard = self.database.lock();
            if let Some(db) = guard.as_ref() {
                if let Err(e) = db.save_speleo_undo_watermark(instant) {
                    tracing::warn!(
                        error = %e,
                        purpose = "record the inherited-history watermark (#303)",
                        "could not persist the undo watermark; the next push may carry a \
                         loaded revision's history as if this session had authored it"
                    );
                }
            }
        }
        // [#303] Mirror it onto the stack, which needs it to stamp new entries ABOVE the
        // watermark - see `UndoStack::next_stamp`. This is the single funnel every set and
        // clear goes through, so the mirror cannot drift by a caller forgetting it.
        //
        // Sequenced after the database guard is dropped rather than inside it: the house
        // order is database-then-`undo_stack`, and holding neither across the other keeps
        // that trivially true.
        let parsed = instant.and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc))
        });
        self.undo_stack.lock().set_inherited_through(parsed);
    }

    /// [#303] Stamp the watermark for a stack that has just been loaded by a chain step.
    ///
    /// `now` would almost always do, but not quite: the entries came off another machine,
    /// and a pusher with a skewed clock can stamp them in the FUTURE. Those would then sit
    /// above the watermark and ride into the commit anyway, which is the whole bug. Taking
    /// the later of `now` and the newest loaded entry makes "at or before" true of every
    /// row that was just loaded, whatever clock wrote it.
    fn speleodb_stamp_inherited_watermark(&self) {
        let newest = self
            .undo_stack
            .lock()
            .undo_entries()
            .filter_map(|entry| entry.timestamp)
            .max();
        let now = chrono::Utc::now();
        let watermark = newest.map_or(now, |newest| newest.max(now));
        self.speleodb_set_inherited_through(Some(&watermark.to_rfc3339()));
    }

    /// [#303] A forward edit means the working copy is no longer where the Unwind walk
    /// left it, so the chain pointer stops describing it - and, more to the point, stops
    /// EXCUSING the dirty bit.
    ///
    /// That excuse is the whole reason walking the chain does not warn: the dirt is the
    /// Unwind's own doing, and every operation at risk came from a commit the server still
    /// holds, so another Unwind fetches it straight back. An edit breaks that. The new work
    /// exists nowhere but this copy, and a later Unwind would replace it - which is exactly
    /// the case the red warning is for, so dropping the pointer re-arms it.
    ///
    /// Called from the three forward-command sites in `commands.rs` and nowhere else:
    /// undo / redo must NOT clear it, since walking back through loaded history is the
    /// thing the pointer exists to track.
    pub(super) fn speleodb_forget_unwind_position(&self) {
        // Skip the write on the overwhelmingly common path - no Unwind in play at all.
        if self.speleodb_unwound_from().is_some() {
            self.speleodb_set_unwound_from(None);
        }
    }

    /// [#303] Record (or clear) the Unwind chain pointer. Best-effort: losing it costs
    /// the next Unwind its position - it restarts from BASE, which re-loads a history the
    /// user already has rather than destroying anything.
    pub(super) fn speleodb_set_unwound_from(&self, sha: Option<&str>) {
        let guard = self.database.lock();
        if let Some(db) = guard.as_ref() {
            if let Err(e) = db.save_speleo_undo_chain(sha) {
                tracing::warn!(
                    error = %e,
                    purpose = "record the Unwind chain position (#303)",
                    "could not persist the undo chain pointer; the next Unwind will restart from BASE"
                );
            }
        }
    }

    /// [#303] Which revision the next Unwind would load, and whether it is the commit the
    /// working copy already equals.
    ///
    /// `None` when there is nothing to reach: no link, an empty repo, the root of the
    /// history, or a chain pointer naming a commit outside the fetched range. The
    /// frontend hides the affordance on `None` rather than offering a step that fails.
    ///
    /// Needs the revision topology, so it costs one `GET /revisions/`. Called when the
    /// undo dropdown opens (and by the confirmation dialog), not on the status poll.
    pub fn speleodb_unwind_target(&self) -> Result<Option<SpeleoUnwindTarget>, SpeleoErrorDto> {
        let link = self.speleodb_validated_link()?;
        // Disabled mode never SHIPPED history, so there is nothing in those commits to
        // come back for. Older commits pushed while the project was Full still have it,
        // but offering the affordance here would mostly find empty stacks.
        if !self.undo_policy().pushes_history() {
            return Ok(None);
        }
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        let (_, commits) = self
            .speleodb_server_head(&endpoint, &token, insecure_http, &link.project_id)
            .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;

        let unwound_from = self.speleodb_unwound_from();
        // The same predicate the guard decides with, so the dialog's red warning and the
        // backend's refusal can never disagree about this copy.
        let discards_unpushed =
            unwind_discards_unpushed(link.has_unpushed_edits, unwound_from.as_deref());

        Ok(sync::unwind_target(
            unwound_from.as_deref(),
            link.base_commit_sha.as_deref(),
            &commits,
        )
        .map(|(hexsha, is_current)| SpeleoUnwindTarget {
            hexsha,
            is_current,
            discards_unpushed,
        }))
    }

    /// Resolve the topology-only forward candidate while keeping the authenticated
    /// request context that the public target query needs to inspect its policy.
    fn speleodb_rewind_candidate(&self) -> Result<Option<RewindCandidate>, SpeleoErrorDto> {
        let link = self.speleodb_validated_link()?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        let (_, commits) = self
            .speleodb_server_head(&endpoint, &token, insecure_http, &link.project_id)
            .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;
        Ok(sync::rewind_target(
            self.speleodb_unwound_from().as_deref(),
            link.base_commit_sha.as_deref(),
            &commits,
        )
        .map(|(hexsha, is_latest)| RewindCandidate {
            target: SpeleoRewindTarget { hexsha, is_latest },
            endpoint,
            token,
            insecure_http,
            project_id: link.project_id,
        }))
    }

    /// [#303] Which revision a forward chain step would load, and whether that step returns
    /// the copy to the latest version.
    ///
    /// `None` whenever there is nothing ahead: not mid-walk (so the copy is already at
    /// BASE), already home, a chain pointer that cannot be walked from, or a target child
    /// whose policy did not publish undo history. The first of those is what makes an edit
    /// remove this direction - a forward command drops the pointer, so the target stops
    /// resolving and the frontend hides the entry rather than offering a step that would
    /// discard the edit.
    ///
    /// Costs one `GET /revisions/` plus one download of the resolved child. The revisions
    /// API does not carry undo policy, and the CHILD'S mode is the answer: that is the
    /// revision whose history a forward step loads, regardless of the mode in the older
    /// copy currently open.
    pub fn speleodb_rewind_target(&self) -> Result<Option<SpeleoRewindTarget>, SpeleoErrorDto> {
        /// [#576 A8 NF-A8-BYPASS] This query's download licence. It reads the child
        /// revision's undo policy and installs nothing, so nothing has to run before
        /// it. Local to this function, so no other flow can borrow it.
        struct AffordanceProbe;
        impl client::DumpLicence for AffordanceProbe {}
        let Some(candidate) = self.speleodb_rewind_candidate()? else {
            return Ok(None);
        };

        let zip = self
            .speleodb_handle_auth(
                Endpoint::DownloadDump,
                client::download_dump_at_hash(
                    &AffordanceProbe,
                    &candidate.endpoint,
                    &candidate.token,
                    candidate.insecure_http,
                    &candidate.project_id,
                    &candidate.target.hexsha,
                ),
            )
            .map_err(|e| dto_for_error(Endpoint::DownloadDump, &e))?;
        let mut dump = dump::Dump::open(&zip).map_err(local_dto)?;
        let entry = self
            .speleodb_resolve_entry_remembering(
                &candidate.project_id,
                &mut dump,
                None,
                ArianeRoute::Refuse,
                DumpAt::Revision(&candidate.target.hexsha),
                StalxAmbiguity::Refuse,
            )
            .map_err(local_dto)?;
        let scratch_base = self
            .speleodb_cache_path(&candidate.project_id)
            .map_err(local_dto)?;
        let target_mode =
            downloaded_revision_undo_mode(&mut dump, &entry, &scratch_base).map_err(local_dto)?;

        Ok(target_mode.pushes_history().then_some(candidate.target))
    }

    /// [#218] The project's revision history for the read-only history view:
    /// `GET /projects/{id}/revisions/`, projected to [`SpeleoRevision`] DTOs (no
    /// token, only the display fields). Takes an arbitrary project id (a GlobalView
    /// remote row need not be the open project), so it validates the id at the trust
    /// boundary exactly like `speleodb_release_lock`, then reuses the same server-head
    /// fetch the sync path uses. The server's own order is NOT preserved: the list is
    /// re-ordered newest-first by walking the parent chain from the tip (the server
    /// sorts by `authored_date`, which ties on a freshly-created project), and the
    /// automated project-creation commit is dropped - so a project with nothing but
    /// that commit yields an empty list. Both live in [`sync::display_history`], which
    /// touches this view only; every fence reads the raw history. No lock required
    /// (reading history is permission-free). [#576 W90] The open copy of another
    /// server than the one signed in to gets an empty list, with nothing sent
    /// ([`Self::speleodb_names_a_copy_of_another_server`]).
    pub fn speleodb_get_revisions(
        &self,
        project_id: String,
    ) -> Result<Vec<SpeleoRevision>, SpeleoErrorDto> {
        validate_project_id(&project_id).map_err(local_dto)?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        // [#576 W90] The open copy of another server: this server's history for its
        // id is not this copy's. Nothing is sent; an empty history, nothing to show.
        if self.speleodb_names_a_copy_of_another_server(&project_id, &endpoint) {
            return Ok(Vec::new());
        }
        let insecure_http = self.speleodb_insecure_http();
        let (_, commits) = self
            .speleodb_server_head(&endpoint, &token, insecure_http, &project_id)
            .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;
        // Display ordering + the creation-commit filter live in `sync` so they are
        // unit-tested offline, and so it is obvious they touch ONLY this view: every
        // fence reads the raw history through `speleodb_server_head` directly.
        Ok(sync::display_history(commits)
            .into_iter()
            .map(SpeleoRevision::from)
            .collect())
    }

    /// [#218] Check the open remote project's working copy out to a past revision.
    /// Download the project tree AT `hexsha` (the server's `FileDownloadAtHashView`),
    /// replace the cached `.stalx`, re-stamp the hand-off link, and reopen the swapped
    /// tree in place. Two modes, keyed on `restore`:
    ///
    /// - `restore == true` (**Restore**, the user holds the lock): a git-FORWARD
    ///   restore. BASE stays at the CURRENT server HEAD and `has_unpushed_edits` is set
    ///   true, so the tree rides as an unpushed edit ON TOP of HEAD; editing is
    ///   re-enabled. The normal push / Done flow lands the old content as a NEW commit,
    ///   so project history is never rewritten (SpeleoDB exposes no server-side revert).
    /// - `restore == false` (**View**, read-only, no lock): BASE = `hexsha` and the
    ///   copy is stamped CLEAN, so the working tree IS that revision, behind tip. The
    ///   project stays read-only and a later Update fast-forwards straight back to the
    ///   latest - a non-destructive peek.
    ///
    /// Both ids are validated at the trust boundary before the download URL is built,
    /// and the swap is BOUND to the open project (a mismatched id is refused). A
    /// force-edit session (#213) is refused outright (both View and Restore), and
    /// Restore additionally requires the write lock genuinely held while View is refused
    /// under it - the backend never trusts the FE's View/Restore choice. Returns the
    /// local path (like a pull); the FE pulls the swapped geometry.
    pub fn speleodb_restore_revision(
        &self,
        project_id: String,
        project_name: String,
        hexsha: String,
        restore: bool,
        discard_local: bool,
        entry_path: Option<String>,
        keep_undo_history: bool,
        discard_unpushed: bool,
        chain_forward: bool,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        validate_project_id(&project_id).map_err(local_dto)?;
        validate_hexsha(&hexsha).map_err(local_dto)?;

        // Bind to the OPEN project. A revision swap replaces the whole tree in place,
        // so a stale / mismatched FE project_id must never close the open project and
        // swap in a DIFFERENT cached one behind the unsaved-changes guard - every other
        // edit verb goes through the open link, and so does this.
        // [#576 RVF-c-C1] Read in one `command_lock` span with the identity check: a
        // swap made for another project than the open one (another copy of the same
        // SpeleoDB project included) is refused with PROJECT_CHANGED.
        let open_link = {
            let _commands = self.hold_commands();
            self.speleodb_refuse_other_project("Viewing or restoring a revision")?;
            self.speleodb_open_link().map_err(local_dto)?
        };
        if open_link.project_id != project_id {
            return Err(local_dto(SpeleoDbError::NoRemoteProject));
        }
        // [#213] A force-edit session holds unsaved edits with NO lock protecting them.
        // Swapping the tree (View OR Restore) would silently clobber that work, and
        // clearing force_editing below would hide the big-red "no lock held" warning.
        // Refuse both until the user finishes or discards the force-edit session.
        if self.force_editing() {
            return Err(local_dto(SpeleoDbError::refused(
                "finish or discard force-editing before viewing or restoring a revision".to_owned(),
            )));
        }
        // [#576 RV2-R-15] Restore and Unwind replace the survey, so the read-only Plan
        // and Export workspaces refuse them like every other survey change (C5's
        // `guard_workspace`), with the same coded notice - and before the lock check,
        // so the user is told to switch to Survey rather than to take a lock. View is a
        // read-only peek and stays allowed in every workspace.
        if restore {
            let what = if keep_undo_history {
                "Unwind"
            } else {
                "Restore revision"
            };
            // The coded refusal verbatim (not wrapped in an error's own prefix), so
            // the frontend recognises it as the edit gate and shows its notice.
            self.speleodb_guard_workspace(what)?;
        }
        // Restore (editable) additionally requires the write lock genuinely held (with
        // force-editing already excluded above, edit_enabled here means a real lock).
        // View is a read-only peek and needs no lock.
        if restore && !self.edit_enabled() {
            return Err(local_dto(SpeleoDbError::refused(
                "acquire the write lock before restoring a revision".to_owned(),
            )));
        }
        // [#303] Keeping the downloaded copy's undo history is a LOCK-HELD RESTORE or
        // nothing. (Spelled "the Keep variant" rather than in full: the
        // `undo_history_keep_is_unwind_only` tripwire counts raw occurrences of that
        // literal across the production sources, prose included, so writing it here would
        // read as a second waiver site.)
        // The Keep variant hands a server tree's history to a stack the user
        // can then push from, and writes the chain pointer and the inherited watermark that
        // decide what the next upload cuts - which is why `repair.rs` and the
        // `InheritedUndo` docs both describe it as reachable only through an explicit,
        // confirmed, lock-held Unwind. Nothing in the backend made that true: the gate above
        // asks about `restore` alone, so `keep_undo_history: true` with `restore: false`
        // took the Keep branch, stamped the watermark and wrote the pointer with no lock
        // held. Only `useRestoreRevision.ts`'s clamp stood in the way, and an invariant the
        // whole push path is written against cannot live in one frontend caller.
        //
        // Spelled as the full precondition rather than `!restore`, so it does not quietly
        // depend on the gate above running first: this refusal has to hold on its own.
        if keep_undo_history && !(restore && self.edit_enabled()) {
            return Err(local_dto(SpeleoDbError::refused(
                "loading a revision's undo history needs a restore with the write lock held"
                    .to_owned(),
            )));
        }
        // [#576 FZ-SDB-4] View is the read-only, no-lock peek, so it is refused while
        // editing under the write lock (force-editing is refused above, so an open
        // gate here means a lock). The View reload shuts the gate without handing the
        // lock claim back, so the next status poll forgets the claim and the quit-time
        // silent release then leaves the lock held. The UI offers View only without the
        // lock; this holds for a direct IPC call too, and refuses before anything is
        // captured or closed. (Below the Keep refusal so a View asking to keep history
        // is still told what it asked for.)
        //
        // [#576 SDB-ACCT-D1, SDB-QUIT-D1] The open gate alone decides, as it does for
        // the silent releases: a lock this session adopted from the account (the
        // poll's inherited-lock reopen) is one the quit hands back too, so a View
        // over it would strand it the same way. It used to read this session's claim
        // as well (#576 RVF-b), when an adopted lock had none and no release was ever
        // going to hand it back.
        if !restore && self.edit_enabled() {
            return Err(local_dto(SpeleoDbError::refused(
                "release the write lock (or restore) before viewing a revision".to_owned(),
            )));
        }
        // [#303] The Unwind flavour: keeping the downloaded copy's undo history is a
        // waiver of the #194 b3 guard, so its preconditions are checked HERE and never
        // taken on trust from the frontend. `discard_unpushed` is not a decision the FE
        // gets to make either - it only reports that the dialog showed the user what this
        // throws away; the guard still decides whether that matters.
        //
        // [#576 A8] The download below is a method of the proof this returns, so the
        // preconditions cannot move behind it (a check after the download would refuse
        // a working copy that had already been replaced).
        let unwind = self.speleodb_unwind_preconditions(
            keep_undo_history,
            &open_link,
            discard_unpushed,
            chain_forward,
            &hexsha,
        )?;

        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        // [#576 W89] View and Restore download the signed-in server's project under the
        // link's id and stamp the link with the session's server, so a copy of another
        // server is refused before the download.
        super::speleodb_session::refuse_a_copy_on_another_server(
            &open_link.endpoint,
            Some(&endpoint),
        )
        .map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        let zip = self
            .speleodb_handle_auth(
                Endpoint::DownloadDump,
                unwind.download(&endpoint, &token, insecure_http, &project_id, &hexsha),
            )
            .map_err(|e| dto_for_error(Endpoint::DownloadDump, &e))?;

        // Open the dump once: list entries (to classify) and extract the chosen
        // `.stalx` from the same parsed archive (identical to the pull path).
        let mut dump = dump::Dump::open(&zip).map_err(local_dto)?;
        // [#258] Consults the session's existing answer, so a user who resolved the
        // ambiguity when they OPENED the project - before any lock existed - is not
        // asked again here, where asking costs a lock round trip.
        // Ariane data REFUSES here rather than routing to adoption: adoption imports
        // the project's tip, so on a revision swap it would answer a question about
        // `hexsha` with today's contents, and there is no import-at-hash path to do
        // otherwise. The resolver records the user's pick itself when it honours one;
        // re-recording it here would also record an entry nobody chose, turning an
        // unambiguous revision's sole `.stalx` into a standing session answer.
        //
        // `hexsha` dates the dump, and it is the revision being checked out - so a
        // session answer given at the TIP correctly does not apply here, and the
        // restore falls through to asking. That is not a regression on the sentence
        // above: the probe in front of this flow is what puts the question before the
        // lock, and it now asks about this same revision for the same reason. Where an
        // answer WAS given for this revision (the picker's replay, which arrives as
        // `entry_path`) it is honoured and recorded against it.
        let entry = self
            .speleodb_resolve_entry_remembering(
                &project_id,
                &mut dump,
                entry_path.as_deref(),
                ArianeRoute::Refuse,
                DumpAt::Revision(&hexsha),
                // Both hosts of this verb raise the picker on the candidates:
                // `SpeleoRestoreDialog` and the revert composite.
                StalxAmbiguity::Ask,
            )
            .map_err(local_dto)?;
        // [#576 RV4-R-S7] Before the replace: a newer build's file is refused, so
        // the open copy keeps its link, BASE, un-pushed mark and Unwind position.
        super::speleodb_pull::refuse_a_newer_download(&mut dump, &entry).map_err(local_dto)?;

        let dest = self.speleodb_cache_path(&project_id).map_err(local_dto)?;
        // A forward step loads the CHILD revision's stack. Its policy is therefore the
        // one that decides whether there is any published history to continue with -
        // the currently loaded parent may have the opposite setting. The affordance
        // checks this too, but the backend repeats it against the exact dump and entry
        // it is about to install so a stale or direct IPC call cannot bypass the rule.
        // [#576 A8] The replace below is a method of this check's proof.
        let child_policy = unwind.check_child_policy(chain_forward, &mut dump, &entry, &dest)?;
        // Carry the user's local view toggles across the download-replace, same as a
        // re-pull (display_settings are stripped from the server copy), and drop the
        // live handle to this file first - the reload below re-opens it anyway, so
        // hoisting the close ahead of the extract costs nothing and keeps the swap
        // off a mmap'd/locked destination.
        //
        // GUARDED BY DEFAULT: a revision swap replaces the whole working tree, so
        // with un-pushed edits present it destroys them - for View just as much as
        // for Restore, which makes the documented "non-destructive peek" untrue in
        // exactly the case where the user has most to lose.
        //
        // `discard_local` is the ONE waiver, and only a Restore may carry it, after
        // the frontend's second confirmation has named the edits being discarded
        // (`SpeleoRestoreDialog`'s armed discard step). It exists because a restore
        // STAMPS the working copy as un-pushed itself, so the guard otherwise made
        // Restore single-use: restoring a second revision - or reverting a restore
        // the user disliked - refused over edits the previous restore had just
        // created, reported as "local edits never pushed". A View can never waive
        // it: a read-only peek has no confirmation surface and must stay
        // non-destructive.
        let policy = if restore && discard_local {
            LocalEditsPolicy::Discard
        } else {
            LocalEditsPolicy::Protect
        };
        let preserved_view = child_policy
            .replace(
                self,
                &mut dump,
                &entry,
                &dest,
                &project_id,
                policy,
                // [#303] A View/Restore adopts a server tree like any other download, so
                // its undo history is not inherited implicitly. The Unwind is the ONE
                // caller that asks to keep it - that history is what it came for.
                if keep_undo_history {
                    InheritedUndo::Keep
                } else {
                    InheritedUndo::Drop
                },
            )
            .map_err(local_dto)?;

        // Restore rides ON TOP of the current server HEAD (BASE = tip, dirty SET).
        // Resolve HEAD from the revision topology; if that blips, fall back to the open
        // link's last-known BASE rather than `None` - a None BASE + dirty makes the next
        // fence read `Diverged` and forces a spurious divergence prompt after a simple
        // restore. View pins BASE to the viewed revision (see `revision_swap_base`).
        let server_tip = if restore {
            match client::get_revisions(&endpoint, &token, insecure_http, &project_id) {
                Ok(history) => sync::tip_of(&history.commits)
                    .map(str::to_owned)
                    .or_else(|| open_link.base_commit_sha.clone()),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        project_id = %project_id,
                        purpose = "resolve server HEAD on restore",
                        "speleodb_restore_revision: could not resolve server HEAD; falling back to last-known BASE"
                    );
                    open_link.base_commit_sha.clone()
                }
            }
        } else {
            None
        };
        let (base_sha, has_unpushed_edits) = revision_swap_base(restore, &hexsha, server_tip);
        // `last_edit_at = now` on a Restore that changed something, so the g7 push-start
        // comparison treats it as an edit. A View - and a Restore of the tip itself,
        // which changed nothing - stays clean and carries no edit timestamp.
        let last_edit_at = has_unpushed_edits.then(|| chrono::Utc::now().to_rfc3339());

        match crate::persistence::Database::open_existing(&dest) {
            Ok(db) => {
                // [#303] The checked-out revision's own undo policy, read before this
                // stamp overwrites the row. A revision swap answers "what did the project
                // look like then", and the policy is part of the project file, so the
                // swapped-in file speaks for itself rather than inheriting today's value.
                let stamped = stamp_pulled_file(&db, &preserved_view, |swapped_undo_mode| {
                    crate::models::SpeleoLink {
                        endpoint,
                        project_id: project_id.clone(),
                        project_name,
                        base_commit_sha: base_sha,
                        has_unpushed_edits,
                        last_edit_at,
                        last_verify_at: None,
                        undo_mode: Some(self.effective_undo_mode(swapped_undo_mode)),
                    }
                });
                if let Err(e) = stamped {
                    tracing::warn!(
                        error = %e,
                        project_id = %project_id,
                        purpose = "stamp SpeleoDB hand-off link on restore",
                        "speleodb_restore_revision: failed to stamp speleo_link; project may open local-only"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    purpose = "stamp SpeleoDB hand-off link on restore",
                    "speleodb_restore_revision: could not reopen extracted .stalx to stamp speleo_link"
                );
            }
        }

        let local_path = dest.to_string_lossy().into_owned();
        // Reopen the swapped tree in place. close_project drops the live handle to
        // `dest` so do_load_project_full's "already open" guard won't reject the same
        // path (and the reopened connection reads the overwritten file, not a stale
        // page cache). A fresh load of a remote project defaults to read-only (#191):
        // for Restore the user holds the lock, so re-enable editing (the restored tree
        // is a pushable unpushed edit); for View leave it read-only (a peek that Update
        // fast-forwards away). Mirrors the fast-forward reload (#193); the FE then
        // pulls the swapped geometry via refreshProjectState.
        //
        // `replace_cache_stalx` above already closed the project when it WAS this
        // file, and replaced it atomically (temp + rename), so the reopen below reads
        // a whole file rather than a possibly-torn one. This close stays because it
        // is not guaranteed to have fired (a stale/never-opened cache path) and it is
        // a cheap no-op when it did.
        //
        // Captured BEFORE the reload, which shuts the gate: a status poll landing in
        // that window clears the acquisition record for a lock we still genuinely
        // hold, and the `if restore` block below reopens the GATE without it - so the
        // record would be gone for good and our own landed push would later read as a
        // conflict. The third in-place reload site, alongside both fast-forward arms.
        //
        // Unlike `speleodb_update`'s arm this needs no gate/force PAIR: Restore's
        // entry guards above already refused a force-editor and already required an
        // open gate, so the record alone cannot mint - it is only ever handed back to
        // a session that demonstrably had one on the way in.
        //
        // [#576 A8] As types: the reload is a method of the capture, and the hand-back
        // and the chain position below are methods of what the reload returns.
        let captured = self.speleodb_capture_lock_record(&project_id);
        let reloaded = captured.reload(self, dest)?;
        reloaded.reopen_for_restore(self, restore, &project_id);
        self.set_force_editing(false);

        // [#303] Record where the Unwind chain now stands - AFTER the reload, because the
        // reload replaced the file this writes into (and its `speleo_undo_chain` table
        // came from the server copy, i.e. empty). The next Unwind reads this and walks to
        // this revision's first parent.
        //
        // A plain View / Restore CLEARS it instead: the working copy is no longer where
        // the chain thought it was, so the next Unwind must restart from BASE rather than
        // continue a walk that no longer describes anything.
        reloaded.record_chain_position(self, keep_undo_history, &hexsha);

        Ok(SpeleoOpenResult {
            local_path,
            concurrent_session: false,
        })
    }
}

/// [#576 A8] The revision swap's orderings as types. Each later step is a method of
/// the proof the earlier one returns, and every proof is made only here:
/// - the Unwind preconditions -> the download ([`UnwindCleared`]);
/// - the forward step's child-policy check -> the replace ([`ChildPolicyChecked`]);
/// - the acquisition-record capture -> the in-place reload ([`CapturedRecord`]);
/// - the reload -> the record's hand-back and the chain position ([`Reloaded`]).
mod restore_order {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use super::super::speleodb::local_dto;
    use super::super::speleodb_cache::{InheritedUndo, LocalEditsPolicy, ReplaceLicence};
    use super::{downloaded_revision_undo_mode, AppState};
    use crate::speleodb::{client, dump, SpeleoDbError, SpeleoErrorDto};

    /// Proof that the Unwind's backend preconditions ran - or that this swap is no
    /// Unwind and has none.
    pub(super) struct UnwindCleared {
        keep_undo_history: bool,
    }

    /// Proof that a forward Unwind step's child revision publishes history (or that
    /// this is no forward step).
    pub(super) struct ChildPolicyChecked {
        _checked: (),
    }

    /// [#576 A8 NF-A8-BYPASS] The swap's download takes the Unwind preconditions'
    /// proof and its cache replace the child-policy proof, so neither raw step can
    /// be written above the check it must follow.
    impl client::DumpLicence for UnwindCleared {}
    impl ReplaceLicence for ChildPolicyChecked {}

    /// The acquisition record as it stood BEFORE the reload: the last point the
    /// pre-reload truth exists.
    pub(super) struct CapturedRecord {
        had_lock_record: bool,
        had_resumed_record: bool,
    }

    /// Proof that the swapped tree was reloaded in place, carrying the capture.
    pub(super) struct Reloaded {
        captured: CapturedRecord,
    }

    impl AppState {
        /// [#303] The Unwind flavour's preconditions (`keep_undo_history`), enforced in
        /// the backend: the guard, then the check that the direction it branched on was
        /// the truth.
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn speleodb_unwind_preconditions(
            &self,
            keep_undo_history: bool,
            open_link: &crate::models::SpeleoLink,
            discard_unpushed: bool,
            chain_forward: bool,
            hexsha: &str,
        ) -> Result<UnwindCleared, SpeleoErrorDto> {
            if keep_undo_history {
                self.speleodb_guard_unwind(open_link, discard_unpushed, chain_forward)?;
                // ...and check the direction the guard just branched on was the truth.
                self.speleodb_verify_chain_target(hexsha, chain_forward)?;
            }
            Ok(UnwindCleared { keep_undo_history })
        }

        /// Capture the acquisition record (and whether it was a resumed one, #576
        /// RV4-R-S3) before the reload shuts the gate.
        pub(super) fn speleodb_capture_lock_record(&self, project_id: &str) -> CapturedRecord {
            CapturedRecord {
                had_lock_record: self.speleodb_lock_acquired_here(project_id),
                had_resumed_record: self.speleodb_lock_resumed_here(project_id),
            }
        }
    }

    impl UnwindCleared {
        /// Download the revision being checked out, by hash.
        pub(super) fn download(
            &self,
            endpoint: &str,
            token: &str,
            insecure_http: bool,
            project_id: &str,
            hexsha: &str,
        ) -> Result<Vec<u8>, SpeleoDbError> {
            client::download_dump_at_hash(self, endpoint, token, insecure_http, project_id, hexsha)
        }

        /// A forward step loads the CHILD revision's stack, so the child's own policy
        /// decides whether there is published history to continue with - checked
        /// against the exact dump and entry about to be installed.
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn check_child_policy(
            &self,
            chain_forward: bool,
            dump: &mut dump::Dump<'_>,
            entry: &str,
            dest: &Path,
        ) -> Result<ChildPolicyChecked, SpeleoErrorDto> {
            if self.keep_undo_history && chain_forward {
                let target_mode =
                    downloaded_revision_undo_mode(dump, entry, dest).map_err(local_dto)?;
                if !target_mode.pushes_history() {
                    return Err(local_dto(SpeleoDbError::refused(
                        "the next revision did not publish undo history, so there is no \
                         forward history step to load"
                            .to_owned(),
                    )));
                }
            }
            Ok(ChildPolicyChecked { _checked: () })
        }
    }

    impl ChildPolicyChecked {
        /// The download-replace of the working copy
        /// ([`AppState::replace_cache_stalx_with`]).
        pub(super) fn replace(
            &self,
            state: &AppState,
            dump: &mut dump::Dump<'_>,
            entry: &str,
            dest: &Path,
            project_id: &str,
            policy: LocalEditsPolicy,
            undo: InheritedUndo,
        ) -> Result<HashMap<String, String>, SpeleoDbError> {
            state.replace_cache_stalx_with(self, dump, entry, dest, project_id, policy, undo)
        }
    }

    impl CapturedRecord {
        /// Reopen the swapped tree in place (the close first, so the load's
        /// already-open guard does not reject the same path).
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn reload(
            self,
            state: &AppState,
            dest: PathBuf,
        ) -> Result<Reloaded, SpeleoErrorDto> {
            state.close_project();
            state.do_load_project_full(dest).map_err(|e| {
                local_dto(SpeleoDbError::Io(format!(
                    "reload after restore failed: {e}"
                )))
            })?;
            Ok(Reloaded { captured: self })
        }
    }

    impl Reloaded {
        /// The reload's follow-ups, for their own tests: production reaches a
        /// `Reloaded` only through [`CapturedRecord::reload`].
        #[cfg(test)]
        pub(super) fn for_tests(had_lock_record: bool, had_resumed_record: bool) -> Self {
            Self {
                captured: CapturedRecord {
                    had_lock_record,
                    had_resumed_record,
                },
            }
        }

        /// A Restore re-enables editing (the user holds the lock) and hands the
        /// captured record back - an acquisition only where one existed, so an
        /// inherited lock is never given a provenance it did not have.
        ///
        /// [#576 SDB-ACCT-D1] Every other edit session is claimed as an ADOPTED lock
        /// (a resumed claim), as the commit claims it: the release verbs hand it
        /// back, the authorship shortcut stays off.
        ///
        /// [#576 W88] Not over a copy linked to another server than the one signed in
        /// now, or with nobody signed in: the restore holds no lock-ops span, so a
        /// sign-in elsewhere or a 401 auto-drop can land inside it, and an open gate
        /// would then lock, push and release on the session's server under this copy's
        /// project id. The restored copy stays read-only, nothing claimed, and says why
        /// in the log. Checked under `project`, as the commit checks it.
        pub(super) fn reopen_for_restore(&self, state: &AppState, restore: bool, project_id: &str) {
            if restore {
                let _proj_guard = state.project.lock();
                if let Ok(link) = state.speleodb_open_link() {
                    let signed_in = state.speleodb.lock().endpoint.clone();
                    if let Err(e) = super::super::speleodb_session::refuse_a_copy_on_another_server(
                        &link.endpoint,
                        signed_in.as_deref(),
                    ) {
                        tracing::warn!(
                            error = %e,
                            project_id = %project_id,
                            purpose = "never reopen editing over a copy of another server",
                            "speleodb restore: the session changed during the restore; \
                             leaving the restored copy read-only"
                        );
                        return;
                    }
                }
                state.set_edit_enabled(true);
                if self.captured.had_lock_record && !self.captured.had_resumed_record {
                    state.speleodb_note_lock_acquired(Some(project_id));
                } else {
                    state.speleodb_note_resumed_lock(project_id);
                }
            }
        }

        /// [#303] Record where the Unwind chain now stands, AFTER the reload (it
        /// replaced the file this writes into), or clear it for a plain View / Restore.
        pub(super) fn record_chain_position(
            &self,
            state: &AppState,
            keep_undo_history: bool,
            hexsha: &str,
        ) {
            if keep_undo_history {
                state.speleodb_set_unwound_from(Some(hexsha));
                // ...and mark the stack that just came back as INHERITED. Everything in
                // it was authored in a commit the server already holds, so the next push
                // must not record those operations again on top of whatever the user does
                // from here. Stamped after the reload for the same reason as the pointer:
                // the reload is what put these entries in the stack being measured.
                state.speleodb_stamp_inherited_watermark();
            } else {
                state.speleodb_set_unwound_from(None);
                // A plain View / Restore dropped the downloaded history
                // (`InheritedUndo::Drop`), so nothing in the stack is inherited any more
                // and a stale watermark here would cut the user's own work out of the
                // next push.
                state.speleodb_set_inherited_through(None);
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
#[expect(
    clippy::print_stdout,
    reason = "the driven A8 tests print the values they observed, so a failure shows them"
)]
mod tests {
    use super::{
        downloaded_revision_undo_mode, restore_order, revision_swap_base, unwind_discards_unpushed,
        validate_hexsha, AppState,
    };
    use crate::models::{Project, SpeleoLink, UndoMode};
    use crate::persistence::Database;
    use crate::speleodb::{dump::Dump, SpeleoDbError};

    /// The project id every fixture here is stamped with; `validate_project_id` requires
    /// a UUID, so it cannot be an arbitrary string.
    const FIXTURE_PROJECT_ID: &str = "11111111-1111-1111-1111-111111111111";

    /// An `AppState` holding a LINKED project, which is as far as the revision-swap
    /// verb's entry guards can be driven offline: everything past them needs a session
    /// token, a server and the app cache dir.
    fn linked_state_with_undo_mode(undo_mode: Option<UndoMode>) -> (AppState, tempfile::TempDir) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let path = temp.path().join("linked.stalx");
        let mut db = Database::open(&path).expect("open fixture .stalx");
        db.save_project(&Project::new("Linked".to_owned(), None))
            .expect("save fixture project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "https://example.invalid".to_owned(),
            project_id: FIXTURE_PROJECT_ID.to_owned(),
            project_name: "Linked".to_owned(),
            base_commit_sha: Some("deadbeefcafe".to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode,
        }))
        .expect("stamp the fixture link");

        let state = AppState::new().expect("construct AppState");
        state.set_database_for_tests(db);
        (state, temp)
    }

    fn linked_state() -> (AppState, tempfile::TempDir) {
        linked_state_with_undo_mode(None)
    }

    fn revision_dump_with_undo_mode(mode: Option<UndoMode>) -> Vec<u8> {
        use std::io::Write as _;

        let temp = tempfile::TempDir::new().expect("revision temp dir");
        let path = temp.path().join("revision.stalx");
        {
            let mut db = Database::open(&path).expect("open revision fixture");
            db.save_project(&Project::new("Revision".to_owned(), None))
                .expect("save revision project");
            db.save_speleo_link(Some(&SpeleoLink {
                endpoint: "https://example.invalid".to_owned(),
                project_id: FIXTURE_PROJECT_ID.to_owned(),
                project_name: "Revision".to_owned(),
                base_commit_sha: Some("deadbeefcafe".to_owned()),
                has_unpushed_edits: false,
                last_edit_at: None,
                last_verify_at: None,
                undo_mode: mode,
            }))
            .expect("stamp revision policy");
        }

        let bytes = std::fs::read(path).expect("read revision fixture");
        let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
        writer
            .start_file(
                "misc/project.stalx",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .expect("start revision entry");
        writer.write_all(&bytes).expect("write revision entry");
        writer.finish().expect("finish revision dump").into_inner()
    }

    /// A forward step loads the child revision's stack, so the child policy wins even
    /// when the older working copy says the exact opposite.
    #[test]
    fn downloaded_child_policy_decides_forward_history_eligibility() {
        crate::test_helpers::with_isolated_config(|| {
            for (current, child) in [
                (UndoMode::Disabled, UndoMode::Full),
                (UndoMode::Full, UndoMode::Disabled),
            ] {
                let (state, temp) = linked_state_with_undo_mode(Some(current));
                assert_eq!(
                    state.undo_policy().mode,
                    current,
                    "sanity: the open older revision carries the opposite policy"
                );
                let zip = revision_dump_with_undo_mode(Some(child));
                let mut dump = Dump::open(&zip).expect("open revision dump");
                let mode = downloaded_revision_undo_mode(
                    &mut dump,
                    "misc/project.stalx",
                    &temp.path().join("project.stalx"),
                )
                .expect("read child policy");
                assert_eq!(
                    mode, child,
                    "forward eligibility must come from the downloaded child, not the open parent"
                );
                let leftovers = std::fs::read_dir(temp.path())
                    .expect("read temp directory")
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().contains(".undo-mode."))
                    .count();
                assert_eq!(leftovers, 0, "the policy probe must clean up its `.stalx`");
            }
        });
    }

    #[test]
    fn legacy_null_revision_does_not_inherit_the_current_global_default() {
        crate::test_helpers::with_isolated_config(|| {
            let (state, temp) = linked_state_with_undo_mode(Some(UndoMode::Full));
            state.config.lock().speleodb.undo_mode = UndoMode::Full;
            let zip = revision_dump_with_undo_mode(None);
            let mut dump = Dump::open(&zip).expect("open revision dump");

            let mode = downloaded_revision_undo_mode(
                &mut dump,
                "misc/project.stalx",
                &temp.path().join("project.stalx"),
            )
            .expect("read legacy policy");

            assert_eq!(mode, UndoMode::Disabled);
        });
    }

    /// Ask for a revision swap that keeps the downloaded copy's undo history.
    fn keep_history_swap(state: &AppState, restore: bool) -> Result<(), String> {
        state
            .speleodb_restore_revision(
                FIXTURE_PROJECT_ID.to_owned(),
                "Linked".to_owned(),
                "deadbeefcafe".to_owned(),
                restore,
                false,
                None,
                true,
                false,
                false,
            )
            .map(|_| ())
            .map_err(|e| e.message)
    }

    /// [#303] Keeping a downloaded revision's undo history is a lock-held RESTORE or
    /// nothing, and the backend is where that is decided.
    ///
    /// `InheritedUndo::Keep` hands a server tree's history to a stack the user can push
    /// from, and stamps the watermark that decides what the next upload cuts - which is
    /// exactly the "explicit, confirmed, lock-held Unwind" that `repair.rs` and the
    /// `InheritedUndo` docs are written against. The verb gated only `restore`, so a
    /// read-only View asking for `keep_undo_history` took the Keep branch anyway; the sole
    /// thing preventing it was `useRestoreRevision.ts` clamping the flag on its way out,
    /// and an invariant the whole push path relies on cannot live in one frontend caller.
    #[test]
    fn keeping_the_undo_history_is_refused_without_a_lock_held_restore() {
        crate::test_helpers::with_isolated_config(|| {
            let (state, _temp) = linked_state();

            // A View (`restore: false`) can never inherit history, lock or no lock. This is
            // the case the frontend clamp was the only guard for.
            state.set_edit_enabled(true);
            let err = keep_history_swap(&state, false)
                .expect_err("a View must not be able to inherit a revision's undo history");
            assert!(
                err.contains("undo history"),
                "the refusal must name what was refused, got {err:?}"
            );

            // ...and a Restore without the write lock is refused too, so no arm of the verb
            // reaches the Keep branch outside an edit session.
            state.set_edit_enabled(false);
            let err = keep_history_swap(&state, true)
                .expect_err("a lockless restore must not inherit a revision's undo history");
            assert!(
                err.contains("write lock"),
                "the refusal must name the missing lock, got {err:?}"
            );
        });
    }

    /// The production region of this file, split out of the test module so the
    /// source-level tripwire below cannot match its own assertions.
    fn prod_source() -> &'static str {
        crate::models::source_scan::prod_region(
            include_str!("speleodb_revisions.rs"),
            "pub fn speleodb_restore_revision(",
        )
    }

    /// The target query and the destructive swap both enforce the same child-policy
    /// rule. The first keeps a useless forward action out of the UI; the second stops a
    /// stale or direct IPC call before it replaces the working copy.
    ///
    /// [#576 A8] The swap's half - checked BEFORE the replace - is the type's job now
    /// (`restore_order::ChildPolicyChecked`). The affordance's half stays a sanctioned
    /// source scan (docs/testing-recipes.md, "Source scans"): which policy a query
    /// consults is a site rule over one function, and reaching it needs a server.
    #[test]
    fn forward_chain_steps_are_gated_by_the_downloaded_child_policy() {
        use crate::models::source_scan::fn_body;
        let target = fn_body(prod_source(), "pub fn speleodb_rewind_target(");
        assert!(
            target.contains("downloaded_revision_undo_mode("),
            "RULE (a forward step is offered on the CHILD's policy, #303): the affordance \
             must inspect the child revision's own undo policy"
        );
        assert!(
            !target.contains("self.undo_policy()"),
            "RULE (a forward step is offered on the CHILD's policy, #303): the currently \
             open parent policy cannot decide whether the child shipped history"
        );
    }

    /// [#303] The predicate behind both the warning and the refusal.
    ///
    /// Dirty ALONE is not enough to mean "un-pushed work at risk": an Unwind stamps the copy
    /// dirty itself, so from its second step onwards the dirt is its own doing and the chain
    /// pointer says so. Only dirt with no pointer is work the user would actually lose -
    /// which is the state that undoing everything leaves behind (the undone steps sit on the
    /// redo stack), and the state that used to make the offered entry fail when pressed.
    #[test]
    fn only_dirt_without_a_chain_pointer_counts_as_unpushed_work() {
        assert!(
            unwind_discards_unpushed(true, None),
            "dirty with nothing to explain it is real un-pushed work"
        );
        assert!(
            !unwind_discards_unpushed(true, Some("deadbeefcafe")),
            "dirty because a previous Unwind made it so discards nothing new"
        );
        assert!(
            !unwind_discards_unpushed(false, None),
            "a clean copy has nothing to lose"
        );
        assert!(
            !unwind_discards_unpushed(false, Some("deadbeefcafe")),
            "a clean copy mid-chain has nothing to lose either"
        );
    }

    /// [#303] Spec item 24: the Unwind's refusals, driven through the guard itself.
    ///
    /// [#576 A8] That they run BEFORE the download is the type's job
    /// (`restore_order::UnwindCleared`: the download is its method). What they decide is
    /// behaviour, so it is driven here instead of read from the guard's source.
    #[test]
    fn the_unwind_guard_refuses_unwalked_steps_and_unacknowledged_work() {
        let (state, _temp) = linked_state();
        let clean = state.speleodb_open_link().expect("linked fixture");
        let step = || crate::core::Command::CreateProject {
            name: "Step".to_owned(),
            location: None,
        };

        // A backward step: the UNDO stack is what it would discard.
        state.undo_stack.lock().push(step());
        assert!(
            state.speleodb_guard_unwind(&clean, false, false).is_err(),
            "a chain step over a non-empty undo stack would discard the un-walked steps"
        );
        let parked = state.undo_stack.lock().pop_undo().expect("the pushed step");
        state.undo_stack.lock().push_redo(parked);
        assert!(
            state.speleodb_guard_unwind(&clean, false, false).is_ok(),
            "a parked redo step is not what a backward step discards"
        );

        // A forward step: the REDO stack decides, not the undo stack.
        assert!(
            state.speleodb_guard_unwind(&clean, false, true).is_err(),
            "a FORWARD step over a half-redone copy would drop the operations still parked \
             on the redo stack"
        );
        let redone = state.undo_stack.lock().pop_redo().expect("the parked step");
        state.undo_stack.lock().push_undo(redone);
        assert!(
            state.speleodb_guard_unwind(&clean, false, true).is_ok(),
            "a forward step does not need the undo stack empty"
        );
        state.undo_stack.lock().clear();

        // Un-pushed work no Unwind put there: refused unless the caller acknowledged it.
        let dirty = SpeleoLink {
            has_unpushed_edits: true,
            last_edit_at: Some("2026-08-01T00:00:00+00:00".to_owned()),
            ..clean
        };
        assert!(
            state.speleodb_guard_unwind(&dirty, false, false).is_err(),
            "un-pushed work may only be discarded by a caller that acknowledged it"
        );
        assert!(
            state.speleodb_guard_unwind(&dirty, true, false).is_ok(),
            "...and the acknowledgement is what lets it through"
        );
    }

    /// After the in-place reload: a Restore reopens editing and hands the acquisition
    /// record back ONLY where one was captured, and the Unwind chain pointer is written
    /// (an Unwind) or cleared (a plain View / Restore).
    ///
    /// [#576 A8] That these run after the reload, and the capture before it, is the
    /// types' job (`restore_order::CapturedRecord` / `Reloaded`); what they do is driven
    /// here through the step itself, instead of read from the verb's source.
    #[test]
    fn after_the_reload_a_restore_hands_back_only_the_record_it_had() {
        use super::super::speleodb::authed_session_at;
        let fresh = || {
            let (state, temp) = linked_state();
            // A claim is stamped with the account that took it, signed in to the server
            // the copy is linked to (#576 W88).
            *state.speleodb.lock() = authed_session_at("https://example.invalid");
            // Where the reload leaves it: a fresh load of a remote project is read-only.
            state.set_edit_enabled(false);
            (state, temp)
        };

        let (state, _temp) = fresh();
        restore_order::Reloaded::for_tests(true, false).reopen_for_restore(
            &state,
            true,
            FIXTURE_PROJECT_ID,
        );
        assert!(state.edit_enabled(), "a Restore reopens editing");
        assert!(
            state.speleodb_lock_acquired_here(FIXTURE_PROJECT_ID),
            "and hands back the record it captured, or our own next push reads as a conflict"
        );

        let (state, _temp) = fresh();
        restore_order::Reloaded::for_tests(false, false).reopen_for_restore(
            &state,
            true,
            FIXTURE_PROJECT_ID,
        );
        assert!(
            !state.speleodb_lock_taken_here(FIXTURE_PROJECT_ID),
            "no captured record, no acquisition handed back - re-recording one would MINT \
             provenance for an inherited lock"
        );
        // [#576 SDB-ACCT-D1] The edit session under the account's lock is claimed as
        // ADOPTED (a resumed claim), so the release verbs hand that lock back.
        assert!(
            state.speleodb_lock_resumed_here(FIXTURE_PROJECT_ID),
            "an inherited lock reopened for a Restore is adopted, for the release only"
        );

        let (state, _temp) = fresh();
        restore_order::Reloaded::for_tests(true, false).reopen_for_restore(
            &state,
            false,
            FIXTURE_PROJECT_ID,
        );
        assert!(!state.edit_enabled(), "a View stays read-only");

        let (state, _temp) = fresh();
        restore_order::Reloaded::for_tests(false, false)
            .record_chain_position(&state, true, "abc1234");
        assert_eq!(
            state.speleodb_unwound_from().as_deref(),
            Some("abc1234"),
            "an Unwind records where the chain now stands"
        );
        restore_order::Reloaded::for_tests(false, false)
            .record_chain_position(&state, false, "abc1234");
        assert_eq!(
            state.speleodb_unwound_from(),
            None,
            "a plain View / Restore CLEARS the pointer: the copy is no longer where the \
             chain thought it was"
        );
    }

    /// [#576 W88] A Restore holds no lock-ops span, so a sign-in to another server (or
    /// a 401 auto-drop) can land inside it: the reload then leaves a copy linked to the
    /// server the restore started on while the session is somewhere else, or nowhere.
    /// Its reopen opened the gate anyway, so the next push, Done or release sent the
    /// session's token to the session's server naming this copy's project. It stays
    /// read-only now, with nothing claimed; the same server spelled another way still
    /// reopens.
    #[test]
    fn a_restore_never_reopens_the_gate_over_a_copy_linked_to_another_server() {
        use super::super::speleodb::authed_session_at;
        // `linked_state` links its copy to `https://example.invalid`.
        let reopened = |signed_in: Option<&str>| {
            let (state, _temp) = linked_state();
            if let Some(endpoint) = signed_in {
                *state.speleodb.lock() = authed_session_at(endpoint);
            }
            // Where the reload leaves it: a fresh load of a remote project is read-only.
            state.set_edit_enabled(false);
            restore_order::Reloaded::for_tests(true, false).reopen_for_restore(
                &state,
                true,
                FIXTURE_PROJECT_ID,
            );
            (
                state.edit_enabled(),
                state.speleodb_lock_acquired_here(FIXTURE_PROJECT_ID),
            )
        };
        let elsewhere = reopened(Some("http://y.invalid/"));
        let signed_out = reopened(None);
        let same_server = reopened(Some("HTTPS://Example.invalid:443/"));
        println!(
            "[W88 item 1, Restore reopen] linked to https://example.invalid: signed in to \
             http://y.invalid/ -> (gate, claim) {elsewhere:?}; signed out -> {signed_out:?}; \
             signed in to HTTPS://Example.invalid:443/ -> {same_server:?}"
        );
        assert_eq!(
            elsewhere,
            (false, false),
            "signed in to another server: the restored copy stays read-only, unclaimed"
        );
        assert_eq!(signed_out, (false, false), "signed out: the same");
        assert_eq!(
            same_server,
            (true, true),
            "the same server spelled another way reopens and hands the record back"
        );
    }

    #[test]
    fn validate_hexsha_accepts_git_ids_rejects_traversal() {
        // #218 restore: a full 40-hex commit id and a shorter prefix both pass.
        assert!(validate_hexsha("0123456789abcdef0123456789abcdef01234567").is_ok());
        assert!(validate_hexsha("abc1234").is_ok());
        assert!(validate_hexsha("ABCDEF0").is_ok()); // upper-case hex accepted
                                                     // Anything that could reshape the download-at-hash URL is rejected before
                                                     // it is interpolated into the path (route traversal / injection).
        for bad in [
            "",
            "abc123",                                    // 6 chars: below the 7-min
            "0123456789abcdef0123456789abcdef012345678", // 41 chars: over the 40-max
            "abc123g",                                   // non-hex digit
            "../../etc",
            "abc/1234",
            "abc 1234",
        ] {
            assert!(
                matches!(validate_hexsha(bad), Err(SpeleoDbError::InvalidRevision)),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn revision_swap_base_stamps_restore_on_tip_view_on_hexsha() {
        // #218 Restore: BASE = the resolved server tip and the copy is dirty (rides as
        // an unpushed edit on top of HEAD).
        let (base, dirty) = revision_swap_base(true, "abc1234", Some("deadbeefcafe".to_owned()));
        assert_eq!(base.as_deref(), Some("deadbeefcafe"));
        assert!(dirty, "restore must set the dirty bit");

        // Restore with no resolvable tip (the caller's fallback already yielded None):
        // BASE stays None and the copy is still dirty. It is NOT stamped with the viewed
        // hexsha - a restore never pins BASE to the old revision.
        let (base, dirty) = revision_swap_base(true, "abc1234", None);
        assert_eq!(base, None);
        assert!(dirty);

        // View: BASE = the viewed hexsha itself and the copy is clean, so the working
        // tree IS that revision (a later Update fast-forwards back). The server tip is
        // irrelevant to View and must be ignored.
        let (base, dirty) = revision_swap_base(false, "abc1234", Some("deadbeefcafe".to_owned()));
        assert_eq!(base.as_deref(), Some("abc1234"));
        assert!(!dirty, "view must leave the copy clean");
    }

    #[test]
    fn restoring_the_tip_leaves_the_copy_clean_so_done_pushes_nothing() {
        // [#303] The Unwind's FIRST step restores the commit the copy already equals,
        // to load its undo history back. Stamping that dirty made Done upload a commit
        // with no change in it, and a deployment that closes the connection on one
        // (rather than answering 304) leaves the project stuck: the push is ambiguous,
        // so the lock is kept and the user is asked to retry, and the retry re-sends
        // the same empty commit. `speleodb_done`'s dirty-bit gate is what makes the
        // clean answer skip the upload entirely.
        let (base, dirty) =
            revision_swap_base(true, "deadbeefcafe", Some("deadbeefcafe".to_owned()));
        assert_eq!(
            base.as_deref(),
            Some("deadbeefcafe"),
            "BASE is still the tip - only the dirty bit differs"
        );
        assert!(
            !dirty,
            "restoring the revision the copy was downloaded at has nothing to push"
        );

        // The comparison is against the TIP, not just any match: restoring an ancestor
        // really does change the tree, so it must stay dirty even though `hexsha` is a
        // commit the server holds.
        let (_, dirty) = revision_swap_base(true, "abc1234", Some("deadbeefcafe".to_owned()));
        assert!(dirty, "an ancestor restore is a real change to push");

        // And the degenerate case keeps the safer answer: with no resolvable tip we
        // cannot show the copy matches it, so the work stays flagged.
        let (_, dirty) = revision_swap_base(true, "abc1234", None);
        assert!(dirty, "unknown tip must not silently clear the dirty bit");
    }

    /// [#576 A11] The chain walk is one of the named refusal sites: a crash that
    /// poisoned the undo stack leaves a history nobody can vouch for, so the walk is
    /// refused - with a reason, where it used to fail closed as "undo the remaining
    /// steps", which a user looking at an empty stack could not act on.
    #[test]
    fn the_chain_walk_refuses_after_a_crash_poisoned_the_undo_stack() {
        let (state, _temp) = linked_state();
        let link = state.speleodb_open_link().expect("linked fixture");
        assert!(
            state.speleodb_guard_unwind(&link, false, false).is_ok(),
            "precondition: a healthy, empty stack may be walked"
        );

        let crashed = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _held = state.undo_stack.lock();
                    std::panic::resume_unwind(Box::new("synthetic crash (#576 A11 test)"));
                })
                .join()
        });
        assert!(crashed.is_err() && state.undo_stack.is_poisoned());

        for chain_forward in [false, true] {
            let refused = state
                .speleodb_guard_unwind(&link, false, chain_forward)
                .expect_err("a poisoned history is not walked");
            assert!(
                refused.message.contains("internal error") && refused.message.contains("restart"),
                "the refusal says why: {}",
                refused.message
            );
        }
    }

    // ---- [#576 A8] the Unwind chain markers, driven ---------------------------------

    /// The chain pointer and the inherited-history watermark as a linked copy carries
    /// them mid-walk.
    const CHAIN_SHA: &str = "deadbeefcafe";
    const WATERMARK: &str = "2026-07-31T00:00:00+00:00";

    /// A LINKED project OPEN in an `AppState` from its own file, mid Unwind walk (both
    /// markers set) and editable - the state a forward edit or an undo meets.
    fn open_mid_walk(dir: &std::path::Path) -> (AppState, std::path::PathBuf) {
        let path = dir.join("walked.stalx");
        let mut db = Database::open(&path).expect("open fixture .stalx");
        db.save_project(&Project::new("CaveA".to_owned(), None))
            .expect("save fixture project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "https://example.invalid".to_owned(),
            project_id: FIXTURE_PROJECT_ID.to_owned(),
            project_name: "CaveA".to_owned(),
            base_commit_sha: Some(CHAIN_SHA.to_owned()),
            has_unpushed_edits: true,
            last_edit_at: Some(WATERMARK.to_owned()),
            last_verify_at: None,
            undo_mode: Some(UndoMode::Full),
        }))
        .expect("stamp the fixture link");
        db.save_speleo_undo_chain(Some(CHAIN_SHA))
            .expect("chain pointer");
        db.save_speleo_undo_watermark(Some(WATERMARK))
            .expect("watermark");
        drop(db);
        let state = AppState::new().expect("state");
        assert!(
            state.open_project(path.clone()).success,
            "open the walked copy"
        );
        state.set_edit_enabled(true);
        (state, path)
    }

    fn watermark(state: &AppState) -> Option<String> {
        state
            .database
            .lock()
            .as_ref()
            .and_then(|db| db.load_speleo_undo_watermark().ok().flatten())
    }

    /// One forward edit: a section, through the shared forward epilogue.
    fn forward_edit(state: &AppState) {
        let created = state.execute(crate::core::Command::CreateSection {
            name: Some("SectionA".to_owned()),
            surveyor: None,
            date: None,
            color: "#336699".to_owned(),
            direction: None,
            shots: vec![crate::models::ShotData {
                length: 4.0,
                ..Default::default()
            }],
        });
        assert!(created.success, "{:?}", created.error);
    }

    /// [#303] The watermark outlives an edit; the chain pointer does not. A forward
    /// command drops the pointer (the walk no longer describes the copy) but must LEAVE
    /// the watermark: it is what still knows, at push time, which rows the user never
    /// authored. Clear it there and the loaded history ships again.
    ///
    /// [#576 A8] Was a source scan of `app_state.rs`; a real forward edit runs here.
    #[test]
    fn a_forward_edit_drops_the_chain_pointer_and_keeps_the_watermark() {
        let _cfg = crate::test_helpers::isolated_config();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let (state, _) = open_mid_walk(temp.path());
        forward_edit(&state);
        println!(
            "after a forward edit -> chain pointer {:?}, watermark {:?}",
            state.speleodb_unwound_from(),
            watermark(&state)
        );
        assert_eq!(
            state.speleodb_unwound_from(),
            None,
            "the walk position goes"
        );
        assert_eq!(
            watermark(&state).as_deref(),
            Some(WATERMARK),
            "the inherited-history watermark stays"
        );
    }

    /// [#303] A forward edit drops the Unwind chain position; undo / redo must not.
    /// Walking back through loaded history risks nothing the server has not already got,
    /// so the pointer stays and the confirmation stays quiet; clearing it from the undo
    /// path would warn on every step of an ordinary walk.
    ///
    /// [#576 A8] Was the undo half of a source scan (`undo.rs` must not name the clear);
    /// an undo runs here. The site half stays a sanctioned inventory below.
    #[test]
    fn an_undo_keeps_the_unwind_position_a_forward_edit_drops() {
        let _cfg = crate::test_helpers::isolated_config();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let (state, _) = open_mid_walk(temp.path());
        forward_edit(&state);
        let after_edit = state.speleodb_unwound_from();
        state.speleodb_set_unwound_from(Some(CHAIN_SHA));
        let undone = state.undo();
        println!(
            "edit -> pointer {after_edit:?}; then undo ({}) -> pointer {:?}",
            undone.success,
            state.speleodb_unwound_from()
        );
        assert_eq!(after_edit, None, "precondition: the edit dropped it");
        assert!(undone.success, "{:?}", undone.error);
        assert_eq!(
            state.speleodb_unwound_from().as_deref(),
            Some(CHAIN_SHA),
            "an undo walks loaded history and keeps the position"
        );
    }

    /// [#303] The fourth writer of the project file: `speleodb_set_undo_mode` writes
    /// `speleo_link.undo_mode`, which rides to the server in the next push, so it is an
    /// edit like any other - the mode, the dirty stamp and the chain clear in one
    /// transaction.
    ///
    /// [#576 A8] Was a source scan for the transactional helper's name; the setter runs
    /// here.
    #[test]
    fn the_undo_mode_setter_dirties_the_copy_and_forgets_the_unwind_position() {
        let _cfg = crate::test_helpers::isolated_config();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let (state, _) = open_mid_walk(temp.path());
        state
            .speleodb_mutate_link(|link| {
                link.has_unpushed_edits = false;
                link.last_edit_at = None;
            })
            .expect("a clean copy mid-walk");
        let set = state.speleodb_set_undo_mode(UndoMode::Disabled);
        let link = state.speleodb_open_link().expect("link");
        println!(
            "set undo mode -> {:?}; mode {:?}, dirty {}, pointer {:?}",
            set.as_ref().err().map(|e| &e.message),
            link.undo_mode,
            link.has_unpushed_edits,
            state.speleodb_unwound_from()
        );
        assert!(set.is_ok());
        assert_eq!(link.undo_mode, Some(UndoMode::Disabled));
        assert!(
            link.has_unpushed_edits,
            "the setting is an edit of the project"
        );
        assert_eq!(state.speleodb_unwound_from(), None, "and it ends the walk");
    }

    /// [#303] A save writes a brand-new `.stalx` and renames it over the destination, so
    /// the side tables it does not re-apply are destroyed - the chain pointer among them,
    /// which is what EXCUSES the dirt an Unwind leaves (`save_sidecar_tests` drives the
    /// carry). The pipeline's order is what keeps a refusal free: the tables are
    /// snapshotted from the open file BEFORE any temp file exists, so a snapshot that
    /// refuses has written nothing.
    ///
    /// [#576 A8] Was a source scan of `project_io.rs`'s call order. The write-into-the-
    /// temp-before-it-closes half is the borrow checker's (`write_sidecars` borrows the
    /// staged database the close then moves); the snapshot-first half is driven here.
    #[test]
    fn a_refused_save_stages_nothing_beside_the_file() {
        let _cfg = crate::test_helpers::isolated_config();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let (state, path) = open_mid_walk(temp.path());
        rusqlite::Connection::open(&path)
            .expect("open the fixture's file")
            .execute_batch(
                "ALTER TABLE speleo_undo_watermark RENAME COLUMN inherited_through TO broken_value;",
            )
            .expect("make the markers unreadable");
        let saved = state.save_project(None);
        let staged = path.with_extension("stalx.tmp");
        println!(
            "unreadable markers -> saved {}, error {:?}; staging file left: {}",
            saved.success,
            saved.error,
            staged.exists()
        );
        assert!(!saved.success, "precondition: the snapshot refuses");
        assert!(
            !staged.exists(),
            "a refusal at the snapshot must leave no staged file behind"
        );
    }

    /// The site half of the forward-edit rule, a sanctioned inventory (#576 A8-D2,
    /// docs/testing-recipes.md "Source scans"): forward writers flip the dirty bit
    /// through `mark_remote_forward_edit` (which also ends an Unwind walk), never the
    /// raw marker, and every direct forward writer is enumerated so a new path is
    /// reviewed - a statement about which files may call what, which no type expresses.
    #[test]
    fn every_direct_forward_writer_is_enumerated() {
        let state_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/state");
        let mut forward_calls = Vec::new();
        for entry in std::fs::read_dir(state_dir).expect("read state source directory") {
            let path = entry.expect("state source entry").path();
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .expect("file name")
                .to_owned();
            if path.extension().and_then(|v| v.to_str()) != Some("rs") || name.ends_with("tests.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read state source");
            let production = source
                .get(
                    ..crate::models::source_scan::test_module_start(&source)
                        .unwrap_or(source.len()),
                )
                .unwrap_or(&source);
            if !matches!(
                name.as_str(),
                "app_state.rs" | "undo.rs" | "speleodb.rs" | "speleodb_revisions.rs"
            ) {
                assert!(
                    !production.contains("mark_remote_dirty();"),
                    "RULE (a forward edit also ends an Unwind walk, #303): {name} calls the \
                     raw dirty marker; forward writers use mark_remote_forward_edit"
                );
            }
            if name != "speleodb_revisions.rs" && production.contains("mark_remote_forward_edit();")
            {
                forward_calls.push(name);
            }
        }
        forward_calls.sort();
        // [#576 A4] Every forward mutation entry point flips the bit through the shared
        // forward epilogue in `command_epilogue.rs`; `forward_epilogue_tests.rs` checks
        // each one runs it.
        assert_eq!(
            forward_calls,
            ["command_epilogue.rs", "recent_projects.rs", "warnings.rs"],
            "RULE (every direct forward writer is reviewed, #303): the set moved"
        );
    }
}
