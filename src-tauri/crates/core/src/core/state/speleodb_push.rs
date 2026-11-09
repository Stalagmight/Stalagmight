//! The push half of the SpeleoDB hand-off: everything between "the user is done
//! editing" and "the server has the bytes".
//!
//! Split out of `speleodb.rs` along the axis that module's own doc declares, and
//! the one the #275 plan names for this module: the BASE-SHA fence, the quiescent
//! VACUUM snapshot, the native SQLite upload, and the
//! Done / Push / Update verdicts built on their outcomes.
//!
//! What holds this together rather than being a grab bag: every path here decides
//! what the SERVER ends up holding, and each has to answer the same two questions
//! the fence exists for - is our BASE still the tip, and did the upload actually
//! land. [`PushOutcome`] is the shared answer type; `Unconfirmed` is why the dirty
//! bit and the lock outlive a failed attempt.
//!
//! Also here: the two escape hatches a push offers when the copy is NOT something
//! the user wants to publish - `speleodb_discard_changes` and the unwound-copy
//! return-to-tip (#303) - because both exist only as the alternative to the push
//! the dialog was about to make, and both end in the same lock decision.
//!
//! Stays in `speleodb.rs`: the lock + edit gate and the `speleo_link` persistence
//! spine (the residual the plan describes).

use std::path::Path;

use super::speleodb::{local_dto, managed_type_patch, type_needs_managed_label};
use super::speleodb_cache::LocalEditsPolicy;
use super::AppState;
use crate::speleodb::client;
use crate::speleodb::contract::{dto_for_error, map_status, Endpoint};
use crate::speleodb::sync::{self, PushPlan, ReconcileVerdict};
use crate::speleodb::SpeleoDbAction;
use crate::speleodb::{
    DoneResult, PushResult, SpeleoDbError, SpeleoErrorDto, StartEditingResult, UpdateResult,
};

/// Internal verdict of the push half of `speleodb_done`.
#[derive(Debug)]
pub(super) enum PushOutcome {
    /// The push landed (200 → BASE advanced; or 304 → already at the tip). The dirty
    /// bit was cleared UNLESS an edit landed after push_start (then it is retained for
    /// the next push), so the caller must re-check it before releasing the lock.
    Confirmed,
    /// A stranger commit sits between our BASE and the server tip - never pushed.
    /// Stay locked + dirty for the user to reconcile (#198).
    Diverged,
    /// The push outcome was ambiguous (transport drop / retried-then-exhausted
    /// 5xx): we don't know whether it landed. Stay locked + dirty; the next
    /// attempt recovers via ancestry (#193).
    Unconfirmed,
}

/// Sanitize a push SNAPSHOT copy before upload: clear the dirty bit and strip the
/// per-machine / per-session LOCAL tables so they don't ride to the server (#197).
/// All durable project data - survey, config, import-dedup, resolved-warnings -
/// proliferates with the project. See
/// [`crate::persistence::Database::sanitize_push_snapshot`] for the exact set and
/// for which failures are fatal (stripping the redo history and clearing the
/// dirty bit are; the privacy-neutral local prefs are not).
///
/// `keep_undo` (#303) is the one policy input: a Full-mode linked project ships its
/// undo history since the last commit, which is what gives undo cross-commit reach.
/// The redo stack is stripped in every mode.
///
/// `inherited_through` (#303) narrows what "since the last commit" means when a chain
/// step has loaded a past commit's history into the stack: everything at or before that
/// instant was inherited rather than authored here, and is cut from the COPY so it does
/// not ride into the new commit twice. `None` on the ordinary lock-edit-push path, where
/// every entry is this session's.
///
/// FATAL for the push: an unsanitized snapshot must never be uploaded.
pub(super) fn clear_snapshot_dirty(
    snapshot: &Path,
    keep_undo: bool,
    inherited_through: Option<&str>,
) -> Result<(), SpeleoDbError> {
    crate::persistence::Database::sanitize_push_snapshot(snapshot, keep_undo, inherited_through)
        .map_err(|e| SpeleoDbError::Io(format!("could not sanitize the push snapshot: {e}")))
}

/// Read the undo-sharing inputs from the snapshot that will actually be uploaded.
/// The live database handle may be absent after a successful save whose reopen failed,
/// but the installed file and its snapshot still carry the authoritative project policy.
fn snapshot_undo_metadata(
    state: &AppState,
    snapshot: &crate::persistence::Database,
) -> Result<(bool, Option<String>), SpeleoDbError> {
    let link = snapshot.load_speleo_link().map_err(|e| {
        SpeleoDbError::Io(format!(
            "could not read the undo policy from the push snapshot: {e}"
        ))
    })?;
    let inherited_through = snapshot.load_speleo_undo_watermark().map_err(|e| {
        SpeleoDbError::Io(format!(
            "could not read the undo watermark from the push snapshot: {e}"
        ))
    })?;
    Ok((
        state.undo_policy_for(link.as_ref()).pushes_history(),
        inherited_through,
    ))
}

/// The server `type` an un-adopted Ariane project carries. Compared
/// case-insensitively: the API answers `ARIANE` on a GET and takes `ariane` on a
/// PATCH, and which one a given payload used is not a distinction worth trusting.
const ARIANE_PROJECT_TYPE: &str = "ariane";

/// Does the server still class this project as an Ariane one?
///
/// `None` - the type could not be read - is NOT "yes". A missing answer is the one
/// case where acting would be a write issued against a project whose current state we
/// failed to establish, and the cost of waiting is one push: the next one reads it
/// again. Unknown types are left alone for the same reason, since only `ariane` is
/// the state adoption is meant to leave behind.
pub(super) fn type_still_says_ariane(server_kind: Option<&str>) -> bool {
    server_kind.is_some_and(|kind| kind.trim().eq_ignore_ascii_case(ARIANE_PROJECT_TYPE))
}

