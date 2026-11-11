//! The SpeleoDB SESSION generation and the guards built on it.
//!
//! There is no session object in this codebase: what the UI calls an edit
//! session is the `project` slot and a lock claim bound to the session (the edit
//! gate is derived from that binding, see `speleodb_edit_gate`), neither of
//! which could tell "the same project, before and after a teardown" apart on
//! its own (`docs/plans/2026-07-30_speleodb-lock-session-races.md`).
//! This module adds the missing discriminator - a monotonic generation bumped
//! on every session boundary - plus the two guards F7/F8 need:
//!
//! - **The commit guard** ([`AppState::speleodb_commit_editing`]): Start
//!   Editing runs three network round trips between reading the open project
//!   and opening its edit gate. The guard re-checks, under the `project`
//!   mutex, that the world it started from is still there before a single bit
//!   is committed - a delete / close / switch / logout landing inside the
//!   window now refuses the commit (F7) instead of opening a gate onto a
//!   torn-down world and minting a claim nothing can ever release.
//! - **The reclaim check** ([`AppState::speleodb_release_unless_reclaimed`]):
//!   a release captured by a session that has since ended (the delete's
//!   captured release, the janitor below) must stand down when a SUCCESSOR
//!   session has meanwhile claimed the same project's mutex - the server
//!   treats a same-account re-acquire as a no-op, so the successor inherited
//!   the very lock the stale release would free (F8). A successor accounts for
//!   the lock in exactly one shape: a claim. One whose probe proved the mutex
//!   unheld records an authored claim; one that INHERITED the still-held mutex
//!   records an unauthored one - and cannot edit without it, since the claim is
//!   what opens its gate. (There used to be a second "live edit session without
//!   a record" eye for the inherited case; that state no longer exists.)
//!
//! What makes both guards sound is the `speleodb_lock_ops` mutex, which every
//! SpeleoDB LOCK OPERATION holds for its whole span:
//!
//! - Start Editing, from before its FIRST read (generation, link, token,
//!   mutex holder) through the commit. Each of those facts is one another
//!   lock operation can invalidate mid-flight, so the span has to start
//!   before the earliest of them, not at the acquire.
//! - Every reclaim-checked release, across check -> POST, so a claim cannot
//!   be minted between a release's decision and its network call.
//! - LOGOUT, which ends an edit session and hands its mutex back. It is the
//!   one boundary the identity check cannot see (the project slot never
//!   moves), so excluding it from the spans above is what lets the
//!   fast-forward arm commit on identity alone.
//!
//! Deliberately held across network I/O - which is exactly why it is
//! OUTERMOST: nothing that holds `project` / `database` / `undo_stack` may
//! block on it (decide under `project`, drop, then take it), and code already
//! inside a lock-ops span uses the `_locked` variants instead of re-taking it.
//!
//! ## Generation rules
//!
//! 1. Bumped ONLY while holding the `project` mutex. The commit guard checks
//!    and commits under that same mutex, so a boundary either happens entirely
//!    before the commit (and is seen) or entirely after (and finds the commit
//!    already made by a live session).
//! 2. A bump means "the session ended or another began": committing a project
//!    into the slot (open / create / the fast-forward reload), clearing it
//!    (close, which delete and project-switch route through), and logout
//!    (which ends the edit session without touching the slot - footgun 5.8).
//!    Cancelling native loading also invalidates older cache-writing downloads,
//!    including when the newly confirmed file fails to open. Authentication
//!    publication/rejection and download reservations also invalidate old work;
//!    installing a cache consumes its reservation before another can replace it.
//! 3. NOT a bump: an undo rollback (`restore_snapshot_into` restores the SAME
//!    open project - footgun 5.7), and every save (`save_project` never
//!    replaces the slot; a Save a Copy does not even move `file_path`).
//!
//! The bump-site inventory is pinned by `bump_sites_are_exactly_the_known_set`
//! below - a new project-slot writer that forgets to classify itself trips it.

use std::sync::atomic::Ordering;

use super::speleodb::local_dto;
use super::speleodb_lock_release::{HolderCheckedRelease, LockReleaseParams};
use super::AppState;
use crate::speleodb::auth::normalize_endpoint;
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

/// WHICH project this process took a write lock on, and under WHOSE account.
///
/// The account half is not bookkeeping. A lock lives on ONE server under ONE
/// user, and this record outlives the session that made it on purpose - it is
/// the only local pointer to a mutex a torn-down session left held, and the
/// janitor's whole job is to hand that mutex back later. "Later" can be after a
/// sign-out and a sign-in as somebody else, or to a different SpeleoDB
/// deployment: without the account stamped on it, a bare project id gets paired
/// with whatever credentials happen to be around when the cleanup finally runs.
/// That POSTs project A's id to server B, reads B's idempotent success as
/// cleanup, and clears the only pointer to the lock still held on A - or takes
/// a 403 under the wrong account and retries forever.
///
/// It is also what makes the authorship shortcut honest
/// (`AppState::speleodb_remote_state`): a claim earned as one user says nothing
/// about what another user's session pushed.
///
/// And it is the ONLY thing that can open a SpeleoDB project's edit gate: the gate
/// is a function of the claim bound to the open session (`editing`), never a flag
/// of its own - see `speleodb_edit_gate`.
#[derive(Debug, Clone)]
pub(super) struct LockClaim {
    /// The project whose server mutex this process took.
    pub(super) project_id: String,
    /// The SpeleoDB deployment it was taken on.
    endpoint: String,
    /// The account it was taken as.
    email: String,
    /// Whether THIS process made the unheld -> ours transition (Start Editing's
    /// pre-acquire probe found the mutex free, or held by someone who let go).
    /// The authorship licence: a lock this account merely INHERITED (it already
    /// held it - another window, an earlier run, a kept lock) or a provisional
    /// claim left by an ambiguous acquire proves nothing about who pushed what.
    authored: bool,
    /// The session generation whose edit gate this claim holds open, if any.
    ///
    /// Bound only by Start Editing's commit (and carried across an in-place reload
    /// that keeps the session editing). A claim with no binding is a pointer to a
    /// mutex this process may still hold - for the next close, the janitor or a
    /// later Start Editing - and opens nothing. Any boundary that ends the session
    /// (close, open, logout, login, a 401 drop) moves the generation on, which
    /// leaves the binding stale and the gate shut without anyone having to
    /// remember to shut it.
    editing: Option<u64>,
    /// Whether an internal close deliberately retained this lock, or the
    /// un-pushed-edits veto declined a silent release.
    ///
    /// The janitor exists for claims whose session died without ever deciding;
    /// these decided. Left unmarked they look identical once the project is
    /// closed - a claim, no project open - and the very next status refresh would
    /// hand back a mutex the user asked to keep, freeing a project whose local
    /// work has not been pushed for the next editor to lock and push over.
    ///
    /// Cleared by a fresh acquisition, which replaces the whole claim: taking
    /// the lock again is a new decision, and the next release is the ordinary
    /// one.
    kept: bool,
}

impl LockClaim {
    /// A claim on `project_id`, stamped with the account it was taken under.
    ///
    /// `endpoint` and `email` must be read from the SAME session-lock
    /// acquisition that stores the result, so the stamp cannot name an account
    /// other than the one whose token the acquire used.
    pub(super) fn taken(project_id: &str, endpoint: String, email: String, authored: bool) -> Self {
        Self {
            project_id: project_id.to_owned(),
            endpoint,
            email,
            kept: false,
            authored,
            editing: None,
        }
    }

    /// Whether this lock is deliberately held back (see [`LockClaim::kept`]).
    pub(super) fn is_kept(&self) -> bool {
        self.kept
    }

    /// Whether this process took the lock itself (see [`LockClaim::authored`]).
    pub(super) fn is_authored(&self) -> bool {
        self.authored
    }

    /// The session generation this claim holds the edit gate open for, if any.
    pub(super) fn editing(&self) -> Option<u64> {
        self.editing
    }

    /// Bind this claim to the edit session of `generation` (or unbind it).
    /// Binding a claim IS opening the gate, so outside `speleodb_edit_gate` only
    /// ever UNbinds (a restored claim is a pointer, never a session) - and the
    /// fixture that records claims for tests.
    pub(super) fn bind_editing(&mut self, generation: Option<u64>) {
        self.editing = generation;
    }

    /// Whether this claim belongs to the account `(endpoint, email)` names -
    /// i.e. whether that account's token is the one that can release it.
    ///
    /// Both halves are compared the way the CREDENTIAL layer compares them, and
    /// that is not a detail: the token this claim needs is stored under
    /// `auth::account_key`, so any pair that key treats as one account has one
    /// token, and a claim that refuses to recognise its own token is a lock
    /// nothing ever hands back. Email case-insensitively (SpeleoDB may normalise
    /// it), endpoint through [`normalize_endpoint`], which collapses a trailing
    /// slash, host case and a default port - all three of which a user can
    /// change simply by retyping the server on the next sign-in.
    pub(super) fn taken_by(&self, endpoint: Option<&str>, email: Option<&str>) -> bool {
        matches!(
            (endpoint, email),
            (Some(e), Some(m))
                if self.on_deployment(e) && m.eq_ignore_ascii_case(&self.email)
        )
    }

