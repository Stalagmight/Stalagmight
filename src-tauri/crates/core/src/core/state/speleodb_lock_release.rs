//! The SILENT, best-effort release paths for a held SpeleoDB write lock.
//!
//! These are the releases nobody asked for explicitly: closing a project,
//! switching to another project, and quitting the app. They exist so a normal
//! close/quit doesn't strand the server mutex locked for every other editor
//! (fix 1 / M1 / M2). Split out of `core::state::speleodb` so the one gate they
//! all share - [`AppState::speleodb_held_lock_release_params`] - and the
//! data-safety rule it enforces live in one small, readable place.
//!
//! The DELIBERATE (explicit) release verbs do NOT go through the veto:
//! `speleodb_done` (push, then release once the push is confirmed),
//! `speleodb_release_lock` / `speleodb_release_all_locks` (the held-lock prompt's
//! per-project / release-all), `speleodb_release_current_lock`, and the two
//! ABANDON verbs - `speleodb_unlink` and `speleodb_take_remote` - via
//! [`AppState::speleodb_release_held_lock_abandoning`]. A user who asks to drop
//! the lock still gets to drop it, un-pushed edits or not.
//!
//! `speleodb_logout` is NOT in that list: it releases through
//! [`AppState::speleodb_release_held_lock_best_effort`], veto included. Logging
//! out is not the same act as surrendering the lock - unlike the abandon verbs it
//! neither deletes the link row nor discards the edits - and the lock it leaves
//! held is recoverable, because the next login's scan raises the held-lock prompt
//! over exactly that project.
//!
//! The abandon variant exists because those two verbs are only ever reached WITH
//! un-pushed edits and deliberately walk away from them: unlink deletes the link
//! row (a lock kept back could never be released from this project again), and
//! take-remote discards the local edits by definition, behind the frontend's
//! second destroy-confirmation.
//!
//! `AppState::delete_project` is the third abandon caller and the only one that
//! cannot use a single verb: it destroys the `.stalx` the link row lives in, so it
//! has to snapshot with [`AppState::speleodb_delete_lock_release_params`] while the
//! database handle is still alive and release with
//! [`AppState::speleodb_release_captured_lock`] once its teardown - and the `project`
//! mutex - are out of the way. Destroying the file is the plainest abandon there is,
//! which is why its snapshot skips the un-pushed-edits veto.
//!
//! [#576 SDB-ACCT-D1, SDB-QUIT-D1] Every one of these hands back the lock the session
//! is EDITING under, however the account came to hold it: taken here, or adopted from
//! another Stalagmight session, another machine or the SpeleoDB website. How the user
//! acquired a lock is their responsibility. The silent releases and the delete used
//! to ask for this session's claim as well (decision RV2-R-13, #576 RV4-R-S2 and
//! RV5-F4-4), so an adopted lock was left held at quit.

use std::time::Duration;

use super::speleodb_session::KeptFor;
use super::AppState;
use crate::speleodb::client;
use crate::speleodb::SpeleoDbError;

/// [#576 NF-A8R-GAPS] Proof the delete cleared its own claim under the `project`
/// guard that decided the file's identity. Made only by
/// [`AppState::speleodb_forget_lock_acquired_under`] (the field is private to this
/// module), and the delete's deferred release
/// ([`AppState::speleodb_release_captured_lock`]) takes it: a delete that cleared
/// through the plain [`AppState::speleodb_forget_lock_acquired_for`] instead - which
/// stays, for the clears that run outside any slot - has nothing to hand that
/// release, and does not compile. Carries the claim it removed, for the
/// trash-failure path that puts it back.
#[must_use = "the delete's deferred release takes this proof, and a failed trash puts its claim back"]
pub(super) struct ClaimClearedUnderSlot {
    claim: Option<super::speleodb_session::LockClaim>,
}

impl ClaimClearedUnderSlot {
    /// The claim the clear removed, for the path that has to restore it.
    pub(super) fn into_claim(self) -> Option<super::speleodb_session::LockClaim> {
        self.claim
    }
}

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
    /// The persisted public-`http://` allowance, threaded to `tls::enforce`.
    pub(super) insecure_http: bool,
    /// The project whose write lock is being released.
    pub(super) project_id: String,
}

