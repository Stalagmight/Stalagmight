//! Lock release for project close, switch, app exit, and logout.
//!
//! THE INVARIANT: no silent or lifecycle release ever hands the lock back while
//! the open project still has edits SpeleoDB has not seen (`has_unpushed_edits`).
//! Close, switch, quit, the update restart and logout all keep the lock (and mark
//! it kept, so the stranded-claim janitor leaves it alone) - the next open then
//! resumes editing under the same lock instead of meeting a "the server copy
//! differs" prompt. The frontend asks the user first (upload / keep / discard),
//! but this module does not rely on that: a path that skipped the prompt, or
//! cannot show one (an OS-forced exit), lands on the data-safe answer.
//!
//! The one exception is a release that CARRIES the user's discard decision:
//! [`AppState::speleodb_close_discarding_local`] (close with `discard_local`) and
//! the other explicit-abandon verbs (delete, unlink, take-remote), which destroy
//! the local work before the lock goes back.
//!
//! Deferred releases remain scoped to the account, project, and session that
//! initiated them, so they cannot release a successor editing session's lock.
//!
//! THE HOLDER CHECK (lock#4): no release - close, switch, quit, the update
//! restart, logout, an account switch, the explicit-abandon verbs, Start Editing's
//! unwind, the stranded-claim janitor, and the explicit verbs too (Done with
//! nothing to upload, the per-project Release, the stale-context unwind; Switch to
//! Read Only reads the holder itself) - is sent unless SpeleoDB first names this
//! account as the lock's holder ([`release_if_held_by_us`]). Another
//! account's lock, or no lock at all, just ends the local claim. An unreadable
//! holder sends nothing and keeps the claim for a later holder-checked retry;
//! there is no blind fallback. The server lets an admin release anybody's lock,
//! so this check is the only thing standing between an admin's ordinary exit
//! and a colleague's live edit session.
//!
//! NEVER STRAND A LOCK SILENTLY: the releases the user drives - a close, and the
//! settlement a project switch or a quit runs before leaving the project - report
//! what they came to ([`crate::core::LockReleaseOutcome`]) on the reply of the
//! call that released. One that failed or is uncertain closes nothing: the claim
//! stays (unbound, so editing is paused) and the frontend asks "Couldn't hand back
//! the lock" (Retry / Close and keep the lock / Cancel). Only the exits the app
//! cannot intercept still release silently and bounded (`RunEvent::ExitRequested`).

use std::path::Path;
use std::time::Duration;

use super::AppState;
use crate::core::{Command, CommandResult, LockReleaseOutcome, ValidationWarning, WarningSeverity};
use crate::speleodb::client;
use crate::speleodb::SpeleoDbError;

/// What one lock release needs: the server to talk to, the credential, the
/// http-policy allowance, and which project's mutex to drop.
///
/// A struct rather than the `(String, String, bool, String)` this used to be. Three
/// same-typed `String`s around a bare `bool`, destructured positionally at four
/// call sites, is a swap the compiler cannot catch - and swapping `endpoint` with
/// `token` sends the bearer token into a URL, or the project id into the auth
/// header, with a type error nowhere in sight.
#[derive(Clone)]
pub(super) struct LockReleaseParams {
    /// The SpeleoDB base URL this session is authenticated against.
    pub(super) endpoint: String,
    /// The bearer token for that endpoint.
    pub(super) token: String,
    /// The account `token` belongs to: the holder the server must name before
    /// the release is sent (see [`release_if_held_by_us`]). Read in the same
    /// session acquisition as the token, so the two describe one account.
    pub(super) email: String,
    /// The persisted plain-`http://` allowance, threaded to `tls::validate_endpoint`.
    pub(super) insecure_http: bool,
    /// The project whose write lock is being released.
    pub(super) project_id: String,
}

/// Whether a release may proceed over un-pushed local edits.
///
/// The same distinction [`super::speleodb_cache::LocalEditsPolicy`] draws for a
/// download-replace, drawn here for the lock: named at every call site so a reader
/// sees which act they are looking at instead of decoding a bare `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseIntent {
    /// Logout and the account switch: keep a lock protecting unpublished work.
    Silent,
    /// End the open editing session (close, switch, quit, update restart). Like
    /// `Silent`, it KEEPS a lock over un-pushed edits: releasing there is what used
    /// to strand the work - the next open downloaded, refused to overwrite the
    /// local copy, and offered only "take the server copy and discard mine". Both
    /// are decided from the claim, so both may finish an owned claim after the
    /// edit gate has shut; they differ only in what the log says they were.
    Lifecycle,
    /// A DELIBERATE abandon of the project's SpeleoDB identity (`speleodb_unlink`,
    /// `speleodb_take_remote`): the user has said outright they are walking away
    /// from the local work, so there is no future push for the veto to protect.
    Abandon,
}

/// How long a release may spend asking SpeleoDB who holds the lock before it
/// gives up WITHOUT releasing.
///
/// The holder check runs inside the callers' existing deadlines, never on top of
/// them. The tightest is the app-exit / update-restart wait
/// (`EXIT_LOCK_RELEASE_TIMEOUT`, 5 s, in the app crate): this leaves the release
/// POST the rest of it. A healthy round trip takes a fraction of either.
pub(super) const HOLDER_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// What a holder-checked release did. Both are successes: either way this
/// account holds no lock on the project any more, so its claim is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HolderCheckedRelease {
    /// SpeleoDB named this account as the holder, and the release was sent.
    Released,
    /// SpeleoDB named another account, or nobody (including a project it no
    /// longer has): nothing was sent.
    NotHeldByUs,
}

/// Release `params.project_id`'s write lock ONLY if SpeleoDB says the account
/// in `params` holds it (lock#4).
///
/// The server lets an ADMIN release anybody's mutex, so a bare release from a
/// session whose lock was force-unlocked on the web and taken by someone else
/// frees that someone's live lock - and the next writer can lock and push over
/// their work. Asking first is the only defence the client has: SpeleoDB has no
/// compare-and-release, so a change between the probe and the POST remains a
/// protocol limitation, over a window of one round trip.
///
/// A holder that cannot be read is an `Err`, and NOTHING is sent: a release that
/// cannot be shown to be ours is never sent blind. Callers keep their claim on
/// the error (it is the only local pointer to a mutex that may still be ours),
/// so a later holder-checked release can retry it - the no-project janitor on
/// the next status refresh, or the held-lock prompt after the next sign-in when
/// the process is exiting. A 404 is not an error here: a project the server no
/// longer has holds no lock to hand back.
pub(super) fn release_if_held_by_us(
    params: &LockReleaseParams,
) -> Result<HolderCheckedRelease, SpeleoDbError> {
    let holder = match client::get_project_probe_within(
        &params.endpoint,
        &params.token,
        params.insecure_http,
        &params.project_id,
        HOLDER_CHECK_TIMEOUT,
    ) {
        Ok(project) => project.active_mutex.map(|mutex| mutex.user),
        Err(SpeleoDbError::Status { status: 404, .. }) => None,
        Err(e) => return Err(e),
    };
    if !super::speleodb::lock_held_by_me(holder.as_deref(), Some(&params.email)) {
        tracing::info!(
            project_id = %params.project_id,
            held = holder.is_some(),
            purpose = "never release another account's lock (lock#4)",
            "speleodb: not releasing; the server says this account does not hold the lock"
        );
        return Ok(HolderCheckedRelease::NotHeldByUs);
    }
    let released = client::release_lock(
        &params.endpoint,
        &params.token,
        params.insecure_http,
        &params.project_id,
    )?;
    // A 200 is not proof by itself: the reply is the project after the release,
    // and one that still names this account as the holder released nothing. That
    // is an uncertain release, never a success - the lock may well still be ours.
    if super::speleodb::lock_held_by_me(
        released
            .active_mutex
            .as_ref()
            .map(|mutex| mutex.user.as_str()),
        Some(&params.email),
    ) {
        return Err(SpeleoDbError::Parse(
            "the release was answered, but SpeleoDB still names this account as the lock holder"
                .to_owned(),
        ));
    }
    Ok(HolderCheckedRelease::Released)
}