    /// Whether this claim's lock lives on `endpoint`'s deployment.
    ///
    /// Separate from the account test because a captured release knows which
    /// SERVER it is for without necessarily being the account signed in now -
    /// and a project UUID means nothing across deployments. A cloned or restored
    /// deployment can hold the very same id, which is exactly how a successor
    /// check that compared ids alone stood down for an unrelated session.
    pub(super) fn on_deployment(&self, endpoint: &str) -> bool {
        normalize_endpoint(endpoint) == normalize_endpoint(&self.endpoint)
    }

    /// Whether both claims name the SAME lock: one project, on one deployment,
    /// taken by one account. The full identity, for deciding whether a claim
    /// that was removed has been re-made by somebody else in the meantime.
    pub(super) fn is_same_lock(&self, other: &Self) -> bool {
        self.project_id == other.project_id
            && self.on_deployment(&other.endpoint)
            && self.email.eq_ignore_ascii_case(&other.email)
    }
}

impl AppState {
    /// Record a session boundary. Call ONLY while holding the `project` mutex
    /// (rule 1 above); every call site takes a `_proj_guard`-shaped local or
    /// sits inside a block that owns one.
    pub(super) fn bump_speleodb_session(&self) {
        self.speleodb_session_generation
            .fetch_add(1, Ordering::SeqCst);
    }

    /// The current session generation. Meaningful for equality comparison only.
    pub(super) fn speleodb_session_generation(&self) -> u64 {
        self.speleodb_session_generation.load(Ordering::SeqCst)
    }

    /// The generation and the open project's validated link, read together
    /// under the `project` mutex so they describe ONE world. Start Editing's
    /// entry point: capturing them separately is D6 from the abandoned attempt
    /// (a teardown between the two reads pairs the old world's generation with
    /// the new world's link, and the eventual refusal then unwinds the lock of
    /// the project the user actually has open).
    pub(super) fn speleodb_session_snapshot(
        &self,
    ) -> Result<(u64, crate::models::SpeleoLink), SpeleoErrorDto> {
        let _proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = self.speleodb_session_generation();
        let link = self.speleodb_open_link().map_err(local_dto)?;
        super::speleodb::validate_project_id(&link.project_id).map_err(local_dto)?;
        Ok((generation, link))
    }

    /// Start Editing's commit: open the edit gate by binding this account's claim
    /// on `project_id` to the current session - the ONLY production path that
    /// opens a SpeleoDB project's gate (see `speleodb_edit_gate`).
    ///
    /// `acquired_here` is the pre-acquire probe's verdict: the claim is AUTHORED
    /// (the authorship licence, and this flow's unwind slot) only where the probe
    /// proved the mutex free; an inherited lock is an edit session, not an
    /// acquisition, so its claim licenses nothing.
    pub(super) fn speleodb_commit_editing(
        &self,
        entry_generation: Option<u64>,
        project_id: &str,
        acquired_here: bool,
    ) -> Result<(), SpeleoDbError> {
        let _proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(expected) = entry_generation {
            if self.speleodb_session_generation() != expected {
                return Err(SpeleoDbError::SessionMoved);
            }
        }
        // Identity, not just epoch: the open project must still be linked to
        // the project this flow acquired for. Under `Some(gen)` this is
        // belt-and-braces over the generation (a missed bump site degrades to
        // a same-project blind spot instead of a wrong-project commit); under
        // `None` it is the whole check.
        let open = self
            .speleodb_open_link()
            .map_err(|_| SpeleoDbError::SessionMoved)?;
        if open.project_id != project_id {
            return Err(SpeleoDbError::SessionMoved);
        }
        // The claim and the gate are ONE write: there is no moment in which the
        // gate is open without the claim that holds it, or the claim exists for
        // this session without the gate being open.
        self.speleodb_bind_edit_claim(project_id, &open.endpoint, acquired_here, acquired_here)?;
        if entry_generation.is_some() {
            let _ = self.speleodb_mutate_link(|l| {
                l.last_verify_at = Some(chrono::Utc::now().to_rfc3339());
            });
        }
        Ok(())
    }

    /// The unwind release for Start Editing's abort arms, now that a session
    /// boundary can have passed mid-flight. Same session: exactly the release
    /// this always was (`speleodb_release_best_effort`, which also ends OUR
    /// acquisition record). Session moved: the record is NOT ours to touch
    /// (footgun 5.5 / D1 - the refusal fires precisely when another session
    /// may have minted its own claim), so release the lock raw - and only if
    /// no successor has claimed this project's mutex, which the caller's
    /// lock-ops span makes a stable answer.
    ///
    /// Called ONLY from inside Start Editing's lock-ops span - hence the
    /// `_locked` reclaim variant; taking `speleodb_lock_ops` here would
    /// self-deadlock. Both arms are holder-checked (lock#4).
    pub(super) fn speleodb_unwind_unless_kept(
        &self,
        keep_lock_on_unwind: bool,
        failure: Option<&SpeleoDbError>,
        params: &LockReleaseParams,
        entry_gen: u64,
    ) {
        // Rule one (see Start Editing): an inherited lock, or one of unknown
        // provenance over un-uploaded edits, is left exactly as it was found.
        if keep_lock_on_unwind {
            return;
        }
        // Rule two: the cache copy refused to be overwritten over its own
        // un-uploaded work, so the lock this call took is the one guarding it.
        // Keep it - and keep a pointer to it, marked kept so no automatic release
        // hands it back over that work. It used to be kept with no claim at all:
        // held, with nothing local left to find it.
        if failure.is_some_and(super::speleodb::is_local_edits_refusal) {
            self.speleodb_record_kept_claim(&params.project_id);
            return;
        }
        // Session-aware since F7/F8: same session releases exactly as before; a
        // moved one never touches the record and stands down for a successor's
        // claim.
        self.speleodb_unwind_release(params, entry_gen);
    }

    /// The release half of [`Self::speleodb_unwind_unless_kept`].
    pub(super) fn speleodb_unwind_release(&self, params: &LockReleaseParams, entry_gen: u64) {
        if self.speleodb_session_generation() == entry_gen {
            self.speleodb_release_best_effort(params);
            return;
        }
        if let Err(e) = self.speleodb_release_unless_reclaimed_locked(params) {
            tracing::warn!(
                error = %e,
                project_id = %params.project_id,
                purpose = "hand back a lock acquired by a session that no longer exists",
                "speleodb: post-session-move unwind release failed"
            );
        }
    }

    /// Serialized, reclaim-checked release: takes the lock-ops mutex, then
    /// releases `params.project_id`'s mutex UNLESS a live claim for it exists -
    /// in which case a successor session inherited the lock (a same-account
    /// re-acquire is a server-side no-op) and releasing would free the mutex
    /// out from under them (F8). Standing down reports `Ok`: the lock is held
    /// by a session that accounts for it, so nothing is stranded.
    ///
    /// For releases that run detached from the session that captured them (the
    /// delete's captured release, the janitor). Callers already inside a
    /// lock-ops span use [`Self::speleodb_release_unless_reclaimed_locked`].
    pub(super) fn speleodb_release_unless_reclaimed(
        &self,
        params: &LockReleaseParams,
    ) -> Result<(), SpeleoDbError> {
        let _lock_ops = self
            .speleodb_lock_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.speleodb_release_unless_reclaimed_locked(params)
            .map(|_| ())
    }

    /// The body of [`Self::speleodb_release_unless_reclaimed`], for callers that
    /// already hold the lock-ops mutex. Never touches `lock_acquired_here`: a
    /// record present here is by definition a successor's (the capturing
    /// session's own record was cleared when the release was captured), and a
    /// record absent needs no clearing.
    ///
    /// ONE stand-down test, because a successor accounts for the lock in one
    /// shape now: a claim. A successor whose acquire was real records an
    /// authored claim, and one that INHERITED the still-held mutex (a same-account
    /// re-acquire is a server-side no-op) records an unauthored one - it cannot
    /// edit without it, since the claim is what opens the edit gate. There used to
    /// be a second eye for the inherited, record-less live session; there is no
    /// such session any more.
    ///
    /// Asked about the DEPLOYMENT this release is for, not just the project id.
    /// `params` names one server, and a project UUID is only unique on one: a
    /// cloned or restored deployment holding the same id had a session there
    /// answering for a mutex on a server it has never touched, so the release was
    /// never sent and that server's lock stayed held - over a file the delete may
    /// already have destroyed.
    pub(super) fn speleodb_release_unless_reclaimed_locked(
        &self,
        params: &LockReleaseParams,
    ) -> Result<HolderCheckedRelease, SpeleoDbError> {
        if self.speleodb_claim_on_deployment(&params.endpoint, &params.project_id) {
            tracing::info!(
                project_id = %params.project_id,
                purpose = "avoid freeing a successor session's mutex",
                "speleodb: standing down a captured release; a live session has \
                 reclaimed this project's lock"
            );
            // The releasing session holds nothing any more: the lock is the
            // successor's to account for.
            return Ok(HolderCheckedRelease::NotHeldByUs);
        }
        // No successor accounts for it, so it is ours to hand back - if the
        // server still says it is ours at all (lock#4). Not held by us is a
        // success: the caller's claim is over either way.
        super::speleodb_lock_release::release_if_held_by_us(params)
    }