/// Resolve the server head, re-fetching ONCE when a recorded BASE meets an empty
/// tip (#192). `(base Some, tip None)` is the fence's fail-closed trigger - but a
/// momentary empty read (replication lag, or a race right after project creation)
/// must not be mistaken for "the history is gone," so re-fetch a single time
/// before letting the caller fail closed. `(base None, tip None)` is the valid
/// empty-repo state and never retries. Generic over the injected `fetch` so the
/// retry is unit-tested without HTTP.
pub(super) fn head_with_empty_retry<F>(
    base: Option<&str>,
    mut fetch: F,
) -> Result<(Option<String>, Vec<crate::speleodb::models::RevisionCommit>), SpeleoDbError>
where
    F: FnMut() -> Result<
        (Option<String>, Vec<crate::speleodb::models::RevisionCommit>),
        SpeleoDbError,
    >,
{
    let first = fetch()?;
    // Key the retry on the TRUE transient signal - an EMPTY commit list - not on
    // `tip.is_none()`: tip_of also returns None for a multi-leaf FORK, which is not
    // transient and must fail closed immediately rather than waste a second GET
    // (#197 review). A recorded BASE meeting an empty history is the momentary
    // empty-read case (replication lag / a race right after project creation).
    if base.is_some() && first.1.is_empty() {
        // Re-fetch exactly once, then trust the result (a still-empty second read
        // lets the fence fail closed).
        return fetch();
    }
    Ok(first)
}

impl AppState {
    /// Push the open project's edits (if any) and release the write lock,
    /// returning to read-only (#197). On a confirmed push (or nothing to push) the
    /// lock is released and the edit gate dropped; on divergence or an ambiguous
    /// push, the lock + dirty bit are retained so no edits are lost. `message` is
    /// the commit message (a generic default is used when blank).
    #[cfg(test)]
    pub(crate) fn speleodb_done(
        &self,
        message: Option<String>,
    ) -> Result<DoneResult, SpeleoErrorDto> {
        self.speleodb_done_acknowledged(message, false)
    }

    pub fn speleodb_done_acknowledged(
        &self,
        message: Option<String>,
        accept_older_version: bool,
    ) -> Result<DoneResult, SpeleoErrorDto> {
        let link = self.speleodb_validated_link()?;

        // #213: a force-editor is edit-enabled but holds NO lock, so Done cannot push
        // (the server would 403 the lockless upload). The FE offers "Reconnect to
        // sync" (Start editing) instead; guard here as a backstop.
        if self.force_editing() {
            return Err(local_dto(SpeleoDbError::Io(
                "reconnect to acquire the write lock before pushing".to_owned(),
            )));
        }

        // Edit-session gate: Done acts only on an ACTIVE edit session. The edit gate
        // is set true only by Start editing (after acquiring the lock) / its FF arm,
        // so `!edit_enabled` means we hold no lock - opened read-only, or already
        // finished. Releasing here would POST /release/ for a mutex this session
        // never acquired (a confusing Fence error, or a 401 that auto-drops the
        // token). We are already read-only, so answer without a server call.
        //
        // `NotEditing`, NOT `Released`: the two are only the same when the project
        // is clean, and the gate can be closed over a project that is still dirty -
        // a 401 auto-drop runs `speleodb_forget`, which closes the gate and
        // deliberately leaves `has_unpushed_edits` set so the work is not forgotten.
        // Reporting that as `Released` told the frontend the edits had landed, which
        // cleared its dirty state and disarmed the quit guard over work nothing ever
        // uploaded. The honest answer is "there was no edit session to finish".
        if !self.edit_enabled() {
            return Ok(DoneResult::NotEditing);
        }

        self.speleodb_guard_older_version_push(&link, accept_older_version)?;

        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        // Dirty-bit gate: only push when there are un-pushed edits.
        let outcome = if link.has_unpushed_edits {
            self.speleodb_push(&endpoint, &token, insecure_http, &link, message)?
        } else {
            PushOutcome::Confirmed
        };

        match outcome {
            PushOutcome::Confirmed => {
                // A 200 push advances BASE but RETAINS the dirty bit when an edit
                // landed after push_start (edits_captured_by_push == false): that edit
                // is not on the server yet. Re-read the link and, if it is still dirty,
                // keep the lock + editability and report PushUnconfirmed so a retry of
                // Done re-pushes the late edit - releasing now would strand it
                // read-only until a fresh Start editing. Only release once clean.
                let link_after = self.speleodb_open_link().map_err(local_dto)?;
                if link_after.has_unpushed_edits {
                    // Say WHY. Both routes to `PushUnconfirmed` render the same banner
                    // ("the upload could not be confirmed"), and this one used to log
                    // nothing at all - so a report of it was indistinguishable from an
                    // ambiguous upload (which does warn) and could not be placed without
                    // re-deriving the whole push from first principles.
                    //
                    // This arm is not a failure: the upload LANDED and an edit recorded
                    // after push-start is deliberately being kept for the next push. The
                    // timestamps are what make that checkable.
                    tracing::info!(
                        last_edit_at = ?link_after.last_edit_at,
                        purpose = "explain a PushUnconfirmed that is not an upload failure",
                        "speleodb done: the push landed, but an edit recorded after it \
                         started is still un-pushed; keeping the lock so a retry uploads it"
                    );
                    return Ok(DoneResult::PushUnconfirmed);
                }
                // Edits are safely on the server; release the lock and go
                // read-only. Drop the edit gate only after a confirmed release -
                // if release fails the lock is still held, so a retry of Done
                // (now a no-op push, dirty already cleared) re-attempts it.
                self.speleodb_handle_auth(
                    Endpoint::Release,
                    client::release_lock(&endpoint, &token, insecure_http, &link.project_id),
                )
                .map_err(|e| dto_for_error(Endpoint::Release, &e))?;
                self.set_edit_enabled(false);
                // The lock this process took is back with the server; the edit session
                // it licensed is over. This project's claim, not the account's whole
                // set - Done released one mutex.
                self.speleodb_forget_lock_acquired_for(&link.project_id);
                Ok(DoneResult::Released)
            }
            // Stay locked + dirty so the user can reconcile / a retry can recover.
            PushOutcome::Diverged => Ok(DoneResult::DivergenceNeeded),
            PushOutcome::Unconfirmed => Ok(DoneResult::PushUnconfirmed),
        }
    }