/// The user-facing reason a release of the open project's lock did not happen,
/// carried as the `error` of a result whose `lock_release` is `Failed`.
fn release_failure_message(error: &SpeleoDbError) -> String {
    format!("The SpeleoDB lock could not be handed back: {error}")
}

impl AppState {
    /// Snapshot of what a best-effort lock release needs, or `None` when no held
    /// remote edit session is active - or when releasing would strand local work.
    /// Cheap + non-blocking (reads the open link + config, no network), so a caller
    /// that must not block on the OLD project's server - the app-exit path, a
    /// project switch - can hand the actual HTTP release to a bounded / detached
    /// thread. Reads the link BEFORE the caller tears down or swaps the database.
    pub(super) fn speleodb_held_lock_release_params(&self) -> Option<LockReleaseParams> {
        self.speleodb_lock_release_params(ReleaseIntent::Silent)
    }

    /// The same snapshot WITHOUT the un-pushed-edits veto, for a deliberate
    /// abandon (see [`AppState::speleodb_release_held_lock_abandoning`]).
    pub(super) fn speleodb_abandon_lock_release_params(&self) -> Option<LockReleaseParams> {
        self.speleodb_lock_release_params(ReleaseIntent::Abandon)
    }

    /// Shared body of the snapshots above. `intent` is what separates a silent
    /// release from a deliberate one. A release that should happen but cannot be
    /// attempted (see [`Self::speleodb_lock_release_decision`]) is `None` here, as
    /// it always was for these callers: they have no one to ask.
    fn speleodb_lock_release_params(&self, intent: ReleaseIntent) -> Option<LockReleaseParams> {
        self.speleodb_lock_release_decision(intent).unwrap_or(None)
    }

