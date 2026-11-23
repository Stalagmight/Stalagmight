//! The push half of the SpeleoDB hand-off: everything between "the user is done
//! editing" and "the server has the bytes".
//!
//! Split out of `speleodb.rs` along the axis that module's own doc declares, and
//! the one the #275 plan names for this module: the BASE-SHA fence, the push
//! snapshot (the canonical version 2 file in its "for push" mode, plan 6.5), the
//! upload, and the Done / Push / Update verdicts built on their outcomes.
//!
//! What holds this together rather than being a grab bag: every path here decides
//! what the SERVER ends up holding, and each has to answer the same two questions
//! the fence exists for - is our BASE still the tip, and did the upload actually
//! land. [`PushOutcome`] is the shared answer type; `Unconfirmed` is why the dirty
//! bit and the lock outlive a failed attempt.
//!
//! "Is our BASE still the tip" is asked of every upload, and only this session's
//! own landed upload may stand between them (R3): SpeleoDB's lock is the
//! account's, so another session of the same account holds it too and its
//! revisions carry our email. What this session uploaded is recorded in
//! `speleodb_own_uploads`; a HEAD that moved past BASE by anything else is the
//! divergence prompt, and nothing is uploaded. A race lost between that check
//! and the upload is found in the history right after it and reported with the
//! push's result (`replaced_revisions`).
//!
//! Every state change a landed push makes (BASE, the dirty bit, the cut history,
//! the cleared Unwind chain) is durable at once through the local-only save
//! (plan 6.5 mode 4, R2 B3), so a close right after Done never reopens to a stale
//! BASE over a "dirty" copy. A save refused after the landing (another program
//! changed the file) is logged, not a push failure: memory holds the landing and
//! the next save writes it (`speleodb_record_landing`).
//!
//! Rollout (plan 12.1): a SpeleoDB server that still refuses non-SQLite uploads
//! answers the version 2 upload with a refusal that names the file type or the
//! SQLite it expected (or a server error saying so, after which nothing
//! landed). The upload recognises it by status and reason
//! (`contract::upload_refusal`) and fails with one error wherever the push ran,
//! `SpeleoDbError::UploadFormatRefused` ("This SpeleoDB server does not accept
//! projects saved by this version yet"); any other refusal shows the server's
//! own reason (review finding O3). Either way the project stays dirty and the
//! lock stays held, as after any failed push.
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
use crate::speleodb::contract::{self, dto_for_error, map_status, Endpoint};
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
    ///
    /// `undo_history_omitted` (sdk#3): the uploaded copy left its undo history out
    /// to fit under the server's upload limit (see
    /// [`push_snapshot`]); a new revision landed without it.
    Confirmed { undo_history_omitted: bool },
    /// A stranger commit sits between our BASE and the server tip - never pushed.
    /// Stay locked + dirty for the user to reconcile (#198).
    Diverged,
    /// The push outcome was ambiguous (transport drop / retried-then-exhausted
    /// 5xx): we don't know whether it landed. Stay locked + dirty; the next
    /// attempt recovers via ancestry (#193).
    Unconfirmed,
}

/// The bytes a push uploads, and what they left out.
#[derive(Debug)]
pub(super) struct PushSnapshot {
    pub(super) bytes: Vec<u8>,
    /// sdk#3: the push had undo history to carry (Full mode), and every entry of
    /// it was left out to fit the server's upload limit: a new revision lands
    /// without it. A push that keeps only its newest entries is not this (the
    /// cut is logged).
    pub(super) undo_history_omitted: bool,
}