    /// Push the open project's edits to the server **without** releasing the lock
    /// (T2d "Push"): the same fence + ancestry-recovery + upload as
    /// [`Self::speleodb_done`]'s push half, but the mutex is retained in every arm
    /// so editing continues. A no-op (reports `Pushed`) when not in an edit session
    /// or when there is nothing to push.
    #[cfg(test)]
    pub(crate) fn speleodb_push_only(
        &self,
        message: Option<String>,
    ) -> Result<PushResult, SpeleoErrorDto> {
        self.speleodb_push_only_acknowledged(message, false)
    }

    pub fn speleodb_push_only_acknowledged(
        &self,
        message: Option<String>,
        accept_older_version: bool,
    ) -> Result<PushResult, SpeleoErrorDto> {
        let link = self.speleodb_validated_link()?;

        // #213: a force-editor holds no lock, so a push would 403. Refuse with the
        // same reconnect hint as Done (the FE never offers push in force-edit).
        if self.force_editing() {
            return Err(local_dto(SpeleoDbError::Io(
                "reconnect to acquire the write lock before pushing".to_owned(),
            )));
        }

        // Push acts only on an ACTIVE edit session (a held lock); without it there
        // is no lock and nothing to push - answer locally rather than earn a
        // confusing 403 Fence.
        //
        // The two no-ops are reported DIFFERENTLY on purpose. A closed gate is not
        // proof the work landed: `speleodb_forget` (the 401 auto-drop) closes the
        // gate while leaving `has_unpushed_edits` set, so folding this case into
        // `Pushed` told the frontend the edits were on the server, cleared its dirty
        // state and disarmed the quit guard over an upload that never happened. A
        // gate that IS open with a clean link is a true no-op and stays `Pushed`.
        if !self.edit_enabled() {
            return Ok(PushResult::NotEditing);
        }
        self.speleodb_guard_older_version_push(&link, accept_older_version)?;
        if !link.has_unpushed_edits {
            return Ok(PushResult::Pushed);
        }

        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        match self.speleodb_push(&endpoint, &token, insecure_http, &link, message)? {
            PushOutcome::Confirmed => {
                // A late edit after push_start leaves the dirty bit set (not yet on
                // the server); report PushUnconfirmed so a retry re-pushes it. The
                // lock is retained either way.
                let still_dirty = self
                    .speleodb_open_link()
                    .map_err(local_dto)?
                    .has_unpushed_edits;
                if still_dirty {
                    Ok(PushResult::PushUnconfirmed)
                } else {
                    Ok(PushResult::Pushed)
                }
            }
            PushOutcome::Diverged => Ok(PushResult::DivergenceNeeded),
            PushOutcome::Unconfirmed => Ok(PushResult::PushUnconfirmed),
        }
    }

    fn speleodb_guard_older_version_push(
        &self,
        link: &crate::models::SpeleoLink,
        acknowledged: bool,
    ) -> Result<(), SpeleoErrorDto> {
        if link.has_unpushed_edits && self.speleodb_unwound_from().is_some() && !acknowledged {
            return Err(local_dto(SpeleoDbError::Io(
                "uploading an older unwound revision must be confirmed".to_owned(),
            )));
        }
        Ok(())
    }

    /// [#303] Replace the local copy with the server tip, throwing away whatever it held.
    ///
    /// The shared body of the two ways out of a copy the user does not want: Refresh while
    /// mid-walk, and the explicit Discard action. `Discard` rather than `Protect` because
    /// destroying the local copy is the request, not a side effect - both callers have
    /// already put that in front of the user.
    ///
    /// Its own method rather than an inline call, so `speleodb_update` still contains
    /// exactly one `speleodb_fast_forward` - the FastForward arm - and the tripwire that
    /// pins the lock-record capture ahead of it keeps measuring what it was written to
    /// measure.
    fn speleodb_return_to_tip_discarding(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        link: &crate::models::SpeleoLink,
        tip: Option<&str>,
    ) -> Result<String, SpeleoErrorDto> {
        self.speleodb_fast_forward(
            endpoint,
            token,
            insecure_http,
            link,
            tip,
            LocalEditsPolicy::Discard,
        )
        .map_err(local_dto)
    }