    /// What a release of the open project's lock needs, `Ok(None)` when there is
    /// nothing to release, or an `Err` when a lock this account may hold cannot be
    /// released now - which the user-driven releases report instead of folding it
    /// into "nothing to release" (a close then stays open, see
    /// [`Self::speleodb_release_held_lock_on_close`]).
    ///
    /// Decided from the CLAIM, never from the edit gate. The gate is a function of
    /// the claim (`speleodb_edit_gate`), so every open gate has one; and a session
    /// whose gate a boundary (logout's bump) or a lost-lock signal has already shut
    /// still holds the claim a release has to end. No claim, no release: a lock
    /// this process cannot account for - another account's, or one this read-only
    /// open never took - is not this path's to hand back.
    fn speleodb_lock_release_decision(
        &self,
        intent: ReleaseIntent,
    ) -> Result<Option<LockReleaseParams>, SpeleoDbError> {
        // A `.ok()?` here used to collapse REAL failures into the ordinary "nothing
        // to release" no-op with no trace at all: `speleodb_open_link` returns
        // `Io("database lock poisoned")` as readily as `NoRemoteProject`. A poisoned
        // mutex at close / switch / quit then stranded the server lock permanently
        // and silently - the exact outcome this module exists to prevent, and
        // indistinguishable in the log from a local project holding no lock. The
        // un-pushed-edits veto below has always logged; so must this.
        let link = match self.speleodb_open_link() {
            Ok(link) => link,
            Err(SpeleoDbError::NoRemoteProject) => return Ok(None),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    ?intent,
                    purpose = "release a held SpeleoDB write lock",
                    "speleodb: could not read the open project's link; the server lock \
                     (if any) is being left held"
                );
                return Err(e);
            }
        };
        // Same reasoning for the claim: an unreadable (poisoned) session cannot say
        // whether this account holds the lock, which is not the same as "it does
        // not".
        let kept = match self.speleodb_open_claim_kept(&link.project_id) {
            Ok(Some(kept)) => kept,
            Ok(None) => return Ok(None),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %link.project_id,
                    ?intent,
                    purpose = "release a held SpeleoDB write lock",
                    "speleodb: could not read the lock claims; the server lock (if any) is \
                     being left held"
                );
                return Err(e);
            }
        };
        // A lock already held back on purpose stays held: an earlier veto, the exit
        // prompt's "Keep the lock", or Start Editing keeping a lock it took because
        // the cache copy holds un-uploaded work the open copy does not show. The
        // open copy being clean says nothing about that work. Only an explicit
        // abandon (or Release, or a new edit session's binding) ends a kept claim.
        if intent != ReleaseIntent::Abandon && kept {
            tracing::info!(
                project_id = %link.project_id,
                ?intent,
                purpose = "honour a lock deliberately kept back",
                "speleodb: keeping the write lock; it was deliberately kept"
            );
            return Ok(None);
        }
        // The invariant (module doc): only an explicit abandon may release over
        // edits the server has not seen. Logout, close, switch, quit and the update
        // restart all keep the lock, so the next open resumes instead of conflicting.
        if intent != ReleaseIntent::Abandon && link.has_unpushed_edits {
            tracing::info!(
                project_id = %link.project_id,
                ?intent,
                purpose = "avoid stranding un-pushed local edits behind a freed lock",
                "speleodb: keeping the write lock; the project has un-pushed edits"
            );
            // A DECISION, not an accident, and the record has to say so: once
            // this project closes, a claim with no project open is exactly what
            // the stranded-claim janitor cleans up - and it would hand back the
            // mutex this veto just protected, freeing the project for the next
            // editor to lock and push over work that is still only local.
            self.speleodb_keep_lock_deliberately(&link.project_id);
            return Ok(None);
        }
        // Same reasoning as the link read above: `speleodb_active_token` returns
        // `Io("SpeleoDB session lock poisoned")` alongside the ordinary
        // `NotAuthenticated`, and a silently-dropped poisoned mutex here strands the
        // lock just as thoroughly. `NotAuthenticated` is not logged as a fault, but
        // it is still an `Err`: the signed-in account holds this claim and has no
        // usable token to hand it back with.
        let (endpoint, token, email) = match self.speleodb_active_account() {
            Ok(account) => account,
            Err(e @ SpeleoDbError::NotAuthenticated) => return Err(e),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %link.project_id,
                    ?intent,
                    purpose = "release a held SpeleoDB write lock",
                    "speleodb: could not read the session token; the server lock is \
                     being left held"
                );
                return Err(e);
            }
        };
        if crate::speleodb::auth::normalize_endpoint(&endpoint)
            != crate::speleodb::auth::normalize_endpoint(&link.endpoint)
        {
            return Ok(None);
        }
        Ok(Some(LockReleaseParams {
            endpoint,
            token,
            email,
            insecure_http: self.speleodb_insecure_http(),
            project_id: link.project_id,
        }))
    }

    /// Whether the signed-in account holds a claim on `project_id` (`None` when it
    /// does not) and whether that claim is kept - or an `Err` when the session
    /// cannot be read at all, which the two predicates read as "no claim".
    fn speleodb_open_claim_kept(&self, project_id: &str) -> Result<Option<bool>, SpeleoDbError> {
        drop(self.speleodb_session_lock()?);
        if !self.speleodb_holds_claim(project_id) {
            return Ok(None);
        }
        Ok(Some(self.speleodb_claim_is_kept(project_id)))
    }

    /// Record that the open project's lock is being kept on purpose (the exit
    /// prompt's "Keep the lock", and the update restart over un-uploaded work).
    ///
    /// Not gated on the edit gate - like the release snapshots, it is about the claim: a reconcile can
    /// shut the gate while this process still owns the claim, and an unmarked
    /// claim with no project open is exactly what the stranded-claim janitor hands
    /// back on the next status refresh - undoing the user's choice. Marking a claim
    /// that does not exist is a no-op, so there is nothing for the gate to protect.
    pub fn speleodb_keep_open_lock_deliberately(&self) {
        if let Ok(link) = self.speleodb_open_link() {
            tracing::info!(
                project_id = %link.project_id,
                purpose = "preserve an explicitly retained lock across an internal close",
                "speleodb: keeping the write lock held after closing the project"
            );
            self.speleodb_keep_lock_deliberately(&link.project_id);
        }
    }

    /// Logout's best-effort release. Saved unpublished work retains its lock;
    /// lifecycle transitions instead call the close/detached/bounded variants.
    pub fn speleodb_release_held_lock_best_effort(&self) {
        if let Some(p) = self.speleodb_held_lock_release_params() {
            self.speleodb_release_best_effort(&p);
        }
    }

    /// The open project's lifecycle release - holder-checked, never over
    /// un-uploaded work, never sent blind - REPORTING what it came to, for the
    /// releases the user drives: a close, and the settlement a project switch or a
    /// quit runs before it leaves the project ([`Self::speleodb_settle_open_lock`]).
    /// Reclaim checks protect a newer session if another open wins the transition.
    ///
    /// - `Ok(None)`: nothing to release (a local project, no claim, a lock kept on
    ///   purpose or over un-uploaded work).
    /// - `Ok(Some(Released | NotHeld))`: the lock is not this account's any more,
    ///   and the claim is over.
    /// - `Err`: the release failed or its outcome is unknown, or it could not be
    ///   attempted (an unreadable link or session, no usable credential). The lock
    ///   may still be ours: the claim stays (put back UNBOUND after a failed
    ///   attempt) and the edit gate is shut, so editing is paused. Nothing is
    ///   closed - the caller reports `Failed` and the user decides.
    pub fn speleodb_release_held_lock_on_close(
        &self,
    ) -> Result<Option<LockReleaseOutcome>, SpeleoDbError> {
        let captured = match self.speleodb_capture_lifecycle_release_checked() {
            Ok(captured) => captured,
            Err(error) => {
                // Not even attempted, and the lock may be ours: pause editing over
                // it exactly as a failed attempt does.
                self.speleodb_shut_edit_gate();
                return Err(error);
            }
        };
        let Some((params, claim, generation)) = captured else {
            return Ok(None);
        };
        match self.speleodb_stand_down_and_release(&params, generation, claim) {
            Ok(HolderCheckedRelease::Released) => Ok(Some(LockReleaseOutcome::Released)),
            Ok(HolderCheckedRelease::NotHeldByUs) => Ok(Some(LockReleaseOutcome::NotHeld)),
            Err(error) => {
                tracing::warn!(
                    %error,
                    project_id = %params.project_id,
                    purpose = "hand back the open project's lock before leaving it",
                    "speleodb: lock release failed; the project stays open and the claim is kept"
                );
                Err(error)
            }
        }
    }

    /// `close_project`'s default: hand the open project's lock back
    /// ([`Self::speleodb_release_held_lock_on_close`]), then close - and only then.
    /// A release that failed or is uncertain closes NOTHING: the result is a
    /// failure whose `lock_release` is `Failed`, the project stays open with
    /// editing paused, and the frontend asks "Couldn't hand back the lock" (Retry,
    /// which is this call again; Close and keep the lock, which is `close_project`
    /// with `release_lock: false`; Cancel).
    pub fn speleodb_close_releasing_lock(&self) -> CommandResult {
        match self.speleodb_release_held_lock_on_close() {
            Ok(outcome) => self
                .execute(Command::CloseProject)
                .with_lock_release(outcome),
            Err(error) => CommandResult::failure(release_failure_message(&error))
                .with_lock_release(Some(LockReleaseOutcome::Failed)),
        }
    }

    /// Settle the open project's lock BEFORE a project switch or a quit leaves it,
    /// without closing anything: the release with its outcome reported
    /// (`release: true`, the same release a close runs), or "Close and keep the
    /// lock" (`release: false`: the claim is marked kept, so neither the switch's
    /// own release nor the exit's hands it back, and the lock waits for the next
    /// sign-in's held-lock prompt).
    ///
    /// A failed or uncertain release fails the call with `lock_release: Failed`;
    /// the switch or quit then waits for the user's answer. A successful one shuts
    /// the edit gate (the claim is over), so a switch that is then cancelled or
    /// fails leaves the project open read-only.
    pub fn speleodb_settle_open_lock(&self, release: bool) -> CommandResult {
        if !release {
            self.speleodb_keep_open_lock_deliberately();
            return CommandResult::success();
        }
        match self.speleodb_release_held_lock_on_close() {
            Ok(outcome) => CommandResult::success().with_lock_release(outcome),
            Err(error) => CommandResult::failure(release_failure_message(&error))
                .with_lock_release(Some(LockReleaseOutcome::Failed)),
        }
    }

    fn speleodb_capture_lifecycle_release(
        &self,
    ) -> Option<(
        LockReleaseParams,
        Option<super::speleodb_session::LockClaim>,
        u64,
    )> {
        self.speleodb_capture_lifecycle_release_checked()
            .unwrap_or(None)
    }

    /// The capture behind every lifecycle release: decided, and the claim taken
    /// out, under the `project` mutex - with a release that cannot be attempted
    /// reported as an `Err` for the callers that tell the user.
    fn speleodb_capture_lifecycle_release_checked(
        &self,
    ) -> Result<
        Option<(
            LockReleaseParams,
            Option<super::speleodb_session::LockClaim>,
            u64,
        )>,
        SpeleoDbError,
    > {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(params) = self.speleodb_lock_release_decision(ReleaseIntent::Lifecycle)? else {
            return Ok(None);
        };
        let claim = self.speleodb_forget_lock_acquired_for(&params.project_id);
        Ok(Some((params, claim, self.speleodb_session_generation())))
    }

    /// Release a held lock for a caller that is ABANDONING the project's SpeleoDB
    /// identity (`speleodb_unlink`): same best-effort release as
    /// [`AppState::speleodb_release_held_lock_best_effort`], minus the
    /// un-pushed-edits veto. Unlink deletes the `speleo_link` row, so a lock kept
    /// back here could not be released from this project again - and the user has
    /// said outright that the managed copy is being walked away from, so there is
    /// no future push for the veto to protect.
    pub(crate) fn speleodb_release_held_lock_abandoning(&self) {
        if let Some(p) = self.speleodb_abandon_lock_release_params() {
            self.speleodb_release_best_effort(&p);
        }
    }

    /// Forget the acquisition record IF it still names `project_id`: one
    /// compare-and-clear under a SINGLE session-lock acquisition.
    ///
    /// For the releases that clear the record at a different moment than they decided
    /// to release it. [`AppState::speleodb_release_captured_lock`] is the reason this
    /// exists: `AppState::delete_project` snapshots the release params under the
    /// `project` mutex, tears the session down, drops that mutex, and only THEN runs
    /// the network release - so there is a real gap between "we decided to hand this
    /// project's lock back" and "we say so in the session". Another SpeleoDB project
    /// can be opened and Started editing inside that gap, and a successful Start
    /// editing records ITSELF in `lock_acquired_here`.
    ///
    /// An unconditional `speleodb_note_lock_acquired(None)` after the gap then erases
    /// the NEW project's record while releasing the OLD project's lock. Nothing looks
    /// broken - the new lock is still held and still editable - but the acquisition
    /// PROVENANCE for it is gone, and that provenance is the entire licence for the
    /// authorship shortcut in `AppState::speleodb_remote_state`: it is what feeds
    /// `we_hold_the_lock` into `crate::speleodb::sync::status_verdict`, which without it
    /// refuses to explain a commit in the `BASE..tip` gap as our own landed push. The
    /// concrete outcome is that OUR OWN next push comes back as
    /// `RemoteState::Conflict` - a self-conflict against our own commit on a project
    /// nobody else touched - with the status mark routing the user's next click into the
    /// divergence dialog, which is exactly the misreport the record was introduced to
    /// stop.
    ///
    /// Comparing project ids is what closes THIS window. The interloper is by
    /// definition a DIFFERENT project - there is one `project` mutex and one open
    /// project, and the old one was torn down before this runs - so the id IS the
    /// signal. The gate is no use at all: after the delete it is shut for the project
    /// being released and OPEN for whatever session took its place, so a gate test
    /// would either never fire or fire on the wrong session's record. (The refresh's
    /// old gate-keyed catch-all clear, which an id comparison could NOT have made
    /// safe, is gone: the gate is now the claim bound to the session, see
    /// `speleodb_edit_gate`.)
    ///
    /// The compare and the clear MUST share one guard. A
    /// `speleodb_lock_acquired_here` read followed by a separate
    /// `speleodb_note_lock_acquired(None)` write is two acquisitions with a gap of
    /// their own, and an acquisition landing in THAT gap is erased just the same: a
    /// narrower window is still the same bug. (Which is why
    /// `AppState::speleodb_release_lock`, the per-project release that had the id
    /// comparison right from the start, clears through here too.)
    ///
    /// A poisoned session lock silently declines to clear, matching
    /// `speleodb_note_lock_acquired`: it costs the shortcut rather than granting one,
    /// since the same poisoned mutex fails the token read the refresh needs to identify a
    /// lock holder at all.
    /// Returns the claim it removed, for the one caller that may have to put it
    /// back: `AppState::delete_project` clears at snapshot time and can still
    /// fail before anything is destroyed.
    pub(super) fn speleodb_forget_lock_acquired_for(
        &self,
        project_id: &str,
    ) -> Option<super::speleodb_session::LockClaim> {
        self.speleodb_forget_lock_acquired_inner(project_id, None)
    }

    /// [`AppState::speleodb_forget_lock_acquired_for`] for the account a release
    /// was captured for, which is not necessarily the one signed in by the time
    /// it runs: an instance switch captures the release, forgets the session,
    /// and only then sends it. Matching the live session there found nothing, so
    /// the claim could neither end on success nor come back on failure.
    pub(super) fn speleodb_forget_claim_released_by(
        &self,
        params: &LockReleaseParams,
    ) -> Option<super::speleodb_session::LockClaim> {
        self.speleodb_forget_lock_acquired_inner(
            &params.project_id,
            Some((&params.endpoint, &params.email)),
        )
    }

    /// Shared body of the two above. `as_account` overrides the identity the claim
    /// is matched against; `None` reads it from the live session.
    ///
    /// One session-lock acquisition either way - the identity is resolved INSIDE
    /// the guard, never read separately and passed in, because a read followed by a
    /// clear is two acquisitions with a gap of their own and an acquisition landing
    /// in that gap is erased just the same.
    fn speleodb_forget_lock_acquired_inner(
        &self,
        project_id: &str,
        as_account: Option<(&str, &str)>,
    ) -> Option<super::speleodb_session::LockClaim> {
        let Ok(mut session) = self.speleodb_session_lock() else {
            return None;
        };
        // The named account's claim on that project, and only that one. The id is
        // what closes this function's race (see above); the account is what keeps
        // the clear from reaching a claim some OTHER account is still owed a
        // release for - callers that pass `None` released with the credentials
        // signed in now, so those are the only claims they can have ended.
        let (endpoint, email) = match as_account {
            Some((endpoint, email)) => (Some(endpoint.to_owned()), Some(email.to_owned())),
            None => (session.endpoint.clone(), session.email.clone()),
        };
        let removed = session
            .locks_acquired_here
            .iter()
            .position(|claim| {
                claim.project_id == project_id
                    && claim.taken_by(endpoint.as_deref(), email.as_deref())
            })
            .map(|at| session.locks_acquired_here.remove(at));
        // The unwind slot names a claim too, and a released lock is no longer an
        // acquisition anything may hand back - leaving it would let the
        // stale-context unwind release this mutex a second time, which on a
        // project since re-locked by a sibling session is a release of THEIR
        // lock.
        if session
            .acquired_by_this_flow
            .as_ref()
            .is_some_and(|flow| removed.as_ref().is_some_and(|gone| flow.is_same_lock(gone)))
        {
            session.acquired_by_this_flow = None;
        }
        removed
    }

    /// Run the network half of a release for params captured EARLIER, and REPORT the
    /// outcome instead of only logging it.
    ///
    /// Every verb above snapshots and releases in one breath, which suits any caller
    /// still holding the database handle the `speleo_link` row is read from. The
    /// project DELETE (`AppState::delete_project`) can do neither:
    ///
    /// - It must snapshot while the handle is alive but release after its own
    ///   teardown has dropped that handle - and after it has released the `project`
    ///   mutex, because an HTTP round trip inside that critical section would stall
    ///   every other state-layer operation for its duration. So the two halves are
    ///   necessarily separate calls there, with
    ///   [`AppState::speleodb_abandon_lock_release_params`] as the first one.
    /// - It has to surface a failure to the user, not just the log. Every other
    ///   caller leaves a lock whose `.stalx` (and link row) survive, so the release
    ///   is retryable from the project itself; a delete destroys exactly that, and
    ///   the user's only remaining route to the stranded mutex is the held-lock
    ///   prompt - which they will not look for unless they are told.
    ///
    /// The ABANDON snapshot is the one to pair this with: a delete forfeits any
    /// un-pushed edits by definition, so the veto that protects them on
    /// close / quit / switch has nothing left to protect and would only strand the
    /// server lock.
    ///
    /// The same deferral is why the acquisition-record clear below is CONDITIONAL,
    /// unlike the one every other release verb makes - see
    /// [`AppState::speleodb_forget_lock_acquired_for`].
    pub(super) fn speleodb_release_captured_lock(
        &self,
        params: &LockReleaseParams,
    ) -> Result<(), SpeleoDbError> {
        // The capturing session's own claim was already cleared - by
        // `delete_project`, under the SAME `project` guard that snapshotted these
        // params and decided the file's identity. So a claim naming this project
        // when the release finally runs can only be a SUCCESSOR session's (a
        // Start Editing that slipped into the gap and inherited the mutex via the
        // server's same-account re-acquire no-op), and the reclaim-checked
        // release stands down for it instead of freeing the lock out from under
        // a live edit session (F8). Serialized under the lock-ops mutex, so the
        // check and the POST cannot interleave with an acquire -> commit span.
        self.speleodb_release_unless_reclaimed(params)
    }

    /// Snapshot the release params, then run the network release on a detached
    /// thread, returning a receiver for its outcome (or `None` when no lock is held,
    /// so nothing was spawned). Because the params are captured synchronously first,
    /// the release targets the CURRENT project even if the caller swaps the database
    /// immediately after. Shared by the two non-blocking callers below.
    ///
    /// The POST is RECLAIM-CHECKED, like the delete's captured release and the
    /// janitor's, and for the same reason: this thread can be delayed
    /// arbitrarily, and the project switch that spawned it is exactly the event
    /// that lets the user reopen that same remote project. A Start Editing on it
    /// INHERITS the still-held same-account mutex (a re-acquire is a server-side
    /// no-op) and binds only an unauthored claim, so an unguarded POST landing afterwards frees
    /// the mutex out from under a live edit session - editable, with no server
    /// lock behind it. Going through
    /// [`AppState::speleodb_release_unless_reclaimed`] puts this thread in the
    /// same `speleodb_lock_ops` queue as every other lock operation, so it
    /// either completes before that acquire or sees the session it would have
    /// stranded and stands down.
    ///
    /// A detached thread cannot borrow `self`, so it goes back in through the
    /// event sink's `with_app_state` - the one `'static` route to managed
    /// state. Without a sink (unit tests, pre-`setup`) there is no managed
    /// state to consult and the raw POST is what this always did.
    fn speleodb_spawn_lock_release(
        &self,
    ) -> Option<std::sync::mpsc::Receiver<Result<(), SpeleoDbError>>> {
        // WHAT is being released, WHOSE session is releasing it, and the claim
        // that said so - captured as ONE transition, under the `project` mutex.
        //
        // Separately, they can describe different worlds. Every session boundary
        // bumps the generation under this mutex, so a close-and-reopen slipping
        // between the claim clear and the generation read left `entry_gen`
        // naming the SUCCESSOR: the deferred release would then find its
        // generation "unchanged", shut that successor's gate, see neither a
        // claim nor a live session, and free the mutex the successor had just
        // inherited. Holding the mutex across all three makes them one fact.
        // (`delete_project` snapshots its release params under this same guard,
        // for the same reason.)
        //
        // Taking the claim out here also shuts the gate (it IS the claim bound to
        // the session, `speleodb_edit_gate`): nothing is edited under a lock that
        // is about to be handed back, and no status refresh can reopen it - only
        // Start Editing binds a claim. The release's own lock-ops span says so
        // again before it decides (`speleodb_stand_down_and_release`).
        let (p, claim, entry_gen) = self.speleodb_capture_lifecycle_release()?;
        let (tx, rx) = std::sync::mpsc::channel();
        let sink = self.get_event_sink();
        std::thread::spawn(move || {
            let ran = sink.is_some_and(|sink| {
                // Clones of what the fallback still needs: a `false` return
                // means the closure was never run, and Rust does not un-move
                // on the not-called path.
                let tx = tx.clone();
                let p = p.clone();
                sink.with_app_state(Box::new(move |state| {
                    let _ = tx.send(
                        state
                            .speleodb_stand_down_and_release(&p, entry_gen, claim)
                            .map(|_| ()),
                    );
                }))
            });
            if !ran {
                // No managed state to consult (unit tests, pre-`setup`): no
                // reclaim check is possible, but the holder check still is - a
                // release is never sent for a lock the server does not say is ours.
                let _ = tx.send(release_if_held_by_us(&p).map(|_| ()));
            }
        });
        Some(rx)
    }

    /// Release a held write lock WITHOUT blocking the caller: fire the network
    /// release on a detached thread and return immediately. For a project switch,
    /// where waiting would stall opening the NEXT project on the OLD project's
    /// (possibly unreachable) server (fix M1). No-op when no lock is held.
    pub(crate) fn speleodb_release_held_lock_detached(&self) {
        // Drop the receiver: the release still runs on its thread (best-effort); we
        // just don't wait for or read the result.
        let _ = self.speleodb_spawn_lock_release();
    }

    /// Release a held write lock with a hard deadline, for the app-exit path: run
    /// the network release on a detached thread and wait at most `deadline`. A hung
    /// / unreachable server must not freeze quit for the full HTTP timeout; on
    /// timeout we return and let the process exit. (fix M2). No-op when no lock is
    /// held, and - the invariant - when the open project has un-pushed edits.
    ///
    /// Also the update restart's release (`install_app_update_and_restart`):
    /// `app.restart()` and the Windows installer's own exit never raise
    /// `RunEvent::ExitRequested`, so that path calls this BEFORE installing.
    pub fn speleodb_release_held_lock_bounded(&self, deadline: Duration) {
        let Some(rx) = self.speleodb_spawn_lock_release() else {
            return;
        };
        match rx.recv_timeout(deadline) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(
                error = %e,
                purpose = "release held lock on exit",
                "speleodb: exit-time lock release failed"
            ),
            Err(_) => tracing::warn!(
                purpose = "release held lock on exit",
                "speleodb: exit-time lock release timed out; exiting anyway"
            ),
        }
    }

    /// The exit prompt's confirmed "Discard changes": throw the local copy away,
    /// hand the lock back, and close - the one lifecycle exit that may release
    /// over un-pushed edits, because the caller carries the user's decision
    /// (`close_project`'s `discard_local`).
    ///
    /// Goes through [`AppState::delete_project`] rather than a release of its own:
    /// that verb closes the session, removes the `.stalx`, and only THEN hands the
    /// lock back under the explicit-abandon policy - so a removal that fails can
    /// never leave un-pushed work behind a freed lock. The next open finds no local
    /// copy and downloads the server's current revision with nothing to refuse.
    ///
    /// Refuses before touching anything unless the open project is a SpeleoDB
    /// project at its canonical cache path: "discard" means "the next open gets the
    /// server copy", which is only true of the copy the app manages.
    pub fn speleodb_close_discarding_local(&self) -> CommandResult {
        match super::speleodb_local_state::cache_root() {
            Some(root) => self.speleodb_close_discarding_local_in(&root),
            None => CommandResult::failure(
                "Could not find Stalagmight's SpeleoDB storage. Nothing was changed.".to_owned(),
            ),
        }
    }

    /// [`AppState::speleodb_close_discarding_local`] against an explicit cache root,
    /// so tests can stand a managed copy up in a temp directory.
    pub(super) fn speleodb_close_discarding_local_in(&self, root: &Path) -> CommandResult {
        let Some(path) = self.open_project_path() else {
            return CommandResult::failure("No project is open.".to_owned());
        };
        let link = match self.speleodb_open_link() {
            Ok(link) => link,
            Err(e) => {
                return CommandResult::failure(format!(
                    "This project is not a SpeleoDB project, so there is nothing to discard: {e}"
                ))
            }
        };
        if !super::speleodb_local_state::is_the_cached_copy(root, &link.project_id, &path) {
            return CommandResult::failure(
                "Only the SpeleoDB copy that Stalagmight manages can be discarded. Nothing was changed."
                    .to_owned(),
            );
        }
        tracing::info!(
            project_id = %link.project_id,
            purpose = "discard un-uploaded local changes on exit (user confirmed)",
            "speleodb: discarding the local copy and releasing the write lock"
        );
        // Into the trash under the project's name, like every removal of a managed
        // copy (the open file's own link names it; no second read of the file).
        let trash_name = super::speleodb_cached::trash_file_name(&link.project_name);
        let result = self.delete_project_as(path, trash_name.as_deref());
        if result.success || result.session_closed != Some(true) {
            return result;
        }
        // The session is gone but the file (and its edits) is not, and
        // `delete_project` put the lock claim back without releasing. That is the
        // "Keep the lock" end state, so say so on the claim - otherwise the janitor
        // would hand the lock back over the very edits that survived - and report a
        // CLOSE that succeeded, with the discard failure as a warning: the frontend
        // must follow the backend to "no project open".
        self.speleodb_keep_lock_deliberately(&link.project_id);
        let reason = result.error.unwrap_or_default();
        CommandResult::success()
            .with_warnings(vec![ValidationWarning {
                code: "speleodb_discard_failed".to_owned(),
                message: format!(
                    "Your changes could not be removed from this computer ({reason}). They are \
                     still there, and the project stays reserved for you on SpeleoDB."
                ),
                severity: WarningSeverity::Warning,
            }])
            .with_session_closed(true)
    }

    /// Sign-out's per-project "Discard" for a cached copy that is NOT open: remove
    /// the managed copy of `project_id` (and its recents row) so no un-uploaded
    /// work is left behind a sign-out the user confirmed.
    ///
    /// Releases nothing itself: the copy is not open, so there is no session to
    /// end, and the sign-out releases every remaining lock of the account
    /// explicitly once the user's choices are done. Refuses the OPEN project - its
    /// discard is the close-with-discard (`close_project`'s `discard_local`) - and
    /// asks that again under the project lock at the delete itself, so a copy
    /// opened after the check is refused rather than closed and trashed. Refuses
    /// as well a `.stalx` no longer linked to SpeleoDB and a project directory
    /// that is a symlink (`speleodb_cached`).
    ///
    /// The consent is checked HERE, not only in the dialog: a copy with un-uploaded
    /// (or unreadable) work is removed only when `discard_unpushed` carries the
    /// user's explicit Discard, and a copy the session is not the account of
    /// (another account's or instance's, or no session at all) is refused
    /// whatever the flag says (`AppState::cached_copy_discard_refusal`).
    pub fn speleodb_discard_cached_copy(
        &self,
        project_id: &str,
        discard_unpushed: bool,
    ) -> CommandResult {
        match super::speleodb_local_state::cache_root() {
            Some(root) => self.speleodb_discard_cached_copy_in(&root, project_id, discard_unpushed),
            None => CommandResult::failure(
                "Could not find Stalagmight's SpeleoDB storage. Nothing was changed.".to_owned(),
            ),
        }
    }

    /// [`AppState::speleodb_discard_cached_copy`] against an explicit cache root.
    pub(super) fn speleodb_discard_cached_copy_in(
        &self,
        root: &Path,
        project_id: &str,
        discard_unpushed: bool,
    ) -> CommandResult {
        if let Err(e) = super::speleodb::validate_project_id(project_id) {
            return CommandResult::failure(e.to_string());
        }
        if let Some(refusal) = self.cached_copy_discard_refusal(root, project_id, discard_unpushed)
        {
            return CommandResult::failure(refusal);
        }
        tracing::info!(
            project_id = %project_id,
            discard_unpushed,
            purpose = "discard un-uploaded changes before signing out (user confirmed)",
            "speleodb: discarding a cached copy"
        );
        self.discard_cached_copy_files(root, project_id)
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit test fixtures fail loudly on setup errors; the panic POISONS a mutex on purpose"
)]
mod tests {
    use super::AppState;
    use crate::models::{Project, SpeleoLink};
    use crate::persistence::Database;
    use crate::speleodb::auth::TokenStore;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    const PID: &str = "22222222-2222-2222-2222-222222222222";