/// Serialize the open project for a push (plan 6.5 mode 2, 10.1): the canonical
/// version 2 file without `local` (the dirty bit, the view, the sync state and
/// the Unwind position never ride to the server, #197) and without redo; the
/// undo history only when `keep_undo` (the project's undo mode is Full, #303),
/// cut at the inherited watermark (`local.speleodb.inherited_through`: history
/// loaded by a chain step is not authored here and must not ride into the new
/// commit twice), then from the oldest end so the whole file fits `limit`
/// (sdk#3, R2 H8: the newest entries are the ones kept).
///
/// SpeleoDB refuses a file over its upload limit only after the whole body has
/// arrived, so the size is settled here, before a byte is sent: a survey that
/// does not fit even without its history is refused with `UploadTooLarge`.
///
/// `extras` must be the open file's, with `history` the undo stack as it is now:
/// the same in-memory snapshot as `project` (taken under one `project`
/// acquisition by the push).
pub(super) fn push_snapshot(
    project: &crate::models::survey::Project,
    extras: &crate::persistence::stalx_v2::Extras,
    keep_undo: bool,
    limit: u64,
) -> Result<PushSnapshot, SpeleoDbError> {
    use crate::persistence::stalx_v2::{encode_with_report, EncodeError, Mode, PushOptions};
    let options = PushOptions {
        keep_undo,
        watermark: extras.local.speleodb.inherited_through,
        byte_cap: usize::try_from(limit).unwrap_or(usize::MAX),
    };
    match encode_with_report(project, extras, Mode::ForPush(options)) {
        Ok((bytes, report)) => {
            let cut = &report.history;
            if cut.size_cut > 0 || cut.inherited_cut > 0 {
                tracing::info!(
                    kept = cut.undo_kept,
                    inherited_cut = cut.inherited_cut,
                    size_cut = cut.size_cut,
                    size = report.size,
                    limit,
                    purpose = "fit the upload under the server size limit (sdk#3)",
                    "speleodb push: cut the uploaded undo history; the live project is untouched"
                );
            }
            Ok(PushSnapshot {
                bytes,
                undo_history_omitted: cut.size_cut > 0 && cut.undo_kept == 0,
            })
        }
        Err(EncodeError::TooLarge { size, .. }) => Err(SpeleoDbError::UploadTooLarge {
            size: u64::try_from(size).unwrap_or(u64::MAX),
            limit,
        }),
        Err(e) => Err(SpeleoDbError::Io(format!(
            "could not serialize the push snapshot: {e}"
        ))),
    }
}