/// [#576 FZ-SDB-6-D1] What re-reading the holder says after a lock verb failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AfterLockVerb {
    /// The server answered the verb (a 4xx, a refusal, no session): it did not
    /// happen, and there is nothing to re-read.
    Answered,
    /// The reply may have been lost, and the re-read says the lock is this
    /// account's.
    Ours,
    /// The reply may have been lost, and the re-read says the lock is free (`None`)
    /// or another account's.
    NotOurs(Option<String>),
    /// The reply may have been lost and the re-read failed too (offline, server
    /// down). The caller keeps what it has; the next successful poll reconciles.
    Unknown,
}

/// Whether `err` can hide a reply the server applied: the connection failed or
/// timed out after the request may have gone out, the body was unreadable, or a
/// server or gateway error (5xx) stood in for the answer.
pub(super) fn reply_may_be_lost(err: &SpeleoDbError) -> bool {
    match err {
        SpeleoDbError::Transport { .. } | SpeleoDbError::Parse(_) => true,
        SpeleoDbError::Status { status, .. } => *status >= 500,
        _ => false,
    }
}

/// Whether a release may proceed over un-pushed local edits.
///
/// The same distinction [`super::speleodb_cache::LocalEditsPolicy`] draws for a
/// download-replace, drawn here for the lock: named at every call site so a reader
/// sees which act they are looking at instead of decoding a bare `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseIntent {
    /// A SILENT release nobody asked for (close, project switch, quit, logout):
    /// vetoed when the project still holds un-pushed edits.
    Silent,
    /// A DELIBERATE abandon of the project's SpeleoDB identity (`speleodb_unlink`,
    /// `speleodb_take_remote`): the user has said outright they are walking away
    /// from the local work, so there is no future push for the veto to protect.
    Abandon,
    /// [#576 RV5-F4-4] Deleting the OPEN project's local file
    /// (`AppState::delete_project`). An abandon of the local work, so no
    /// un-pushed-edits veto. [#576 SDB-ACCT-D1] It hands back the lock the session
    /// edits under, adopted or taken, like every other release here.
    DeleteLocal,
}

impl AppState {
    /// Snapshot of what a best-effort lock release needs, or `None` when no held
    /// remote edit session is active - or when releasing would strand local work.
    /// [#576 SDB-QUIT-D1] An edit session under a lock this session adopted from the
    /// account is a held one: its lock is released too.
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

    /// The snapshot for deleting the open project's file (#576 RV5-F4-4): no
    /// un-pushed-edits veto (the file those edits live in is being destroyed).
    pub(super) fn speleodb_delete_lock_release_params(&self) -> Option<LockReleaseParams> {
        self.speleodb_lock_release_params(ReleaseIntent::DeleteLocal)
    }