    /// A deferred release, from the moment it can finally be decided: stand the
    /// releasing session down from the lock, then hand it back - all inside ONE
    /// lock-ops span.
    ///
    /// Closing the gate and deciding release share one lock-ops span so a new
    /// Start Editing cannot resume the session between these operations.
    ///
    /// `entry_gen` is the session generation read when the release was decided.
    /// Unchanged, the gate still belongs to the session that decided to hand the
    /// lock back and closing it states a fact. Changed, that session is gone and
    /// the gate now belongs to whatever replaced it - not this release's to
    /// touch, and the reclaim check judges the new world on its own terms. (The
    /// capture already took this session's claim out, which shut the gate; this
    /// says so again under the span rather than relying on it.)
    ///
    /// `claim` is what the caller took out of the claim list, returned on a
    /// failed release so the mutex still has a local pointer for the janitor.
    pub(super) fn speleodb_stand_down_and_release(
        &self,
        params: &LockReleaseParams,
        entry_gen: u64,
        claim: Option<LockClaim>,
    ) -> Result<HolderCheckedRelease, SpeleoDbError> {
        let _lock_ops = self.speleodb_lock_ops_guard();
        {
            let _proj_guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.speleodb_session_generation() == entry_gen {
                self.speleodb_shut_edit_gate();
            }
        }
        let outcome = self.speleodb_release_unless_reclaimed_locked(params);
        if outcome.is_err() {
            // The mutex may still be held and now has no local pointer: put the
            // claim back (unbound) so the no-project janitor can retry it,
            // exactly as the janitor restores its own.
            self.speleodb_restore_lock_claim(claim);
        }
        outcome
    }

    /// Mark this account's claim on `project_id` as deliberately held back, so
    /// the janitor leaves it alone (see [`LockClaim::kept`]).
    ///
    /// Two callers, one meaning: an internal close retaining the lock, and the
    /// un-pushed-edits veto declining a silent release. Both are explicit
    /// decisions, and the janitor exists for the
    /// claims nobody ever decided about. Without the mark the two are the same
    /// state once the project closes (a claim, no project open), so the next
    /// status refresh handed the mutex back and the decision was silently undone.
    pub(super) fn speleodb_keep_lock_deliberately(&self, project_id: &str) {
        let Ok(mut session) = self.speleodb_session_lock() else {
            return;
        };
        let (endpoint, email) = (session.endpoint.clone(), session.email.clone());
        for claim in &mut session.locks_acquired_here {
            if claim.project_id == project_id
                && claim.taken_by(endpoint.as_deref(), email.as_deref())
            {
                claim.kept = true;
            }
        }
    }

    /// Put a claim back after a clear whose reason turned out not to happen.
    ///
    /// `AppState::delete_project` clears at snapshot time, under the same
    /// `project` guard that decided the file's identity, and only afterwards
    /// tries to destroy the file. When THAT fails nothing has been destroyed:
    /// the lock is still ours, still guarding a project still on disk, and the
    /// claim is the only local pointer to it - the session is closed by then, so
    /// no refresh catch-all will ever see the mutex again.
    ///
    /// Declines if the same lock has been claimed meanwhile: a successor's entry
    /// is newer and describes the same mutex, so two would say one thing twice.
    ///
    /// Restored UNBOUND: a claim put back for a retry is a pointer to a mutex,
    /// never an edit session. A release that failed must not reopen the gate it
    /// was shutting (see `speleodb_edit_gate`).
    pub(super) fn speleodb_restore_lock_claim(&self, claim: Option<LockClaim>) {
        let Some(mut claim) = claim else {
            return;
        };
        claim.bind_editing(None);
        let Ok(mut session) = self.speleodb_session_lock() else {
            return;
        };
        if !session
            .locks_acquired_here
            .iter()
            .any(|live| live.is_same_lock(&claim))
        {
            session.locks_acquired_here.push(claim);
        }
    }