/// Write the push snapshot to a scratch file in `dir` (the cache directory) for
/// the upload to stream; removed when the handle drops.
fn write_push_snapshot(dir: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile, SpeleoDbError> {
    use std::io::Write as _;
    let io =
        |e: std::io::Error| SpeleoDbError::Io(format!("could not stage the push snapshot: {e}"));
    std::fs::create_dir_all(dir).map_err(io)?;
    let mut file = tempfile::Builder::new()
        .prefix(".speleodb-push-")
        .suffix(".stalx")
        .tempfile_in(dir)
        .map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.flush().map_err(io)?;
    Ok(file)
}

/// The time an undo history entry was recorded (its `"t"`), when it says.
fn history_entry_time(
    entry: &crate::persistence::stalx_v2::RawJson,
) -> Option<chrono::DateTime<chrono::Utc>> {
    #[derive(serde::Deserialize)]
    struct Stamp {
        #[serde(default)]
        t: Option<String>,
    }
    let text = serde_json::from_str::<Stamp>(entry.as_str()).ok()?.t?;
    chrono::DateTime::parse_from_rfc3339(&text)
        .ok()
        .map(|at| at.with_timezone(&chrono::Utc))
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
    fn speleodb_push_project_guard(
        &self,
        generation: u64,
    ) -> Result<std::sync::MutexGuard<'_, Option<crate::models::survey::Project>>, SpeleoErrorDto>
    {
        let project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.speleodb_session_generation() != generation || project.is_none() {
            return Err(local_dto(SpeleoDbError::SessionMoved));
        }
        Ok(project)
    }

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
        let _lock_ops = self.speleodb_lock_ops_guard();
        let (generation, link) = self.speleodb_session_snapshot()?;

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

        let (endpoint, token, email) = self.speleodb_active_account().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        // Dirty-bit gate: only push when there are un-pushed edits.
        let outcome = if link.has_unpushed_edits {
            self.speleodb_push(&endpoint, &token, insecure_http, &link, message, generation)?
        } else {
            PushOutcome::Confirmed {
                undo_history_omitted: false,
            }
        };

        match outcome {
            PushOutcome::Confirmed {
                undo_history_omitted,
            } => {
                // A 200 push advances BASE but RETAINS the dirty bit when an edit
                // landed after push_start (edits_captured_by_push == false): that edit
                // is not on the server yet. Re-read the link and, if it is still dirty,
                // keep the lock + editability and report PushUnconfirmed so a retry of
                // Done re-pushes the late edit - releasing now would strand it
                // read-only until a fresh Start editing. Only release once clean.
                let link_after = {
                    let _project = self.speleodb_push_project_guard(generation)?;
                    self.speleodb_open_link().map_err(local_dto)?
                };
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
                    // The landing is reported with this result: a revision it
                    // replaced (R3) must not wait for a later one the session may
                    // never reach (F4R3-locks-push-3).
                    return Ok(DoneResult::PushUnconfirmed {
                        replaced_revisions: self.speleodb_take_replaced(&link.project_id),
                    });
                }
                // Edits are safely on the server; release the lock and go
                // read-only. Drop the edit gate only after the release is settled -
                // if it fails (or the holder cannot be read) the lock may still be
                // ours, so the session stays open and a retry of Done (now a no-op
                // push, dirty already cleared) re-attempts it.
                //
                // Holder-checked (lock#4): the server lets an admin release anybody's
                // mutex, and with nothing to upload no fresh ownership probe ran
                // above, so a lock force-taken by someone else since Start Editing
                // would be freed by a bare release. Somebody else's lock (or none)
                // just ends this session: nothing of ours is held any more.
                let released = self
                    .speleodb_handle_auth(
                        Endpoint::Release,
                        super::speleodb_lock_release::release_if_held_by_us(
                            &super::speleodb_lock_release::LockReleaseParams {
                                endpoint,
                                token,
                                email,
                                insecure_http,
                                project_id: link.project_id.clone(),
                            },
                        ),
                    )
                    .map_err(|e| {
                        // The upload may have landed and replaced a revision (R3):
                        // that rides the failure, the only result this Done gives
                        // (F4R3-locks-push-3).
                        let mut dto = dto_for_error(Endpoint::Release, &e);
                        dto.replaced_revisions = self.speleodb_take_replaced(&link.project_id);
                        dto
                    })?;
                let _project = self.speleodb_push_project_guard(generation)?;
                if released == super::speleodb_lock_release::HolderCheckedRelease::NotHeldByUs {
                    tracing::info!(
                        project_id = %link.project_id,
                        purpose = "never release another account's lock (lock#4)",
                        "speleodb done: the lock is no longer this account's; ending the \
                         edit session without releasing"
                    );
                }
                // The lock this process took is back with the server; the edit session
                // it licensed is over, and ending the claim shuts the gate with it
                // (`speleodb_edit_gate`). This project's claim, not the account's whole
                // set - Done released one mutex.
                self.speleodb_forget_lock_acquired_for(&link.project_id);
                Ok(DoneResult::Released {
                    undo_history_omitted: undo_history_omitted.then_some(true),
                    replaced_revisions: self.speleodb_take_replaced(&link.project_id),
                })
            }
            // Stay locked + dirty so the user can reconcile / a retry can recover.
            PushOutcome::Diverged => Ok(DoneResult::DivergenceNeeded),
            PushOutcome::Unconfirmed => Ok(DoneResult::PushUnconfirmed {
                replaced_revisions: self.speleodb_take_replaced(&link.project_id),
            }),
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
        let _lock_ops = self.speleodb_lock_ops_guard();
        let (generation, link) = self.speleodb_session_snapshot()?;

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
            // Nothing to upload, but an earlier landing may have a replacement
            // left to report (R3; see the late-edit arm below).
            return Ok(PushResult::Pushed {
                undo_history_omitted: None,
                replaced_revisions: self.speleodb_take_replaced(&link.project_id),
            });
        }

        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        match self.speleodb_push(&endpoint, &token, insecure_http, &link, message, generation)? {
            PushOutcome::Confirmed {
                undo_history_omitted,
            } => {
                // A late edit after push_start leaves the dirty bit set (not yet on
                // the server); report PushUnconfirmed so a retry re-pushes it. The
                // lock is retained either way.
                let _project = self.speleodb_push_project_guard(generation)?;
                let still_dirty = self
                    .speleodb_open_link()
                    .map_err(local_dto)?
                    .has_unpushed_edits;
                if still_dirty {
                    // A revision this landing replaced (R3) is reported now, with
                    // the late edit's result (F4R3-locks-push-3).
                    Ok(PushResult::PushUnconfirmed {
                        replaced_revisions: self.speleodb_take_replaced(&link.project_id),
                    })
                } else {
                    Ok(PushResult::Pushed {
                        undo_history_omitted: undo_history_omitted.then_some(true),
                        replaced_revisions: self.speleodb_take_replaced(&link.project_id),
                    })
                }
            }
            PushOutcome::Diverged => Ok(PushResult::DivergenceNeeded),
            PushOutcome::Unconfirmed => Ok(PushResult::PushUnconfirmed {
                replaced_revisions: self.speleodb_take_replaced(&link.project_id),
            }),
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
            (
                LocalEditsPolicy::Discard,
                self.speleodb_session_generation(),
                None,
            ),
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
            self.speleodb_release_held_lock_best_effort();
            self.speleodb_shut_edit_gate();
            return Ok(None);
        }
        // Re-acquire through the ordinary verb rather than by hand, so the fence, the
        // authorship record and the concurrent-session notice all behave exactly as they
        // do for Start editing. The copy is clean at the tip by now, so its fence is a
        // no-op UpToDate.
        self.speleodb_start_editing().map(Some)
    }

    /// Reload never acquires a lock. Discard consent belongs to one backend session.
    #[expect(
        clippy::result_large_err,
        reason = "preserve the shared structured IPC recovery payload"
    )]
    pub fn speleodb_reload(
        &self,
        discard_changes: bool,
        expected: Option<crate::speleodb::SpeleoReleaseContext>,
    ) -> Result<UpdateResult, SpeleoErrorDto> {
        let _lock_ops = self.speleodb_lock_ops_guard();
        self.speleodb_check_expected_context(expected.as_ref())
            .map_err(local_dto)?;
        self.speleodb_reload_locked(discard_changes, expected.as_ref())
    }

    #[expect(
        clippy::result_large_err,
        reason = "preserve the shared structured IPC recovery payload"
    )]
    fn speleodb_reload_locked(
        &self,
        discard_changes: bool,
        expected: Option<&crate::speleodb::SpeleoReleaseContext>,
    ) -> Result<UpdateResult, SpeleoErrorDto> {
        // The consent is for the project open NOW, exactly as it is: the session
        // generation and the project revision (`OpenProjectToken`), re-checked after
        // the network round trip and again inside the reload's commit.
        let consent = {
            let project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let token = self.open_project_token_locked();
            if expected.is_some_and(|expected| {
                expected.session_generation != token.generation.to_string()
                    || project.as_ref().is_none_or(|project| {
                        project.modified_at.to_rfc3339() != expected.project_modified_at
                    })
            }) {
                return Err(local_dto(SpeleoDbError::SessionMoved));
            }
            token
        };
        if !discard_changes {
            let project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.speleodb_require_clean_project(project.as_ref())
                .map_err(local_dto)?;
        }
        let link = self.speleodb_validated_link()?;
        if link.has_unpushed_edits && !discard_changes {
            return Err(local_dto(SpeleoDbError::Io(
                "Reload Project requires confirmation to discard unsaved and unuploaded changes. Saving first preserves your work.".into(),
            )));
        }
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        let (tip, commits) = head_with_empty_retry(link.base_commit_sha.as_deref(), || {
            self.speleodb_server_head(&endpoint, &token, insecure_http, &link.project_id)
        })
        .map_err(|e| dto_for_error(Endpoint::GetProject, &e))?;
        {
            let _project = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !self.open_project_token_holds_locked(consent) {
                return Err(local_dto(SpeleoDbError::SessionMoved));
            }
        }
        if tip.is_none() && !commits.is_empty() {
            return Ok(UpdateResult::DivergenceNeeded);
        }
        let verdict = sync::reconcile_verdict(
            link.base_commit_sha.as_deref(),
            tip.as_deref(),
            link.has_unpushed_edits,
            &commits,
        );
        let replace = discard_changes || self.speleodb_unwound_from().is_some();
        if !replace {
            match verdict {
                ReconcileVerdict::UpToDate => return Ok(UpdateResult::UpToDate),
                ReconcileVerdict::Diverged => return Ok(UpdateResult::DivergenceNeeded),
                ReconcileVerdict::FastForward => {}
            }
        }
        // The reload carries an edit session only over a lock SpeleoDB still
        // records as this account's (farm finding P2-take-remote-editable: the
        // account's other session can hand it back without this one hearing).
        let editing_stopped = self.speleodb_confirm_lock_for_reload(
            (&endpoint, &token, insecure_http),
            &link.project_id,
            consent.generation,
        );
        let local_path = self
            .speleodb_fast_forward(
                &endpoint,
                &token,
                insecure_http,
                &link,
                tip.as_deref(),
                (
                    if discard_changes {
                        LocalEditsPolicy::Discard
                    } else {
                        LocalEditsPolicy::Protect
                    },
                    consent.generation,
                    Some(consent.revision),
                ),
            )
            .map_err(local_dto)?;
        Ok(UpdateResult::FastForwarded {
            local_path,
            editing_stopped,
        })
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
        generation: u64,
    ) -> Result<PushOutcome, SpeleoErrorDto> {
        let project_id = &link.project_id;
        {
            let _project = self.speleodb_push_project_guard(generation)?;
        }
        let (verified_endpoint, verified_token) = self.speleodb_require_owned_lock_locked(link)?;
        if verified_endpoint != endpoint || verified_token != token {
            return Err(local_dto(SpeleoDbError::SessionMoved));
        }

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

        // R3: only what THIS session uploaded may be adopted as "our own landed
        // push". The account's email is not enough - another session of the same
        // account holds the same per-account lock and signs with it.
        let own = self.speleodb_own_uploads(project_id);

        // push-start is not persisted, so the ancestry timing stays conservative
        // (clear_dirty == false → AdoptThenUpload, never AdoptConfirmed) and we
        // re-push on top of any landed commit - a no-op 304 if nothing changed.
        match sync::plan_push(
            link.base_commit_sha.as_deref(),
            tip.as_deref(),
            &commits,
            &my_email,
            &own,
            link.last_edit_at.as_deref(),
            None,
        ) {
            // base == tip: push our edits directly.
            PushPlan::Upload => {}
            // Our prior (ambiguous) push landed; adopt the tip as BASE, then push the
            // current state on top (a 304 if nothing changed since).
            PushPlan::AdoptThenUpload => {
                // The unanswered upload is answered now: it made the tip.
                self.speleodb_note_upload_answered(generation, project_id, tip.as_deref());
                let project = self.speleodb_push_project_guard(generation)?;
                self.speleodb_mutate_link_held(project.as_ref(), |l| {
                    l.base_commit_sha = tip.clone();
                })
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
                let landed = PushOutcome::Confirmed {
                    undo_history_omitted: false,
                };
                self.speleodb_note_upload_answered(generation, project_id, tip.as_deref());
                let project = self.speleodb_push_project_guard(generation)?;
                self.speleodb_record_landing(project.as_ref(), None, |l| {
                    l.base_commit_sha = tip.clone();
                    l.has_unpushed_edits = false;
                    l.last_edit_at = None;
                })?;
                return Ok(landed);
            }
            // HEAD moved past BASE by a commit this session did not upload - a
            // stranger's, or another session's of our own account (R3) - OR a server
            // inconsistency (non-linear / multi-root / a recorded BASE with no tip):
            // never overwrite or guess - surface the divergence prompt (#193 b2 + b5),
            // uploading nothing. Stays locked + dirty in speleodb_done.
            PushPlan::Diverged => {
                tracing::info!(
                    project_id,
                    base = ?link.base_commit_sha,
                    head = ?tip,
                    purpose = "never upload over a revision this session did not make (R3)",
                    "speleodb push: SpeleoDB's latest revision is not this copy's BASE \
                     and not an upload of ours; uploading nothing, asking the user to reconcile"
                );
                return Ok(PushOutcome::Diverged);
            }
        }

        // Normal push (Done / Push): fold the SDB-authoritative name pre-push (#216).
        self.speleodb_upload(
            endpoint,
            token,
            insecure_http,
            link,
            message,
            true,
            generation,
        )
    }

    /// Save, build the push snapshot ([`push_snapshot`]) and push it as one
    /// commit, then reconcile the dirty bit, BASE and the history from the result.
    pub(super) fn speleodb_upload(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        link: &crate::models::SpeleoLink,
        message: Option<String>,
        fold_metadata: bool,
        generation: u64,
    ) -> Result<PushOutcome, SpeleoErrorDto> {
        {
            let _project = self.speleodb_push_project_guard(generation)?;
        }
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
                    self.speleodb_fold_local_name(&fetched.name, project_id, generation);
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

        // The snapshot is written beside the open project's cache file (the same
        // volume, never the system temp) for the upload to stream, and removed
        // after it.
        let live = self.speleodb_cache_path(project_id).map_err(local_dto)?;
        let dir = live
            .parent()
            .ok_or_else(|| local_dto(SpeleoDbError::Io("cache path has no parent".to_owned())))?;

        // Push-start fence (#197): capture the instant BEFORE the save and the
        // snapshot. Any edit recorded after this (last_edit_at > push_start) may not
        // be in the commit, so a confirmed push must NOT clear its dirty bit - it is
        // re-pushed on the next Done instead of being silently dropped. (dirty edits
        // always carry a last_edit_at: mark_remote_dirty stamps both together, so the
        // None branch of edits_captured_by_push is unreachable for a real dirty bit
        // here.)
        let push_floor = chrono::Utc::now();
        let push_start = push_floor.to_rfc3339();

        // From here the upload may land, and when its answer is lost nobody knows
        // whether SpeleoDB's latest revision is BASE or this upload (#193). BASE's
        // survey digest (S2) says a copy holding BASE's survey has nothing to
        // upload, which is true only while the server holds nothing of this copy
        // past BASE: after a lost answer, an edit taken back would read as clean
        // and stay on the server. So the digest goes now, in the save just below
        // (a crash during the upload cannot bring it back), and the landing
        // records the new one when the answer names the revision, or a 304 says
        // BASE holds what went up. Until then the mark is taken at its word, as
        // on any copy without a digest.
        {
            let _project = self.speleodb_push_project_guard(generation)?;
            self.with_file(|file| {
                let sync = &mut file.extras.local.speleodb;
                sync.base_survey = None;
                sync.base_survey_edit = None;
            });
        }

        // A push first saves (plan 6.5): the local copy and the commit describe the
        // same survey, and an unsaved survey is never uploaded without being on disk.
        // The save's own refusal rides the error (review G-speleodb-8): a file open
        // read-only says READ_ONLY_FILE, a file another program changed says
        // EXTERNALLY_MODIFIED, so the user is not left with a bare "failed".
        let saved = self.save_project_at_generation(None, Some(generation));
        if !saved.success {
            return Err(local_dto(SpeleoDbError::Io(format!(
                "failed to flush project before push: {}",
                saved
                    .error
                    .as_deref()
                    .unwrap_or("the save did not complete")
            ))));
        }

        // Then the push serializes from the in-memory project, its file state and its
        // undo stack, all under ONE `project` acquisition, so the bytes, the undo
        // policy and the history cutoff describe one instant. The policy is the open
        // link's (the project's own undo mode, #303); the encoder backs it up with the
        // link it writes.
        let (snapshot, snapshot_newest) = {
            let project = self.speleodb_push_project_guard(generation)?;
            let mut extras = self.with_file(|file| file.extras.clone());
            let keep_undo = self
                .undo_policy_for(self.speleo_link().as_ref())
                .pushes_history();
            let (history, newest) = {
                let stack = self
                    .undo_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (stack.persisted(), stack.newest_stamp())
            };
            extras.history = history;
            let survey = project
                .as_ref()
                .ok_or_else(|| local_dto(SpeleoDbError::SessionMoved))?;
            let snapshot = push_snapshot(
                survey,
                &extras,
                keep_undo,
                client::SERVER_UPLOAD_LIMIT_BYTES,
            )
            .map_err(|e| dto_for_error(Endpoint::UploadStalx, &e))?;
            (snapshot, newest)
        };
        let undo_history_omitted = snapshot.undo_history_omitted;
        // Plan 12.1: only a version 2 body can be refused for being one.
        let uploading_v2 = matches!(
            crate::persistence::stalx_v2::sniff(&snapshot.bytes),
            crate::persistence::stalx_v2::Format::V2
        );
        // [#303] The clear-on-push cutoff: every undo entry in the stack when the
        // snapshot was taken is on the server now (or older than what fit), so it is
        // the later of the stack's newest stamp at that instant and the push floor.
        // An entry recorded afterwards stamps strictly above both, keeps its history,
        // and rides the next push.
        let undo_cutoff = snapshot_newest.map_or(push_floor, |newest| newest.max(push_floor));
        let staged = write_push_snapshot(dir, &snapshot.bytes).map_err(local_dto)?;

        // R3: the BASE this upload sits on is the open link's NOW (an ancestry
        // adoption just before it moved it), and until the answer says what the
        // upload made it is one of this session's unanswered uploads - the only thing
        // the next push may adopt as its own if the answer is lost.
        let upload_base = {
            let _project = self.speleodb_push_project_guard(generation)?;
            self.speleo_link().and_then(|l| l.base_commit_sha)
        };
        self.speleodb_note_upload_sent(
            generation,
            project_id,
            upload_base.clone(),
            &commit_message,
        );
        let result = self.speleodb_handle_auth(
            Endpoint::UploadStalx,
            client::upload_stalx(
                endpoint,
                token,
                insecure_http,
                project_id,
                staged.path(),
                &commit_message,
            ),
        );
        drop(staged);
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
                let landed = PushOutcome::Confirmed {
                    undo_history_omitted,
                };
                let new_base = pushed.latest_commit.as_ref().map(|c| c.id.clone());
                // R3: the answer names the revision this upload made, which is ours
                // from now on (a status refresh that read BASE before this landing
                // meets it). A 200 that names nothing leaves the upload unanswered,
                // so the next push still recovers it by ancestry.
                // What was unanswered before this answer: one of those landing
                // under this upload is ours, not a revision this one replaced.
                let unanswered = self.speleodb_own_uploads(project_id).unanswered;
                if let Some(head) = new_base.as_deref() {
                    self.speleodb_note_upload_answered(generation, project_id, Some(head));
                }
                self.speleodb_heal_managed_type(
                    endpoint,
                    token,
                    insecure_http,
                    project_id,
                    server_kind.as_deref(),
                );
                // What went up is the new BASE only when the landing names it.
                let landed_bytes = new_base.is_some().then_some(snapshot.bytes.as_slice());
                let head = new_base.clone();
                let project = self.speleodb_push_project_guard(generation)?;
                self.speleodb_record_landing(project.as_ref(), landed_bytes, |l| {
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
                    } else {
                        // An edit after push_start is not in the commit. Set, not
                        // kept: a save may have cleared a stale mark while the
                        // upload ran (S2), over the BASE this landing replaces.
                        l.has_unpushed_edits = true;
                    }
                })?;
                self.speleodb_clear_undo_after_push(project.as_ref(), undo_cutoff);
                drop(project);
                // R3, the lost race: SpeleoDB has no conditional upload, so another
                // session of the account may have landed a revision between the
                // HEAD check and this upload, and ours went on top of it. Only the
                // history says so; what it finds rides the push's result.
                if let Some(head) = head.as_deref() {
                    self.speleodb_check_replaced_by_upload(
                        (endpoint, token, insecure_http),
                        generation,
                        project_id,
                        &unanswered,
                        upload_base.as_deref(),
                        head,
                    );
                }
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
                    // A 304 made no new revision, so nothing landed without history.
                    let landed = PushOutcome::Confirmed {
                        undo_history_omitted: false,
                    };
                    // Nothing new landed from this upload.
                    self.speleodb_note_upload_answered(generation, project_id, None);
                    self.speleodb_heal_managed_type(
                        endpoint,
                        token,
                        insecure_http,
                        project_id,
                        server_kind.as_deref(),
                    );
                    // The server holds exactly what went up, at BASE.
                    let project = self.speleodb_push_project_guard(generation)?;
                    self.speleodb_record_landing(
                        project.as_ref(),
                        Some(snapshot.bytes.as_slice()),
                        |l| {
                            if sync::edits_captured_by_push(
                                l.last_edit_at.as_deref(),
                                Some(&push_start),
                            ) {
                                l.has_unpushed_edits = false;
                                l.last_edit_at = None;
                            } else {
                                // As in the 200 arm: a late edit stays un-uploaded.
                                l.has_unpushed_edits = true;
                            }
                        },
                    )?;
                    self.speleodb_clear_undo_after_push(project.as_ref(), undo_cutoff);
                    return Ok(landed);
                }
                // Plan 12.1: a server that does not take version 2 yet refuses this
                // build's file. One clear error, wherever the push ran; the copy
                // stays dirty and the lock held, as after any failed push.
                if let (true, SpeleoDbError::Status { status, message }) = (uploading_v2, &e) {
                    if self.speleodb_upload_refused_format(
                        endpoint,
                        token,
                        insecure_http,
                        project_id,
                        *status,
                        message,
                    ) {
                        self.speleodb_note_upload_answered(generation, project_id, None);
                        tracing::warn!(
                            status,
                            detail = %message,
                            project_id,
                            purpose = "the server refused this build's project file format (plan 12.1)",
                            "speleodb push: refused by a server without version 2 support; \
                             staying dirty and locked"
                        );
                        return Err(dto_for_error(
                            Endpoint::UploadStalx,
                            &SpeleoDbError::UploadFormatRefused {
                                status: *status,
                                detail: message.clone(),
                            },
                        ));
                    }
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
                // A hard rejection (403 fence, 4xx, parse): surface it. A status the
                // server answered with (or a file refused before sending) landed
                // nothing; anything else - a 200 whose body could not be read - may
                // have, so the upload stays the unanswered one the next push may
                // recover (R3).
                if matches!(
                    e,
                    SpeleoDbError::Status { .. } | SpeleoDbError::UploadTooLarge { .. }
                ) {
                    self.speleodb_note_upload_answered(generation, project_id, None);
                }
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

    /// Record what a CONCLUDED landing changes on the link (BASE, the dirty bit)
    /// and, when `pushed` (the bytes that went up) is the copy's BASE now, the
    /// digest of their survey as BASE's (S2): in memory first, then made durable
    /// by the local-only save (`speleodb_record_landed_link`).
    ///
    /// The commit is on the server whatever the disk says, so a failed save is
    /// logged, never returned (review G-speleodb-7). Returning it reported a push
    /// failure for a commit that landed and skipped the clear-on-push, so the next
    /// push shipped the same history again. Memory is the authority from here: the
    /// clear-on-push still runs, and the next save writes both (a file another
    /// program changed goes through the save-conflict prompt first; a copy reopened
    /// from the stale disk recovers by ancestry, its BASE behind our own commit).
    /// Only a link that is gone is an error: then nothing was recorded at all.
    #[expect(
        clippy::result_large_err,
        reason = "preserve the shared structured IPC recovery payload"
    )]
    fn speleodb_record_landing(
        &self,
        open: Option<&crate::models::survey::Project>,
        pushed: Option<&[u8]>,
        record: impl FnOnce(&mut crate::models::SpeleoLink),
    ) -> Result<(), SpeleoErrorDto> {
        match self.speleodb_record_landed_link(open, pushed, record) {
            Ok(()) => Ok(()),
            Err(SpeleoDbError::NoRemoteProject) => Err(local_dto(SpeleoDbError::NoRemoteProject)),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    purpose = "record a landed push (review G-speleodb-7)",
                    "speleodb push: the commit landed, but its bookkeeping could not be \
                     saved; it is recorded in memory and the next save writes it"
                );
                Ok(())
            }
        }
    }

    /// Plan 12.1: does the refused upload of a version 2 file (`status`, the
    /// server's reason `message`) mean the server does not take that format? A
    /// refusal that names the format does ([`contract::UploadRefusal::Content`]);
    /// one with another reason is that reason, shown as the server gave it. A
    /// server error naming the format does only when the upload certainly did
    /// not land ([`sync::upload_not_landed`] over the history read right after
    /// it): a 5xx after a commit is the ambiguous case the next push recovers by
    /// ancestry, so it stays "could not be confirmed".
    ///
    /// The history read is a direct GET, NOT through the auth handler, like the
    /// pre-push name fold: a spurious 401 on this follow-up must not drop the
    /// session over a push that already failed. A failed read is not certain.
    fn speleodb_upload_refused_format(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
        status: u16,
        message: &str,
    ) -> bool {
        match contract::upload_refusal(status, message) {
            contract::UploadRefusal::Content => true,
            contract::UploadRefusal::Other => false,
            contract::UploadRefusal::IfNotLanded => {
                // The BASE the upload sat on: the open link's NOW, since an
                // ancestry adoption just before the upload moved it in memory.
                let base = self.speleo_link().and_then(|l| l.base_commit_sha);
                let my_email = self.speleodb_my_email().unwrap_or_default();
                match client::get_revisions(endpoint, token, insecure_http, project_id) {
                    Ok(history) => sync::upload_not_landed(
                        base.as_deref(),
                        sync::tip_of(&history.commits),
                        &history.commits,
                        &my_email,
                    ),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            project_id,
                            purpose = "tell a format refusal from an ambiguous upload (plan 12.1)",
                            "speleodb push: could not read the history after a server error; \
                             the upload stays unconfirmed"
                        );
                        false
                    }
                }
            }
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
    ///
    /// Runs under the push's `project` guard (`open` is the project in it), so it
    /// serializes with every session boundary.
    pub(super) fn speleodb_clear_undo_after_push(
        &self,
        open: Option<&crate::models::survey::Project>,
        undo_cutoff: chrono::DateTime<chrono::Utc>,
    ) {
        let (dropped, remaining) = {
            let mut stack = self
                .undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dropped = stack.drop_entries_up_to(undo_cutoff);
            // Nothing left in the stack is inherited: what was is on the server.
            stack.set_inherited_through(None);
            (dropped, stack.len())
        };
        self.with_file(|file| {
            // While the survey has unsaved changes the local-only save writes the
            // history the file last held, not the stack (open_file.rs): cut that one
            // the same way, so the disk never keeps pushed entries as fresh history.
            let saved = &mut file.saved_history;
            saved
                .undo
                .retain(|entry| history_entry_time(entry).is_none_or(|at| at > undo_cutoff));
            saved.redo.clear();
            // The chain position and the watermark described the history that just
            // went; the next Unwind restarts from the new BASE.
            file.extras.local.speleodb.unwound_from = None;
            file.extras.local.speleodb.inherited_through = None;
        });
        // ONE local-only save writes the cut history and the cleared chain metadata
        // together. If it fails the disk keeps both as they were (the old rows still
        // fall under the old watermark), and the next save writes them together.
        match self.persist_local_state_at(open.and_then(|p| p.file_path.clone())) {
            Ok(()) => tracing::info!(
                dropped,
                remaining,
                undo_cutoff = %undo_cutoff,
                purpose = "avoid duplicating pushed history in the next commit (#303)",
                "Persisted the undo cleanup for a confirmed push"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                dropped,
                purpose = "persist confirmed-push undo cleanup (#303)",
                "could not persist the pruned history; it is cut in memory and the next save writes it"
            ),
        }
    }
}
