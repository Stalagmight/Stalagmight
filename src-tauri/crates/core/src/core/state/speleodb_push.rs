//! The push half of the SpeleoDB hand-off: everything between "the user is done
//! editing" and "the server has the bytes".
//!
//! Split out of `speleodb.rs` along the axis that module's own doc declares, and
//! the one the #275 plan names for this module: the BASE-SHA fence, the quiescent
//! VACUUM snapshot, the `ariane.tml` co-render, the upload itself, and the
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

use super::speleodb::{local_dto, managed_type_patch, type_needs_managed_label, Landed};
use super::speleodb_cache::LocalEditsPolicy;
use super::AppState;
use crate::core::events::IpcEvent as _;
use crate::core::{SpeleoDoneReport, SpeleoPushReport, ValidationWarning};
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

/// [#576 Q-W44-6-D1] The file a push was made from and the link it read there: where
/// its landing is recorded when the session has switched away by then
/// (`AppState::speleodb_record_landed_upload`).
pub(super) struct UploadMadeFor<'a> {
    /// The `.stalx` the snapshot was taken from.
    pub(super) file: &'a Path,
    /// The link the push read from it.
    pub(super) link: &'a crate::models::SpeleoLink,
}

/// A concluded upload, recorded in `link`: BASE advances to `new_base` when there is
/// one, and the dirty mark clears when no edit was recorded after `push_start`.
fn record_landing(link: &mut crate::models::SpeleoLink, new_base: Option<&str>, push_start: &str) {
    if let Some(base) = new_base {
        link.base_commit_sha = Some(base.to_owned());
    }
    if sync::edits_captured_by_push(link.last_edit_at.as_deref(), Some(push_start)) {
        link.has_unpushed_edits = false;
        link.last_edit_at = None;
    }
}

/// [#576 Q-W44-6-D1] [`record_landing`] in the link row of `made_for.file`, through a
/// handle of its own: that handle and the link as written, or `Ok(None)` when that
/// file holds no link, or a link to another project or deployment than the push read.
fn record_landing_in_file(
    made_for: &UploadMadeFor<'_>,
    new_base: Option<&str>,
    push_start: &str,
) -> Result<Option<(crate::persistence::Database, crate::models::SpeleoLink)>, String> {
    let db =
        crate::persistence::Database::open_existing(made_for.file).map_err(|e| e.to_string())?;
    let Some(mut link) = db.load_speleo_link().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let same_project = link.project_id == made_for.link.project_id
        && crate::speleodb::auth::normalize_endpoint(&link.endpoint)
            == crate::speleodb::auth::normalize_endpoint(&made_for.link.endpoint);
    if !same_project {
        return Ok(None);
    }
    record_landing(&mut link, new_base, push_start);
    db.save_speleo_link(Some(&link))
        .map_err(|e| e.to_string())?;
    Ok(Some((db, link)))
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

/// [#576 FZ-SDB-1] A push's two scratch files beside the cache copy - the `.tml`
/// render and the VACUUM snapshot - removed when the push that wrote them returns,
/// on every path (landed, refused, failed). Nothing reads them afterwards (the next
/// push rebuilds both from the live file), and left behind they kept two more full
/// copies of the survey in the cache.
///
/// [#576 RVF-b-4] Each push gets its own directory, `<cache>/<id>/push-<uuid>/`,
/// holding both files under their usual names, and the guard removes that directory
/// and nothing else. On names shared by every push of a project, two overlapping
/// pushes collided: the upload half runs outside `command_lock`, so the first
/// push's drop unlinked the second's snapshot before the second's upload opened
/// it. The local names never reach the server - `client::build_upload_parts` sends
/// the canonical wire names - so only the directory is new.
struct PushScratch {
    dir: std::path::PathBuf,
}

impl PushScratch {
    /// Make this push's directory inside `cache_dir`. `create_dir`, not
    /// `create_dir_all`: it fails on ANY entry already at the name, a link included
    /// (mkdir never follows its last component), so the files go into a directory
    /// this call made, empty, and the writers below find nothing there to follow.
    fn create(cache_dir: &Path) -> std::io::Result<Self> {
        sweep_stale_push_scratch(cache_dir, STALE_PUSH_SCRATCH);
        let dir = cache_dir.join(format!("push-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&dir)?;
        Ok(Self { dir })
    }

    fn snapshot(&self) -> std::path::PathBuf {
        self.dir.join("snapshot.stalx")
    }

    fn tml(&self) -> std::path::PathBuf {
        self.dir.join("ariane.tml")
    }
}

/// How old a `push-*` entry has to be before the next push of the project sweeps it.
/// A running push never lasts this long, so only a crashed push's scratch qualifies.
const STALE_PUSH_SCRATCH: std::time::Duration = std::time::Duration::from_hours(24);

/// [#576 RVF-b-4] Remove the `push-*` entries in `cache_dir` last modified at least
/// `older_than` ago: the scratch of a push that crashed before its guard ran. The
/// fixed names were bounded (the next push replaced them); without this, every
/// crashed push kept a full copy of the survey forever. An age test, not a registry
/// of live pushes, because it holds across processes. Best-effort: nothing here can
/// fail the push. `symlink_metadata`, so a `push-*` link is removed, never followed.
fn sweep_stale_push_scratch(cache_dir: &Path, older_than: std::time::Duration) {
    remove_legacy_push_scratch(cache_dir);
    let entries = match std::fs::read_dir(cache_dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    error = %e,
                    path = %cache_dir.display(),
                    "speleodb_upload: could not list the cache dir to sweep stale push scratch"
                );
            }
            return;
        }
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("push-") {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // A modification time in the future (a clock set back) is not stale.
        let stale = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= older_than);
        if !stale {
            continue;
        }
        let removed = if meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match removed {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                error = %e,
                path = %path.display(),
                "speleodb_upload: could not sweep a stale push scratch entry"
            ),
        }
    }
}

/// The fixed names v0.5.0-rc5 wrote a push's scratch at, beside the cache copy.
const LEGACY_PUSH_SCRATCH: [&str; 2] = ["snapshot.stalx", "ariane.tml"];

/// [#576 RVF-d-12] Remove the scratch a v0.5.0-rc5 push left at its fixed names
/// in `cache_dir`. That build never removed them and nothing since writes there,
/// so every project an rc5 build pushed kept a full extra copy of its survey in
/// the cache. Whatever their age; a link there is removed, never followed, and a
/// folder at either name is left alone. Best-effort, like the sweep.
fn remove_legacy_push_scratch(cache_dir: &Path) {
    for name in LEGACY_PUSH_SCRATCH {
        let path = cache_dir.join(name);
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| !meta.is_dir()) {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "speleodb_upload: could not remove an old build's push scratch"
                ),
            }
        }
    }
}

impl Drop for PushScratch {
    fn drop(&mut self) {
        // `remove_dir_all` removes a link it meets instead of following it, so
        // nothing outside this push's directory goes with it; it also takes any
        // sidecar a failed SQLite write left beside the snapshot.
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                error = %e,
                path = %self.dir.display(),
                "speleodb_upload: could not remove a push's scratch directory; a push a day later sweeps it"
            ),
        }
    }
}

impl AppState {
    /// Render the open remote project to a scratch `ariane.tml` at `dest` for
    /// co-push (#196). Mints a stable UUID for any shot still missing one - a
    /// no-op for modern files; for legacy pre-v6 shots it mints AND persists so
    /// the canonical `.stalx` (hence its push snapshot) shares the identity the
    /// `.tml` carries. Renders via `export_tml_for_speleodb` into a throwaway
    /// sidecar - the same document as the user-facing export, hardened for the
    /// server's GeoJSON conversion.
    fn speleodb_render_tml(&self, dest: &Path) -> Result<(), SpeleoDbError> {
        // Snapshot the in-memory project so the UUID-persist + render run without
        // holding the project lock across the database lock.
        let (project, minted) = {
            let mut proj_guard = self.project.lock();
            let project = proj_guard.as_mut().ok_or(SpeleoDbError::NoRemoteProject)?;
            let minted = project.sections.iter().any(|s| {
                s.shots
                    .iter()
                    .any(|sh| sh.uuid.as_deref().map_or(true, str::is_empty))
            });
            project.ensure_shot_uuids();
            (project.clone(), minted)
        };
        // Persist freshly-minted UUIDs so the snapshot .stalx matches the .tml.
        // save_project does NOT clear the SpeleoDB dirty bit (speleo_link is
        // excluded), so this is safe to call inside the push pipeline.
        if minted {
            {
                let mut db_guard = self.database.lock();
                if let Some(db) = db_guard.as_mut() {
                    if let Err(e) = db.save_project(&project) {
                        tracing::warn!(
                            error = %e,
                            purpose = "persist freshly-minted shot UUIDs before push",
                            "speleodb_render_tml: failed to persist minted UUIDs"
                        );
                    }
                }
            }
        }
        crate::export::tml::export_tml_for_speleodb(&project, dest)
            .map_err(|e| SpeleoDbError::Io(format!("render .tml failed: {e}")))?;
        Ok(())
    }