    /// [#303] Throw the local copy away and come back at the server tip.
    ///
    /// Both surfaces that offer it have already said what it destroys: the push
    /// confirmation raised when a copy is dirty only because it was walked back, and the
    /// explicit "Discard changes" menu action, which offers it over ordinary edits too.
    /// So this does not re-litigate the decision - it does not check
    /// `unwound_not_edited`, because the menu action deliberately reaches it with real
    /// edits pending.
    ///
    /// Ends holding the lock, which is the point: the user is discarding in order to carry
    /// on working from the latest, not to be dropped into read-only. A lock we cannot get
    /// (someone else took it meanwhile) surfaces as the ordinary contended-lock error, by
    /// which time the discard has already happened and the copy is at the tip - which is
    /// still the state they asked for.
    ///
    /// `acknowledged` is the one thing it does insist on, and it is not a second guess at
    /// the decision: it is the difference between a caller that ASKED to destroy unpushed
    /// work and a caller that arrived here. Every sibling verb re-derives its own
    /// preconditions - Done checks the gate and the dirty bit, the Unwind guard checks the
    /// stack and the acknowledgement - while this one, the only verb that destroys work
    /// with no way back, validated nothing but a link and a token. Mirrors
    /// `speleodb_guard_unwind`'s `discard_unpushed`: the caller reports that the waiver was
    /// shown, and the backend refuses without it.
    ///
    /// Deliberately NOT gated on the edit gate or a held lock, which was considered and
    /// rejected: a 401 auto-drop (`speleodb_forget`) leaves the session read-only while the
    /// copy still holds unpushed work, and discard is the escape hatch for exactly that
    /// state. A lock gate would close the only door out of it.
    pub fn speleodb_discard_changes(
        &self,
        keep_lock: bool,
        acknowledged: bool,
    ) -> Result<Option<StartEditingResult>, SpeleoErrorDto> {
        if !acknowledged {
            return Err(local_dto(SpeleoDbError::Io(
                "discarding the local copy destroys un-pushed work and must be confirmed"
                    .to_owned(),
            )));
        }
        let link = self.speleodb_validated_link()?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        let (tip, _) = head_with_empty_retry(link.base_commit_sha.as_deref(), || {
            self.speleodb_server_head(&endpoint, &token, insecure_http, &link.project_id)
        })
        .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;

        tracing::info!(
            project_id = %link.project_id,
            keep_lock,
            purpose = "discard the local copy (#303)",
            "discarding local changes and returning to the server tip"
        );
        self.speleodb_return_to_tip_discarding(
            &endpoint,
            &token,
            insecure_http,
            &link,
            tip.as_deref(),
        )?;

        if !keep_lock {
            // Hand it back. `None` rather than a fabricated editing result, so no caller
            // can mistake a released project for one it may still write to.
            self.speleodb_release_current_lock(true, false, None)?;
            return Ok(None);
        }
        // Re-acquire through the ordinary verb rather than by hand, so the fence, the
        // authorship record and the concurrent-session notice all behave exactly as they
        // do for Start editing. The copy is clean at the tip by now, so its fence is a
        // no-op UpToDate.
        self.speleodb_start_editing().map(Some)
    }

    /// Read-only refresh of the open project against the server (#197): no lock is
    /// taken. UpToDate / FastForward (re-pull + reload) / DivergenceNeeded.
    pub fn speleodb_update(&self) -> Result<UpdateResult, SpeleoErrorDto> {
        let link = self.speleodb_validated_link()?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        let (tip, commits) = head_with_empty_retry(link.base_commit_sha.as_deref(), || {
            self.speleodb_server_head(&endpoint, &token, insecure_http, &link.project_id)
        })
        .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;

        // [#303] A copy that is dirty ONLY because a chain step walked it back is not a
        // reconciliation problem, it is a copy sitting somewhere the user asked to look.
        // "Refresh from SpeleoDB" should therefore do what it says and return them to the
        // tip - so it discards the walk rather than consulting the verdict.
        //
        // The verdict cannot help here anyway: a restore stamps BASE at the server tip, so
        // BASE == tip and `reconcile_verdict` answers UpToDate. Update then reported
        // "already up to date" while the user was staring at an older survey, which is the
        // least useful true statement available.
        if link.has_unpushed_edits && self.speleodb_unwound_from().is_some() {
            tracing::info!(
                project_id = %link.project_id,
                purpose = "refresh from SpeleoDB while mid-walk (#303)",
                "discarding an unwound copy and returning to the server tip"
            );
            let local_path = self.speleodb_return_to_tip_discarding(
                &endpoint,
                &token,
                insecure_http,
                &link,
                tip.as_deref(),
            )?;
            return Ok(UpdateResult::FastForwarded { local_path });
        }

        match sync::reconcile_verdict(
            link.base_commit_sha.as_deref(),
            tip.as_deref(),
            link.has_unpushed_edits,
            &commits,
        ) {
            ReconcileVerdict::UpToDate => Ok(UpdateResult::UpToDate),
            ReconcileVerdict::FastForward => {
                // speleodb_fast_forward reloads in place, which re-derives the edit
                // gate from is_remote (= false for a remote project). If the caller
                // was mid-edit-session (gate on, lock held - reachable if the lock
                // lapsed/was admin-released and another writer advanced the tip),
                // a clean FF must NOT silently strip editability; restore it after
                // the reload, mirroring speleodb_start_editing's FF arm. Captured
                // BEFORE the reload (which sets the gate false). FF only fires with
                // dirty == false, so there are no un-pushed edits to strand.
                //
                // The pair, not `edit_enabled` alone (#213, matching the release
                // verbs): a force-editor is edit-enabled while holding NO lock, and
                // the reload clears `force_editing`. Restoring on the gate alone
                // therefore laundered a lockless force-edit session into an
                // ordinary-looking editing one - editable, un-flagged, and with the
                // big-red "this .stalx cannot be merged" banner gone. Only a genuine
                // lock-holding session is restored; a force-editor lands read-only
                // (nothing to strand - FF requires a clean copy) and re-enters
                // deliberately via Start editing or Force edit.
                let was_editing = self.edit_enabled() && !self.force_editing();
                // Captured BEFORE the reload, which shuts the gate and lets a poll
                // landing in that window clear the record: this is the last point the
                // pre-reload truth exists. Kept SEPARATE from `was_editing` on
                // purpose - the gate is a permission ("may I edit?"), the record is
                // provenance ("did I take this lock?"), and the poll at the bottom of
                // `speleodb_link_status` can re-derive the gate from nothing but a
                // matching holder email. Restoring the record from the gate would
                // therefore mint provenance for a lock this process never took - an
                // inherited or sibling-session lock - and license it to explain away
                // another session's commit as our own unrecorded push.
                let had_lock_record = self.speleodb_lock_acquired_here(&link.project_id);
                let local_path = self
                    .speleodb_fast_forward(
                        &endpoint,
                        &token,
                        insecure_http,
                        &link,
                        tip.as_deref(),
                        LocalEditsPolicy::Protect,
                    )
                    .map_err(local_dto)?;
                if was_editing {
                    self.set_edit_enabled(true);
                    // Restore the record only where there was one to restore: the
                    // reload shut the gate, and a status poll landing in that window
                    // drops the record (as start-editing's FF arm re-records for the
                    // same reason). A session that held no record keeps none - the
                    // gate is restored either way, so an inherited lock stays as
                    // editable as it was, just without an authorship claim it never
                    // earned.
                    if had_lock_record {
                        self.speleodb_note_lock_acquired(Some(&link.project_id));
                    }
                }
                Ok(UpdateResult::FastForwarded { local_path })
            }
            // A fork, a moved tip our BASE isn't an ancestor of, OR a server
            // inconsistency (non-linear / multi-root / a recorded BASE with no tip):
            // surface the divergence prompt, never a hard error (#193 b2).
            ReconcileVerdict::Diverged => Ok(UpdateResult::DivergenceNeeded),
        }
    }