    /// The guard Start Editing holds across acquire -> commit (see the module
    /// doc). A poisoned mutex is recovered: the guarded value is `()`, so there
    /// is no state to protect - only the serialization matters.
    pub(super) fn speleodb_lock_ops_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.speleodb_lock_ops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The stranded-claim janitor: with NO project open, a surviving
    /// `lock_acquired_here` record can only belong to a session that no longer
    /// exists (records are minted under the `project` mutex with a project
    /// open), so clear it and hand its lock back. Called from the
    /// `NoRemoteProject` branch of `speleodb_link_status`. Holder-checked like every
    /// automatic release (lock#4): a claim whose lock the server says is not ours
    /// is simply cleared, and one whose holder cannot be read is put back.
    ///
    /// The decision - and the clear - happen under the `project` mutex, so a
    /// racing open/commit serializes against them; the network release happens
    /// after that guard drops (never network under `project`), reclaim-checked
    /// under lock-ops so a Start Editing that slipped in between keeps the
    /// mutex it just claimed.
    ///
    /// The record is the ONLY pointer to the stranded lock, so it is cleared
    /// exactly when the cleanup can no longer need it: an unusable project id
    /// clears it permanently (it can never be joined into a URL), and a usable
    /// one is cleared only once a release POST is in hand - signed out, the
    /// record stays so the first refresh after a sign-in retries, and a failed
    /// POST puts it back (below) so the next refresh of this branch retries too.
    /// A take-first shape lost the record to a transient network blip and
    /// turned "retry on the next refresh" into a promise nothing could keep.
    ///
    /// Which is exactly why the claim carries the account it was taken under
    /// ([`LockClaim`]) and this pairs the id with THAT account's credentials,
    /// never with whoever happens to be signed in when the cleanup finally
    /// runs. A record surviving a sign-out is the normal case here, and the
    /// next sign-in can be a different user or a different deployment: posting
    /// project A's id to server B would read B's idempotent success as cleanup
    /// and drop the only pointer to the lock still held on A.
    pub(super) fn speleodb_janitor_stranded_claim(&self) {
        let params = {
            let proj_guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if proj_guard.is_some() {
                return;
            }
            {
                // Unusable ids first, whoever they belong to: one that can never
                // be joined into a URL can never be released either, so clearing
                // is its only safe end and nobody's later sign-in helps.
                let Ok(mut session) = self.speleodb_session_lock() else {
                    return;
                };
                session.locks_acquired_here.retain(|claim| {
                    super::speleodb::validate_project_id(&claim.project_id).is_ok()
                });
                if session.locks_acquired_here.is_empty() {
                    return;
                }
            }
            // The ACCOUNT is resolved BEFORE a claim is chosen, because it is what
            // makes a claim choosable at all: only the account that took a lock can
            // hand it back. Picking first and testing afterwards let one account's
            // retained claim sit at the head of the list and answer every refresh -
            // failing the test, returning, and starving a claim the live account
            // COULD have released until the other user happened to sign in.
            //
            // Both reads take the session lock themselves, so neither may run under
            // it (the mutex is not reentrant). `proj_guard` is still held, so no
            // session can begin underneath them.
            // Endpoint, token and email as ONE read: separately, a login landing
            // between them pairs account A's token with account B's email, and
            // the search below then takes B's claim out of the retry list and
            // posts it with A's token. See `speleodb_active_account`.
            let Ok((endpoint, token, me)) = self.speleodb_active_account() else {
                // No token, no POST. Keep the records: they describe dead sessions
                // either way, but with no project open they drive nothing except
                // this janitor, and the refresh after a sign-in walks this branch
                // again WITH a token.
                return;
            };
            let Ok(mut session) = self.speleodb_session_lock() else {
                return;
            };
            // One claim per visit; a subsequent status request handles another claim.
            let Some(position) = session.locks_acquired_here.iter().position(|claim| {
                // Deliberately kept locks are not this cleanup's business: the
                // internal retention or the un-pushed-edits veto already
                // decided that this mutex stays held. The janitor is for claims
                // whose session died without deciding anything.
                !claim.is_kept() && claim.taken_by(Some(&endpoint), Some(&me))
            }) else {
                // Everything retained belongs to another account or another
                // deployment. Their tokens are the only ones that can release
                // them, so hold the records until one of those signs in; using
                // this token would either succeed against the wrong project on
                // the wrong server or 403 forever.
                tracing::debug!(
                    purpose = "clean up after a torn-down edit session",
                    "speleodb: nothing to clean up - every retained claim is either \
                     deliberately kept or belongs to another account"
                );
                return;
            };
            // Removed by position, so the entry taken is exactly the entry
            // chosen - no second predicate to drift from the first.
            let claim = session.locks_acquired_here.remove(position);
            drop(session);
            tracing::info!(
                project_id = %claim.project_id,
                purpose = "clean up after a torn-down edit session",
                "speleodb: clearing a lock claim with no project open"
            );
            (
                LockReleaseParams {
                    endpoint,
                    token,
                    email: me,
                    insecure_http: self.speleodb_insecure_http(),
                    project_id: claim.project_id.clone(),
                },
                claim,
            )
        };
        let (params, claim) = params;
        // One lock-ops span over release AND restore: a Start Editing cannot
        // mint or inherit between a failed POST and the record going back, so
        // the restore can never overwrite a successor's claim.
        let _lock_ops = self.speleodb_lock_ops_guard();
        if let Err(e) = self.speleodb_release_unless_reclaimed_locked(&params) {
            tracing::warn!(
                error = %e,
                project_id = %params.project_id,
                purpose = "clean up after a torn-down edit session",
                "speleodb: stranded-claim release failed; retained for a later status request"
            );
            // Restore for the retry - the WHOLE claim, account included, or the
            // retry would be the account-blind pairing this cleanup just
            // refused to make. Only into the same no-project-open world the
            // decision read: with a project open the field belongs to the live
            // world (its own verbs and catch-alls own it), and this janitor
            // never runs there anyway.
            let proj_guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if proj_guard.is_none() {
                if let Ok(mut session) = self.speleodb_session_lock() {
                    // Only if nothing has re-claimed THIS lock meanwhile: a
                    // successor's entry is newer than ours and must stand.
                    // Compared on the full identity - project, deployment AND
                    // account - because a same-UUID claim on a cloned
                    // deployment is a different lock entirely, and reading it
                    // as a successor would suppress this restore and lose the
                    // retry pointer for a mutex still held over here.
                    if !session
                        .locks_acquired_here
                        .iter()
                        .any(|live| live.is_same_lock(&claim))
                    {
                        session.locks_acquired_here.push(claim);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit test fixtures fail loudly on setup errors; the source-scan \
              tripwires panic with the path they could not read"
)]
mod tests {
    use std::path::Path;

    use super::AppState;
    use crate::models::{Project, SpeleoLink};
    use crate::persistence::Database;
    use crate::speleodb::SpeleoDbError;

    const ID: &str = "11111111-1111-1111-1111-111111111111";
    /// A loopback address nothing listens on: token resolution succeeds and
    /// every POST fails fast with "connection refused".
    const UNREACHABLE: &str = "http://127.0.0.1:1/";
    const OTHER_ID: &str = "22222222-2222-2222-2222-222222222222";

    /// A `.stalx` at `path` linked to `project_id` (same idiom as
    /// `speleodb_foreign`'s fixture).
    fn seed_linked(path: &Path, project_id: &str) {
        let mut db = Database::open(path).expect("open db");
        db.save_project(&Project::new("Linked".to_owned(), None))
            .expect("save project");
        db.save_speleo_link(Some(&SpeleoLink {
            // The same dead loopback the fixtures sign in against, so a link,
            // a claim and a captured release all name ONE deployment - which
            // the reclaim check now compares, and a POST to port 1 still fails
            // fast rather than waiting on a DNS lookup.
            endpoint: UNREACHABLE.to_owned(),
            project_id: project_id.to_owned(),
            project_name: "Linked".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            // [#303] NULL exercises the legacy initialization path.
            undo_mode: None,
        }))
        .expect("save link");
    }

    /// An open, linked project (read-only remote open: the gate is shut).
    fn open_linked(state: &AppState, dir: &Path, name: &str, project_id: &str) {
        let path = dir.join(name);
        seed_linked(&path, project_id);
        assert!(state.open_project(path).success, "fixture opens");
        assert!(!state.edit_enabled(), "remote open shuts the gate");
    }

    /// A signed-in session, so a claim has an account to be stamped with
    /// ([`super::LockClaim`]). Points at a dead endpoint: token resolution
    /// succeeds locally and every POST fails fast (nothing listens on port 1).
    /// The in-memory token store keeps it hermetic - no persistent credential lookup, and
    /// `endpoint.is_some()` short-circuits the rehydration.
    fn sign_in_unreachable(state: &AppState) {
        sign_in_as(state, UNREACHABLE, "diver@example.com");
    }

    /// [`sign_in_unreachable`] for a named account on a named deployment, for
    /// the tests about a claim outliving the account that made it.
    fn sign_in_as(state: &AppState, endpoint: &str, email: &str) {
        let mut session = state.speleodb_session_lock().expect("session lock");
        session.endpoint = Some(endpoint.to_owned());
        session.email = Some(email.to_owned());
        session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token("tok");
    }

    /// Every claim still retained, whatever account it belongs to.
    ///
    /// The accessors deliberately answer only for the account signed in NOW
    /// (`speleodb_lock_acquired_here`, `speleodb_acquired_here_project`), so a
    /// claim held FOR a signed-out or different account is invisible through
    /// them - which is the whole point, and exactly what these tests have to
    /// look past to see that it survived.
    fn retained_projects(state: &AppState) -> Vec<String> {
        state
            .speleodb_session_lock()
            .expect("session lock")
            .locks_acquired_here
            .iter()
            .map(|claim| claim.project_id.clone())
            .collect()
    }

    /// Sign out, WITHOUT touching the claim - what a logout that could not run
    /// its release leaves behind, and the state the janitor is written for.
    fn sign_out(state: &AppState) {
        let mut session = state.speleodb_session_lock().expect("session lock");
        session.endpoint = None;
        session.email = None;
        session.token_store = crate::speleodb::auth::TokenStore::default();
    }

    #[test]
    fn open_close_and_create_are_session_boundaries_but_saves_and_rollbacks_are_not() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        let g0 = state.speleodb_session_generation();

        open_linked(&state, temp.path(), "a.stalx", ID);
        let g1 = state.speleodb_session_generation();
        assert_ne!(g0, g1, "an open is a session boundary");

        // An undo rollback restores the SAME open project (footgun 5.7).
        let snapshot = state
            .capture_project_snapshot()
            .expect("snapshot the open project");
        state.rollback_project(&snapshot);
        assert_eq!(
            state.speleodb_session_generation(),
            g1,
            "an undo rollback is NOT a session boundary"
        );

        // Neither is a save - implicit or Save a Copy (the slot never moves).
        assert!(state.save_project(None).success);
        assert!(
            state
                .save_project(Some(temp.path().join("copy.stalx")))
                .success
        );
        assert_eq!(
            state.speleodb_session_generation(),
            g1,
            "saves are NOT session boundaries"
        );

        state.close_project();
        let g2 = state.speleodb_session_generation();
        assert_ne!(g1, g2, "a close is a session boundary");
    }

    #[test]
    fn the_session_snapshot_pairs_the_link_with_its_generation() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        open_linked(&state, temp.path(), "a.stalx", ID);

        let (entry_gen, link) = state
            .speleodb_session_snapshot()
            .expect("a linked open project has a snapshot");
        assert_eq!(entry_gen, state.speleodb_session_generation());
        assert_eq!(link.project_id, ID);

        state.close_project();
        assert!(
            state.speleodb_session_snapshot().is_err(),
            "no open project, no snapshot"
        );
    }

    #[test]
    fn a_commit_in_an_unmoved_session_opens_the_gate_and_mints_the_claim() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        open_linked(&state, temp.path(), "a.stalx", ID);
        let (entry_gen, link) = state.speleodb_session_snapshot().expect("snapshot");

        state
            .speleodb_commit_editing(Some(entry_gen), &link.project_id, true)
            .expect("nothing moved, so the commit lands");

        assert!(state.edit_enabled(), "the gate is open");

        assert!(
            state.speleodb_lock_acquired_here(ID),
            "the acquisition record was minted"
        );
        let stamped = state.speleodb_open_link().expect("link");
        assert!(
            stamped.last_verify_at.is_some(),
            "a fresh-verdict commit starts the #202 re-verify window"
        );
    }

    #[test]
    fn a_commit_without_acquired_here_opens_the_gate_with_an_unauthored_claim() {
        // An inherited lock is an edit session, not an acquisition: its claim
        // licenses no authorship. But it IS a claim - the gate is one, so an edit
        // session without a claim is no longer something the commit can produce.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        open_linked(&state, temp.path(), "a.stalx", ID);
        let (entry_gen, link) = state.speleodb_session_snapshot().expect("snapshot");

        state
            .speleodb_commit_editing(Some(entry_gen), &link.project_id, false)
            .expect("commit lands");

        assert!(state.edit_enabled());
        assert!(
            state.speleodb_holds_claim(ID),
            "the open gate is this account's claim"
        );
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "an inherited lock is an edit session, not an acquisition"
        );
    }

    #[test]
    fn a_commit_with_nobody_signed_in_opens_nothing() {
        // A claim nobody can release is not one an edit session may hang on: the
        // commit that used to open the gate WITHOUT recording a claim (no account
        // to stamp it with) now refuses outright.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        open_linked(&state, temp.path(), "a.stalx", ID);
        let (entry_gen, link) = state.speleodb_session_snapshot().expect("snapshot");

        let refused = state
            .speleodb_commit_editing(Some(entry_gen), &link.project_id, true)
            .expect_err("no account, no claim, no gate");

        assert!(matches!(refused, SpeleoDbError::SessionMoved));
        assert!(!state.edit_enabled());
        assert!(retained_projects(&state).is_empty());
    }