    /// Push the open project's edits (if any) and release the write lock,
    /// returning to read-only (#197). On a confirmed push (or nothing to push) the
    /// lock is released and the edit gate dropped; on divergence or an ambiguous
    /// push, the lock + dirty bit are retained so no edits are lost. When the server
    /// says the lock is no longer ours (#576 B35(e)), nothing is pushed or released:
    /// the gate drops, the dirty bit stays, and the verb fails with
    /// [`SpeleoDbError::LockNotHeld`] naming the holder. `message` is the commit
    /// message (a generic default is used when blank).
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
        self.speleodb_done_reported(message, accept_older_version)
            .map(|report| report.result)
    }

    /// [`Self::speleodb_done_acknowledged`] with the warnings of the save the push
    /// ran first (#576 NF-W2SAVE-1): what the `speleodb_done` command answers, so the
    /// frontend shows them like a normal save's warnings.
    #[expect(
        clippy::result_large_err,
        reason = "the SpeleoErrorDto every SpeleoDB verb returns; boxing the error type is a \
                  repo-wide change outside #576 RV4"
    )]
    pub fn speleodb_done_reported(
        &self,
        message: Option<String>,
        accept_older_version: bool,
    ) -> Result<SpeleoDoneReport, SpeleoErrorDto> {
        let (result, save_warnings) = self.speleodb_with_pre_push_save(|collected| {
            self.speleodb_done_collecting(message, accept_older_version, collected)
        })?;
        Ok(SpeleoDoneReport {
            result,
            save_warnings,
        })
    }

    /// Emit the pre-push save's warnings of a failed Done / Push (see
    /// [`Self::speleodb_with_pre_push_save`]) as the save's own outcome.
    pub(super) fn speleodb_surface_pre_push_save(&self, warnings: Vec<ValidationWarning>) {
        if warnings.is_empty() {
            return;
        }
        let Some(sink) = self.get_event_sink() else {
            return;
        };
        let outcome = crate::core::ProjectFileOutcome { warnings };
        match crate::core::events::payload::<crate::core::events::SpeleoPrePushSave>(&outcome) {
            Ok(value) => sink.emit(crate::core::events::SpeleoPrePushSave::NAME, value),
            Err(e) => tracing::warn!(
                error = %e,
                purpose = "tell the user what the pre-push save could not keep (#576 RV4-R-S6)",
                "speleodb: could not serialize the pre-push save's warnings"
            ),
        }
    }

    fn speleodb_done_collecting(
        &self,
        message: Option<String>,
        accept_older_version: bool,
        save_warnings: &mut pre_push_save::PrePushSave<'_>,
    ) -> Result<DoneResult, SpeleoErrorDto> {
        let link = self.speleodb_link_made_for("Done")?;

        // #213: a force-editor is edit-enabled but holds NO lock, so Done cannot push
        // (the server would 403 the lockless upload). The FE offers "Reconnect to
        // sync" (Start editing) instead; guard here as a backstop.
        if self.force_editing() {
            return Err(local_dto(SpeleoDbError::refused(
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
        let holder =
            self.speleodb_push_lock_holder(&endpoint, &token, insecure_http, &link.project_id)?;

        // [#576 B35(e)-D1] Never push under, or release, a lock that is not ours. The
        // release used to go out blind; for a project ADMIN it force-releases
        // whoever holds the mutex now (`speleodb_admin.rs`), so a colleague who took
        // the lock after ours was freed lost it to our Done. Same read as the
        // authorship licence below.
        self.speleodb_require_our_lock(&link.project_id, holder.as_deref(), "done")?;

        // Dirty-bit gate: only push when there are un-pushed edits.
        let outcome = if link.has_unpushed_edits {
            let we_hold_the_lock =
                self.speleodb_authorship_licence(&link.project_id, holder.as_deref());
            self.speleodb_push(
                &endpoint,
                &token,
                insecure_http,
                &link,
                message,
                we_hold_the_lock,
                save_warnings,
            )?
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
                // [#576 SDB-ACCT-D1, SDB-QUIT-D1] No claim check: the open gate (not
                // force-editing, both checked above) under a lock the server names our
                // account as holding is the edit session, and its lock is released
                // whether this session took it or adopted it from the account (another
                // window or computer, the website, this app before a restart) - the
                // rule close, switch, logout and quit release by. Done used to release
                // only a lock this session held a claim on (decision RV2-R-13) and
                // finish without one as `LockHeldElsewhere`.
                //
                // Edits are safely on the server; release the lock and go
                // read-only. Drop the edit gate only after a confirmed release -
                // if release fails the lock is still held, so a retry of Done
                // (now a no-op push, dirty already cleared) re-attempts it.
                if let Err(e) = self.speleodb_handle_auth(
                    Endpoint::Release,
                    client::release_lock(&endpoint, &token, insecure_http, &link.project_id),
                ) {
                    // [#576 FZ-SDB-6-D1] A lost reply: re-read the holder at once. Not
                    // ours any more: the release landed, so Done finishes as Released.
                    // Still ours, answered, or unreadable: keep editing and the claim
                    // so a retry of Done re-attempts it (the next poll reconciles).
                    match self.speleodb_reread_after_lock_verb(
                        &endpoint,
                        &token,
                        insecure_http,
                        &link.project_id,
                        &e,
                    ) {
                        super::speleodb_lock_release::AfterLockVerb::NotOurs(holder) => {
                            tracing::info!(
                                project_id = %link.project_id,
                                error = %e,
                                holder = ?holder,
                                purpose = "follow a release whose reply was lost (#576 FZ-SDB-6)",
                                "speleodb done: the release reply was lost, but the lock \
                                 is no longer ours"
                            );
                        }
                        _ => return Err(dto_for_error(Endpoint::Release, &e)),
                    }
                }
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
        self.speleodb_push_only_reported(message, accept_older_version)
            .map(|report| report.result)
    }

    /// [`Self::speleodb_push_only_acknowledged`] with the warnings of the save the
    /// push ran first (#576 NF-W2SAVE-1): what the `speleodb_push` command answers.
    #[expect(
        clippy::result_large_err,
        reason = "the SpeleoErrorDto every SpeleoDB verb returns; boxing the error type is a \
                  repo-wide change outside #576 RV4"
    )]
    pub fn speleodb_push_only_reported(
        &self,
        message: Option<String>,
        accept_older_version: bool,
    ) -> Result<SpeleoPushReport, SpeleoErrorDto> {
        let (result, save_warnings) = self.speleodb_with_pre_push_save(|collected| {
            self.speleodb_push_only_collecting(message, accept_older_version, collected)
        })?;
        Ok(SpeleoPushReport {
            result,
            save_warnings,
        })
    }

    fn speleodb_push_only_collecting(
        &self,
        message: Option<String>,
        accept_older_version: bool,
        save_warnings: &mut pre_push_save::PrePushSave<'_>,
    ) -> Result<PushResult, SpeleoErrorDto> {
        let link = self.speleodb_link_made_for("Push")?;

        // #213: a force-editor holds no lock, so a push would 403. Refuse with the
        // same reconnect hint as Done (the FE never offers push in force-edit).
        if self.force_editing() {
            return Err(local_dto(SpeleoDbError::refused(
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
        let holder =
            self.speleodb_push_lock_holder(&endpoint, &token, insecure_http, &link.project_id)?;
        // [#576 W2S-PUSH] Done's lost-lock rule, for the same reason: the upload needs
        // the lock, so under another account's lock (or none) Push used to attempt it
        // anyway and surface whatever the server made of it, with editing still on.
        self.speleodb_require_our_lock(&link.project_id, holder.as_deref(), "push")?;
        let we_hold_the_lock =
            self.speleodb_authorship_licence(&link.project_id, holder.as_deref());

        match self.speleodb_push(
            &endpoint,
            &token,
            insecure_http,
            &link,
            message,
            we_hold_the_lock,
            save_warnings,
        )? {
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
            return Err(local_dto(SpeleoDbError::refused(
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
            tip.map(str::to_owned),
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
        // [#576 RV4-R-NITS S-8] Replaces the survey: refused in the read-only
        // Plan and Export workspaces, before any network call (as Restore, RV2-R-15).
        self.speleodb_guard_workspace("Discard changes")?;
        if !acknowledged {
            return Err(local_dto(SpeleoDbError::refused(
                "discarding the local copy destroys un-pushed work and must be confirmed"
                    .to_owned(),
            )));
        }
        let link = self.speleodb_link_made_for("Discard changes")?;
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
            self.speleodb_release_current_lock()?;
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
        let link = self.speleodb_link_made_for("Update from SpeleoDB")?;
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
                // A clean FF must not silently strip a lock-holding session's
                // editability, nor launder a lockless force-edit one into an ordinary
                // one (#213) - see `speleodb_capture_editing` for what is captured and
                // why, and `FastForwarded::restore` for what comes back.
                //
                // [#576 B35(a)] The Start editing shape, for the race Start editing
                // documents (`speleodb_commit_editing`): the lock-ops span opens
                // BEFORE the gate is read and stays open through the reload and the
                // restore. Logout and login take the same guard, so a Log out clicked
                // during the download now runs strictly before this arm (the capture
                // below sees the gate it shut) or strictly after it (and shuts the
                // gate this arm restored). Without it, logout closed the gate mid-
                // download and the restore reopened it for a signed-out session.
                //
                // [#576 A8] That order is the types' (`update_restore`): the capture takes
                // the span's proof, the reload is the capture's method, and the restore -
                // which re-reads the account AFTER the reload and reopens only through the
                // identity-checked commit, under the span - is the reload's.
                // [#576 NF-W10BE-2] And ONE span: the capture borrows `lock_ops` and the
                // restore commits through that borrow, so the span cannot end (or be
                // swapped for a fresh one) between them.
                let lock_ops = self.speleodb_lock_ops_guard();
                let capture = self.speleodb_capture_editing(&lock_ops, &link.project_id);
                let local_path = capture
                    .fast_forward(self, &endpoint, &token, insecure_http, &link, tip)
                    .map_err(local_dto)?
                    .restore(self, &endpoint, &token, &link.project_id);
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

    /// Who holds the open project's write lock, read once for a Done or Push (#576
    /// B32): `Ok(None)` is a mutex the server says is unheld, `Ok(Some(user))` names
    /// the holder.
    ///
    /// The holder is half of the authorship licence the push hands `plan_push`
    /// ([`Self::speleodb_authorship_licence`]), the same licence the status poll
    /// reads off its own `get_project`. Routed through the auth handler like every
    /// other call of these verbs, and failing the verb when it cannot be read:
    /// "could not say who holds the lock" must neither license the shortcut nor,
    /// for Done, a release.
    fn speleodb_push_lock_holder(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
    ) -> Result<Option<String>, SpeleoErrorDto> {
        self.speleodb_handle_auth(
            Endpoint::GetProject,
            self.speleodb_lock_holder_checked(endpoint, token, insecure_http, project_id),
        )
        .map_err(|e| dto_for_error(Endpoint::GetProject, &e))
    }

    /// The lost-lock rule Done and Push share (#576 B35(e)-D1, W2S-PUSH): with
    /// someone else - or nobody - on the mutex there is nothing either verb can
    /// finish, since the upload needs the lock and a release would free somebody
    /// else's. So switch editing off - the same rule the status poll applies - keep
    /// any un-pushed edits marked for the next Start editing, and fail with
    /// [`SpeleoDbError::LockNotHeld`] naming the holder.
    fn speleodb_require_our_lock(
        &self,
        project_id: &str,
        holder: Option<&str>,
        verb: &'static str,
    ) -> Result<(), SpeleoErrorDto> {
        if super::speleodb::lock_held_by_me(holder, self.speleodb_my_email().as_deref()) {
            return Ok(());
        }
        self.speleodb_shut_gate_over_lost_lock(project_id);
        tracing::warn!(
            project_id,
            holder = ?holder,
            verb,
            purpose = "never release or push under a lock that is not ours (#576 B35(e))",
            "speleodb: the write lock is not ours; nothing pushed or released"
        );
        Err(local_dto(SpeleoDbError::LockNotHeld {
            holder: holder.map(str::to_owned),
        }))
    }

    /// The push half of [`Self::speleodb_done`]: fence BASE vs the server tip,
    /// recover via ancestry (#193) if the tip moved under a lock THIS session took
    /// (`we_hold_the_lock`, see [`Self::speleodb_authorship_licence`]), then upload
    /// the snapshot + render.
    fn speleodb_push(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        link: &crate::models::SpeleoLink,
        message: Option<String>,
        we_hold_the_lock: bool,
        save_warnings: &mut Vec<ValidationWarning>,
    ) -> Result<PushOutcome, SpeleoErrorDto> {
        // [#576 B6-D1] Never upload a project a newer build saved. Unreachable
        // through the verbs today - `speleodb_start_editing` refuses it, so the
        // gate Done/Push need stays closed - and kept here, at the one upload
        // every push funnels through, so it stays that way.
        if let Some(reason) = self.newer_file_refusal() {
            return Err(local_dto(super::speleodb::newer_file_refused(reason)));
        }
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
        //
        // [#576 B32] Only under the licence: without it a gap commit carrying our
        // email may be another session's on the same account, and adopting it would
        // upload our older copy over it. The poll already calls that gap a conflict.
        match sync::plan_push(
            link.base_commit_sha.as_deref(),
            tip.as_deref(),
            &commits,
            &my_email,
            link.last_edit_at.as_deref(),
            None,
            we_hold_the_lock,
        ) {
            // base == tip: push our edits directly.
            PushPlan::Upload => {}
            // Our prior (ambiguous) push landed; adopt the tip as BASE, then push the
            // current state on top (a 304 if nothing changed since).
            PushPlan::AdoptThenUpload => {
                // [#258] This arm PROVES a prior upload landed, so the remembered
                // choice is stale here just as surely as in the arms that return
                // Confirmed - and it is the reachable one of the two adoption plans.
                // Ahead of the fallible write for the usual reason, and ahead of the
                // upload below because that upload can come back ambiguous and return
                // without concluding anything.
                let landed = self.speleodb_note_prior_push_landed(project_id);
                // [#576 B35(d)] Under `command_lock`, like every link write of the push:
                // an autosave copies the link at its start and renames a fresh file over
                // the live one, so a write landing inside it was lost.
                self.speleodb_adopt_tip_after_landing(&landed, tip, false)?;
            }
            // Forward-compatible only (unreachable while push-start is unpersisted):
            // the prior push landed and no edit followed, so adopt the tip and clear
            // dirty without a redundant re-push.
            PushPlan::AdoptConfirmed => {
                // The prior push DID land; this arm only recognises it late. Concluded
                // ahead of the fallible link write, like the 200 and 304 arms: the
                // recall went stale when that upload landed, so a failure to record
                // the adoption locally must not be what leaves it standing.
                let landed = self.speleodb_revision_landed(project_id);
                // [#576 B35(d)] Under `command_lock`; see the arm above.
                self.speleodb_adopt_tip_after_landing(&landed, tip, true)?;
                return Ok(landed.outcome());
            }
            // A stranger's commit, a gap this session cannot prove it pushed (#576
            // B32), OR a server inconsistency (non-linear / multi-root / a recorded
            // BASE with no tip): never overwrite or guess - surface the divergence
            // prompt (#193 b2 + b5). Stays locked + dirty in speleodb_done.
            PushPlan::Diverged => return Ok(PushOutcome::Diverged),
        }

        // Normal push (Done / Push): fold the SDB-authoritative name pre-push (#216).
        self.speleodb_upload(
            endpoint,
            token,
            insecure_http,
            link,
            message,
            true,
            save_warnings,
        )
    }

    /// The pre-push flush: save the in-memory project to the live `.stalx` the
    /// snapshot is taken from (see [`Self::speleodb_upload`]).
    ///
    /// [#576 NF-W2SAVE-1] A refused or failed save fails the push with the SAVE's
    /// own reason, verbatim - its routing marker included, which the frontend turns
    /// into the same line a refused save gets. It used to be replaced by "failed to
    /// flush project before push", which named none of the save's causes (a newer
    /// file, unread settings, an unreadable side table, a full disk). A successful
    /// save's warnings (what it could not keep) are logged and handed back: the
    /// Done / Push answer carries them so the frontend shows them like a normal
    /// save's (they are one-shot - the save after the push does not repeat them).
    /// [#576 A8] The adoption plans' link write (`AdoptThenUpload`, `AdoptConfirmed`):
    /// BASE moves to the tip a prior upload landed, under `command_lock` like every
    /// link write of the push, and `clear_dirty` clears the dirty bit with it. It takes
    /// the [`Landed`] proof so the invalidation that landing implies cannot be put
    /// after this fallible write (#258).
    #[expect(
        clippy::result_large_err,
        reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                  repo-wide change outside #576 A8"
    )]
    fn speleodb_adopt_tip_after_landing(
        &self,
        _landed: &Landed,
        tip: Option<String>,
        clear_dirty: bool,
    ) -> Result<(), SpeleoErrorDto> {
        let _commands = self.hold_commands();
        // [#576 RVF-c-C1] Never into the link of a project a switch opened meanwhile.
        self.speleodb_refuse_other_project("Recording the upload")?;
        self.speleodb_mutate_link(|l| {
            l.base_commit_sha = tip;
            if clear_dirty {
                l.has_unpushed_edits = false;
                l.last_edit_at = None;
            }
        })
        .map_err(local_dto)
    }

    pub(super) fn speleodb_flush_before_push(
        &self,
    ) -> Result<Vec<ValidationWarning>, SpeleoErrorDto> {
        let flushed = self.save_project(None);
        for warning in &flushed.warnings {
            tracing::warn!(
                code = %warning.code,
                message = %warning.message,
                purpose = "say what the pre-push save could not keep (#576 NF-W2SAVE-1)",
                "speleodb: the pre-push save completed with a warning"
            );
        }
        if !flushed.success {
            // [#576 C4] The save's own refusal, its code included (a coded save
            // refusal routes as that refusal; an unclassified one stays fatal).
            return Err(local_dto(super::speleodb::refused_by_save(
                flushed,
                "the project could not be saved before the push",
            )));
        }
        Ok(flushed.warnings)
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
        save_warnings: &mut Vec<ValidationWarning>,
    ) -> Result<PushOutcome, SpeleoErrorDto> {
        // [#576 A11] Refused rather than recovered after an internal crash, before
        // anything is read or sent: a half-written edit must not become every
        // collaborator's (`refuse_after_crash`). Done and Push both come through here.
        self.refuse_after_crash("Pushing to SpeleoDB")
            .map_err(|refusal| local_dto(super::speleodb::refused_by(refusal)))?;
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
        // The authoritative name, folded below under `command_lock` - the GET itself
        // stays outside it, like every network call here.
        let mut fetched_name: Option<String> = None;
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
                    fetched_name = Some(fetched.name);
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
        // snapshot / render paths in this push's own directory beside it.
        let live = self.speleodb_cache_path(project_id).map_err(local_dto)?;
        let dir = live
            .parent()
            .ok_or_else(|| local_dto(SpeleoDbError::Io("cache path has no parent".to_owned())))?;
        let scratch = PushScratch::create(dir).map_err(|e| {
            local_dto(SpeleoDbError::Io(format!(
                "could not create the push's scratch directory: {e}"
            )))
        })?;
        let snapshot_path = scratch.snapshot();
        let tml = scratch.tml();

        // [#576 B35(d)] The LOCAL half of the push - the name fold, the push-start
        // fence, the flush, the render and the snapshot - under `command_lock`, so an
        // autosave (which runs through `execute`) can neither interleave with Done's own
        // save nor slip an edit between the fence and the snapshot. Never across the
        // network calls: an edit made while the upload runs must keep working, and the
        // push-start fence keeps it (and its undo entry) for the next push.
        let commands = self.hold_commands();
        // [#576 RVF-c-C1] A push made for another project than the open one (a switch
        // landed since the verb read its link) folds, saves and snapshots nothing.
        self.speleodb_refuse_other_project("Pushing to SpeleoDB")?;
        if let Some(name) = fetched_name.as_deref() {
            self.speleodb_fold_local_name(name);
        }

        // Push-start fence (#197): capture the instant BEFORE the flush/snapshot.
        // Any edit recorded after this (last_edit_at > push_start) is NOT in the
        // snapshot we upload, so a confirmed push must NOT clear its dirty bit - it
        // is re-pushed on the next Done instead of being silently dropped. (dirty
        // edits always carry a last_edit_at: mark_remote_dirty stamps both
        // together, so the None branch of edits_captured_by_push is unreachable for
        // a real dirty bit here.)
        let push_start = chrono::Utc::now().to_rfc3339();

        // Flush in-memory state to the live `.stalx` FIRST, THEN render the `.tml`.
        // save_project heals the in-memory project (mints + DEDUPS shot UUIDs and
        // sanitizes non-finite geometry); rendering the `.tml` after that heal is
        // what guarantees the co-pushed `.tml` and the `.stalx` snapshot agree on
        // shot identity + geometry. Rendering first would ship a `.tml` built from
        // the un-healed project for a legacy file carrying within-section duplicate
        // UUIDs or non-finite values (#196).
        let (flushed, flush_losses) = self.speleodb_flush_for_snapshot()?;
        save_warnings.extend(flush_losses);
        self.speleodb_render_tml(&tml).map_err(local_dto)?;

        // Self-contained snapshot of the live `.stalx` (#195), dirty bit cleared in
        // the COPY so it never rides to the server. It takes the flush's proof, so it
        // cannot be taken ahead of the flush (#576 A8).
        let snapshot = push_snapshot::PushSnapshot::take(flushed, &live, snapshot_path)?;

        // Derive policy and history cleanup from the file just captured, never from the
        // live handle or stack. `save_project` can return success with a
        // DATABASE_NOT_REOPENED warning, which deliberately leaves `self.database` empty;
        // consulting `undo_policy()` there classifies a linked Full project as local and
        // strips the history from an upload whose snapshot still says Full.
        // An edit can land after save_project and before this point: it is absent from the
        // snapshot and must retain its undo entry after the confirmed push. Sampling the
        // live stack here included that edit in the cutoff even though it was not uploaded.
        let snapshot_db = crate::persistence::Database::open_for_metadata(snapshot.path())
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

        drop(commands);

        // FATAL if it fails: an unsanitized snapshot still carries the pusher's redo
        // history (and a stale dirty bit) and must never be uploaded. In Disabled mode
        // the undo history is fatal to leak too; in Full mode it is the payload.
        clear_snapshot_dirty(snapshot.path(), keep_undo, inherited_through.as_deref())
            .map_err(local_dto)?;

        let result = self.speleodb_handle_auth(
            Endpoint::UploadAuto,
            snapshot.upload(
                endpoint,
                token,
                insecure_http,
                project_id,
                &tml,
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
                let landed = self.speleodb_revision_landed(project_id);
                self.speleodb_heal_managed_type(
                    &landed,
                    endpoint,
                    token,
                    insecure_http,
                    project_id,
                    server_kind.as_deref(),
                );
                let new_base = pushed.latest_commit.as_ref().map(|c| c.id.clone());
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
                // `parse_upload_auto_response` now populates `latest_commit`
                // SOLELY from the 200 envelope's authoritative top-level
                // `hexsha`. It used to fall back to the response's own
                // eventually-consistent `latest_commit`, which is documented (and
                // was observed live) still pointing at the PRIOR tip right after
                // a push - a stale ancestor that sails through this check and
                // moves BASE backward past every intervening commit, which is the
                // exact failure the guard is here to prevent.
                if new_base.is_none() {
                    tracing::warn!(
                        project_id,
                        "upload 200 carried no commit sha; keeping the previous BASE \
                         rather than resetting it to the empty-repo sentinel"
                    );
                }
                let dropped = self
                    .speleodb_record_landed_upload(
                        &landed,
                        &UploadMadeFor { file: &live, link },
                        new_base.as_deref(),
                        &push_start,
                        &undo_cutoff,
                        snapshot_newest,
                    )
                    .map_err(local_dto)?;
                report_dropped_history(save_warnings, dropped);
                Ok(landed.outcome())
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
                    let landed = self.speleodb_revision_landed(project_id);
                    self.speleodb_heal_managed_type(
                        &landed,
                        endpoint,
                        token,
                        insecure_http,
                        project_id,
                        server_kind.as_deref(),
                    );
                    // BASE unchanged: the tree already matched.
                    let dropped = self
                        .speleodb_record_landed_upload(
                            &landed,
                            &UploadMadeFor { file: &live, link },
                            None,
                            &push_start,
                            &undo_cutoff,
                            snapshot_newest,
                        )
                        .map_err(local_dto)?;
                    report_dropped_history(save_warnings, dropped);
                    return Ok(landed.outcome());
                }
                // Ambiguous (retried-then-exhausted 5xx, or a transport drop): we
                // don't know whether it landed - stay dirty, recover via ancestry
                // on the next attempt.
                let ambiguous = match &e {
                    SpeleoDbError::Status { status, .. } => {
                        map_status(Endpoint::UploadAuto, *status) == SpeleoDbAction::StayDirty
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
                Err(dto_for_error(Endpoint::UploadAuto, &e))
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
    /// successful push, so a spurious 401 here would drop the session - keychain entry
    /// included - over work that already landed.
    fn speleodb_heal_managed_type(
        &self,
        landed: &Landed,
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
            landed,
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

    /// [#576 B35(d)] Record an upload that CONCLUDED (a 200, or a 304 whose tree
    /// already matched) on the open copy: advance BASE to `new_base` when there is
    /// one, clear the dirty bit when no edit was recorded after `push_start`, and cut
    /// the history the commit carried ([`Self::speleodb_clear_undo_after_push`]).
    ///
    /// Under `command_lock`. An autosave runs through `execute`, copies the link at
    /// its start and renames a fresh file over the live one - with the handle out of
    /// the slot in between - so a link write landing inside it either went into the
    /// file about to be replaced (BASE and the dirty bit came back stale: a later
    /// self-conflict) or failed with `NoRemoteProject` (Done reported an error for a
    /// revision that landed, and skipped the history cut so it rode again).
    ///
    /// [#576 Q-W44-6-D1] `made_for` is the file the push was made from and the link it
    /// read there. When a project switch landed after the upload reached the server
    /// but before this write, the open project is another one: the answer stays
    /// `PROJECT_CHANGED` (nothing is written to the project now open, and the verb's
    /// release half does not run), but the landing is still recorded in the link of
    /// the file it was made for ([`Self::speleodb_record_landed_upload_in_file`]), so
    /// that link matches the server. It used to keep its old BASE and dirty mark, and
    /// the next open of that project read our own landed commit as somebody else's.
    pub(super) fn speleodb_record_landed_upload(
        &self,
        _landed: &Landed,
        made_for: &UploadMadeFor<'_>,
        new_base: Option<&str>,
        push_start: &str,
        undo_cutoff: &str,
        captured_through: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Option<ValidationWarning>, SpeleoDbError> {
        let _commands = self.hold_commands();
        // [#576 RVF-c-C1] Never into the link of a project a switch opened meanwhile.
        if let Err(refusal) = self.refuse_other_project("Recording the upload") {
            self.speleodb_record_landed_upload_in_file(
                made_for,
                new_base,
                push_start,
                undo_cutoff,
                captured_through,
            );
            return Err(super::speleodb::refused_by(refusal));
        }
        self.speleodb_mutate_link(|l| record_landing(l, new_base, push_start))?;
        Ok(self.speleodb_clear_undo_after_push(undo_cutoff, captured_through))
    }

    /// [#576 Q-W44-6-D1] Record a concluded upload in the link of `made_for.file`, a
    /// file the session has switched away from: BASE and the dirty mark, as
    /// [`Self::speleodb_record_landed_upload`] records them for the open copy.
    ///
    /// Called under `command_lock` (the caller's), and holds `project` across the
    /// write, so no open, close or switch can commit between the check and the write
    /// (the shape of `speleodb_remove_link_at`). Writes nothing when the file is the
    /// open project's (a session of that very file owns its link now), when it holds
    /// no link or a link to another project or deployment (the path is not that file
    /// any more), or when it cannot be opened. Best-effort: the verb is answered
    /// `PROJECT_CHANGED` either way, and a failure is logged.
    ///
    /// [#303] And the undo history the commit carried, cut from that file's history
    /// by the rule a landed push applies to the open copy's
    /// ([`Self::speleodb_drop_pushed_history_in_file`]). It used to stay in the file,
    /// so the next push of that project carried it a second time.
    ///
    /// [#576 SDB-ACCT-D1] And the lock: the switch's (or close's) silent release ran
    /// while this copy was still dirty, so the un-pushed-edits veto kept the lock.
    /// When the landing leaves the copy clean, that lock is released the way the
    /// silent release would have released a clean copy: the veto's claim is taken
    /// out under the same `project` guard as the write, and the reclaim-checked
    /// release runs detached once the guard is dropped
    /// ([`Self::speleodb_release_kept_lock_detached`]). A lock the user chose to keep
    /// ("Keep lock"), or a copy an edit after the push start left dirty, stays held.
    fn speleodb_record_landed_upload_in_file(
        &self,
        made_for: &UploadMadeFor<'_>,
        new_base: Option<&str>,
        push_start: &str,
        undo_cutoff: &str,
        captured_through: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let open = self.project.lock();
        if open
            .as_ref()
            .and_then(|project| project.file_path.as_deref())
            .is_some_and(|open| super::project_io::same_file(open, made_for.file))
        {
            tracing::info!(
                path = %made_for.file.display(),
                purpose = "record a push whose project was switched away mid-verb (#576 Q-W44-6-D1)",
                "speleodb: the file the push was made for is open again; its session owns the link"
            );
            return;
        }
        let recorded = record_landing_in_file(made_for, new_base, push_start);
        // [#576 Q-W44-6-D1, #303] The undo history the commit carried, cut from that file's
        // history as the open copy's is (`speleodb_clear_undo_after_push`), still
        // under `project`: the file is not open, and no open can read it meanwhile.
        let history = match &recorded {
            Ok(Some((db, _))) => {
                Some(self.speleodb_drop_pushed_history_in_file(db, undo_cutoff, captured_through))
            }
            _ => None,
        };
        let recorded = recorded.map(|landed| landed.map(|(_db, link)| link));
        let kept_lock = match &recorded {
            Ok(Some(link)) if !link.has_unpushed_edits => {
                self.speleodb_take_claim_kept_for_unpushed_edits(&link.project_id, &link.endpoint)
            }
            _ => None,
        };
        drop(open);
        match history {
            Some(Ok(dropped)) => tracing::info!(
                path = %made_for.file.display(),
                dropped,
                purpose = "avoid duplicating pushed history in the next commit (#303)",
                "speleodb: dropped the undo history the landed push carried from the file it \
                 was made for"
            ),
            Some(Err(e)) => tracing::warn!(
                error = %e,
                path = %made_for.file.display(),
                purpose = "avoid duplicating pushed history in the next commit (#303)",
                "speleodb: could not drop the undo history the landed push carried from the \
                 file it was made for; the next push may carry it again"
            ),
            None => {}
        }
        if let Some((params, claim)) = kept_lock {
            tracing::info!(
                project_id = %params.project_id,
                purpose = "release a lock kept over edits a landed push has uploaded (#576 SDB-ACCT-D1)",
                "speleodb: the copy switched away from is clean now; releasing the lock the \
                 silent release kept for its un-pushed edits"
            );
            self.speleodb_release_kept_lock_detached(params, claim);
        }
        match recorded {
            Ok(Some(_)) => tracing::info!(
                path = %made_for.file.display(),
                project_id = %made_for.link.project_id,
                new_base = ?new_base,
                purpose = "record a push whose project was switched away mid-verb (#576 Q-W44-6-D1)",
                "speleodb: recorded the landed push in the link of the project it was made for"
            ),
            Ok(None) => tracing::warn!(
                path = %made_for.file.display(),
                project_id = %made_for.link.project_id,
                purpose = "record a push whose project was switched away mid-verb (#576 Q-W44-6-D1)",
                "speleodb: the file the push was made for no longer holds that project's link; \
                 nothing recorded"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                path = %made_for.file.display(),
                project_id = %made_for.link.project_id,
                purpose = "record a push whose project was switched away mid-verb (#576 Q-W44-6-D1)",
                "speleodb: could not record the landed push in the link of the project it was \
                 made for; its next open reconciles"
            ),
        }
    }

    /// [#303] [`Self::speleodb_clear_undo_after_push`] for the file a landed push was
    /// made from when the session has switched away from it (#576 Q-W44-6-D1): that
    /// file's history - which its pre-push save and any later save wrote - is loaded
    /// as an open would load it, the entries the commit carried and the whole redo
    /// stack are dropped by the same cut ([`crate::core::UndoStack::drop_pushed_entries`],
    /// same `undo_cutoff` and `captured_through`), what remains is written back the way
    /// the push writes the open copy's, and the Unwind chain pointer and the
    /// inherited-history watermark are cleared as the push clears them. Returns how
    /// many undo entries went.
    ///
    /// No crash rule here: it guards the history of THIS session's in-memory stack,
    /// and this file's history was written by the session that had it open, whose
    /// saves applied that rule. A replay mark (`UndoEntry::replayed_at`) is not
    /// persisted, so an entry redone while the upload ran, and saved by the switch, is
    /// cut as one the commit carried.
    fn speleodb_drop_pushed_history_in_file(
        &self,
        db: &crate::persistence::Database,
        undo_cutoff: &str,
        captured_through: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<usize, String> {
        let cutoff = chrono::DateTime::parse_from_rfc3339(undo_cutoff)
            .map_err(|e| format!("could not parse the undo cutoff: {e}"))?
            .with_timezone(&chrono::Utc);
        let link = db.load_speleo_link().map_err(|e| e.to_string())?;
        let budget = crate::persistence::Database::load_history_budget();
        let row_cap = self.history_load_row_cap(link.as_ref());
        let rows = |loaded: Vec<(i64, String, String, Option<String>)>| {
            loaded
                .into_iter()
                .map(|(_, command, undo, stamp)| (command, undo, stamp))
                .collect::<Vec<_>>()
        };
        let undo = db
            .load_undo_stack_with_timestamps_within(row_cap, &budget)
            .map_err(|e| e.to_string())?;
        let redo = db
            .load_redo_stack_with_timestamps_within(row_cap, &budget)
            .map_err(|e| e.to_string())?;
        let watermark = db
            .load_speleo_undo_watermark()
            .map_err(|e| e.to_string())?
            .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(&stamp).ok())
            .map(|stamp| stamp.with_timezone(&chrono::Utc));
        // The bound, the watermark, then the entries: the order the open installs
        // them in (`commit_open_session_into`).
        let mut stack =
            crate::core::UndoStack::new_with_limit(self.undo_stack.lock().history_limit());
        super::undo_policy::arm_undo_depth_limit(&mut stack, self.undo_policy_for(link.as_ref()));
        stack.set_inherited_through(watermark);
        stack.load_from_persisted_with_timestamps(rows(undo), rows(redo));
        let dropped = stack.drop_pushed_entries(cutoff, captured_through);
        if !self.persist_locked_undo_stacks_into(db, &stack) {
            return Err("could not write the remaining history back".to_owned());
        }
        db.save_speleo_undo_chain(None).map_err(|e| e.to_string())?;
        db.save_speleo_undo_watermark(None)
            .map_err(|e| e.to_string())?;
        Ok(dropped)
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
    /// `captured_through` is the newest undo stamp the uploaded snapshot carried (#576
    /// A18(b)): with it the cut can tell an entry the commit holds from one a redo
    /// put back while the upload ran - see [`crate::core::undo::UndoStack::drop_pushed_entries`].
    ///
    /// [#576 RV8a-F4] Returns the `UNDO_HISTORY_DROPPED` warning when the crash rule
    /// below cleared the file's history, so the push can report it the way the save
    /// does (`save_warnings` of the push answer).
    pub(super) fn speleodb_clear_undo_after_push(
        &self,
        undo_cutoff: &str,
        captured_through: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Option<ValidationWarning> {
        let Ok(cutoff) = chrono::DateTime::parse_from_rfc3339(undo_cutoff) else {
            tracing::warn!(
                undo_cutoff,
                purpose = "clear undo history after a confirmed push (#303)",
                "could not parse the undo cutoff; leaving the undo stack alone"
            );
            return None;
        };
        let cutoff = cutoff.with_timezone(&chrono::Utc);
        let database = self.database.lock();
        let mut stack = self.undo_stack.lock();
        let dropped = stack.drop_pushed_entries(cutoff, captured_through);
        let Some(db) = database.as_ref() else {
            tracing::warn!(
                dropped,
                purpose = "persist confirmed-push undo cleanup (#303)",
                "no live database handle; retaining the chain metadata so stale disk rows remain classified"
            );
            return None;
        };
        // [#576 RV6a-1] The save's crash rule (`persist_or_clear_undo_stacks_into`):
        // the upload releases `command_lock` while it is on the network, and an edit
        // that crashed in that window can have left the history out of step with the
        // survey. Writing the remaining entries would hand them to the restarted
        // session, which replays them; the file's history is cleared instead, as the
        // save after a crash does (`UNDO_HISTORY_DROPPED`).
        let mut dropped_history = None;
        let persisted = if self.refuse_after_crash("Saving the Undo history").is_err() {
            match db.clear_history_stacks() {
                Ok(()) => {
                    dropped_history = Some(
                        super::save_sidecars::Sidecar::History
                            .lost_warning(super::save_sidecars::UNDO_HISTORY_DROPPED_CODE),
                    );
                    tracing::warn!(
                        dropped,
                        code = super::save_sidecars::UNDO_HISTORY_DROPPED_CODE,
                        purpose = "never persist a crashed session's history (#576 RV6a-1)",
                        "Cleared the Undo and Redo history from the file after a confirmed push: \
                         an internal error happened in this session"
                    );
                    true
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        purpose = "never persist a crashed session's history (#576 RV6a-1)",
                        "Could not clear the history after a crash"
                    );
                    false
                }
            }
        } else {
            self.persist_locked_undo_stacks_into(db, &stack)
        };
        if !persisted {
            tracing::warn!(
                dropped,
                purpose = "persist confirmed-push undo cleanup (#303)",
                "could not persist the pruned history; retaining the chain metadata so stale disk rows remain classified"
            );
            return dropped_history;
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
        dropped_history
    }
}

/// [#576 RV8a-F4] Report the push cleanup's crash-time history drop in the push answer,
/// once: after a crash the pre-push save has usually said the same thing already.
fn report_dropped_history(
    save_warnings: &mut Vec<ValidationWarning>,
    dropped: Option<ValidationWarning>,
) {
    if let Some(warning) = dropped {
        if !save_warnings.iter().any(|w| w.code == warning.code) {
            save_warnings.push(warning);
        }
    }
}

/// [#576 A8] The pre-push save's collector as a type: Done's and Push's bodies collect
/// the warnings of the save they run before their upload into a [`PrePushSave`], which
/// only [`AppState::speleodb_with_pre_push_save`] makes - so both verbs surface those
/// warnings when the body then fails, by construction.
mod pre_push_save {
    use crate::core::state::AppState;
    use crate::core::ValidationWarning;
    use crate::speleodb::SpeleoErrorDto;

    /// Where a Done / Push body collects the pre-push save's warnings.
    pub(in crate::core::state) struct PrePushSave<'a> {
        warnings: &'a mut Vec<ValidationWarning>,
    }

    impl std::ops::Deref for PrePushSave<'_> {
        type Target = Vec<ValidationWarning>;
        fn deref(&self) -> &Self::Target {
            self.warnings
        }
    }

    impl std::ops::DerefMut for PrePushSave<'_> {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.warnings
        }
    }

    impl AppState {
        /// [#576 RV4-R-S6] Run a Done / Push body that collects the warnings of the save
        /// it runs before its upload, and hand those warnings to the frontend through the
        /// event sink ([`crate::core::events::SpeleoPrePushSave`]) when the body then
        /// FAILS.
        ///
        /// The Ok answer carries them (`SpeleoDoneReport` / `SpeleoPushReport`); an error
        /// cannot. The save did happen, and its warnings are one-shot - the next save does
        /// not repeat them - so a push that fails after it (a refused or dropped upload, a
        /// failed render or snapshot) used to leave the user never told what that save
        /// could not keep. Nothing is sent when the save did not run or lost nothing.
        #[expect(
            clippy::result_large_err,
            reason = "the SpeleoErrorDto every SpeleoDB verb returns; boxing the error type is a \
                      repo-wide change outside #576 RV4"
        )]
        ///
        /// [#576 A8] The body's collector is a [`PrePushSave`], made only here - and the
        /// Done and Push bodies take one - so a verb that ran its body without this helper
        /// does not compile.
        pub(in crate::core::state) fn speleodb_with_pre_push_save<R>(
            &self,
            body: impl FnOnce(&mut PrePushSave<'_>) -> Result<R, SpeleoErrorDto>,
        ) -> Result<(R, Vec<ValidationWarning>), SpeleoErrorDto> {
            let mut collected = Vec::new();
            match body(&mut PrePushSave {
                warnings: &mut collected,
            }) {
                Ok(result) => Ok((result, collected)),
                Err(e) => {
                    self.speleodb_surface_pre_push_save(collected);
                    Err(e)
                }
            }
        }
    }
}

/// [#576 A8] `speleodb_update`'s fast-forward arm as types, every proof made only here:
/// the editing state is captured inside the lock-ops span ([`EditingCapture`]), the
/// in-place reload is the capture's method (so the capture precedes it - the reload
/// shuts the gate, and a poll in that window clears the record), and the restore is the
/// reload's ([`FastForwarded::restore`]): it re-reads the account AFTER the reload and
/// reopens editing only through the identity-checked commit, under the span.
///
/// [#576 NF-W10BE-2] Under the capture's OWN span: the capture borrows the lock-ops
/// span it was taken in, and the restore commits through that borrow rather than a
/// span its caller hands it - so a fresh span opened between the capture and the
/// restore (a logout or login landing in the gap) is a borrow error (E0505), not a
/// reorder a reviewer has to spot. The `AdoptSettled` shape.
mod update_restore {
    use super::super::speleodb_cache::LocalEditsPolicy;
    use super::super::speleodb_session::LockOpsHeld;
    use super::AppState;
    use crate::speleodb::SpeleoDbError;

    /// The editing state as it stood BEFORE the reload: the last point the pre-reload
    /// truth exists. Borrows the span it was read in, which the restore commits under.
    pub(super) struct EditingCapture<'s, 'g> {
        span: &'s LockOpsHeld<'g>,
        /// A genuine lock-holding session: edit-enabled and NOT force-editing.
        was_editing: bool,
        had_lock_record: bool,
        had_resumed_record: bool,
    }

    /// Proof the fast-forward reloaded in place, carrying the capture (and its span).
    pub(super) struct FastForwarded<'s, 'g> {
        capture: EditingCapture<'s, 'g>,
        local_path: String,
    }

    impl AppState {
        /// speleodb_fast_forward reloads in place, which re-derives the edit gate from
        /// is_remote (= false for a remote project). If the caller was mid-edit-session
        /// (gate on, lock held - reachable if the lock lapsed/was admin-released and
        /// another writer advanced the tip), a clean FF must NOT silently strip
        /// editability, so what is captured here is restored after the reload. FF only
        /// fires with dirty == false, so there are no un-pushed edits to strand.
        ///
        /// The PAIR, not `edit_enabled` alone (#213, matching the release verbs): a
        /// force-editor is edit-enabled while holding NO lock, and the reload clears
        /// `force_editing`. Restoring on the gate alone therefore laundered a lockless
        /// force-edit session into an ordinary-looking editing one - editable,
        /// un-flagged, and with the big-red "this .stalx cannot be merged" banner gone.
        /// Only a genuine lock-holding session is restored; a force-editor lands
        /// read-only (nothing to strand - FF requires a clean copy) and re-enters
        /// deliberately via Start editing or Force edit.
        ///
        /// The record is captured SEPARATELY from the gate on purpose - the gate is a
        /// permission ("may I edit?"), the record is provenance ("did I take this
        /// lock?"), and the poll at the bottom of `speleodb_link_status` can re-derive
        /// the gate from nothing but a matching holder email. Restoring the record from
        /// the gate would therefore mint provenance for a lock this process never took.
        /// Of the same kind: a resumed claim stays resumed (#576 RV4-R-S3).
        pub(super) fn speleodb_capture_editing<'s, 'g>(
            &self,
            lock_ops: &'s LockOpsHeld<'g>,
            project_id: &str,
        ) -> EditingCapture<'s, 'g> {
            EditingCapture {
                span: lock_ops,
                was_editing: self.edit_enabled() && !self.force_editing(),
                had_lock_record: self.speleodb_lock_acquired_here(project_id),
                had_resumed_record: self.speleodb_lock_resumed_here(project_id),
            }
        }
    }

    impl<'s, 'g> EditingCapture<'s, 'g> {
        /// The fast-forward itself (`Protect`: an update never discards local work).
        pub(super) fn fast_forward(
            self,
            state: &AppState,
            endpoint: &str,
            token: &str,
            insecure_http: bool,
            link: &crate::models::SpeleoLink,
            tip: Option<String>,
        ) -> Result<FastForwarded<'s, 'g>, SpeleoDbError> {
            let local_path = state.speleodb_fast_forward(
                endpoint,
                token,
                insecure_http,
                link,
                tip,
                LocalEditsPolicy::Protect,
            )?;
            Ok(FastForwarded {
                capture: self,
                local_path,
            })
        }
    }

    #[cfg(test)]
    impl<'s, 'g> FastForwarded<'s, 'g> {
        /// A reload that captured `capture` in `lock_ops`, for the restore's own tests:
        /// production reaches a `FastForwarded` only through
        /// [`EditingCapture::fast_forward`].
        pub(super) fn for_tests(
            state: &AppState,
            lock_ops: &'s LockOpsHeld<'g>,
            project_id: &str,
        ) -> Self {
            Self {
                capture: state.speleodb_capture_editing(lock_ops, project_id),
                local_path: String::new(),
            }
        }
    }

    impl FastForwarded<'_, '_> {
        /// Restore the captured session after the reload, and hand back the local path.
        ///
        /// [#576 B35(a)] Reopen only for the session that ran the download: the span
        /// keeps logout and login out, but a 401 auto-drop on a concurrent call
        /// (`speleodb_forget`) takes no guard and can end the session during the
        /// reload. Then through the commit Start editing uses, which also refuses when
        /// the open project is no longer this one (a switch during the download).
        ///
        /// [#576 NF-W10BE-2] The commit runs under the capture's span, the one the
        /// pre-reload state was read in: there is no span parameter to pass another.
        pub(super) fn restore(
            self,
            state: &AppState,
            endpoint: &str,
            token: &str,
            project_id: &str,
        ) -> String {
            let EditingCapture {
                span: lock_ops,
                was_editing,
                had_lock_record,
                had_resumed_record,
            } = self.capture;
            let same_session =
                state
                    .speleodb_active_account()
                    .is_ok_and(|(now_endpoint, now_token, _)| {
                        now_endpoint == endpoint && now_token == token
                    });
            if was_editing && !same_session {
                tracing::info!(
                    project_id = %project_id,
                    purpose = "never reopen editing for a session that ended mid-refresh",
                    "speleodb update: the session changed during the fast-forward; \
                     leaving the project read-only"
                );
            } else if was_editing {
                match state.speleodb_commit_editing(lock_ops, None, project_id, false) {
                    // Restore the record only where there was one to restore: the
                    // reload shut the gate, and a status poll landing in that
                    // window drops the record (as start-editing's FF arm
                    // re-records for the same reason). A session that held no
                    // record gets the commit's ADOPTED claim (#576 SDB-ACCT-D1): it
                    // edits under the account's lock, so the release verbs hand it
                    // back, but without an authorship claim it never earned.
                    Ok(()) => {
                        if had_lock_record {
                            if had_resumed_record {
                                state.speleodb_note_resumed_lock(project_id);
                            } else {
                                state.speleodb_note_lock_acquired(Some(project_id));
                            }
                        }
                    }
                    Err(e) => tracing::info!(
                        error = %e,
                        project_id = %project_id,
                        purpose = "never reopen editing on a project that changed mid-refresh",
                        "speleodb update: the open project changed during the \
                         fast-forward; leaving it read-only"
                    ),
                }
            }
            self.local_path
        }
    }
}

/// [#576 A8] The push's quiescent-point chain (#195) as types: the VACUUM snapshot
/// takes the flush's proof, and the upload streams only a snapshot taken that way.
/// Moving the snapshot above the flush, or the upload above the snapshot, is a
/// compile error. Both proofs are made only here.
mod push_snapshot {
    use std::path::{Path, PathBuf};

    use super::super::speleodb::local_dto;
    use super::AppState;
    use crate::core::ValidationWarning;
    use crate::speleodb::{client, SpeleoDbError, SpeleoErrorDto};

    /// Proof that the pre-push save flushed the in-memory edits to the live file.
    /// Made only by [`AppState::speleodb_flush_for_snapshot`], below.
    pub(super) struct Flushed {
        _saved: (),
    }

    impl AppState {
        /// [`AppState::speleodb_flush_before_push`] as the upload runs it: the save,
        /// and the [`Flushed`] proof the snapshot requires.
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn speleodb_flush_for_snapshot(
            &self,
        ) -> Result<(Flushed, Vec<ValidationWarning>), SpeleoErrorDto> {
            let saved = self.speleodb_flush_before_push()?;
            Ok((Flushed { _saved: () }, saved))
        }
    }

    /// The self-contained snapshot of the live `.stalx` that the upload streams.
    pub(super) struct PushSnapshot {
        path: PathBuf,
    }

    /// [#576 A8 NF-A8-BYPASS] The upload's licence: `client::upload_auto` takes
    /// it, so a push cannot stream a file that never went through
    /// [`PushSnapshot::take`].
    impl client::UploadLicence for PushSnapshot {}

    impl PushSnapshot {
        /// VACUUM the live file into `path`. Takes the flush's proof: a snapshot of a
        /// file the last edits never reached would upload without them.
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn take(
            _flushed: Flushed,
            live: &Path,
            path: PathBuf,
        ) -> Result<Self, SpeleoErrorDto> {
            crate::persistence::Database::vacuum_snapshot(live, &path)
                .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
            Ok(Self { path })
        }

        pub(super) fn path(&self) -> &Path {
            &self.path
        }

        /// Stream this snapshot (and the `.tml` beside it) as one commit.
        pub(super) fn upload(
            &self,
            endpoint: &str,
            token: &str,
            insecure_http: bool,
            project_id: &str,
            tml: &Path,
            commit_message: &str,
        ) -> Result<crate::speleodb::models::Project, SpeleoDbError> {
            client::upload_auto(
                self,
                endpoint,
                token,
                insecure_http,
                project_id,
                &self.path,
                tml,
                commit_message,
            )
        }
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
            state.config.lock().speleodb.undo_mode = UndoMode::Disabled;
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

    /// Write `bytes` at `path` the way a scratch writer leaves it.
    fn put(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).expect("write a scratch file");
    }

    #[test]
    fn a_finished_push_leaves_an_overlapping_push_its_scratch() {
        // [#576 RVF-b-4] Two pushes of one project overlap: B has written its
        // snapshot and render, and A returns before B's upload opens them. On names
        // shared by every push, A's guard unlinked B's files and B's upload failed
        // with NotFound.
        let cache = TempDir::new().expect("temp dir");
        let a = PushScratch::create(cache.path()).expect("A's scratch");
        let b = PushScratch::create(cache.path()).expect("B's scratch");
        put(&a.snapshot(), b"A snapshot");
        put(&a.tml(), b"A render");
        put(&b.snapshot(), b"B snapshot");
        put(&b.tml(), b"B render");

        drop(a);

        assert_eq!(
            std::fs::read(b.snapshot()).ok(),
            Some(b"B snapshot".to_vec()),
            "dropping A's guard must leave B's snapshot in place"
        );
        assert_eq!(
            std::fs::read(b.tml()).ok(),
            Some(b"B render".to_vec()),
            "dropping A's guard must leave B's render in place"
        );
    }

    #[test]
    fn each_push_scratch_is_its_own_directory_and_its_guard_removes_only_that() {
        let cache = TempDir::new().expect("temp dir");
        let live = cache.path().join("project.stalx");
        put(&live, b"live copy");
        let a = PushScratch::create(cache.path()).expect("A's scratch");
        let b = PushScratch::create(cache.path()).expect("B's scratch");
        assert_ne!(
            a.snapshot(),
            b.snapshot(),
            "two pushes share a snapshot path"
        );
        assert_ne!(a.tml(), b.tml(), "two pushes share a render path");
        assert_eq!(
            (a.snapshot().file_name(), a.tml().file_name()),
            (
                Some(std::ffi::OsStr::new("snapshot.stalx")),
                Some(std::ffi::OsStr::new("ariane.tml"))
            ),
            "the files keep the names the writers and the upload expect"
        );
        let a_dir = a.snapshot().parent().expect("A's dir").to_path_buf();
        let b_dir = b.snapshot().parent().expect("B's dir").to_path_buf();
        assert_eq!(a_dir.parent(), Some(cache.path()), "beside the cache copy");
        put(&a.snapshot(), b"A snapshot");
        put(&a.tml(), b"A render");
        // What a failed SQLite write on the snapshot leaves beside it.
        put(&a_dir.join("snapshot.stalx-journal"), b"journal");

        drop(a);
        assert!(!a_dir.exists(), "the guard removes its own directory");
        assert!(b_dir.is_dir(), "and not the other push's");
        assert_eq!(
            std::fs::read(&live).ok(),
            Some(b"live copy".to_vec()),
            "and not the cache copy"
        );

        drop(b);
        let left: Vec<_> = std::fs::read_dir(cache.path())
            .expect("read the cache dir")
            .map(|e| e.expect("a dir entry").file_name())
            .collect();
        assert_eq!(
            left,
            vec![std::ffi::OsString::from("project.stalx")],
            "only the cache copy is left"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_scratch_guard_removes_a_link_inside_it_without_following_it() {
        let cache = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        let precious = outside.path().join("keep.stalx");
        put(&precious, b"not the push's");
        let scratch = PushScratch::create(cache.path()).expect("scratch");
        let dir = scratch.snapshot().parent().expect("dir").to_path_buf();
        std::os::unix::fs::symlink(outside.path(), dir.join("linked-dir"))
            .expect("plant a directory link");
        std::os::unix::fs::symlink(&precious, scratch.snapshot()).expect("plant a file link");

        drop(scratch);

        assert!(!dir.exists(), "the scratch directory went");
        assert_eq!(
            std::fs::read(&precious).ok(),
            Some(b"not the push's".to_vec()),
            "a link's target outside the scratch survives"
        );
    }

    /// The names in `dir`, sorted.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read the cache dir")
            .map(|e| {
                e.expect("a dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_scratch_sweep_takes_only_push_entries_past_the_threshold() {
        // [#576 RVF-b-4] A crashed push's scratch is not left behind for good: the
        // sweep takes `push-*` entries old enough, and nothing else in the cache dir.
        let cache = TempDir::new().expect("temp dir");
        put(&cache.path().join("project.stalx"), b"live copy");
        put(&cache.path().join("push.log"), b"not scratch");
        std::fs::create_dir(cache.path().join("other")).expect("a non-push dir");
        std::fs::create_dir(cache.path().join("push-crashed")).expect("a push dir");
        put(
            &cache.path().join("push-crashed").join("snapshot.stalx"),
            b"crashed snapshot",
        );
        put(&cache.path().join("push-stray"), b"a push-named file");
        let everything = names_in(cache.path());

        sweep_stale_push_scratch(cache.path(), STALE_PUSH_SCRATCH);
        assert_eq!(
            names_in(cache.path()),
            everything,
            "an entry inside the threshold is kept"
        );

        sweep_stale_push_scratch(cache.path(), std::time::Duration::ZERO);
        assert_eq!(
            names_in(cache.path()),
            vec!["other", "project.stalx", "push.log"],
            "every push-* entry past the threshold went, and only those"
        );
        assert_eq!(
            std::fs::read(cache.path().join("project.stalx")).ok(),
            Some(b"live copy".to_vec()),
            "the live cache copy is untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_new_push_sweeps_a_day_old_scratch_and_keeps_a_recent_one() {
        let cache = TempDir::new().expect("temp dir");
        let old = cache.path().join("push-old");
        let recent = cache.path().join("push-recent");
        std::fs::create_dir(&old).expect("an old push dir");
        put(&old.join("snapshot.stalx"), b"crashed snapshot");
        std::fs::create_dir(&recent).expect("a recent push dir");
        let two_days_ago = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_hours(48))
            .expect("a time two days back");
        std::fs::File::open(&old)
            .expect("open the old push dir")
            .set_modified(two_days_ago)
            .expect("age the old push dir");

        let scratch = PushScratch::create(cache.path()).expect("the new push's scratch");

        assert!(!old.exists(), "the day-old scratch was swept");
        assert!(recent.is_dir(), "a recent push's scratch is kept");
        assert!(
            scratch.snapshot().parent().is_some_and(Path::is_dir),
            "and the new push has its own"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_scratch_sweep_removes_a_push_named_link_without_following_it() {
        let cache = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        let precious = outside.path().join("keep.stalx");
        put(&precious, b"not the push's");
        let link = cache.path().join("push-linked");
        std::os::unix::fs::symlink(outside.path(), &link).expect("plant a push-named link");

        sweep_stale_push_scratch(cache.path(), std::time::Duration::ZERO);

        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "the push-named link was removed"
        );
        assert_eq!(
            std::fs::read(&precious).ok(),
            Some(b"not the push's".to_vec()),
            "and its target was not touched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_scratch_sweep_removes_what_an_rc5_push_left_at_its_fixed_names() {
        // [#576 RVF-d-12] v0.5.0-rc5 pushed through `<cache>/snapshot.stalx` and
        // `<cache>/ariane.tml` and never removed them: a full extra copy of the
        // survey per project, for good. The sweep takes both, whatever their age,
        // and never follows a link at either name.
        let cache = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside dir");
        let precious = outside.path().join("keep.tml");
        put(&precious, b"not the push's");
        put(&cache.path().join("project.stalx"), b"live copy");
        put(
            &cache.path().join("snapshot.stalx"),
            b"an rc5 push snapshot",
        );
        std::os::unix::fs::symlink(&precious, cache.path().join("ariane.tml"))
            .expect("a link at the other rc5 name");

        sweep_stale_push_scratch(cache.path(), STALE_PUSH_SCRATCH);

        assert_eq!(
            names_in(cache.path()),
            vec!["project.stalx"],
            "both rc5 names went, and only those"
        );
        assert_eq!(
            std::fs::read(&precious).ok(),
            Some(b"not the push's".to_vec()),
            "the link's target was not touched"
        );
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test setup failures should stop the regression test immediately"
)]
mod redo_during_push_tests {
    use crate::core::Command;
    use crate::models::ShotData;

    #[test]
    fn a_redo_made_while_the_upload_runs_keeps_its_undo_entry() {
        // [#576 A18(b)] The real redo path, end to end on the stack: undo, the push
        // starts (its cutoff), Ctrl+Y while the upload runs, the upload confirms. The
        // snapshot never held the redone section, so its undo entry has to outlive the
        // post-push cut - before, Ctrl+Z could no longer revert it and the next push
        // uploaded it with no history.
        let state = crate::test_fixtures::state_with_project("CaveA");
        assert!(
            state
                .execute(Command::CreateSection {
                    name: Some("A".to_owned()),
                    surveyor: None,
                    date: None,
                    color: "#abcdef".to_owned(),
                    direction: None,
                    shots: vec![ShotData {
                        length: 5.0,
                        ..Default::default()
                    }],
                })
                .success
        );
        assert!(state.undo().success, "Ctrl+Z before the push");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let cutoff = chrono::Utc::now().to_rfc3339();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(state.redo().success, "Ctrl+Y while the upload runs");

        state.speleodb_clear_undo_after_push(&cutoff, None);

        assert!(
            state.can_undo(),
            "the redone section is not in the upload, so it must stay undoable"
        );
    }
}

/// [#576 A8] The update fast-forward's restore, driven. The three rules it carries used
/// to be source scans of `speleodb_update` (the reload itself needs the app cache dir
/// only the env-gated live tests initialise); the restore is the reload's own step now,
/// so each rule is driven on a remote project as the reload leaves it: the gate shut,
/// force-edit cleared, and - when a status poll lands in that window - the record gone.
#[cfg(test)]
#[expect(
    clippy::print_stdout,
    reason = "the driven A8 tests print the values they observed, so a failure shows them"
)]
mod update_restore_tests {
    use super::update_restore::FastForwarded;
    use crate::core::state::speleodb::{authed_session_at, remote_readonly_fixture, REMOTE_ID};
    use crate::core::state::AppState;

    /// The session the fixture signs in as (`authed_session_at`'s token).
    const AT: &str = "http://x/";
    const TOKEN: &str = "tok-1";

    /// A remote project mid edit session - the gate open, force-edit as asked, our
    /// claim recorded or not - signed in at [`AT`].
    fn session(force_editing: bool, recorded: bool) -> (tempfile::TempDir, AppState) {
        let (temp, state) = remote_readonly_fixture();
        *state.speleodb.lock() = authed_session_at(AT);
        state.set_edit_enabled(true);
        state.set_force_editing(force_editing);
        if recorded {
            state.speleodb_note_lock_acquired(Some(REMOTE_ID));
        }
        (temp, state)
    }

    /// What the in-place reload leaves behind, and a poll landing in its window.
    fn reload(state: &AppState) {
        state.set_edit_enabled(false);
        state.set_force_editing(false);
        let _ = state.speleodb_forget_lock_acquired_for(REMOTE_ID);
    }

    /// The update's fast-forward arm as it runs: the capture, the reload (and `during`,
    /// whatever lands in its window), and the restore, under ONE lock-ops span - the
    /// capture's, which the restore commits through (#576 NF-W10BE-2).
    fn fast_forward(state: &AppState, during: impl FnOnce(&AppState)) {
        let lock_ops = state.speleodb_lock_ops_guard();
        let captured = FastForwarded::for_tests(state, &lock_ops, REMOTE_ID);
        reload(state);
        during(state);
        let _ = captured.restore(state, AT, TOKEN, REMOTE_ID);
    }

    /// A force-editor (edit-enabled while holding NO lock) must not come back out of a
    /// fast-forward as an ordinary editing session: editable, un-flagged, the big-red
    /// "this .stalx cannot be merged" banner gone (#213). A lock-holding one must.
    #[test]
    fn the_update_restores_a_lock_holding_session_and_not_a_force_edit_one() {
        let (_held_dir, held) = session(false, true);
        fast_forward(&held, |_| {});

        let (_forced_dir, forced) = session(true, false);
        fast_forward(&forced, |_| {});

        println!(
            "lock-holding session -> editable {}; force-editor -> editable {}, forcing {}",
            held.edit_enabled(),
            forced.edit_enabled(),
            forced.force_editing()
        );
        assert!(held.edit_enabled(), "a lock-holding session is restored");
        assert!(
            !forced.edit_enabled(),
            "a force-editor lands read-only: it holds no lock to edit under"
        );
    }

    /// The record is provenance, not permission: restored only where one existed, so a
    /// session that inherited its lock (gate open, no claim) comes back editable but
    /// without an authorship claim it never earned - which would let the shortcut in
    /// `speleodb_remote_state` explain another session's commit away as our own.
    ///
    /// [#576 SDB-ACCT-D1] It comes back with the ADOPTED lock's claim instead (a
    /// resumed one): it edits under the account's lock, so the release verbs hand that
    /// lock back.
    #[test]
    fn the_update_restores_the_acquisition_record_only_where_one_existed() {
        let (_owned_dir, owned) = session(false, true);
        fast_forward(&owned, |_| {});

        let (_inherited_dir, inherited) = session(false, false);
        fast_forward(&inherited, |_| {});

        println!(
            "our lock -> editable {}, record {}; an inherited lock -> editable {}, record {}",
            owned.edit_enabled(),
            owned.speleodb_lock_acquired_here(REMOTE_ID),
            inherited.edit_enabled(),
            inherited.speleodb_lock_acquired_here(REMOTE_ID)
        );
        assert!(owned.edit_enabled() && owned.speleodb_lock_acquired_here(REMOTE_ID));
        assert!(inherited.edit_enabled(), "the gate is restored either way");
        assert!(
            !inherited.speleodb_lock_taken_here(REMOTE_ID),
            "but no acquisition is minted for a lock this process never took"
        );
        assert!(
            inherited.speleodb_lock_resumed_here(REMOTE_ID),
            "only the adopted lock's claim, for the release"
        );
        assert!(
            owned.speleodb_lock_taken_here(REMOTE_ID),
            "our own lock keeps its acquisition"
        );
    }

    /// [#576 B35(a)] A session that ended during the reload - a 401 auto-drop takes no
    /// guard - must not get editing back: the restore re-reads the account AFTER the
    /// reload and reopens only for the session that ran the download.
    #[test]
    fn the_update_cannot_reopen_editing_for_a_session_that_ended_mid_reload() {
        let (_dir, state) = session(false, true);
        fast_forward(&state, AppState::speleodb_forget);

        println!(
            "signed out during the reload -> editable {}",
            state.edit_enabled()
        );
        assert!(
            !state.edit_enabled(),
            "editing a remote project for a signed-out session"
        );
    }
}

/// [#195 / #576 RV8a-2] The push's snapshot is a VACUUM of the live file, so the live
/// file must carry the in-memory edits before it: a snapshot of an un-flushed file
/// uploads without the last edits. `Flushed` proves its producer ran before the
/// snapshot; this drives the producer and reads the file it flushed.
#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
#[expect(
    clippy::print_stdout,
    reason = "the driven A8 tests print the values they observed, so a failure shows them"
)]
mod flush_for_snapshot_tests {
    use crate::core::state::AppState;
    use crate::core::Command;
    use crate::models::{Project, ShotData};
    use crate::persistence::Database;

    fn sections_on_disk(path: &std::path::Path) -> usize {
        Database::open_existing(path)
            .expect("open the live file")
            .load_project()
            .expect("read the live file")
            .sections
            .len()
    }

    #[test]
    fn the_snapshot_proof_is_made_only_after_the_live_file_carries_the_edits() {
        let _cfg = crate::test_helpers::isolated_config();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let path = temp.path().join("live.stalx");
        Database::open(&path)
            .expect("create the live file")
            .save_project(&Project::new("CaveA".to_owned(), None))
            .expect("save it");
        let state = AppState::new().expect("state");
        assert!(
            state.open_project(path.clone()).success,
            "open the live file"
        );
        let created = state.execute(Command::CreateSection {
            name: Some("SectionA".to_owned()),
            surveyor: None,
            date: None,
            color: "#336699".to_owned(),
            direction: None,
            shots: vec![ShotData {
                length: 4.0,
                ..Default::default()
            }],
        });
        assert!(created.success, "{:?}", created.error);
        let before = sections_on_disk(&path);

        let flushed = state.speleodb_flush_for_snapshot();
        let after = sections_on_disk(&path);
        println!(
            "sections in the live file: before the flush {before}, after it {after} \
             (flush ok: {})",
            flushed.is_ok()
        );
        assert_eq!(before, 0, "precondition: the edit is in memory only");
        assert!(flushed.is_ok(), "{:?}", flushed.err().map(|e| e.message));
        assert_eq!(
            after, 1,
            "the snapshot's proof must only exist once the live file carries the edit"
        );
    }
}

/// [#576 RV8a-F4] After a crash the push cleanup clears the file's Undo history (the
/// save's crash rule) - and now says so in the push answer, as the save does, instead
/// of only in the log.
#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
#[expect(
    clippy::panic,
    reason = "the crash is a panic while the command lock is held, which poisons it"
)]
#[expect(
    clippy::print_stdout,
    reason = "the driven A8 tests print the values they observed, so a failure shows them"
)]
mod crash_drop_report_tests {
    use super::report_dropped_history;
    use crate::core::state::save_sidecars::UNDO_HISTORY_DROPPED_CODE;
    use crate::core::state::AppState;
    use crate::core::{Command, ValidationWarning, WarningSeverity};
    use crate::models::{Project, ShotData};
    use crate::persistence::Database;

    fn saved_project_with_history(dir: &std::path::Path) -> AppState {
        let path = dir.join("CaveA.stalx");
        Database::open(&path)
            .expect("create the file")
            .save_project(&Project::new("CaveA".to_owned(), None))
            .expect("save it");
        let state = AppState::new().expect("state");
        assert!(state.open_project(path).success, "open it");
        let created = state.execute(Command::CreateSection {
            name: Some("SectionA".to_owned()),
            surveyor: None,
            date: None,
            color: "#336699".to_owned(),
            direction: None,
            shots: vec![ShotData {
                length: 4.0,
                ..Default::default()
            }],
        });
        assert!(created.success, "{:?}", created.error);
        state
    }

    #[test]
    fn the_push_cleanup_reports_the_history_it_dropped_after_a_crash() {
        let _cfg = crate::test_helpers::isolated_config();
        let calm_dir = tempfile::TempDir::new().expect("temp dir");
        let calm = saved_project_with_history(calm_dir.path());
        let cutoff = chrono::Utc::now().to_rfc3339();
        let no_crash = calm.speleodb_clear_undo_after_push(&cutoff, None);

        let crashed_dir = tempfile::TempDir::new().expect("temp dir");
        let crashed = saved_project_with_history(crashed_dir.path());
        let _ = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _guard = crashed.command_lock.lock();
                    panic!("synthetic crash under command_lock (#576 RV8a-F4 test)");
                })
                .join()
        });
        assert!(
            crashed.refuse_after_crash("probe").is_err(),
            "precondition: a crash"
        );
        let after_crash = crashed.speleodb_clear_undo_after_push(&cutoff, None);
        println!(
            "push cleanup -> no crash: {:?}; after a crash: {:?}",
            no_crash.as_ref().map(|w| &w.code),
            after_crash.as_ref().map(|w| &w.code)
        );
        assert!(no_crash.is_none(), "nothing dropped, nothing to report");
        assert_eq!(
            after_crash.map(|w| w.code).as_deref(),
            Some(UNDO_HISTORY_DROPPED_CODE),
            "the dropped history is reported"
        );
    }

    #[test]
    fn a_dropped_history_is_reported_once() {
        let dropped = || ValidationWarning {
            code: UNDO_HISTORY_DROPPED_CODE.to_owned(),
            message: "dropped".to_owned(),
            severity: WarningSeverity::Warning,
        };
        let mut answer = Vec::new();
        report_dropped_history(&mut answer, Some(dropped()));
        report_dropped_history(&mut answer, Some(dropped()));
        report_dropped_history(&mut answer, None);
        assert_eq!(
            answer.len(),
            1,
            "the pre-push save may have said it already"
        );
    }
}