    /// Resolve the server's TRUE HEAD and return it with the full revision
    /// history. The server's `latest_commit` field (carried by acquire /
    /// get_project / list) is `max(authored_date)` and ties to the ROOT creation
    /// commit when commits land within the same second - verified unreliable both
    /// live and in the server source - so every fence resolves the tip from the
    /// parent-id topology ([`sync::tip_of`]) instead. Returns `(tip, commits)`;
    /// `tip` is `None` for an empty / ambiguous (forked) history.
    pub(super) fn speleodb_server_head(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
    ) -> Result<(Option<String>, Vec<crate::speleodb::models::RevisionCommit>), SpeleoDbError> {
        // No GetRevisions endpoint exists; reuse GetProject for the auth/error map.
        let history = self.speleodb_handle_auth(
            Endpoint::GetProject,
            client::get_revisions(endpoint, token, insecure_http, project_id),
        )?;
        let tip = sync::tip_of(&history.commits).map(str::to_owned);
        Ok((tip, history.commits))
    }

    /// The push half of [`Self::speleodb_done`]: fence BASE vs the server tip,
    /// recover via ancestry (#193) if the tip moved under our lock, then upload the
    /// snapshot + render.
    fn speleodb_push(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        link: &crate::models::SpeleoLink,
        message: Option<String>,
    ) -> Result<PushOutcome, SpeleoErrorDto> {
        let project_id = &link.project_id;

        // Authoritative server tip - the revision topology, NOT the unreliable
        // latest_commit field (see speleodb_server_head). The fetched history is
        // reused for ancestry recovery below, so this is the only revisions GET.
        let (tip, commits) = head_with_empty_retry(link.base_commit_sha.as_deref(), || {
            self.speleodb_server_head(endpoint, token, insecure_http, project_id)
        })
        .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;

        // `my_email` identifies our own landed commits in the BASE..tip gap (#193).
        // We are authenticated to be here; if it is somehow absent, plan_push can't
        // match any commit as "mine" and safely routes to the divergence prompt
        // rather than guessing. That is the right default, but it is indistinguishable
        // from a genuine divergence in the UI, so say so in the log - otherwise a
        // "conflict" nobody can explain has no trace of why it was reported.
        let my_email = self.speleodb_my_email().unwrap_or_else(|| {
            tracing::warn!(
                project_id,
                purpose = "push ancestry recovery (#193)",
                "no session email: our own landed commits cannot be identified, so a moved \
                 tip will fail closed to the divergence prompt"
            );
            String::new()
        });

        // push-start is not persisted, so the ancestry timing stays conservative
        // (clear_dirty == false → AdoptThenUpload, never AdoptConfirmed) and we
        // re-push on top of any landed commit - a no-op 304 if nothing changed.
        match sync::plan_push(
            link.base_commit_sha.as_deref(),
            tip.as_deref(),
            &commits,
            &my_email,
            link.last_edit_at.as_deref(),
            None,
        ) {
            // base == tip: push our edits directly.
            PushPlan::Upload => {}
            // Our prior (ambiguous) push landed; adopt the tip as BASE, then push the
            // current state on top (a 304 if nothing changed since).
            PushPlan::AdoptThenUpload => {
                self.speleodb_mutate_link(|l| l.base_commit_sha = tip.clone())
                    .map_err(local_dto)?;
            }
            // Forward-compatible only (unreachable while push-start is unpersisted):
            // the prior push landed and no edit followed, so adopt the tip and clear
            // dirty without a redundant re-push.
            PushPlan::AdoptConfirmed => {
                // The prior push DID land; this arm only recognises it late. Concluded
                // ahead of the fallible link write, like the 200 and 304 arms: the
                // recall went stale when that upload landed, so a failure to record
                // the adoption locally must not be what leaves it standing.
                let landed = PushOutcome::Confirmed;
                self.speleodb_mutate_link(|l| {
                    l.base_commit_sha = tip.clone();
                    l.has_unpushed_edits = false;
                    l.last_edit_at = None;
                })
                .map_err(local_dto)?;
                return Ok(landed);
            }
            // A stranger's commit, OR a server inconsistency (non-linear / multi-root
            // / a recorded BASE with no tip): never overwrite or guess - surface the
            // divergence prompt (#193 b2 + b5). Stays locked + dirty in speleodb_done.
            PushPlan::Diverged => return Ok(PushOutcome::Diverged),
        }

        // Normal push (Done / Push): fold the SDB-authoritative name pre-push (#216).
        self.speleodb_upload(endpoint, token, insecure_http, link, message, true)
    }

    /// Build the upload (VACUUM snapshot with the dirty bit cleared in the copy +
    /// the `.tml` render) and push it as one commit, then reconcile the dirty bit
    /// from the result.
    pub(super) fn speleodb_upload(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        link: &crate::models::SpeleoLink,
        message: Option<String>,
        fold_metadata: bool,
    ) -> Result<PushOutcome, SpeleoErrorDto> {
        let project_id = &link.project_id;
        let commit_message = message
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "Updated in Stalagmight".to_owned());