    /// A loopback SpeleoDB on which the fixture account (`u@x`) holds [`PID`]'s
    /// lock, recording each request line so a test can assert whether a release
    /// reached the server. Never blocks a test that expects no traffic, unlike a
    /// one-shot stub whose `join` would wait forever.
    fn recording_server() -> (String, Arc<Mutex<Vec<String>>>) {
        holder_server(Some("u@x"))
    }

    /// [`recording_server`] with the lock held by `holder` instead (or by nobody).
    fn holder_server(holder: Option<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
        crate::core::state::tests_support::lock_server(PID, holder.map(str::to_owned))
    }

    fn release_requests(seen: &Arc<Mutex<Vec<String>>>) -> usize {
        crate::core::state::tests_support::release_posts(seen)
    }

    /// An editing session on SpeleoDB project [`PID`] whose copy lives at the
    /// managed cache path `<root>/<PID>/project.stalx`, signed in against
    /// `endpoint`, with this process holding the lock claim.
    fn managed_editing_session(endpoint: &str) -> (tempfile::TempDir, PathBuf, PathBuf, AppState) {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path().join("speleodb");
        let path = super::super::speleodb_local_state::cache_path_in(&root, PID);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("cache dir");
        let mut db = Database::open(&path).expect("open db");
        db.save_project(&Project::new("Remote".to_owned(), None))
            .expect("save project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: endpoint.to_owned(),
            project_id: PID.to_owned(),
            project_name: "Remote".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        }))
        .expect("save link");
        drop(db);
        let state = AppState::new().expect("state");
        assert!(
            state.open_project(path.clone()).success,
            "open managed copy"
        );
        let mut token_store = TokenStore::default();
        token_store.store(endpoint, "u@x", "tok-1");
        *state.speleodb.lock().expect("session") = super::super::speleodb::SpeleoDbSession {
            endpoint: Some(endpoint.to_owned()),
            email: Some("u@x".to_owned()),
            token_store,
            rehydrate_attempted: true,
            locks_acquired_here: Vec::new(),
            acquired_by_this_flow: None,
        };
        state.set_edit_enabled(true);
        state.speleodb_note_lock_acquired(Some(PID));
        (temp, root, path, state)
    }

    /// The file names this thread handed to the trash since the last call.
    fn trashed_names() -> Vec<String> {
        super::super::recent_projects::take_trashed_for_tests()
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect()
    }

    /// Stamp the open copy's dirty bit directly, as a fixture: the forward-edit
    /// tripwire (`speleodb_revisions`) keeps production writers on
    /// `mark_remote_forward_edit`, and this is setup, not a writer.
    fn set_unpushed_edits(state: &AppState) {
        state
            .database
            .lock()
            .expect("database")
            .as_ref()
            .expect("open")
            .touch_speleo_link_dirty("2026-11-09T10:00:00Z")
            .expect("stamp the dirty bit");
    }

    fn claim_is_kept(state: &AppState) -> bool {
        state
            .speleodb
            .lock()
            .expect("session")
            .locks_acquired_here
            .iter()
            .any(|claim| claim.project_id == PID && claim.is_kept())
    }

    /// Give a detached / bounded release thread time to reach the server, so an
    /// absence assertion is not just a race the release had not run yet.
    fn settle() {
        std::thread::sleep(std::time::Duration::from_millis(400));
    }

    type Release = fn(&AppState);

    fn every_lifecycle_release() -> [(&'static str, Release); 5] {
        [
            ("close", |state| {
                let _ = state.speleodb_release_held_lock_on_close();
            }),
            ("switch", AppState::speleodb_release_held_lock_detached),
            ("exit / update restart", |state| {
                state.speleodb_release_held_lock_bounded(std::time::Duration::from_secs(5));
            }),
            ("logout", AppState::speleodb_release_held_lock_best_effort),
            // A settings save or reload that moves to another account or instance:
            // the release is captured under the settings lock and sent after it.
            ("account switch", |state| {
                if let Some(params) = state.speleodb_logout_drop_gates_deferring_release() {
                    state.speleodb_release_best_effort(&params);
                }
            }),
        ]
    }

    /// Whether the server was asked who holds the lock.
    fn holder_reads(seen: &Arc<Mutex<Vec<String>>>) -> usize {
        seen.lock()
            .expect("log")
            .iter()
            .filter(|line| line.starts_with("GET") && line.contains(PID))
            .count()
    }

    #[test]
    fn no_lifecycle_release_frees_a_lock_this_account_does_not_hold() {
        // lock#4: the server lets an ADMIN release anybody's mutex. A session whose
        // lock was force-unlocked on the web and then taken by someone else used to
        // POST the release anyway on close / quit / switch / logout - freeing the
        // new holder's live lock for the next writer to lock and push over. Every
        // lifecycle path now asks first, and sends nothing for a lock held by
        // another account or by nobody; the local claim simply ends.
        for (name, release) in every_lifecycle_release() {
            for holder in [Some("someone.else@x"), None] {
                let (server, seen) = holder_server(holder);
                let (_temp, _root, _path, state) = managed_editing_session(&server);
                release(&state);
                settle();
                assert!(
                    holder_reads(&seen) >= 1,
                    "{name} / {holder:?}: the holder was asked first"
                );
                assert_eq!(
                    release_requests(&seen),
                    0,
                    "{name} / {holder:?}: nothing was released"
                );
                assert!(
                    !state.speleodb_lock_acquired_here(PID),
                    "{name} / {holder:?}: the local claim is over"
                );
            }
        }
    }

    #[test]
    fn an_unreadable_holder_is_never_released_blind() {
        // When the holder cannot be read, the release is NOT sent - never a blind
        // fallback. Where the caller is still around to keep it (close, logout,
        // account switch), the claim survives as the pointer a later
        // holder-checked retry needs: the no-project janitor, or the held-lock
        // prompt after the next sign-in.
        for (name, release) in every_lifecycle_release() {
            let (server, seen) = crate::core::state::tests_support::lock_server_unreadable(PID);
            let (_temp, _root, _path, state) = managed_editing_session(&server);
            release(&state);
            settle();
            assert!(holder_reads(&seen) >= 1, "{name}: the holder was asked");
            assert_eq!(release_requests(&seen), 0, "{name}: nothing sent blind");
            if !matches!(name, "switch" | "exit / update restart") {
                assert!(
                    state.speleodb_lock_acquired_here(PID),
                    "{name}: the claim is kept for a later retry"
                );
            }
        }
    }

    #[test]
    fn the_exit_holder_check_fits_inside_the_exit_budget() {
        // A server that accepts the connection and never answers: the holder read
        // must give up inside the exit wait (5 s) rather than eat all of it, and
        // giving up must not turn into a release.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let server = format!(
            "http://127.0.0.1:{}",
            silent.local_addr().expect("addr").port()
        );
        let (_temp, _root, _path, state) = managed_editing_session(&server);
        let started = std::time::Instant::now();
        state.speleodb_release_held_lock_bounded(std::time::Duration::from_secs(5));
        let waited = started.elapsed();
        assert!(
            waited < std::time::Duration::from_millis(4_500),
            "the holder read is bounded by HOLDER_CHECK_TIMEOUT, not the exit wait: {waited:?}"
        );
        assert!(
            waited
                >= super::HOLDER_CHECK_TIMEOUT
                    .saturating_sub(std::time::Duration::from_millis(200)),
            "and it really waited for the read rather than skipping it: {waited:?}"
        );
        drop(silent);
    }

    #[test]
    fn no_lifecycle_release_hands_the_lock_back_over_unpushed_edits() {
        // THE invariant. Each of these used to release (all but logout) while the
        // copy still held edits SpeleoDB had never seen; the next open then
        // downloaded, refused to overwrite the local copy, and offered only "take
        // the server copy and discard mine". Every one must now keep the lock AND
        // mark the claim kept, or the stranded-claim janitor hands it back later.
        for (name, release) in every_lifecycle_release() {
            let (server, seen) = recording_server();
            let (_temp, _root, path, state) = managed_editing_session(&server);
            set_unpushed_edits(&state);
            let before = std::fs::read(&path).expect("bytes");
            release(&state);
            settle();
            assert_eq!(
                release_requests(&seen),
                0,
                "{name}: no release reached the server"
            );
            assert!(
                state.speleodb_lock_acquired_here(PID),
                "{name}: the claim survives"
            );
            assert!(
                claim_is_kept(&state),
                "{name}: and is marked kept for the janitor"
            );
            assert_eq!(
                std::fs::read(&path).expect("bytes"),
                before,
                "{name}: copy untouched"
            );
            assert!(
                state.speleodb_open_link().expect("link").has_unpushed_edits,
                "{name}: the edits are still marked un-uploaded"
            );
        }
    }

    #[test]
    fn a_clean_session_is_still_released_on_every_lifecycle_path() {
        // The invariant must not turn into "never release": with nothing
        // un-uploaded, close / switch / exit / logout hand the lock back as before.
        for (name, release) in every_lifecycle_release() {
            let (server, seen) = recording_server();
            let (_temp, _root, _path, state) = managed_editing_session(&server);
            release(&state);
            settle();
            assert_eq!(
                release_requests(&seen),
                1,
                "{name}: one release reached the server"
            );
            assert!(
                !state.speleodb_lock_acquired_here(PID),
                "{name}: the claim is over"
            );
        }
    }

    #[test]
    fn a_close_hands_the_lock_back_once_and_leaves_the_saved_file_as_is() {
        // #668 scenario: a clean close releases through the holder check, then
        // closes, and the saved project file is byte-identical afterwards.
        let (server, seen) = recording_server();
        let (_temp, _root, path, state) = managed_editing_session(&server);
        let before = std::fs::read(&path).expect("saved project");
        let closed = state.speleodb_close_releasing_lock();
        assert!(closed.success);
        assert_eq!(
            closed.lock_release,
            Some(crate::core::LockReleaseOutcome::Released)
        );
        assert_eq!(release_requests(&seen), 1);
        assert!(holder_reads(&seen) >= 1, "asked who holds the lock first");
        assert!(state.get_project_state().is_none());
        assert_eq!(std::fs::read(&path).expect("file remains"), before);
    }

    #[test]
    fn closing_a_local_project_needs_no_server() {
        // #668 scenario: a project with no SpeleoDB link closes without any
        // network call or credentials.
        let state = AppState::new().expect("state");
        state.set_project_for_tests(Project::new("Synthetic local".into(), None));
        let closed = state.speleodb_close_releasing_lock();
        assert!(closed.success);
        assert_eq!(closed.lock_release, None, "nothing was released");
        assert!(state.get_project_state().is_none());
    }

    #[test]
    fn a_shut_gate_does_not_unlock_the_veto_for_an_owned_claim() {
        // Lifecycle may finish an owned claim after a reconcile shut the edit gate -
        // but the un-pushed-edits veto applies there too.
        let (server, seen) = recording_server();
        let (_temp, _root, _path, state) = managed_editing_session(&server);
        set_unpushed_edits(&state);
        state.set_edit_enabled(false);
        assert_eq!(
            state
                .speleodb_release_held_lock_on_close()
                .expect("a veto is no failure"),
            None
        );
        assert_eq!(release_requests(&seen), 0);
        assert!(claim_is_kept(&state));
    }

    #[test]
    fn keeping_the_lock_marks_the_claim_even_with_the_edit_gate_shut() {
        // The exit prompt's "Keep the lock" (close_project with release_lock=false).
        // It used to return early on a shut gate, leaving the claim unmarked - and
        // an unmarked claim with no project open is what the janitor releases.
        let (server, seen) = recording_server();
        let (_temp, _root, _path, state) = managed_editing_session(&server);
        state.set_edit_enabled(false);
        state.speleodb_keep_open_lock_deliberately();
        assert!(claim_is_kept(&state));
        assert!(state.close_project().success);
        assert!(claim_is_kept(&state), "the mark survives the close");
        assert_eq!(release_requests(&seen), 0);
    }

    #[test]
    fn discarding_on_exit_removes_the_managed_copy_then_releases_the_lock() {
        // The exit prompt's confirmed "Discard changes": the one lifecycle exit
        // allowed past the veto, because it carries the user's decision. The copy
        // goes, the session closes, the lock is released, and the next open finds
        // nothing local to protect - so it gets the server copy with no prompt.
        let (server, seen) = recording_server();
        let (_temp, root, path, state) = managed_editing_session(&server);
        set_unpushed_edits(&state);
        super::super::recent_projects::take_trashed_for_tests();
        let result = state.speleodb_close_discarding_local_in(&root);
        assert!(result.success, "discard closes: {:?}", result.error);
        assert_eq!(result.session_closed, Some(true));
        assert!(!path.exists(), "the local copy is gone");
        assert_eq!(
            trashed_names(),
            vec!["Remote.stalx".to_owned()],
            "trashed under the project's name, not as project.stalx"
        );
        assert!(state.get_project_state().is_none(), "the project is closed");
        assert_eq!(release_requests(&seen), 1, "the lock was handed back");
        assert!(!state.speleodb_lock_acquired_here(PID));
        let row = state
            .local_copy_state(PID, &path)
            .expect("a missing copy is known-absent");
        assert!(
            !row.has_local_copy && !row.has_unpushed_edits,
            "nothing left to refuse"
        );
        assert!(
            state.guard_local_edits(PID, &path).is_ok(),
            "the next pull may install"
        );
    }

    #[test]
    fn discard_refuses_a_copy_outside_the_managed_cache_and_changes_nothing() {
        let (server, seen) = recording_server();
        let (temp, _root, path, state) = managed_editing_session(&server);
        set_unpushed_edits(&state);
        let elsewhere: &Path = &temp.path().join("not-the-cache");
        let result = state.speleodb_close_discarding_local_in(elsewhere);
        assert!(!result.success);
        assert!(path.exists());
        assert!(state.get_project_state().is_some(), "still open");
        assert!(state.edit_enabled(), "still editing");
        assert_eq!(release_requests(&seen), 0);
    }

    #[test]
    fn sign_out_discards_a_cached_copy_that_is_not_open_without_releasing_anything() {
        let (server, seen) = recording_server();
        let (_temp, root, path, state) = managed_editing_session(&server);
        set_unpushed_edits(&state);
        state.speleodb_record_cache_owner(&path, &server, "u@x", "Remote");
        let refused = state.speleodb_discard_cached_copy_in(&root, PID, true);
        assert!(
            !refused.success,
            "the OPEN project goes through close-with-discard"
        );
        assert!(path.exists());
        assert!(state.close_project().success);
        super::super::recent_projects::take_trashed_for_tests();
        let result = state.speleodb_discard_cached_copy_in(&root, PID, true);
        assert!(result.success, "{:?}", result.error);
        assert!(!path.exists(), "the un-uploaded copy is gone");
        assert_eq!(
            trashed_names(),
            vec!["Remote.stalx".to_owned()],
            "trashed under the project's name, not as project.stalx"
        );
        settle();
        assert_eq!(
            release_requests(&seen),
            0,
            "the sign-out releases locks itself, explicitly"
        );
        let bad = state.speleodb_discard_cached_copy_in(&root, "../escape", true);
        assert!(!bad.success, "ids are validated before they become paths");
    }

    /// The sign-out dialog's consent is not the only guard (pre-PR finding): the
    /// backend refuses un-uploaded work without the explicit Discard flag, and
    /// another account's copy whatever the flag says.
    #[test]
    fn sign_out_discard_needs_consent_for_un_uploaded_work_and_only_reaches_this_accounts_copy() {
        let (server, seen) = recording_server();
        let (_temp, root, path, state) = managed_editing_session(&server);
        set_unpushed_edits(&state);
        assert!(state.close_project().success);

        state.speleodb_record_cache_owner(&path, &server, "u@x", "Remote");
        let unasked = state.speleodb_discard_cached_copy_in(&root, PID, false);
        assert!(!unasked.success, "no consent, no discard");
        assert!(unasked
            .error
            .as_deref()
            .is_some_and(|e| e.contains("never uploaded")));
        assert!(path.exists());

        state.speleodb_record_cache_owner(&path, &server, "someone@example.com", "Remote");
        let theirs = state.speleodb_discard_cached_copy_in(&root, PID, true);
        assert!(!theirs.success, "another account's copy is refused");
        assert!(theirs
            .error
            .as_deref()
            .is_some_and(|e| e.contains("account in use")));
        assert!(path.exists(), "their un-uploaded work is intact");

        state.speleodb_record_cache_owner(&path, "https://other.example/", "u@x", "Remote");
        assert!(
            !state
                .speleodb_discard_cached_copy_in(&root, PID, true)
                .success,
            "the same email on another instance is another account"
        );

        state.speleodb_session_lock().expect("session").forget();
        state.speleodb_record_cache_owner(&path, &server, "u@x", "Remote");
        assert!(
            !state
                .speleodb_discard_cached_copy_in(&root, PID, true)
                .success,
            "signed out, no copy is the session's"
        );
        assert!(path.exists());
        settle();
        assert_eq!(release_requests(&seen), 0);
    }

    #[test]
    fn sign_out_discard_of_a_clean_own_copy_needs_no_consent() {
        let (server, _seen) = recording_server();
        let (_temp, root, path, state) = managed_editing_session(&server);
        assert!(state.close_project().success);
        state.speleodb_record_cache_owner(&path, &server, "u@x", "Remote");
        let result = state.speleodb_discard_cached_copy_in(&root, PID, false);
        assert!(result.success, "{:?}", result.error);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_discard_that_cannot_remove_the_copy_keeps_the_lock_and_reports_the_close() {
        // The session is torn down before the file goes. If the removal then
        // fails, the edits are still on disk - so the lock must stay held (and
        // marked kept), and the result must still say "closed" so the frontend
        // follows the backend to no open project.
        use std::os::unix::fs::PermissionsExt as _;
        let (server, seen) = recording_server();
        let (_temp, root, path, state) = managed_editing_session(&server);
        set_unpushed_edits(&state);
        let dir = path.parent().expect("dir").to_path_buf();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        let result = state.speleodb_close_discarding_local_in(&root);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(result.success, "the close itself happened");
        assert_eq!(result.session_closed, Some(true));
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == "speleodb_discard_failed"),
            "the failed discard is reported"
        );
        assert!(path.exists(), "the copy and its edits survive");
        assert_eq!(release_requests(&seen), 0, "and the lock was not released");
        assert!(claim_is_kept(&state));
    }

    #[test]
    fn a_poisoned_database_lock_declines_the_release_without_panicking() {
        // A poisoned mutex is not "there is nothing to release". Both reads in
        // `speleodb_lock_release_params` used to be `.ok()?`, which folded
        // `Io("database lock poisoned")` and `Io("SpeleoDB session lock poisoned")` into
        // the ordinary no-op with no trace at all - so a poisoned mutex at close /
        // switch / quit stranded the server lock permanently and silently, which is the
        // exact outcome this module exists to prevent. The failures are now matched and
        // logged (the veto three lines down always was).
        //
        // Behaviourally the answer is still `None` - keeping the lock held is the
        // recoverable direction - so what is pinned here is that the poisoned path is
        // reached and handled rather than panicking, and the log side is pinned by
        // `both_release_param_reads_log_their_real_failures`.
        let state = AppState::new().expect("state");
        state.set_edit_enabled(true);
        let db = &state.database;
        let _ = std::thread::scope(|s| {
            s.spawn(|| {
                let _guard = db.lock().expect("lock to poison");
                panic!("poison the database mutex on purpose");
            })
            .join()
        });
        assert!(
            state.database.is_poisoned(),
            "precondition: the database mutex is poisoned"
        );

        assert!(
            state.speleodb_held_lock_release_params().is_none(),
            "a poisoned lock declines the release (recoverable) rather than panicking"
        );
        assert!(
            state.speleodb_abandon_lock_release_params().is_none(),
            "and so does the abandoning variant"
        );
    }

    #[test]
    fn both_release_param_reads_log_their_real_failures() {
        // Whether a failure was LOGGED is not observable from the return value (both
        // arms are `None`), and silence is the whole defect: an operator looking at why
        // a project stayed locked forever had nothing to go on. Guard it at the source -
        // neither read may be a bare `.ok()?` again, and each must have a `warn!`.
        let src = include_str!("speleodb_lock_release.rs");
        let start = src
            .find("fn speleodb_lock_release_decision")
            .expect("the shared body is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];
        // Matched on the CALLS, not on a bare `.ok()?` needle - the body's own comment
        // quotes that spelling while explaining why it is gone.
        for call in [
            "self.speleodb_open_link().ok()",
            "self.speleodb_active_token().ok()",
        ] {
            assert!(
                !body.contains(call),
                "`{call}` collapses a real failure into the ordinary no-op; match the \
                 error and warn instead"
            );
        }
        assert_eq!(
            body.matches("tracing::warn!").count(),
            3,
            "the link read, the claim read and the token read must each warn on an \
             unexpected failure"
        );
    }

    #[test]
    fn sign_out_keep_my_changes_releases_over_unpushed_edits_only_as_the_explicit_choice() {
        // Silent: signing out without an answer for the open project's
        // un-uploaded work (the logout's own gate drop) keeps the lock, as every
        // lifecycle release does.
        let (server, seen) = recording_server();
        let (_temp, _root, _path, state) = managed_editing_session(&server);
        set_unpushed_edits(&state);
        state.speleodb_logout_drop_gates();
        settle();
        assert_eq!(release_requests(&seen), 0, "no silent release");
        assert!(
            claim_is_kept(&state),
            "the lock is kept for the next sign-in"
        );

        // Explicit: "Keep my changes on this computer" hands the lock back through
        // the per-project release the sign-out uses for every lock it returns; the
        // copy keeps its edits and its un-uploaded mark for the next sign-in.
        let (server, seen) = recording_server();
        let (_temp, _root, path, state) = managed_editing_session(&server);
        set_unpushed_edits(&state);
        state
            .speleodb_release_lock(PID.to_owned())
            .expect("the explicit release goes through");
        assert_eq!(release_requests(&seen), 1, "handed back once");
        assert!(!state.speleodb_lock_acquired_here(PID), "the claim is over");
        assert!(!state.edit_enabled(), "the open copy is read-only now");
        assert!(
            state.speleodb_open_link().expect("link").has_unpushed_edits,
            "the edits are still marked un-uploaded"
        );
        assert!(path.exists(), "the cached copy is kept");
        // The logout that follows has nothing left to release.
        state.speleodb_logout_drop_gates();
        settle();
        assert_eq!(release_requests(&seen), 1, "no second release");
    }

    /// A sink that gives the release thread this process's managed state, as the
    /// app does - so the exit release really queues behind the lock-ops span.
    struct ManagedSink(&'static AppState);

    impl crate::core::events::EventSink for ManagedSink {
        fn emit(&self, _event: &str, _payload: serde_json::Value) {}

        fn with_app_state(&self, f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
            f(self.0);
            true
        }
    }

    #[test]
    fn an_exit_release_behind_backend_internal_work_gives_up_on_its_budget() {
        // Backend-internal work the quit gate never sees (the background re-verify,
        // the stranded-claim cleanup) may hold the lock-ops span when the app
        // exits. It must not keep a windowless process alive: the exit release
        // waits at most its own budget behind it, then returns so the app exits.
        let (server, seen) = recording_server();
        let (_temp, _root, _path, state) = managed_editing_session(&server);
        let state: &'static AppState = Box::leak(Box::new(state));
        state.set_event_sink(Arc::new(ManagedSink(state)));
        let background = state.speleodb_lock_ops_guard();
        let budget = std::time::Duration::from_millis(300);
        let started = std::time::Instant::now();
        state.speleodb_release_held_lock_bounded(budget);
        let took = started.elapsed();
        drop(background);
        assert!(took >= budget, "it did wait behind the span ({took:?})");
        assert!(
            took < std::time::Duration::from_secs(3),
            "and gave up on its budget ({took:?})"
        );
        // The release still lands once the span frees, if the process lives on.
        settle();
        assert_eq!(release_requests(&seen), 1);
    }
}
