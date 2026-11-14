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

use super::speleodb::{local_dto, validate_project_id};
use super::speleodb_cache::{InheritedUndo, LocalEditsPolicy};
use super::AppState;
use crate::models::UndoMode;
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::models::SpeleoRevision;
use crate::speleodb::{client, sync};
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto, SpeleoOpenResult};

/// Read one downloaded revision's effective undo mode without installing it over the
/// working copy. Revision metadata does not carry this policy, so a forward chain step
/// has to inspect the child `.stalx` it would load.
fn downloaded_revision_undo_mode(
    source: &std::path::Path,
    scratch_base: &std::path::Path,
) -> Result<UndoMode, SpeleoDbError> {
    downloaded_revision_undo_mode_with_limits(
        source,
        scratch_base,
        super::speleodb_pull::DOWNLOADED_VALUE_LIMIT,
        super::speleodb_pull::DOWNLOADED_PROCESSING_DEADLINE,
    )
}

fn downloaded_revision_undo_mode_with_limits(
    source: &std::path::Path,
    scratch_base: &std::path::Path,
    value_limit: i32,
    deadline: std::time::Duration,
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
        std::fs::copy(source, &probe).map_err(|e| SpeleoDbError::Io(e.to_string()))?;
        let db =
            crate::persistence::Database::open_existing_untrusted(&probe, value_limit, deadline)
                .map_err(super::speleodb_cache_install::downloaded_file_error)?;
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
/// `speleodb_guard_unwind` decides with this predicate. It used to have a second reader, the (since
/// removed) Unwind confirmation's warning, and keeping both on one function is what stopped
/// the warning and the refusal from drifting apart - which is how this surfaced: the entry
/// offered an action that only ever failed.
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
    ///    produces, so the button existed only to fail. The waiver has to be acknowledged:
    ///    `discard_unpushed` is the caller reporting that it warned the user. Absent the
    ///    acknowledgement this still refuses, so no caller can discard un-pushed work by
    ///    passing `keep_undo_history` alone.
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
    /// SHA will not match. Costs one `GET /revisions/`, spent only on the
    /// `keep_undo_history` path - the one that gets to keep a downloaded copy's history,
    /// and so the one that has to earn it.
    ///
    /// Case-insensitive, like every other SHA comparison in this feature.
    fn speleodb_verify_chain_target(
        &self,
        hexsha: &str,
        chain_forward: bool,
    ) -> Result<(), SpeleoErrorDto> {
        let expected = if chain_forward {
            self.speleodb_rewind_candidate()?
        } else {
            self.speleodb_unwind_target()?
        };
        if expected
            .as_deref()
            .is_some_and(|sha| sha.eq_ignore_ascii_case(hexsha))
        {
            return Ok(());
        }
        Err(local_dto(SpeleoDbError::Io(
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
        // A poisoned lock reads as "not empty", i.e. REFUSE: the guard exists to stop a
        // chain step over un-walked work, so it has to fail closed on not knowing.
        let stack_empty = self.undo_stack.lock().is_ok_and(|s| {
            if chain_forward {
                s.redo_len() == 0
            } else {
                s.is_empty()
            }
        });
        if !stack_empty {
            return Err(local_dto(SpeleoDbError::Io(
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
            return Err(local_dto(SpeleoDbError::Io(
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
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(crate::persistence::Database::load_speleo_undo_chain)
            })?
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
            let Ok(guard) = self.database.lock() else {
                return;
            };
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
        if let Ok(mut stack) = self.undo_stack.lock() {
            stack.set_inherited_through(parsed);
        }
    }

    /// [#303] Stamp the watermark for a stack that has just been loaded by a chain step.
    ///
    /// `now` would almost always do, but not quite: the entries came off another machine,
    /// and a pusher with a skewed clock can stamp them in the FUTURE. Those would then sit
    /// above the watermark and ride into the commit anyway, which is the whole bug. Taking
    /// the later of `now` and the newest loaded entry makes "at or before" true of every
    /// row that was just loaded, whatever clock wrote it.
    fn speleodb_stamp_inherited_watermark(&self) {
        let newest = self.undo_stack.lock().ok().and_then(|stack| {
            stack
                .undo_entries()
                .filter_map(|entry| entry.timestamp)
                .max()
        });
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
        let Ok(guard) = self.database.lock() else {
            return;
        };
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

    /// [#303] Which revision the next Unwind would load: the commit the working copy already
    /// equals on the first step after a push, an ancestor after that.
    ///
    /// `None` when there is nothing to reach: no link, an empty repo, the root of the
    /// history, or a chain pointer naming a commit outside the fetched range.
    ///
    /// Needs the revision topology, so it costs one `GET /revisions/`. Called only by
    /// `speleodb_verify_chain_target`, for a history-keeping restore going back.
    pub fn speleodb_unwind_target(&self) -> Result<Option<String>, SpeleoErrorDto> {
        let link = self.speleodb_validated_link()?;
        // Disabled mode never SHIPPED history, so there is nothing in those commits to
        // come back for. Older commits pushed while the project was Full still have it,
        // but offering the step here would mostly find empty stacks.
        if !self.undo_policy().pushes_history() {
            return Ok(None);
        }
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        let (_, commits) = self
            .speleodb_server_head(&endpoint, &token, insecure_http, &link.project_id)
            .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;

        Ok(sync::unwind_target(
            self.speleodb_unwound_from().as_deref(),
            link.base_commit_sha.as_deref(),
            &commits,
        )
        .map(|(hexsha, _is_current)| hexsha))
    }

    /// [#303] Which revision a FORWARD chain step would check out, walking back up toward
    /// BASE: topology only, from one `GET /revisions/`. The caller re-checks the child's
    /// own undo policy after downloading it (`speleodb_restore_revision`).
    fn speleodb_rewind_candidate(&self) -> Result<Option<String>, SpeleoErrorDto> {
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
        .map(|(hexsha, _is_latest)| hexsha))
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
    /// (reading history is permission-free).
    pub fn speleodb_get_revisions(
        &self,
        project_id: String,
    ) -> Result<Vec<SpeleoRevision>, SpeleoErrorDto> {
        validate_project_id(&project_id).map_err(local_dto)?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
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

    pub fn speleodb_restore_revision(
        &self,
        project_id: String,
        project_name: String,
        hexsha: String,
        restore: bool,
        discard_local: bool,
        keep_undo_history: bool,
        discard_unpushed: bool,
        chain_forward: bool,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        validate_project_id(&project_id).map_err(local_dto)?;
        validate_hexsha(&hexsha).map_err(local_dto)?;

        // Bound to the project open NOW, re-checked inside the commit that installs
        // the revision: the download below can outlive a switch to another project,
        // and the swap must then leave that project alone (`speleodb_revision_swap`).
        let binding = self.speleodb_swap_binding().map_err(local_dto)?;
        let open_link = &binding.link;
        if open_link.project_id != project_id {
            return Err(local_dto(SpeleoDbError::NoRemoteProject));
        }
        if restore && !self.edit_enabled() {
            return Err(local_dto(SpeleoDbError::Io(
                "acquire the write lock before restoring a revision".to_owned(),
            )));
        }
        if keep_undo_history && !(restore && self.edit_enabled()) {
            return Err(local_dto(SpeleoDbError::Io(
                "loading a revision's undo history needs a restore with the write lock held"
                    .to_owned(),
            )));
        }
        if keep_undo_history {
            self.speleodb_guard_unwind(open_link, discard_unpushed, chain_forward)?;
            self.speleodb_verify_chain_target(&hexsha, chain_forward)?;
        }

        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        let native = self
            .speleodb_handle_auth(
                Endpoint::DownloadStalx,
                client::download_stalx_at_hash(
                    &endpoint,
                    &token,
                    insecure_http,
                    &project_id,
                    &hexsha,
                ),
            )
            .map_err(|e| dto_for_error(Endpoint::DownloadStalx, &e))?;

        let dest = self.speleodb_cache_path(&project_id).map_err(local_dto)?;
        if keep_undo_history && chain_forward {
            let target_mode =
                downloaded_revision_undo_mode(native.path(), &dest).map_err(local_dto)?;
            if !target_mode.pushes_history() {
                return Err(local_dto(SpeleoDbError::Io(
                    "the next revision did not publish undo history, so there is no forward \
                     history step to load"
                        .to_owned(),
                )));
            }
        }
        let policy = if restore && discard_local {
            LocalEditsPolicy::Discard
        } else {
            LocalEditsPolicy::Protect
        };

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
        let last_edit_at = has_unpushed_edits.then(|| chrono::Utc::now().to_rfc3339());

        let link = crate::models::SpeleoLink {
            endpoint,
            project_id: project_id.clone(),
            project_name,
            base_commit_sha: base_sha,
            has_unpushed_edits,
            last_edit_at,
            last_verify_at: None,
            undo_mode: None,
        };

        let local_path = dest.to_string_lossy().into_owned();
        // Replace the cache file and reopen it in ONE commit, and only while the
        // bound project is still the open one. A Restore keeps its edit session
        // through that same commit (`restore` asks to carry it): the claim bound to
        // the generation the swap was bound to moves to the new one, provenance and
        // all, and nothing else can (`speleodb_edit_gate`). A View carries nothing,
        // so it lands read-only whatever was open before.
        self.speleodb_swap_in_revision(
            native.path(),
            &dest,
            link,
            (
                policy,
                if keep_undo_history {
                    InheritedUndo::Keep
                } else {
                    InheritedUndo::Drop
                },
            ),
            binding,
            restore,
        )?;
        self.while_swapped_in_project_is_open(&dest, &project_id, || {
            if keep_undo_history {
                self.speleodb_set_unwound_from(Some(&hexsha));
                self.speleodb_stamp_inherited_watermark();
            } else {
                self.speleodb_set_unwound_from(None);
                self.speleodb_set_inherited_through(None);
            }
        });

        Ok(SpeleoOpenResult {
            local_path,
            offline: None,
            has_unpushed_edits: None,
            read_only: None,
            other_owner: None,
        })
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{
        downloaded_revision_undo_mode, downloaded_revision_undo_mode_with_limits,
        revision_swap_base, unwind_discards_unpushed, validate_hexsha, AppState,
    };
    use crate::models::{Project, SpeleoLink, UndoMode};
    use crate::persistence::Database;
    use crate::speleodb::SpeleoDbError;

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
        let link = SpeleoLink {
            endpoint: "https://example.invalid".to_owned(),
            project_id: FIXTURE_PROJECT_ID.to_owned(),
            project_name: "Linked".to_owned(),
            base_commit_sha: Some("deadbeefcafe".to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode,
        };
        db.save_speleo_link(Some(&link))
            .expect("stamp the fixture link");

        let state = AppState::new().expect("construct AppState");
        // Installed without an open; the fixture setter derives the edit gate from
        // the link the way an open would (read-only until a claim is bound).
        state.set_database_for_tests(db);
        (state, temp)
    }

    fn linked_state() -> (AppState, tempfile::TempDir) {
        linked_state_with_undo_mode(None)
    }

    fn revision_stalx_with_undo_mode(mode: Option<UndoMode>) -> tempfile::NamedTempFile {
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

        let native = tempfile::NamedTempFile::new().expect("native file");
        std::fs::copy(&path, native.path()).expect("copy native file");
        native
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
                let native = revision_stalx_with_undo_mode(Some(child));
                let mode = downloaded_revision_undo_mode(
                    native.path(),
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
    fn downloaded_policy_probe_rejects_oversized_values_and_expired_deadlines() {
        let native = revision_stalx_with_undo_mode(Some(UndoMode::Full));
        let temp = tempfile::tempdir().expect("temp");
        let scratch = temp.path().join("project.stalx");
        rusqlite::Connection::open(native.path())
            .expect("database")
            .execute(
                "UPDATE speleo_link SET project_name = ?1",
                ["X".repeat(8192)],
            )
            .expect("oversized synthetic name");
        assert!(downloaded_revision_undo_mode_with_limits(
            native.path(),
            &scratch,
            1024,
            std::time::Duration::from_secs(30)
        )
        .is_err());
        assert!(downloaded_revision_undo_mode_with_limits(
            native.path(),
            &scratch,
            64 * 1024 * 1024,
            std::time::Duration::ZERO
        )
        .is_err());
        assert_eq!(
            std::fs::read_dir(temp.path()).expect("directory").count(),
            0,
            "both failed probes clean up their private copies"
        );
    }

    #[test]
    fn legacy_null_revision_does_not_inherit_the_current_global_default() {
        crate::test_helpers::with_isolated_config(|| {
            let (state, temp) = linked_state_with_undo_mode(Some(UndoMode::Full));
            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Full;
            let native = revision_stalx_with_undo_mode(None);

            let mode =
                downloaded_revision_undo_mode(native.path(), &temp.path().join("project.stalx"))
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
        include_str!("speleodb_revisions.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production region precedes the test module")
    }

    /// [#303] Spec item 24: the Unwind's preconditions are enforced in the BACKEND, not
    /// taken on trust from the frontend - it is the one action allowed to keep a
    /// downloaded copy's undo history, and (from its second step) to waive the
    /// un-pushed-edits guard.
    ///
    /// Structural, like its siblings in this module: the guard runs inside a verb whose
    /// happy path needs a live cache dir and a server, so the unit-level seam is the
    /// source. What it pins is that the three refusals exist and are reached BEFORE the
    /// download - a check that ran after would have already replaced the working copy.
    #[test]
    fn unwind_preconditions_are_checked_before_the_download() {
        let src = prod_source();
        assert!(
            src.contains("fn speleodb_guard_unwind"),
            "the Unwind guard must exist as its own named function"
        );
        // Reached from the swap verb, and before the download it protects.
        let restore = src
            .split_once("pub fn speleodb_restore_revision")
            .map(|(_, rest)| rest)
            .expect("the swap verb is in this file");
        let guard = restore
            .find("self.speleodb_guard_unwind(")
            .expect("the swap verb must call the Unwind guard");
        let download = restore
            .find("client::download_stalx_at_hash(")
            .expect("the swap verb downloads the revision");
        assert!(
            guard < download,
            "the guard must run BEFORE the download - a check after it would refuse a \
             working copy that had already been replaced"
        );

        // And the two refusals themselves: a non-empty stack, and unacknowledged dirt that
        // no Unwind put there.
        let body = src
            .split_once("fn speleodb_guard_unwind")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n    /// ").next())
            .expect("guard body precedes the next item's doc");
        assert!(
            body.contains("if !stack_empty"),
            "a chain step over a non-empty stack would discard the un-walked steps"
        );
        // ...and which stack that is follows the direction of travel. Checking the undo
        // stack for a forward step would let a half-redone copy jump a commit forward,
        // silently dropping the operations still parked on the redo stack.
        assert!(
            body.contains("if chain_forward") && body.contains("s.redo_len() == 0"),
            "a FORWARD step must require the redo stack empty, not the undo stack"
        );
        assert!(
            body.contains("unwind_discards_unpushed("),
            "the guard must decide with the shared `unwind_discards_unpushed` predicate"
        );
        // The acknowledgement gates that refusal and nothing else: dropping the `!` would
        // let any `keep_undo_history` caller discard un-pushed work silently, which is the
        // invariant the flat refusal used to carry on its own.
        assert!(
            body.contains("if !discard_unpushed"),
            "un-pushed work may only be discarded by a caller that acknowledged it"
        );
    }

    /// The destructive swap enforces the child-policy rule: a forward step re-checks the
    /// downloaded child revision's own undo policy, which stops a stale or direct IPC call
    /// before it replaces the working copy.
    #[test]
    fn forward_chain_steps_are_gated_by_the_downloaded_child_policy() {
        let src = prod_source();
        let restore = src
            .split_once("pub fn speleodb_restore_revision")
            .map(|(_, rest)| rest)
            .expect("restore body follows its signature");
        let policy = restore
            .find("if keep_undo_history && chain_forward")
            .expect("the forward restore must re-check the downloaded child policy");
        let replace = restore
            .find(".speleodb_swap_in_revision(")
            .expect("the restore replaces the working copy");
        assert!(
            policy < replace,
            "the child-policy refusal must happen before the working copy is replaced"
        );
    }

    /// [#303] A save must carry the Unwind chain position onto the fresh file.
    ///
    /// `save_project` writes a brand-new `.stalx` and renames it over the destination, so
    /// every local-only table it does not explicitly re-apply is destroyed - the same shape
    /// of bug as the undo history losing its rows. Here the loss is not just "the walk
    /// restarts from BASE" (documented, benign): the pointer is what EXCUSES the dirty bit
    /// an Unwind leaves, so dropping it makes a copy the user only unwound read as holding
    /// un-pushed work. The confirmation resolves the flag before an autosave and the guard
    /// re-derives it after, they disagree, and the Unwind fails on press. Nothing in a
    /// scripted run saves between two Unwinds; a person pausing between them always does.
    #[test]
    fn a_save_carries_the_unwind_chain_position_onto_the_fresh_file() {
        let io = std::include_str!("project_io.rs");
        let snapshot = io
            .find("load_speleo_undo_chain")
            .expect("the save path must snapshot the chain pointer before rebuilding");
        let reapply = io
            .find("save_speleo_undo_chain")
            .expect("...and re-apply it to the fresh file");
        assert!(
            snapshot < reapply,
            "the snapshot has to be read from the OLD database - taken after the rebuild it \
             would read the fresh file's empty table and preserve nothing"
        );
        // Re-applied while the temp database is still open, like its `speleo_link` sibling.
        let closed = io
            .find("// Close the temp database before renaming")
            .expect("the save path closes the temp db before the rename");
        assert!(
            reapply < closed,
            "the write must land before the temp database is dropped and renamed"
        );

        // [#303] The inherited-history watermark needs the same carry-over, and loses more
        // by missing it: it decides which undo rows are cut from the next upload, so a save
        // that dropped it would let a loaded revision's history ride into the commit as
        // though this session had authored it - the duplication it exists to prevent,
        // reintroduced by an idle autosave.
        let snapshot_wm = io
            .find("load_speleo_undo_watermark")
            .expect("the save path must snapshot the inherited-history watermark");
        let reapply_wm = io
            .find("save_speleo_undo_watermark")
            .expect("...and re-apply it to the fresh file");
        assert!(
            snapshot_wm < reapply_wm && reapply_wm < closed,
            "read from the OLD database, written before the temp file is renamed"
        );
    }

    /// [#303] The watermark outlives an edit; the chain pointer does not.
    ///
    /// They look like the same kind of state and are cleared by nearly the same events, so
    /// the one place they differ is worth pinning: a forward command drops the pointer (the
    /// walk no longer describes the copy) but must LEAVE the watermark, because the whole
    /// point of it is to still know, at push time, which rows the user never authored. Clear
    /// it there and the loaded history ships again.
    #[test]
    fn a_forward_edit_keeps_the_watermark_even_as_it_drops_the_chain_pointer() {
        let forward = std::include_str!("app_state.rs");
        assert!(
            forward.contains("fn mark_remote_forward_edit")
                && forward.contains("self.speleodb_forget_unwind_position();"),
            "the shared forward-edit path drops the chain pointer"
        );
        assert!(
            !forward.contains("speleodb_set_inherited_through"),
            "...and must NOT touch the watermark: an edit is exactly when it starts to \
             matter, since it is what separates the user's new work from what was loaded"
        );
    }

    /// [#303] A forward edit drops the Unwind chain position; undo / redo must not.
    ///
    /// The asymmetry is the point, and it is what makes the confirmation warn again after
    /// an unwound copy is edited. Walking back through loaded history risks nothing the
    /// server has not already got, so the pointer stays and the dialog stays quiet. A new
    /// edit exists only in this copy, so the pointer goes and the dirt stops being excused.
    /// Clearing it from the undo path would collapse the two cases back together and warn
    /// on every step of an ordinary walk.
    #[test]
    fn a_forward_edit_forgets_the_unwind_position_but_an_undo_does_not() {
        let state_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/state");
        let mut forward_calls = Vec::new();
        for entry in std::fs::read_dir(state_dir).expect("read state source directory") {
            let path = entry.expect("state source entry").path();
            if path.extension().and_then(|v| v.to_str()) != Some("rs")
                || path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .is_some_and(|name| name.ends_with("_tests.rs"))
            {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .expect("file name");
            let source = std::fs::read_to_string(&path).expect("read state source");
            // Test fixtures may set up a dirty state directly; this tripwire governs
            // production mutation paths only.
            if !matches!(
                name,
                "app_state.rs" | "undo.rs" | "speleodb.rs" | "speleodb_revisions.rs" | "tests.rs"
            ) {
                assert!(
                    !source.contains("mark_remote_dirty();"),
                    "{name} calls the raw dirty marker; forward writers must use \
                     mark_remote_forward_edit so they also invalidate Unwind"
                );
            }
            if name != "speleodb_revisions.rs" && source.contains("mark_remote_forward_edit();") {
                forward_calls.push(name.to_owned());
            }
        }
        forward_calls.sort();
        assert_eq!(
            forward_calls,
            [
                "command_combine_sections.rs",
                "command_comment.rs",
                "command_delete_split.rs",
                "command_import.rs",
                "command_split_section.rs",
                "command_split_shot.rs",
                "commands.rs",
                "project_create.rs",
                "recent_projects.rs",
                "warnings.rs",
            ],
            "enumerate every direct forward writer so a new path is reviewed explicitly"
        );
        assert!(
            !std::include_str!("undo.rs").contains("speleodb_forget_unwind_position"),
            "undo / redo walk the loaded history - clearing the position there would warn \
             on every ordinary step of an Unwind chain"
        );
    }

    /// [#303] The fourth writer of the project file, which the test above cannot see.
    ///
    /// `a_forward_edit_forgets_the_unwind_position_but_an_undo_does_not` counts sites in
    /// `commands.rs`, so it was blind to `speleodb_set_undo_mode` - which writes
    /// `speleo_link.undo_mode` into the `.stalx`, rides to the server in the next push, and
    /// is therefore an edit of the project like any other. The mode, dirty stamp, and chain
    /// clear now share one database transaction: calling the older best-effort helper here
    /// would re-open both the half-write and silent-Unwind hazards.
    #[test]
    fn the_undo_mode_setter_forgets_the_unwind_position_too() {
        let policy = std::include_str!("undo_policy.rs");
        assert!(
            policy.contains("set_speleo_undo_mode_and_mark_forward_edit"),
            "the undo-mode write must atomically dirty the link and clear the chain pointer"
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

    /// [#303] The chain pointer is written AFTER the reload, and cleared by a plain
    /// View / Restore.
    ///
    /// Ordering is the whole point: the reload replaces the very file the pointer is
    /// written into (and the downloaded copy carries no chain row of its own, since it is
    /// stripped from every push), so a write before it would be thrown away.
    #[test]
    fn unwind_records_its_chain_position_after_the_reload() {
        let restore = prod_source()
            .split_once("pub fn speleodb_restore_revision")
            .map(|(_, rest)| rest)
            .expect("the swap verb is in this file");
        let reload = restore
            .find("self.speleodb_swap_in_revision(")
            .expect("the swap reloads in place");
        let record = restore
            .find("self.speleodb_set_unwound_from(Some(&hexsha));")
            .expect("an Unwind must record where the chain now stands");
        assert!(
            reload < record,
            "the pointer is written after the reload - the reload replaces the file it \
             lives in, so an earlier write would be discarded"
        );
        assert!(
            restore.contains("self.speleodb_set_unwound_from(None);"),
            "a plain View / Restore must CLEAR the pointer: the working copy is no longer \
             where the chain thought it was"
        );
    }

    #[test]
    fn restore_carries_its_edit_session_through_the_swap_and_mints_nothing() {
        // data#3: the edit gate is the claim bound to the session, so a Restore
        // keeps editing by asking the swap's commit to CARRY the binding (the
        // `restore` argument), which moves an existing claim - provenance and all -
        // and cannot create one. The old shape re-recorded the acquisition record
        // after the reload from a capture taken before it; that is gone, and with
        // it any way for a Restore to mint a claim it did not have.
        let restore = prod_source()
            .split_once("fn speleodb_restore_revision")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n    /// ").next())
            .expect("speleodb_restore_revision body precedes the next item's doc");
        assert!(
            !restore.contains("speleodb_note_lock_acquired"),
            "a Restore must not record a claim of its own"
        );
        assert!(
            restore.contains("            restore,\n        )?;"),
            "the swap carries the edit session exactly when this is a Restore"
        );
        assert!(
            restore.contains("if restore && !self.edit_enabled()"),
            "and a Restore is refused without an edit session to carry"
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
}