        // [#216] Pre-push SDB → `.stalx` metadata fold. The project name is stored on
        // BOTH the server and the embedded `.stalx` (via the Project model), and SDB is
        // authoritative for it (edited through the Project Settings tab, or by another
        // session / the web UI). Fold the server's current name into the open project
        // BEFORE the flush below so the pushed snapshot carries the authoritative name.
        // Name is the only dual-stored field today (description/country/visibility live
        // only on the server). Best-effort and NON-fatal: a direct GET (NOT via the auth
        // handler, so a spurious 401 on this consistency refresh can't auto-drop the
        // session mid-push - the upload itself routes through the handler and catches a
        // truly dead token), and the fold never dirties the link. Skipped on the initial
        // publish push (`fold_metadata == false`): that GET would hit a project created
        // milliseconds earlier whose name is already known-current, a wasted round-trip.
        // The server's project `type` at push time, read off the SAME GET as the name
        // fold so the self-heal below costs no extra round-trip. `None` means we could
        // not read it, which is not the same as "it is fine" - see the heal.
        let mut server_kind: Option<String> = None;
        if fold_metadata {
            // `fetched`, not `project`, and deliberately so:
            // `bump_sites_are_exactly_the_known_set` recognises a commit into the
            // AppState project slot by the shape of the assignment, and assigning
            // anything derived from a binding named `project` reads as one. The
            // tripwire is a text heuristic on purpose - keeping look-alikes out of the
            // tree is cheaper than teaching it to parse Rust, and far cheaper than the
            // session-boundary bug it exists to catch. (Which is also why this comment
            // describes the needle rather than quoting it.)
            match client::get_project(endpoint, token, insecure_http, project_id) {
                Ok(fetched) => {
                    self.speleodb_fold_local_name(&fetched.name);
                    server_kind = Some(fetched.kind);
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    project_id,
                    purpose = "pre-push SDB->.stalx name fold (#216)",
                    "speleodb_upload: could not GET project to fold the authoritative name; pushing the local name"
                ),
            }
        }

        // The live on-disk `.stalx` (the open project's cache file) and the scratch
        // snapshot / render paths beside it.
        let live = self.speleodb_cache_path(project_id).map_err(local_dto)?;
        let dir = live
            .parent()
            .ok_or_else(|| local_dto(SpeleoDbError::Io("cache path has no parent".to_owned())))?;
        let snapshot = dir.join("snapshot.stalx");

        // Push-start fence (#197): capture the instant BEFORE the flush/snapshot.
        // Any edit recorded after this (last_edit_at > push_start) is NOT in the
        // snapshot we upload, so a confirmed push must NOT clear its dirty bit - it
        // is re-pushed on the next Done instead of being silently dropped. (dirty
        // edits always carry a last_edit_at: mark_remote_dirty stamps both
        // together, so the None branch of edits_captured_by_push is unreachable for
        // a real dirty bit here.)
        let push_start = chrono::Utc::now().to_rfc3339();

        // Flush in-memory state before taking the native SQLite snapshot.
        // save_project also heals duplicate shot UUIDs and non-finite geometry,
        // so the upload carries the same normalized data as the local file.
        if !self.save_project(None).success {
            return Err(local_dto(SpeleoDbError::Io(
                "failed to flush project before push".to_owned(),
            )));
        }

        // Self-contained snapshot of the live `.stalx` (#195), dirty bit cleared in
        // the COPY so it never rides to the server.
        crate::persistence::Database::vacuum_snapshot(&live, &snapshot)
            .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;

        // Derive policy and history cleanup from the file just captured, never from the
        // live handle or stack. `save_project` can return success with a
        // DATABASE_NOT_REOPENED warning, which deliberately leaves `self.database` empty;
        // consulting `undo_policy()` there classifies a linked Full project as local and
        // strips the history from an upload whose snapshot still says Full.
        // An edit can land after save_project and before this point: it is absent from the
        // snapshot and must retain its undo entry after the confirmed push. Sampling the
        // live stack here included that edit in the cutoff even though it was not uploaded.
        let snapshot_db = crate::persistence::Database::open_for_metadata(&snapshot)
            .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        let (keep_undo, inherited_through) =
            snapshot_undo_metadata(self, &snapshot_db).map_err(local_dto)?;
        let snapshot_newest = snapshot_db
            .newest_undo_timestamp()
            .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
        let push_floor = chrono::DateTime::parse_from_rfc3339(&push_start)
            .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?
            .with_timezone(&chrono::Utc);
        let undo_cutoff = snapshot_newest
            .map_or(push_floor, |newest| newest.max(push_floor))
            .to_rfc3339();
        drop(snapshot_db);

        // FATAL if it fails: an unsanitized snapshot still carries the pusher's redo
        // history (and a stale dirty bit) and must never be uploaded. In Disabled mode
        // the undo history is fatal to leak too; in Full mode it is the payload.
        clear_snapshot_dirty(&snapshot, keep_undo, inherited_through.as_deref())
            .map_err(local_dto)?;