    #[test]
    fn a_commit_after_a_teardown_is_refused_with_nothing_committed() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        open_linked(&state, temp.path(), "a.stalx", ID);
        let (entry_gen, link) = state.speleodb_session_snapshot().expect("snapshot");

        state.close_project();

        let refused = state
            .speleodb_commit_editing(Some(entry_gen), &link.project_id, true)
            .expect_err("the world this flow started from is gone");
        assert!(matches!(refused, SpeleoDbError::SessionMoved));
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "no claim was minted onto the torn-down world"
        );
    }

    #[test]
    fn a_commit_after_a_same_project_reopen_is_still_refused() {
        // The doc's case (c): the same project before and after a teardown. The
        // identity check alone cannot see it - the generation is the
        // discriminator, and this is the test that keeps it load-bearing.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        open_linked(&state, temp.path(), "a.stalx", ID);
        let (entry_gen, link) = state.speleodb_session_snapshot().expect("snapshot");

        state.close_project();
        open_linked(&state, temp.path(), "b.stalx", ID);

        let refused = state
            .speleodb_commit_editing(Some(entry_gen), &link.project_id, true)
            .expect_err("a successor session on the same project is not our session");
        assert!(matches!(refused, SpeleoDbError::SessionMoved));
        assert!(!state.edit_enabled(), "the successor's shut gate stands");
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "and no claim landed"
        );
    }

    #[test]
    fn an_identity_only_commit_asks_who_is_open_not_how_many_boundaries_passed() {
        // The fast-forward arm: its own reload is a boundary, so it commits with
        // `None` and identity does the work.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        open_linked(&state, temp.path(), "a.stalx", ID);
        state.close_project();
        open_linked(&state, temp.path(), "b.stalx", ID);

        state
            .speleodb_commit_editing(None, ID, true)
            .expect("the open project is still linked to this id");
        assert!(state.edit_enabled());
        assert!(state.speleodb_lock_acquired_here(ID));

        state.close_project();
        open_linked(&state, temp.path(), "c.stalx", OTHER_ID);
        let refused = state
            .speleodb_commit_editing(None, ID, true)
            .expect_err("a different project is open");
        assert!(matches!(refused, SpeleoDbError::SessionMoved));

        state.close_project();
        let refused = state
            .speleodb_commit_editing(None, ID, true)
            .expect_err("no project is open at all");
        assert!(matches!(refused, SpeleoDbError::SessionMoved));
    }

    /// Release params for [`ID`] as the fixture account on `endpoint`.
    fn release_params(endpoint: &str) -> super::LockReleaseParams {
        super::LockReleaseParams {
            endpoint: endpoint.to_owned(),
            token: "tok".to_owned(),
            email: "diver@example.com".to_owned(),
            insecure_http: true,
            project_id: ID.to_owned(),
        }
    }

    #[test]
    fn the_unwind_in_an_unmoved_session_clears_the_record_as_before() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        let (server, seen) =
            crate::core::state::tests_support::lock_server(ID, Some("diver@example.com".into()));
        sign_in_as(&state, &server, "diver@example.com");
        open_linked(&state, temp.path(), "a.stalx", ID);
        let entry_gen = state.speleodb_session_generation();
        state.speleodb_note_lock_acquired(Some(ID));

        state.speleodb_unwind_release(&release_params(&server), entry_gen);

        assert_eq!(
            crate::core::state::tests_support::release_posts(&seen),
            1,
            "the server says the lock is ours, so it is handed back"
        );
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "same session: the release ends OUR claim, exactly as it always did"
        );
    }

    #[test]
    fn a_failed_unwind_release_keeps_the_claim_for_a_retry() {
        // The holder cannot be read (nothing listens), so nothing is released
        // blind - and the claim, the only local pointer to a mutex that may still
        // be ours, survives for a later holder-checked retry.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        open_linked(&state, temp.path(), "a.stalx", ID);
        let entry_gen = state.speleodb_session_generation();
        state.speleodb_note_lock_acquired(Some(ID));

        state.speleodb_unwind_release(&release_params(UNREACHABLE), entry_gen);

        assert!(state.speleodb_lock_acquired_here(ID));
    }

    #[test]
    fn the_unwind_after_a_boundary_never_touches_the_record() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        open_linked(&state, temp.path(), "a.stalx", ID);
        let entry_gen = state.speleodb_session_generation();

        state.close_project();
        open_linked(&state, temp.path(), "b.stalx", ID);
        // The successor's claim, minted by ITS Start Editing.
        state.speleodb_note_lock_acquired(Some(ID));

        state.speleodb_unwind_release(&release_params(UNREACHABLE), entry_gen);

        assert!(
            state.speleodb_lock_acquired_here(ID),
            "session moved: the record is the successor's, not ours to blank (5.5)"
        );
    }

    /// A signed-in session pointing at a dead endpoint: token resolution
    /// succeeds locally, every POST fails fast (nothing listens on port 1).
    /// The in-memory token store keeps this hermetic - no persistent credential lookup
    /// (and `endpoint.is_some()` short-circuits the rehydration).
    #[test]
    fn the_janitor_leaves_a_live_claim_and_a_signed_out_stranded_claim_alone() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);

        // A project open: the claim may be live - leave it alone.
        open_linked(&state, temp.path(), "a.stalx", ID);
        state.speleodb_note_lock_acquired(Some(ID));
        state.speleodb_janitor_stranded_claim();
        assert!(
            state.speleodb_lock_acquired_here(ID),
            "a claim with a project open is not the janitor's to judge"
        );

        // No project open, and signed out afterwards: no token means no release
        // POST, and the record is the ONLY pointer to the stranded lock - keep
        // it so the first refresh after a sign-in can retry (round 5).
        state.close_project();
        state.speleodb_note_lock_acquired(Some(ID));
        sign_out(&state);
        state.speleodb_janitor_stranded_claim();
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned()],
            "signed out, the record must survive: it is the retry"
        );
    }

    #[test]
    fn a_new_acquisition_leaves_another_accounts_retained_claim_alone() {
        // Round 7 (P2): a claim can outlive the session that made it - logout
        // declines to release over un-pushed edits, and a torn-down session
        // leaves one for the janitor - and only its own account can ever hand
        // that mutex back. With ONE slot, signing in as somebody else and
        // starting to edit anything overwrote the only pointer to it, and the
        // lock was stranded until somebody released it by hand (SpeleoDB has no
        // lock timeout).
        let state = AppState::new().expect("state");
        sign_in_as(&state, "http://127.0.0.1:1/", "first@example.com");
        state.speleodb_note_lock_acquired(Some(ID));

        sign_in_as(&state, "http://127.0.0.1:1/", "second@example.com");
        state.speleodb_note_lock_acquired(Some(OTHER_ID));

        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned(), OTHER_ID.to_owned()],
            "the new acquisition must not evict the claim held for another account"
        );
        assert!(
            state.speleodb_lock_acquired_here(OTHER_ID),
            "the live account's own claim answers for it"
        );
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "another account's does not - it is no evidence about this session"
        );
        // The unwind is offered nothing here: these claims were recorded
        // directly, not minted by a Start Editing commit, and the unwind hands
        // back only the acquisition its own flow made (round 9 P1).
        assert_eq!(
            state.speleodb_take_acquisition_by_this_flow(),
            None,
            "a claim is not by itself an acquisition this flow may unwind"
        );

        // Re-acquiring the SAME project replaces that entry rather than piling
        // a duplicate on top of it.
        state.speleodb_note_lock_acquired(Some(OTHER_ID));
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned(), OTHER_ID.to_owned()],
            "one claim per (account, project), not one per acquisition"
        );

        // Round 8 (P2): a DIFFERENT project ADDS an entry. One account can be
        // owed more than one mutex - a janitor release that failed transiently
        // leaves exactly such a claim - and locking a second project must not be
        // what loses the first one's retry pointer.
        const THIRD_ID: &str = "33333333-3333-3333-3333-333333333333";
        state.speleodb_note_lock_acquired(Some(THIRD_ID));
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned(), OTHER_ID.to_owned(), THIRD_ID.to_owned()],
            "acquiring one project must not end this account's claim on another"
        );

        // A per-project release ends exactly that project's claim...
        state.speleodb_forget_lock_acquired_for(OTHER_ID);
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned(), THIRD_ID.to_owned()],
            "the per-project release is the one that ends one claim"
        );

        // ...and release-all ends this account's claims without touching the
        // other account's.
        state.speleodb_note_lock_acquired(None);
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned()],
            "the first account's claim survives a second account's release-all"
        );

        sign_out(&state);
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "signed out there is no live account, so no claim answers for one"
        );
    }

    /// A loopback server that answers every request with 200 and a minimal
    /// project body - enough for `release_lock` to read a success.
    fn release_stub(project_id: &str) -> (String, std::net::TcpListener) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let body = format!(r#"{{"id":"{project_id}","name":"Remote"}}"#);
        let accept_on = listener.try_clone().expect("clone listener");
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = accept_on.accept() {
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(150)));
                let mut chunk = vec![0u8; 8 * 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if chunk[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = crate::core::state::tests_support::sdk_fixture(&body);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), listener)
    }

    #[test]
    fn an_inherited_acquire_leaves_the_unwind_with_nothing_to_hand_back() {
        // Round 10 (P1): the unwind slot answers "what did the acquire that just
        // ran take?", and for an acquire that INHERITED a lock the account
        // already held the answer is nothing - it mints no claim by design. The
        // slot was only ever WRITTEN, never reset, so the previous operation's
        // acquisition stood in for that nothing and the revert flow's unwind
        // released it: a different project's mutex, possibly one a logout kept
        // back precisely because it guards un-pushed work.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        open_linked(&state, temp.path(), "a.stalx", ID);

        // An earlier flow that DID acquire.
        state
            .speleodb_commit_editing(None, ID, true)
            .expect("the first flow's commit");

        // A later flow that inherits: it opens its span, commits without an
        // acquisition, and must leave the unwind empty.
        state.speleodb_begin_acquisition_flow();
        state
            .speleodb_commit_editing(None, ID, false)
            .expect("the inheriting flow's commit");

        assert_eq!(
            state.speleodb_take_acquisition_by_this_flow(),
            None,
            "an inherited acquire took nothing, so the unwind must release \
             nothing - not the previous operation's lock"
        );
        assert!(
            state.speleodb_lock_acquired_here(ID),
            "and the earlier acquisition's claim is untouched: it is still a \
             lock this process holds"
        );
    }

    #[test]
    fn a_claim_survives_a_re_login_that_spells_the_endpoint_differently() {
        // Round 9 (P2): the credential layer keys tokens by
        // `auth::account_key`, which collapses a trailing slash, host case and
        // a default port - so all of these spellings share ONE token. Comparing
        // the claim's endpoint exactly meant the janitor could not recognise the
        // very account whose token it was about to release with, and the mutex
        // stayed held for good.
        let state = AppState::new().expect("state");
        sign_in_as(&state, "https://sdb.example.com/", "diver@example.com");
        state.speleodb_note_lock_acquired(Some(ID));

        for spelling in [
            "https://sdb.example.com",
            "https://sdb.example.com:443",
            "https://SDB.Example.com/",
        ] {
            sign_in_as(&state, spelling, "diver@example.com");
            assert!(
                state.speleodb_lock_acquired_here(ID),
                "{spelling} is the same deployment, so the claim is still ours"
            );
        }

        // ...and a genuinely different one is still refused.
        sign_in_as(&state, "https://other.example.com/", "diver@example.com");
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "a different host is a different deployment"
        );
    }

    #[test]
    fn a_captured_release_ignores_a_same_id_session_on_another_deployment() {
        // Round 9 (P1): a project UUID is unique on ONE server. A cloned or
        // restored deployment can hold the same id, and a session editing THAT
        // project accounts for nothing on the server whose mutex is about to be
        // left held - which the delete may have already destroyed the file for.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        // A live edit session on the SAME project id, on the fixture's
        // deployment.
        open_linked(&state, temp.path(), "a.stalx", ID);
        state
            .speleodb_commit_editing(None, ID, false)
            .expect("the successor's commit");

        // The captured release is for the same id on ANOTHER deployment.
        let elsewhere = super::LockReleaseParams {
            endpoint: "http://127.0.0.1:2/".to_owned(),
            token: "tok".to_owned(),
            email: "diver@example.com".to_owned(),
            insecure_http: true,
            project_id: ID.to_owned(),
        };
        assert!(
            state.speleodb_release_unless_reclaimed(&elsewhere).is_err(),
            "the local session answers for its own server's lock, not for a \
             same-UUID project somewhere else - so the release must go out \
             (and error against the dead endpoint) rather than stand down"
        );

        // The same release for THIS deployment still stands down.
        let here = super::LockReleaseParams {
            endpoint: UNREACHABLE.to_owned(),
            token: "tok".to_owned(),
            email: "diver@example.com".to_owned(),
            insecure_http: true,
            project_id: ID.to_owned(),
        };
        state
            .speleodb_release_unless_reclaimed(&here)
            .expect("stood down for the live session on this deployment");
    }

    #[test]
    fn opening_a_second_project_read_only_keeps_the_first_ones_stranded_claim() {
        // Round 9 (P2): the refresh's catch-all clear ran on every claim the
        // account held, so the first status refresh after opening ANY project
        // read-only discarded a claim retained for a different project - the
        // only pointer to a mutex a failed janitor release had left held. The
        // catch-all is gone (data#3: a shut gate already withdraws whatever a
        // claim licensed), so a refresh keeps every claim, the open project's
        // included.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        state.speleodb_note_lock_acquired(Some(OTHER_ID));

        // A second project, opened read-only, with a claim of its own left
        // over from an earlier session: its gate is shut.
        open_linked(&state, temp.path(), "a.stalx", ID);
        state.speleodb_note_lock_acquired(Some(ID));
        let _ = state.speleodb_link_status();

        assert_eq!(
            retained_projects(&state),
            vec![OTHER_ID.to_owned(), ID.to_owned()],
            "a shut gate ends no claim: each is a pointer a holder-checked release needs"
        );
        assert!(
            !state.edit_enabled(),
            "and a claim with no edit session opens nothing"
        );
    }

    #[test]
    fn the_janitor_picks_the_claim_the_live_account_can_actually_release() {
        // Round 8 (P2): another account's retained claim sitting at the head of
        // the list used to answer every refresh - the janitor took the FIRST entry,
        // found it was not this session's to release, and returned. The live
        // account's own stranded claim was never attempted, for as long as that
        // other user did not happen to sign in.
        //
        // The stub answers the release, so the two behaviours are told apart by
        // what is LEFT: choosing correctly releases our claim and drops it,
        // while stopping at the head of the list leaves both standing.
        let (base, _listener) = release_stub(OTHER_ID);
        let state = AppState::new().expect("state");
        sign_in_as(&state, "http://127.0.0.1:1/", "first@example.com");
        state.speleodb_note_lock_acquired(Some(ID));

        sign_in_as(&state, &base, "second@example.com");
        state.speleodb_note_lock_acquired(Some(OTHER_ID));
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned(), OTHER_ID.to_owned()],
            "fixture: the other account's claim is FIRST in the list"
        );

        state.speleodb_janitor_stranded_claim();

        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned()],
            "the live account's claim was found past the head of the list, \
             released, and dropped - and the other account's is untouched"
        );
    }

    #[test]
    fn the_janitor_leaves_a_deliberately_kept_lock_alone() {
        // An internal close can retain the mutex deliberately, and the
        // un-pushed-edits veto does the same on the app's own
        // authority. Once the project is closed both look exactly like the state
        // the janitor exists for - a claim, no project open - so without a mark
        // the very next status refresh handed the mutex back and the choice was
        // undone in silence, freeing a project whose work is still only local.
        //
        // The stub answers the release, so a janitor that ignored the mark would
        // be visible as a claim that disappeared.
        let (base, _listener) = release_stub(ID);
        let state = AppState::new().expect("state");
        sign_in_as(&state, &base, "diver@example.com");
        state.speleodb_note_lock_acquired(Some(ID));
        state.speleodb_keep_lock_deliberately(ID);

        state.speleodb_janitor_stranded_claim();

        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned()],
            "a lock the user chose to keep is not the janitor's to hand back"
        );

        // A fresh acquisition is a new decision, and the ordinary cleanup
        // applies to it again.
        state.speleodb_note_lock_acquired(Some(ID));
        state.speleodb_janitor_stranded_claim();
        assert!(
            retained_projects(&state).is_empty(),
            "re-acquiring clears the keep, so the next orphaned claim is \
             cleaned up as before"
        );
    }

    #[test]
    fn the_janitor_will_not_release_one_accounts_lock_with_anothers_token() {
        // Round 6 (P2): a stranded claim outliving a sign-out is the NORMAL case
        // here - and the next sign-in can be a different user, or the same user
        // against a different deployment. Pairing the id with whatever
        // credentials are around by then posts project A's id to server B,
        // reads B's idempotent success as cleanup, and drops the only pointer
        // to the lock still held on A.
        let state = AppState::new().expect("state");
        sign_in_as(&state, "http://127.0.0.1:1/", "diver@example.com");
        state.speleodb_note_lock_acquired(Some(ID));

        // Same person, different deployment.
        sign_in_as(&state, "http://127.0.0.1:2/", "diver@example.com");
        state.speleodb_janitor_stranded_claim();
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned()],
            "a claim taken on another server is not this one's to clean up"
        );

        // Same deployment, different person.
        sign_in_as(&state, "http://127.0.0.1:1/", "someone.else@example.com");
        state.speleodb_janitor_stranded_claim();
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned()],
            "nor is a claim taken by another account"
        );

        // And the account that DID take it still gets to clean up: the POST
        // fails against the dead endpoint, so the claim comes back for the next
        // refresh rather than being lost.
        sign_in_as(&state, "http://127.0.0.1:1/", "DIVER@example.com");
        state.speleodb_janitor_stranded_claim();
        assert_eq!(
            retained_projects(&state),
            vec![ID.to_owned()],
            "the same account under a different case is still the same account, \
             so this one tried the release and restored the claim when it failed"
        );
    }

    #[test]
    fn the_janitor_restores_the_claim_when_the_release_post_fails() {
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        state.speleodb_note_lock_acquired(Some(ID));

        state.speleodb_janitor_stranded_claim();

        assert!(
            state.speleodb_lock_acquired_here(ID),
            "a transient POST failure must put the record back, or the next \
             refresh has nothing to retry from (round 5)"
        );
    }

    #[test]
    fn the_janitor_permanently_clears_an_unusable_id() {
        // Signed out on purpose: an id that can never be joined into a URL is
        // cleared regardless of whether a release could even be attempted.
        let state = AppState::new().expect("state");
        state.speleodb_note_lock_acquired(Some("not-a-uuid"));

        state.speleodb_janitor_stranded_claim();

        assert!(
            !state.speleodb_lock_acquired_here("not-a-uuid"),
            "an unusable id has no release to retry; clearing is its only end"
        );
    }

    #[test]
    fn link_status_with_no_project_open_runs_the_janitor() {
        // Footgun 5.4 closed: the NoRemoteProject early-return used to be
        // unreachable territory for every cleanup. The unusable-id record is
        // the janitor outcome observable without a live server: it is cleared
        // on the spot, signed in or not.
        let state = AppState::new().expect("state");
        state.speleodb_note_lock_acquired(Some("not-a-uuid"));

        let status = state
            .speleodb_link_status()
            .expect("no project open is a valid status");

        assert!(!status.is_remote);
        assert!(
            !state.speleodb_lock_acquired_here("not-a-uuid"),
            "the refresh's visit to the no-project branch ran the janitor"
        );
    }

    #[test]
    fn a_captured_release_stands_down_for_an_inherited_successor_session() {
        // The round-4 P1: a successor that started editing while the account
        // still held the lock (the capturing session's release had not POSTed)
        // INHERITED it - `acquired_here` false, so no AUTHORED record was minted.
        // The reclaim check must still see that session, or the deferred release
        // frees the mutex out from under it - and it does, because the session's
        // edit gate is its (unauthored) claim.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        open_linked(&state, temp.path(), "a.stalx", ID);
        state
            .speleodb_commit_editing(None, ID, false)
            .expect("the inherited successor's commit");
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "fixture: an inherited session has no acquisition record"
        );

        let params = super::LockReleaseParams {
            endpoint: UNREACHABLE.to_owned(),
            token: "tok".to_owned(),
            email: "diver@example.com".to_owned(),
            insecure_http: true,
            project_id: ID.to_owned(),
        };
        state
            .speleodb_release_unless_reclaimed(&params)
            .expect("stood down without a POST - a POST to a dead endpoint errs");
    }

    #[test]
    fn a_captured_release_posts_when_no_session_accounts_for_the_lock() {
        // The other half of the check: a world with no live claim and no live
        // edit session must NOT stand down - the POST is attempted (and errs
        // against the dead endpoint, which is how the attempt is observable).
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        let params = super::LockReleaseParams {
            endpoint: UNREACHABLE.to_owned(),
            token: "tok".to_owned(),
            email: "diver@example.com".to_owned(),
            insecure_http: true,
            project_id: ID.to_owned(),
        };

        // No project open at all.
        assert!(state.speleodb_release_unless_reclaimed(&params).is_err());

        // The project open READ-ONLY: a shut gate is not an edit session and
        // accounts for nothing.
        open_linked(&state, temp.path(), "a.stalx", ID);
        assert!(state.speleodb_release_unless_reclaimed(&params).is_err());

        // A DIFFERENT project's live session says nothing about this lock.
        state.close_project();
        open_linked(&state, temp.path(), "b.stalx", OTHER_ID);
        state
            .speleodb_commit_editing(None, OTHER_ID, false)
            .expect("the other project's session");
        assert!(state.speleodb_release_unless_reclaimed(&params).is_err());
    }

    // ---- tripwires -------------------------------------------------------

    /// Read a sibling source file of this module at test time.
    fn state_source(name: &str) -> String {
        let path = format!("{}/src/core/state/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    #[test]
    fn bump_sites_are_exactly_the_known_set() {
        // D7's lesson from the abandoned attempt: an incomplete bump-site
        // inventory is invisible until the missed site re-opens the bug. Every
        // file that commits into or clears the project slot must either bump or
        // be on the classified DO-NOT-BUMP list below.
        let dir = format!("{}/src/core/state", env!("CARGO_MANIFEST_DIR"));
        let mut bumping: Vec<String> = Vec::new();
        let mut writing: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("read state dir") {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().into_string().expect("utf-8 name");
            // Skip the shared test dump, THIS module (the scan needles appear
            // in this very test's source), and the test-fixture module: its
            // AppState setters write the slot freely and legitimately, and its
            // `any(test, feature = "test-support")` gate (for the root crate's
            // tests) means the truncation below no longer hides them.
            if !name.ends_with(".rs")
                || name == "tests.rs"
                || name == "speleodb_session.rs"
                || name == "tests_support.rs"
            {
                continue;
            }
            let src = std::fs::read_to_string(entry.path()).expect("read source");
            // Production code only: this tree keeps test modules at the bottom of
            // the file, so truncating at the first test gate is enough for a
            // tripwire (fixtures write the slot freely and legitimately).
            let src = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
            if src.contains("self.bump_speleodb_session()") {
                bumping.push(name.clone());
            }
            // The slot-commit spellings in this tree: a load/create commit
            // (`= Some(project`), the rollback restore (`= Some(restored`), and
            // the close clear (`*project = None`). Space-prefixed so a
            // `== Some(project_id)` comparison does not read as an assignment.
            if src.contains(" = Some(project")
                || src.contains(" = Some(restored")
                || src.contains("*project = None")
            {
                writing.push(name);
            }
        }
        bumping.sort();
        assert_eq!(
            bumping,
            vec![
                "app_state.rs",
                "project_create.rs",
                "project_io.rs",
                "project_load.rs",
                "speleodb_cache_install.rs",
                "speleodb_credentials.rs",
                "speleodb_logout.rs",
            ],
            "the bump-site inventory moved; re-derive the session-boundary \
             classification in `speleodb_session` before updating this list"
        );
        // Classified non-bumping slot writers: the rollback restore
        // (`restore_snapshot_into`) is the SAME session by design (footgun 5.7).
        // Everything else that writes the slot must bump.
        let exempt = ["app_state.rs"];
        for name in writing {
            assert!(
                bumping.contains(&name) || exempt.contains(&name.as_str()),
                "{name} writes the project slot but neither bumps the session \
                 generation nor is classified exempt - decide which it is \
                 (see `speleodb_session`'s module doc)"
            );
        }
    }

    #[test]
    fn start_editing_holds_the_guards_in_the_right_order() {
        let src = state_source("speleodb.rs");
        let start = src
            .find("fn speleodb_start_editing")
            .expect("start editing is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        let lock_ops = body
            .find("self.speleodb_lock_ops_guard()")
            .expect("the acquire span must be serialized against captured releases");
        let snapshot = body
            .find("self.speleodb_session_snapshot()")
            .expect("entry must snapshot generation + link as one world (D6)");
        let token = body
            .find("self.speleodb_active_token()")
            .expect("the flow resolves a token");
        let probe = body
            .find("self.speleodb_lock_holder_checked(")
            .expect("the pre-acquire holder probe is present");
        let acquire = body
            .find("client::acquire_lock(")
            .expect("the acquire is present");
        assert!(
            lock_ops < snapshot && snapshot < token && token < probe && probe < acquire,
            "order must be: lock-ops guard FIRST, then the session snapshot, the \
             token, the holder probe, and the acquire. Every fact after the guard \
             is one another lock operation could invalidate: a probe outside the \
             span reads provenance a captured release can undo before the acquire \
             (round 5), and a token read outside it lets a logout leave this flow \
             committing on behalf of a signed-out session (round 6)"
        );
        let flow_begins = body
            .find("self.speleodb_begin_acquisition_flow()")
            .expect("the operation must clear the previous one's unwind slot");
        assert!(
            lock_ops < flow_begins && flow_begins < acquire,
            "the unwind slot is reset inside the span and before the acquire, so \
             an inheriting or aborting flow leaves nothing behind for the \
             stale-context unwind to release (round 10)"
        );
        assert_eq!(
            body.matches("self.speleodb_commit_editing(").count(),
            2,
            "both success arms must commit through the session guard"
        );
        assert!(
            body.contains("self.speleodb_unwind_unless_kept("),
            "the unwind must route through the session-aware release"
        );
        assert!(
            !body.contains("self.speleodb_bind_edit_claim(") && !body.contains("set_edit_enabled"),
            "no arm may open the gate outside the guarded commit"
        );
    }

    #[test]
    fn every_file_backed_installer_finalizes_under_one_project_guard() {
        // Round 4: the delete's check-and-teardown holds the `project` mutex
        // and reasons "strictly before the load, or strictly after it". That
        // argument is only sound while every installer of a file-backed
        // project commits the slot AND its companion state (database handle,
        // undo stack, recents) inside one unbroken acquisition - a slot commit
        // in its own span reopens the torn-middle interleaving where the
        // delete tears down a half-built session and the loader finishes
        // building into an empty world. `create_project` is exempt: it
        // commits a project with no `file_path`, which the delete's identity
        // check can never match.
        for (file, func, database_commit, undo_commit) in [
            (
                "project_load.rs",
                "fn open_project_prepared_inner",
                " = Some(db);",
                "load_from_persisted_with_timestamps(",
            ),
            (
                "project_load.rs",
                "pub fn do_load_project_full",
                " = Some(db);",
                "load_from_persisted_with_timestamps(",
            ),
            (
                "project_create.rs",
                "fn do_create_project_full",
                " = db;",
                ".clear();",
            ),
        ] {
            let src = state_source(file);
            let start = src
                .find(func)
                .unwrap_or_else(|| panic!("{func} is present in {file}"));
            let body = &src[start..];
            let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

            let guard = body
                .find("let mut proj_guard")
                .unwrap_or_else(|| panic!("{func} takes the project guard by name"));
            let slot = body
                .find(" = Some(project")
                .unwrap_or_else(|| panic!("{func} commits the project slot"));
            let db_commit = body
                .find(database_commit)
                .unwrap_or_else(|| panic!("{func} commits the database handle"));
            let undo = body
                .find(undo_commit)
                .unwrap_or_else(|| panic!("{func} commits the undo stack"));
            let released = body
                .find("drop(proj_guard)")
                .unwrap_or_else(|| panic!("{func} releases the guard explicitly"));
            assert!(
                guard < slot && slot < db_commit && db_commit < undo && undo < released,
                "{func} must commit slot, database and undo history inside one \
                 project-guard span (guard at {guard}, slot {slot}, database \
                 {db_commit}, undo {undo}, drop {released})"
            );
            assert_eq!(
                body.matches("drop(proj_guard)").count(),
                1,
                "{func}: one release, at the end of the finalization"
            );
        }
    }

    #[test]
    fn every_deferred_release_is_reclaim_checked() {
        // Round 10 (P1): the project-switch and app-exit releases run on a
        // detached thread, which is exactly the delay a successor needs - the
        // switch is what lets the user reopen that project, and a Start Editing
        // on it INHERITS the still-held mutex and mints no claim. An unguarded
        // POST landing afterwards frees the lock under a live edit session. Every
        // release that can outlive the moment it was decided goes through the
        // reclaim check; this pins the one that had been left out.
        let src = state_source("speleodb_lock_release.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        let start = src
            .find("fn speleodb_spawn_lock_release")
            .expect("the detached release is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        assert!(
            body.contains("speleodb_stand_down_and_release(&p, entry_gen, claim)"),
            "the detached release must go through the stand-down-and-release \
             span, which is reclaim-checked like the delete's captured release \
             and the janitor's"
        );
        // No bare POST at all: even the no-managed-state fallback (tests,
        // pre-setup), which cannot run the reclaim check, still asks the server
        // who holds the lock first (lock#4).
        assert_eq!(
            body.matches("client::release_lock(").count(),
            0,
            "a lifecycle release is never sent without the holder check"
        );
        assert_eq!(
            body.matches("release_if_held_by_us(&p)").count(),
            1,
            "the fallback for when there is no app handle to reach managed state \
             through is holder-checked"
        );
        assert!(
            !body.contains("self.speleodb_shut_edit_gate("),
            "the gate transition belongs INSIDE the release's own lock-ops span \
             (round 12): shutting it here leaves a shut gate over a mutex the \
             server still says we hold, which is exactly what the status refresh's \
             inherited-lock reopen resurrects"
        );

        // ...and the span it routes to does all three under one guard.
        let own = state_source("speleodb_session.rs");
        let own = &own[..own.find("#[cfg(test)]").unwrap_or(own.len())];
        let start = own
            .find("fn speleodb_stand_down_and_release")
            .expect("the deferred-release span is present");
        let span = &own[start..];
        let span = &span[..span.find("\n    }\n").expect("fn body terminates")];
        let guard = span
            .find("self.speleodb_lock_ops_guard()")
            .expect("the span is serialized");
        let gate = span
            .find("self.speleodb_shut_edit_gate()")
            .expect("the span shuts the gate");
        let decide = span
            .find("self.speleodb_release_unless_reclaimed_locked(params)")
            .expect("the span runs the reclaim-checked release");
        assert!(
            guard < gate && gate < decide,
            "order must be: guard, then the gate, then the decision - the refresh \
             must have no moment between the shut gate and the verdict"
        );
    }

    #[test]
    fn logout_takes_the_lock_ops_guard_before_it_ends_the_session() {
        // The fast-forward arm commits on identity alone, and identity cannot see
        // a logout (the project slot never moves). What keeps that sound is this
        // guard: logout is a lock operation like the acquire and the captured
        // releases, so it cannot run inside a Start Editing span. Drop it here and
        // the identity-only commit silently goes back to reopening the edit gate
        // for a signed-out session (round 6).
        let src = state_source("speleodb_logout.rs");
        // Production region only, like every other scan in this family - and here
        // it also keeps the search off the test-only wrapper whose name starts
        // with the same words.
        let src = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
        let start = src
            .find("fn speleodb_logout(")
            .expect("the logout verb is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        let guard = body
            .find("self.speleodb_lock_ops_guard()")
            .expect("logout must serialize with every other SpeleoDB lock operation");
        let gates = body
            .find("self.speleodb_logout_drop_gates()")
            .expect("logout drops the gates and releases the lock");
        let forget = body
            .find(".forget()")
            .expect("logout forgets the credential");
        assert!(
            guard < gates && gates < forget,
            "the guard has to be taken BEFORE the gates drop and still be held \
             when the credential goes. Round 6 scoped it to the gate half alone, \
             which left a Start Editing free to take it the instant that half \
             returned, read the not-yet-forgotten token, and commit an edit \
             session for an account this call was in the middle of deleting"
        );
        // The gate half must NOT re-take it: the span belongs to the whole verb.
        let gate_half = src
            .find("fn speleodb_logout_drop_gates")
            .expect("the gate-drop half is present");
        let gate_body = &src[gate_half..];
        let gate_body = &gate_body[..gate_body.find("\n    }\n").expect("fn body terminates")];
        assert!(
            !gate_body.contains("self.speleodb_lock_ops_guard()"),
            "the gate half must inherit its caller's span, not open a narrower \
             one of its own (the mutex is not reentrant either)"
        );
        assert!(
            gate_body.contains("self.bump_speleodb_session()"),
            "logout is still a session boundary"
        );
    }

    #[test]
    fn logout_waits_for_an_in_flight_lock_operation() {
        // The tripwire above reads the source; this one proves the exclusion is
        // real, over the WHOLE verb rather than its gate half. With the lock-ops
        // guard held, a logout on another thread must not reach its session
        // boundary - if it does, it is landing in the middle of exactly the span
        // the guard exists to protect.
        //
        // `with_isolated_config` because logout writes the config (it clears the
        // rehydrate seed), and the handoff because the logout under test runs on a
        // SPAWNED thread - config resolution is per-thread, so without it the
        // deferred logout would write a config file this test cannot see.
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            let before = state.speleodb_session_generation();
            let (started, has_started) = std::sync::mpsc::channel();
            let (finished, has_finished) = std::sync::mpsc::channel();
            let config = crate::test_helpers::ConfigHandoff::capture();

            let lock_ops = state.speleodb_lock_ops_guard();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let _config = config.adopt();
                    started.send(()).expect("announce the thread is running");
                    let _ = state.speleodb_logout();
                    finished.send(()).expect("announce the logout got through");
                });

                has_started.recv().expect("the thread runs");
                assert!(
                    has_finished
                        .recv_timeout(std::time::Duration::from_millis(250))
                        .is_err(),
                    "a logout must not end the session while a lock operation \
                     holds the guard"
                );
                assert_eq!(
                    state.speleodb_session_generation(),
                    before,
                    "and it must not have published its boundary either"
                );

                drop(lock_ops);
                has_finished
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("the logout completes once the span ends");
            });
            assert_ne!(
                state.speleodb_session_generation(),
                before,
                "the logout still happens - it is deferred, never dropped"
            );
        });
    }

    #[test]
    fn the_delete_clears_its_own_claim_before_the_guard_drops() {
        let src = state_source("recent_projects.rs");
        let start = src
            .find("fn delete_project")
            .expect("delete_project is present");
        let body = &src[start..];
        let forget = body
            .find("self.speleodb_forget_lock_acquired_for(")
            .expect("the delete must clear its own claim at snapshot time");
        let teardown = body
            .find("self.close_open_project_locked(")
            .expect("the delete tears the session down");
        assert!(
            forget < teardown,
            "the claim clear must happen under the same guard as the snapshot, \
             before the teardown - a later clear cannot tell our record from a \
             successor's (F8)"
        );
    }
}