    /// Shared body of the two snapshots above. `intent` is what separates a silent
    /// release from a deliberate one.
    fn speleodb_lock_release_params(&self, intent: ReleaseIntent) -> Option<LockReleaseParams> {
        // #213: a force-editor is edit-enabled but holds NO lock, so there is nothing
        // to release on close / app-exit / project-switch.
        //
        // The edit gate is a UI state, and there IS one flow where it reads false over a
        // mutex that is still ours: adoption's `PushedLockHeld` closes the project and
        // has the frontend reload it, and a fresh load starts with the gate shut. Quit
        // inside that window and this returns `None` over a held lock. Deliberately not
        // special-cased. `scanLocksOnOpen` asks the SERVER who holds what on every app
        // open and login - `speleodb_list_projects` -> `lock_holder` -> the held-lock
        // resume prompt - so recovery does not depend on this gate, on `edit_enabled`,
        // or on the local claim. A lock outliving a session is a supported state here,
        // not an anomaly: the close dialog offers "Keep lock" outright, which is what
        // `LockClaim::kept` exists to protect. And the window only opens once a release
        // has ALREADY failed, so retrying it a second later on the same connectivity
        // buys little.
        if !self.edit_enabled() || self.force_editing() {
            return None;
        }
        // A `.ok()?` here used to collapse REAL failures into the ordinary "nothing
        // to release" no-op with no trace at all: `speleodb_open_link` returns an
        // `Io(..)` for a link row it cannot read as readily as `NoRemoteProject` (and,
        // before #576 A11 made every lock recover, for a poisoned database mutex). A
        // failure at close / switch / quit then stranded the server lock permanently
        // and silently - the exact outcome this module exists to prevent, and
        // indistinguishable in the log from a local project holding no lock. The
        // un-pushed-edits veto below has always logged; so must this.
        let link = match self.speleodb_open_link() {
            Ok(link) => link,
            Err(SpeleoDbError::NoRemoteProject) => return None,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    ?intent,
                    purpose = "release a held SpeleoDB write lock",
                    "speleodb: could not read the open project's link; the server lock \
                     (if any) is being left held"
                );
                return None;
            }
        };
        // [#576 SDB-QUIT-D1] No claim check: the open gate (not force-editing) is the
        // edit session, and its lock is released whether this session took it or
        // adopted it from the account (another window or computer, the website, this
        // app before a restart; decision SDB-ACCT-D1). This used to release only a
        // lock with this session's claim (decision RV2-R-13, #576 RV4-R-S2, and for a
        // delete RV5-F4-4), and the poll's reopen opened the gate without one, so a
        // clean close, switch, logout, replacing sign-in or quit left it held.
        //
        // Data-safety asymmetry, deliberate: never SILENTLY release a lock whose
        // project still has un-pushed local edits. The two failure modes are not
        // symmetric. Keeping the lock held is recoverable - the user reopens the
        // project and continues, pushes with Done, or releases on purpose - and the
        // server lock times out on its own anyway. Releasing while edits are
        // stranded locally is NOT: the project is immediately free for someone else
        // to lock and push, and the local edits can then only be reconciled through
        // a divergence. `speleodb_done` gets this right (it pushes first and only
        // releases once a re-check confirms the push landed); this gate is what the
        // silent paths get instead. The user-driven verbs listed in the module doc
        // bypass this gate on purpose, so "discard my edits and release" still works.
        if intent == ReleaseIntent::Silent && link.has_unpushed_edits {
            tracing::info!(
                project_id = %link.project_id,
                purpose = "avoid stranding un-pushed local edits behind a freed lock",
                "speleodb: skipping the silent lock release; the project has un-pushed edits"
            );
            // A DECISION, not an accident, and the record has to say so: once
            // this project closes, a claim with no project open is exactly what
            // the stranded-claim janitor cleans up - and it would hand back the
            // mutex this veto just protected, freeing the project for the next
            // editor to lock and push over work that is still only local.
            self.speleodb_keep_lock_deliberately(&link.project_id, KeptFor::UnpushedEdits);
            return None;
        }
        // Same reasoning as the link read above: anything `speleodb_active_token`
        // answers besides the ordinary `NotAuthenticated` is a real failure, and
        // dropping it silently here strands the lock just as thoroughly.
        let (endpoint, token) = match self.speleodb_active_token() {
            Ok(pair) => pair,
            Err(SpeleoDbError::NotAuthenticated) => return None,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %link.project_id,
                    ?intent,
                    purpose = "release a held SpeleoDB write lock",
                    "speleodb: could not read the session token; the server lock is \
                     being left held"
                );
                return None;
            }
        };
        Some(LockReleaseParams {
            endpoint,
            token,
            insecure_http: self.speleodb_insecure_http(),
            project_id: link.project_id,
        })
    }

    /// [#576 FZ-SDB-6-D1] After a lock verb (a release, Done's release, an acquire)
    /// failed, re-read who holds `project_id`'s lock AT ONCE when the reply may have
    /// been lost - the server can have applied the verb and lost only the answer, and
    /// the app's gate and claim must follow what the server did, not what the reply
    /// said. A failure the server answered is [`AfterLockVerb::Answered`], with no
    /// second request.
    ///
    /// The caller decides from the answer: a successful re-read is authoritative
    /// (gone or someone else's: edit mode closes; ours right after our own acquire:
    /// the claim is recorded), and a failed one keeps edit mode and the claim as
    /// they are, for the next successful status poll to reconcile.
    pub(super) fn speleodb_reread_after_lock_verb(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
        err: &SpeleoDbError,
    ) -> AfterLockVerb {
        if !reply_may_be_lost(err) {
            return AfterLockVerb::Answered;
        }
        let me = self.speleodb.lock().email.clone();
        match self.speleodb_lock_holder_checked(endpoint, token, insecure_http, project_id) {
            Ok(holder) if super::speleodb::lock_held_by_me(holder.as_deref(), me.as_deref()) => {
                AfterLockVerb::Ours
            }
            Ok(holder) => AfterLockVerb::NotOurs(holder),
            Err(reread) => {
                tracing::info!(
                    project_id,
                    error = %err,
                    reread = %reread,
                    purpose = "re-read the lock holder after a lost reply (#576 FZ-SDB-6)",
                    "speleodb: the lock verb's reply was lost and the holder could not be \
                     re-read; keeping the edit session as it is until the next poll"
                );
                AfterLockVerb::Unknown
            }
        }
    }

    /// Record that the OPEN project's lock is being held past this close ON
    /// PURPOSE - the close dialog's "Keep lock".
    ///
    /// Called INSTEAD of a release, and before the teardown, because it reads
    /// the link row the teardown is about to drop the handle on. A no-op when
    /// there is no held lock to keep (a local project, a read-only open, a
    /// force-editor). See [`LockClaim::kept`](super::speleodb_session::LockClaim)
    /// for what the mark is for: without it the janitor cannot tell this from a
    /// session that died holding a lock, and hands the mutex back on the next
    /// status poll.
    pub fn speleodb_keep_open_lock_deliberately(&self) {
        if !self.edit_enabled() || self.force_editing() {
            return;
        }
        if let Ok(link) = self.speleodb_open_link() {
            tracing::info!(
                project_id = %link.project_id,
                purpose = "honour the user's Keep lock choice past the close",
                "speleodb: keeping the write lock held after closing the project"
            );
            self.speleodb_keep_lock_deliberately(&link.project_id, KeptFor::UserChoice);
        }
    }

    /// Best-effort release of a held SpeleoDB write lock when a remote EDIT session
    /// is closed, so a normal close/quit doesn't strand the server mutex locked for
    /// every other editor (fix 1). Unlike [`AppState::speleodb_logout`] it keeps the
    /// token and the edit gate (the caller's close teardown resets the gate) - it
    /// only surrenders the server lock. Reads the link BEFORE the caller tears down
    /// the database. The keep-lock close path skips this by passing
    /// `release_lock = false` to the close command. A read-only-open or local
    /// project holds no lock, so this is a no-op there. Synchronous: acceptable on
    /// the close command (Tauri runs it off the main thread); the app-exit and
    /// project-switch paths use the bounded / detached variants below instead.
    pub fn speleodb_release_held_lock_best_effort(&self) {
        if let Some(p) = self.speleodb_held_lock_release_params() {
            self.speleodb_release_best_effort(
                &p.endpoint,
                &p.token,
                p.insecure_http,
                &p.project_id,
            );
        }
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
            self.speleodb_release_best_effort(
                &p.endpoint,
                &p.token,
                p.insecure_http,
                &p.project_id,
            );
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
    /// `we_hold_the_lock` into `crate::speleodb::sync::poll_verdict`, which without it
    /// refuses to explain a commit in the `BASE..tip` gap as our own landed push. The
    /// concrete outcome is that OUR OWN next push comes back as
    /// `RemoteState::Conflict` - a self-conflict against our own commit on a project
    /// nobody else touched - with the status mark routing the user's next click into the
    /// divergence dialog, which is exactly the misreport the record was introduced to
    /// stop.
    ///
    /// Comparing project ids is what closes THIS window, and that is not in tension
    /// with `AppState::speleodb_forget_lock_acquired_if_gate_shut`, whose doc says at
    /// length that an id comparison does NOT close ITS race. The races are different
    /// and neither test substitutes for the other:
    ///
    /// - There, the poll and the fast-forward that can be trampled are on the SAME
    ///   project, so the ids always match and the clear fires regardless; only
    ///   re-reading the edit GATE separates them.
    /// - Here the interloper is by definition a DIFFERENT project - there is one
    ///   `project` mutex and one open project, and the old one was torn down before
    ///   this runs - so the id IS the signal. The gate is no use at all: after the
    ///   delete it is shut for the project being released and OPEN for whatever
    ///   session took its place, so a gate test would either never fire or fire on the
    ///   wrong session's record.
    ///
    /// So: do not unify the two behind one condition. Doing so reintroduces one of the
    /// two bugs, whichever condition loses.
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
    /// since the same poisoned mutex fails the token read the poll needs to identify a
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

    /// [`Self::speleodb_forget_lock_acquired_for`] for a caller that knows the
    /// account whose credentials took the mutex, and must not ask the live session
    /// who that was.
    ///
    /// The sibling resolves the identity from whoever is signed in at call time,
    /// which is right for every caller that released a moment ago with those same
    /// credentials. It is wrong for one that CAPTURED a token earlier and may be
    /// running after a sign-out or an account switch: the live session then names
    /// nobody, or somebody else, so the claim the caller means to end is not the
    /// one the filter looks for and survives untouched.
    ///
    /// That survival is not inert. The reclaim check
    /// ([`AppState::speleodb_claim_on_deployment`]) is deliberately account-blind -
    /// a captured release may outlive the sign-in that captured it, so its question
    /// is "has any session in this process claimed this mutex?" - which means a
    /// stale claim is exactly the shape that stands a later detached close or
    /// delete release down, leaving a mutex held that nothing will now free.
    /// [`Self::speleodb_forget_lock_acquired_for`] for `AppState::delete_project`,
    /// which must clear its own claim while the `project` guard that decided the
    /// file's identity is still held: after the guard drops, a successor Start Editing
    /// on the same remote project can mint a claim of its own, and a later clear
    /// cannot tell ours from it (F8). `_slot` is that guard's slot - so the clear
    /// cannot be written outside the guard's block - and the proof it returns is what
    /// the delete's deferred release takes ([`ClaimClearedUnderSlot`]).
    pub(super) fn speleodb_forget_lock_acquired_under(
        &self,
        _slot: &super::project_slot::ProjectSlot,
        project_id: &str,
    ) -> ClaimClearedUnderSlot {
        ClaimClearedUnderSlot {
            claim: self.speleodb_forget_lock_acquired_inner(project_id, None),
        }
    }

    pub(super) fn speleodb_forget_lock_acquired_as(
        &self,
        _lock_ops: &super::speleodb_session::LockOpsHeld<'_>,
        project_id: &str,
        endpoint: &str,
        email: &str,
    ) -> Option<super::speleodb_session::LockClaim> {
        self.speleodb_forget_lock_acquired_inner(project_id, Some((endpoint, email)))
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
        let mut session = self.speleodb.lock();
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
    ///   [`AppState::speleodb_delete_lock_release_params`] as the first one.
    /// - It has to surface a failure to the user, not just the log. Every other
    ///   caller leaves a lock whose `.stalx` (and link row) survive, so the release
    ///   is retryable from the project itself; a delete destroys exactly that, and
    ///   the user's only remaining route to the stranded mutex is the held-lock
    ///   prompt - which they will not look for unless they are told.
    ///
    /// The DELETE snapshot is the one to pair this with: a delete forfeits any
    /// un-pushed edits by definition, so the veto that protects them on
    /// close / quit / switch has nothing left to protect and would only strand the
    /// server lock; it still releases only a lock this session acquired.
    ///
    /// The same deferral is why the acquisition-record clear below is CONDITIONAL,
    /// unlike the one every other release verb makes - see
    /// [`AppState::speleodb_forget_lock_acquired_for`].
    ///
    /// [#576 NF-A8R-GAPS] `_cleared` is that clear's proof, so the release cannot be
    /// reached by a delete that cleared any other way.
    pub(super) fn speleodb_release_captured_lock(
        &self,
        _cleared: &ClaimClearedUnderSlot,
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
    /// no-op) and mints no claim, so an unguarded POST landing afterwards frees
    /// the mutex out from under a live edit session - editable, with no server
    /// lock behind it and nothing recording one. Going through
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
        // What is NOT captured here is the GATE. It comes off inside the
        // release's own lock-ops span, because shutting it here opens a window
        // the status poll walks straight into: a shut gate over a mutex the
        // server still says this account holds is exactly what its
        // inherited-lock reopen exists for, so the poll takes the guard first,
        // reopens the gate, and the release then stands down for a session that
        // is about to be torn down anyway - lock stranded. Left open, there is
        // nothing for the poll to resurrect, and the release shuts it under the
        // same guard that then decides (`speleodb_stand_down_and_release`).
        let (p, claim, entry_gen) = {
            let _proj_guard = self.project.lock();
            let p = self.speleodb_held_lock_release_params()?;
            let claim = self.speleodb_forget_lock_acquired_for(&p.project_id);
            (p, claim, self.speleodb_session_generation())
        };
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
                    let _ = tx.send(state.speleodb_stand_down_and_release(&p, entry_gen, claim));
                }))
            });
            if !ran {
                // No managed state to consult (unit tests, pre-`setup`): the raw
                // POST this always did.
                let _ = tx.send(
                    client::release_lock(&p.endpoint, &p.token, p.insecure_http, &p.project_id)
                        .map(|_| ()),
                );
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

    /// [#576 SDB-ACCT-D1] Release, on a detached thread, a lock the un-pushed-edits
    /// veto kept at a project switch or close, once a push that was in flight then
    /// has landed and left that copy clean
    /// (`AppState::speleodb_take_claim_kept_for_unpushed_edits` took the claim and
    /// the params). The way the switch releases a clean copy
    /// ([`Self::speleodb_release_held_lock_detached`]): off the caller's thread,
    /// reclaim-checked - a session that has reopened the project and claimed its
    /// lock since keeps it - and with the claim put back when the POST fails, so
    /// the janitor can retry. No gate to shut: the session that edited under it is
    /// gone. Without managed state (unit tests, pre-`setup`) the raw POST.
    pub(super) fn speleodb_release_kept_lock_detached(
        &self,
        params: LockReleaseParams,
        claim: super::speleodb_session::LockClaim,
    ) {
        let sink = self.get_event_sink();
        std::thread::spawn(move || {
            let ran = sink.is_some_and(|sink| {
                // Clones for the fallback below: a `false` return means the
                // closure was never run.
                let p = params.clone();
                sink.with_app_state(Box::new(move |state| {
                    if let Err(e) = state.speleodb_release_unless_reclaimed(&p) {
                        tracing::warn!(
                            error = %e,
                            project_id = %p.project_id,
                            purpose = "release a lock kept over edits a landed push has uploaded",
                            "speleodb: the release failed; the claim is kept for a retry"
                        );
                        state.speleodb_restore_lock_claim(Some(claim));
                    }
                }))
            });
            if !ran {
                if let Err(e) = client::release_lock(
                    &params.endpoint,
                    &params.token,
                    params.insecure_http,
                    &params.project_id,
                ) {
                    tracing::warn!(
                        error = %e,
                        project_id = %params.project_id,
                        purpose = "release a lock kept over edits a landed push has uploaded",
                        "speleodb: the release failed"
                    );
                }
            }
        });
    }

    /// Release a held write lock with a hard deadline, for the app-exit path: run
    /// the network release on a detached thread and wait at most `deadline`. A hung
    /// / unreachable server must not freeze quit for the full HTTP timeout; on
    /// timeout we return and let the process exit (the server lock times out on its
    /// own). (fix M2). No-op when no lock is held.
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
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit test fixtures fail loudly on setup errors; the panic POISONS a mutex on purpose"
)]
#[expect(
    clippy::print_stdout,
    reason = "the driven A8 tests print the values they observed, so a failure shows them"
)]
mod tests {
    use super::AppState;