        let result = self.speleodb_handle_auth(
            Endpoint::UploadStalx,
            client::upload_stalx(
                endpoint,
                token,
                insecure_http,
                project_id,
                &snapshot,
                &commit_message,
            ),
        );
        match result {
            // 200: BASE advances to the adopted hexsha. Clear the dirty bit ONLY if
            // no edit was recorded after push_start (else a concurrent edit during
            // the upload would be dropped - keep it dirty so the next Done re-pushes
            // it). BASE advances: the uploaded snapshot IS the new base; a kept late
            // edit is a clean second commit on top of it.
            Ok(pushed) => {
                // Conclude the landing FIRST, before any fallible local bookkeeping.
                // A 200 means the revision is on the server; the recall went stale at
                // that moment, not when we finished writing the link row. Leaving the
                // invalidation after the `?` below meant a disk/db failure returned
                // with a choice we already knew was stale still standing - and the
                // next discarding open would extract that pre-push entry and stamp it
                // with the new tip, which is the exact rollback the recall drop is
                // here to prevent. The outcome is held and returned unchanged, so the
                // "landed" conclusion and the invalidation still cannot drift apart.
                let landed = PushOutcome::Confirmed;
                self.speleodb_heal_managed_type(
                    endpoint,
                    token,
                    insecure_http,
                    project_id,
                    server_kind.as_deref(),
                );
                let new_base = pushed.latest_commit.as_ref().map(|c| c.id.clone());
                self.speleodb_mutate_link(|l| {
                    // NEVER let a successful push move BASE BACKWARD. A None BASE
                    // over a dirty copy fences straight to Diverged, then walks the
                    // whole history to the service-account creation root and calls
                    // that a stranger's commit; the user gets a divergence prompt
                    // against their own just-landed push. Keeping the previous BASE
                    // instead costs at most one redundant ancestry recovery on the
                    // next push, which is exactly what that machinery is for. Mirrors
                    // the same guard the revision-restore path already applies.
                    //
                    // Presence is a sufficient test only because
                    // the SDK upload response now populates `latest_commit`
                    // SOLELY from the 200 envelope's authoritative top-level
                    // `hexsha`. It used to fall back to the response's own
                    // eventually-consistent `latest_commit`, which is documented (and
                    // was observed live) still pointing at the PRIOR tip right after
                    // a push - a stale ancestor that sails through this check and
                    // moves BASE backward past every intervening commit, which is the
                    // exact failure the guard is here to prevent.
                    if new_base.is_some() {
                        l.base_commit_sha = new_base;
                    } else {
                        tracing::warn!(
                            project_id,
                            "upload 200 carried no commit sha; keeping the previous BASE \
                             rather than resetting it to the empty-repo sentinel"
                        );
                    }
                    if sync::edits_captured_by_push(l.last_edit_at.as_deref(), Some(&push_start)) {
                        l.has_unpushed_edits = false;
                        l.last_edit_at = None;
                    }
                })
                .map_err(local_dto)?;
                self.speleodb_clear_undo_after_push(&undo_cutoff);
                Ok(landed)
            }
            Err(e) => {
                // 304: the remote tree already matches - clear dirty (same push-start
                // guard as the 200 path), BASE unchanged.
                if matches!(&e, SpeleoDbError::Status { status: 304, .. }) {
                    // A 304 does not mean nothing ever landed. The documented recovery
                    // adopts a prior upload whose response was lost, then re-uploads
                    // identical content - and it is THAT upload which made the recall
                    // stale, whatever this attempt returns. Concluded ahead of the
                    // fallible link write for the same reason as the 200 arm.
                    let landed = PushOutcome::Confirmed;
                    self.speleodb_heal_managed_type(
                        endpoint,
                        token,
                        insecure_http,
                        project_id,
                        server_kind.as_deref(),
                    );
                    self.speleodb_mutate_link(|l| {
                        if sync::edits_captured_by_push(
                            l.last_edit_at.as_deref(),
                            Some(&push_start),
                        ) {
                            l.has_unpushed_edits = false;
                            l.last_edit_at = None;
                        }
                    })
                    .map_err(local_dto)?;
                    self.speleodb_clear_undo_after_push(&undo_cutoff);
                    return Ok(landed);
                }
                // Ambiguous (retried-then-exhausted 5xx, or a transport drop): we
                // don't know whether it landed - stay dirty, recover via ancestry
                // on the next attempt.
                let ambiguous = match &e {
                    SpeleoDbError::Status { status, .. } => {
                        map_status(Endpoint::UploadStalx, *status) == SpeleoDbAction::StayDirty
                    }
                    SpeleoDbError::Transport { .. } => true,
                    _ => false,
                };
                if ambiguous {
                    tracing::warn!(
                        error = %e,
                        purpose = "preserve un-pushed edits after an ambiguous push",
                        "speleodb push: ambiguous result; staying dirty for ancestry recovery"
                    );
                    return Ok(PushOutcome::Unconfirmed);
                }
                // A hard rejection (403 fence, 4xx, parse): surface it.
                Err(dto_for_error(Endpoint::UploadStalx, &e))
            }
        }
    }

    /// A project with a `.stalx` on the server is a Stalagmight project, so the server
    /// `type` should say so. Flip it when it still says `ariane`, or the legacy
    /// managed type `other`: SpeleoDB maps only `stalagmight` projects from the
    /// `.stalx`, so an `other` project's map would stay frozen.
    ///
    /// Adoption does this itself on the push it performs, and that covers the ordinary
    /// route. It does NOT cover the one that skips that push: adoption declines to push
    /// through somebody else's write lock (and cannot push at all offline or read-only),
    /// so it converts locally and leaves the flip undone - and the Start-editing → Done
    /// the user then runs had no reason to touch the project type. The result was a
    /// permanent half-state: the `.stalx` up on the server under a project that still
    /// advertises itself as Ariane, keeps offering Adopt, and makes later `.tml` edits
    /// look authoritative to whoever makes them. Nothing in the app ever finished it.
    ///
    /// So the finishing move belongs to the push rather than to adoption: every push
    /// that lands is proof the project is Stalagmight-managed, whichever route got it
    /// there. Idempotent - a project already typed `stalagmight` costs nothing, which
    /// is every push after the first.
    ///
    /// Called only from arms where the upload CONCLUDED, never before one. Flipping
    /// ahead of a landing recreates the state this exists to avoid: a project typed
    /// as managed whose only file is the Ariane one that type says to disregard,
    /// openable by nobody.
    ///
    /// `server_kind` is the type read before the upload. `None` means we do not have
    /// one - either the read failed, or it never ran (`fold_metadata == false`, the
    /// initial publish push, where the project was created milliseconds ago and its
    /// type is ours already). This declines to act on either: the flip is not urgent,
    /// and guessing costs a write against a project we know nothing current about. The
    /// next push reads it again.
    ///
    /// Called directly rather than through the auth handler, like adoption's own flip
    /// and for the same reason: this is a swallowed best-effort call at the end of a
    /// successful push, so a spurious 401 here would drop the session - saved token
    /// included - over work that already landed.
    fn speleodb_heal_managed_type(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
        server_kind: Option<&str>,
    ) {
        if !type_needs_managed_label(server_kind) {
            return;
        }
        match client::patch_project(
            endpoint,
            token,
            insecure_http,
            project_id,
            &managed_type_patch(),
        ) {
            Ok(_) => tracing::info!(
                project_id,
                from = server_kind.unwrap_or_default(),
                purpose =
                    "label a landed project stalagmight (unfinished adoption or legacy other)",
                "speleodb: relabelled the server project type to stalagmight after a landed push"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                project_id,
                purpose = "label a landed project stalagmight (unfinished adoption or legacy other)",
                "speleodb: could not flip the project type after a landed push; the next push retries"
            ),
        }
    }

    /// [#303] Clear-on-push (q8 / Q25): after a CONFIRMED push, drop the undo entries
    /// that push carried and the whole redo stack.
    ///
    /// The operations are on the server now, inside the commit just made. Keeping them
    /// locally would put them in the NEXT commit as well, which is the duplication this
    /// issue exists to remove - and the hybrid model's answer to "undo past the last
    /// commit" is the explicit Unwind, which fetches that commit's history back.
    ///
    /// Bounded by a cutoff, not "clear everything": an edit recorded after the
    /// snapshot was taken is NOT on the server, and the arms above deliberately keep its
    /// dirty bit so the next push carries it. Its undo entry has to survive with it, or
    /// that push would upload a dirty project whose pending edit had no history.
    ///
    /// [#303] That cutoff is `undo_cutoff` from the push site, NOT the wall-clock
    /// `push_start` the dirty-bit arms use. Stack stamps are ordered by the stack itself
    /// and can sit slightly ahead of the wall clock, and an entry above the cutoff is
    /// never cleared - so the next push would carry it a second time. See the push site
    /// for why the two instants have to stay separate.
    ///
    /// Runs in EVERY mode. In Disabled mode nothing was uploaded, but a push still ends
    /// the undo session (Q23) - one rule, so "what happened to my history" has one
    /// answer.
    ///
    /// Best-effort because the push has already landed. Poisoned guards are recovered;
    /// a disk failure retains the chain metadata so stale rows still fail closed on reopen.
    pub(super) fn speleodb_clear_undo_after_push(&self, undo_cutoff: &str) {
        let Ok(cutoff) = chrono::DateTime::parse_from_rfc3339(undo_cutoff) else {
            tracing::warn!(
                undo_cutoff,
                purpose = "clear undo history after a confirmed push (#303)",
                "could not parse the undo cutoff; leaving the undo stack alone"
            );
            return;
        };
        let cutoff = cutoff.with_timezone(&chrono::Utc);
        let database = self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut stack = self
            .undo_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dropped = stack.drop_entries_up_to(cutoff);
        let Some(db) = database.as_ref() else {
            tracing::warn!(
                dropped,
                purpose = "persist confirmed-push undo cleanup (#303)",
                "no live database handle; retaining the chain metadata so stale disk rows remain classified"
            );
            return;
        };
        if !self.persist_locked_undo_stacks_into(db, &stack) {
            tracing::warn!(
                dropped,
                purpose = "persist confirmed-push undo cleanup (#303)",
                "could not persist the pruned history; retaining the chain metadata so stale disk rows remain classified"
            );
            return;
        }
        let remaining = stack.len();
        drop(stack);
        drop(database);

        // Only clear the chain metadata after the disk rows are gone. If persistence
        // failed, retaining these markers makes a reopen fail closed instead of
        // treating stale uploaded rows as fresh history.
        self.speleodb_set_unwound_from(None);
        self.speleodb_set_inherited_through(None);
        tracing::info!(
            dropped,
            remaining,
            undo_cutoff,
            purpose = "avoid duplicating pushed history in the next commit (#303)",
            "Persisted the undo cleanup for a confirmed push"
        );
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test setup failures should stop the regression test immediately"
)]
mod snapshot_undo_metadata_tests {
    use super::*;
    use crate::models::{Project, SpeleoLink, UndoMode};
    use crate::persistence::Database;
    use crate::test_helpers::with_isolated_config;
    use tempfile::TempDir;

    #[test]
    fn full_snapshot_policy_survives_an_absent_live_database_handle() {
        with_isolated_config(|| {
            let state = AppState::new().expect("construct state");
            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Disabled;
            assert!(
                !state.undo_policy().pushes_history(),
                "the old live-handle lookup classifies an unopened state as local"
            );

            let temp = TempDir::new().expect("temp dir");
            let path = temp.path().join("snapshot.stalx");
            {
                let mut db = Database::open(&path).expect("create snapshot");
                db.save_project(&Project::new("Snapshot".to_owned(), None))
                    .expect("save project");
                db.save_speleo_link(Some(&SpeleoLink {
                    endpoint: "https://example.invalid".to_owned(),
                    project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
                    project_name: "Snapshot".to_owned(),
                    base_commit_sha: None,
                    has_unpushed_edits: true,
                    last_edit_at: None,
                    last_verify_at: None,
                    undo_mode: Some(UndoMode::Full),
                }))
                .expect("save link");
                db.save_speleo_undo_watermark(Some("2026-08-02T10:00:00+00:00"))
                    .expect("save watermark");
            }

            let snapshot = Database::open_for_metadata(&path).expect("open snapshot metadata");
            let (keep_undo, inherited_through) =
                snapshot_undo_metadata(&state, &snapshot).expect("read snapshot policy");

            assert!(keep_undo, "the snapshot's explicit Full policy wins");
            assert_eq!(
                inherited_through.as_deref(),
                Some("2026-08-02T10:00:00+00:00")
            );
        });
    }
}