    #[test]
    fn a_poisoned_database_lock_declines_the_release_without_panicking() {
        // A poisoned mutex is not "there is nothing to release". Both reads in
        // `speleodb_lock_release_params` used to be `.ok()?`, which folded a failed
        // read into the ordinary no-op with no trace at all - so a poisoned mutex at
        // close / switch / quit stranded the server lock permanently and silently,
        // which is the exact outcome this module exists to prevent. The failures are
        // now matched and logged (the veto three lines down always was), and since
        // #576 A11 the poisoned mutex itself is recovered rather than failed.
        //
        // With no project open the answer is `None` either way, so what is pinned here
        // is that the poisoned path is reached and handled rather than panicking, and
        // the log side is pinned by `both_release_param_reads_log_their_real_failures`.
        let state = AppState::new().expect("state");
        state.set_edit_enabled(true);
        let db = &state.database;
        let _ = std::thread::scope(|s| {
            s.spawn(|| {
                let _guard = db.lock();
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

    /// Whether a failure was LOGGED is not observable from the return value (both arms
    /// are `None`), and silence is the whole defect: an operator looking at why a project
    /// stayed locked forever had nothing to go on. A link row that cannot be read is a
    /// real failure and must warn; a project with no link at all, or a signed-out
    /// session, is the ordinary no-op and stays quiet.
    ///
    /// [#576 A8] Was a source scan counting the body's `warn!`s; the reads now fail
    /// for real under a captured subscriber. (The token read's own warn arm is not
    /// reachable today: `SpeleoDbSession::active_token` answers only
    /// `NotAuthenticated`, the ordinary no-op.)
    #[test]
    fn an_unreadable_link_is_logged_and_an_ordinary_no_op_is_not() {
        use crate::core::state::speleodb::{logged_events, remote_readonly_fixture};
        let (temp, broken) = remote_readonly_fixture();
        rusqlite::Connection::open(temp.path().join("remote.stalx"))
            .expect("open the fixture's file")
            .execute_batch("ALTER TABLE speleo_link RENAME COLUMN project_name TO project_title;")
            .expect("break the link row");
        broken.set_edit_enabled(true);
        let mut unreadable = Some(LockReleaseProbe::NotRun);
        let unreadable_logs = logged_events(|| {
            unreadable = Some(LockReleaseProbe::of(
                broken.speleodb_held_lock_release_params(),
            ));
        });

        let (_signed_out_dir, signed_out) = remote_readonly_fixture();
        signed_out.set_edit_enabled(true);
        let mut ordinary = Some(LockReleaseProbe::NotRun);
        let ordinary_logs = logged_events(|| {
            ordinary = Some(LockReleaseProbe::of(
                signed_out.speleodb_abandon_lock_release_params(),
            ));
        });
        println!(
            "unreadable link -> {unreadable:?}, logged {unreadable_logs:?}; signed out -> \
             {ordinary:?}, logged {ordinary_logs:?}"
        );
        assert_eq!(unreadable, Some(LockReleaseProbe::NoRelease));
        assert!(
            unreadable_logs.iter().any(|line| line.contains("WARN")
                && line.contains("could not read the open project's link")),
            "a link the release cannot read is a real failure, and it must say so"
        );
        assert_eq!(ordinary, Some(LockReleaseProbe::NoRelease));
        assert!(
            !ordinary_logs.iter().any(|line| line.contains("WARN")),
            "signed out is the ordinary no-op, not a failure to report"
        );
    }

    /// What a release-params read answered, for the assertions above.
    #[derive(Debug, PartialEq, Eq)]
    enum LockReleaseProbe {
        NotRun,
        NoRelease,
        Release,
    }

    impl LockReleaseProbe {
        fn of(params: Option<super::LockReleaseParams>) -> Self {
            if params.is_some() {
                Self::Release
            } else {
                Self::NoRelease
            }
        }
    }
}
