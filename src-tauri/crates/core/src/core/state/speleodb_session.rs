//! The SpeleoDB SESSION generation and the guards built on it.
//!
//! There is no session object in this codebase: what the UI calls an edit
//! session is the `project` slot, two gate booleans and a `lock_acquired_here`
//! record, none of which can tell "the same project, before and after a
//! teardown" apart (`docs/plans/2026-07-30_speleodb-lock-session-races.md`).
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
//!   the very lock the stale release would free (F8). The check has TWO eyes,
//!   because a successor accounts for the lock in two shapes: a claim record
//!   (its probe proved the mutex unheld, so it recorded an acquisition) and a
//!   live edit session WITHOUT one (its probe saw the account still holding -
//!   the pending release had not POSTed - so it inherited, and an inheritance
//!   minted no record). Reading only the record frees the mutex out from under
//!   exactly the inherited successors the stand-down exists for - see
//!   [`AppState::speleodb_live_session_accounts_for`]. [#576 SDB-ACCT-D1] An
//!   inheritance now records an ADOPTED claim with its gate, so the record eye
//!   sees it too; the live-session eye stays for a gate open without a record.
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
//! - The status poll's inherited-lock gate reopen, across its holder re-read
//!   and the gate ([`AppState::speleodb_reopen_gate_for_inherited_lock`]).
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
//! 3. NOT a bump: an undo rollback (`restore_snapshot_into` restores the SAME
//!    open project - footgun 5.7), and every save (`save_project` never
//!    replaces the slot; a Save a Copy does not even move `file_path`).
//!
//! The bump-site inventory is pinned by `bump_sites_are_exactly_the_known_set`
//! below - a new project-slot writer that forgets to classify itself trips it.

use std::sync::atomic::Ordering;

use super::speleodb::local_dto;
use super::speleodb_lock_release::LockReleaseParams;
use super::AppState;
use crate::speleodb::auth::normalize_endpoint;
use crate::speleodb::{client, SpeleoDbError, SpeleoErrorDto};

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
#[derive(Debug, Clone)]
pub(super) struct LockClaim {
    /// The project whose server mutex this process took.
    pub(super) project_id: String,
    /// The SpeleoDB deployment it was taken on.
    endpoint: String,
    /// The account it was taken as.
    email: String,
    /// Whether this lock is being held back ON PURPOSE - the close dialog's
    /// "Keep lock", or the un-pushed-edits veto declining a silent release.
    ///
    /// The janitor exists for claims whose session died without ever deciding;
    /// these decided. Left unmarked they look identical once the project is
    /// closed - a claim, no project open - and the very next status poll would
    /// hand back a mutex the user asked to keep, freeing a project whose local
    /// work has not been pushed for the next editor to lock and push over.
    ///
    /// Cleared by a fresh acquisition, which replaces the whole claim: taking
    /// the lock again is a new decision, and the next release is the ordinary
    /// one.
    ///
    /// [#576 SDB-ACCT-D1] Carries WHY, because one of the two reasons can lapse: a
    /// push in flight when the veto kept the lock can land afterwards and leave the
    /// copy clean, and that lock is then released the way the silent release would
    /// have released a clean copy ([`KeptFor::UnpushedEdits`]). The user's "Keep
    /// lock" never lapses that way.
    kept: Option<KeptFor>,
    /// [#576 RV4-R-S3] Whether this claim records a lock the held-lock prompt's
    /// "Continue editing" RESUMED rather than one this process acquired.
    ///
    /// A resumed claim licenses the release (Done hands the lock back, decision
    /// RV2-R-13-RESTART) but NOT the authorship shortcut (#576 B32): the prompt
    /// lists every lock the account holds at sign-in, so resuming one can happen
    /// on a second computer while the first is still editing under it, and that
    /// session must not explain the first one's commits away as its own push.
    resumed: bool,
}

/// Why a claim's lock is being held back on purpose (see [`LockClaim::kept`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeptFor {
    /// The close dialog's "Keep lock": the user asked for it.
    UserChoice,
    /// The un-pushed-edits veto declined a silent release (close, project switch,
    /// logout, quit): the copy held edits that were not on the server.
    UnpushedEdits,
}

impl LockClaim {
    /// A claim on `project_id`, stamped with the account it was taken under.
    ///
    /// `endpoint` and `email` must be read from the SAME session-lock
    /// acquisition that stores the result, so the stamp cannot name an account
    /// other than the one whose token the acquire used.
    pub(super) fn taken(project_id: &str, endpoint: String, email: String) -> Self {
        Self {
            project_id: project_id.to_owned(),
            endpoint,
            email,
            kept: None,
            resumed: false,
        }
    }

    /// This claim, marked as a lock "Continue editing" resumed (see
    /// [`LockClaim::resumed`]).
    pub(super) fn into_resumed(self) -> Self {
        Self {
            resumed: true,
            ..self
        }
    }

    /// Whether this claim records a resumed lock (see [`LockClaim::resumed`]).
    pub(super) fn is_resumed(&self) -> bool {
        self.resumed
    }

    /// Whether this lock is deliberately held back (see [`LockClaim::kept`]).
    pub(super) fn is_kept(&self) -> bool {
        self.kept.is_some()
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

/// [#576 A8 RV8a-5] The lock-ops span's proof and its one producer, apart: nothing
/// outside this module can build a [`LockOpsHeld`] from a guard of its own.
///
/// [#576 RV10a-3] The borrow is proven by the suite, not by a captured compile
/// error: the doctest below compiles THIS module (the file is `include!`d, over a
/// stand-in `AppState`) and must fail to compile, because it ends the span while
/// a value read in it is still in use (E0505). It never names the lifetime, so
/// taking `'s` off [`InSpan`] - or tying `read_account`'s value to anything but
/// the span's borrow - makes it compile, and the suite fails. The twin after it
/// is the same code in the allowed order, and must compile and run: a broken
/// stand-in or include path fails there, so the first cannot pass vacuously.
///
/// ```compile_fail
/// # // The REAL module below (`include!`), over a stand-in `AppState` that has
/// # // only what the module reads. Paths mirror the crate's, so its
/// # // `pub(in crate::core::state)` items resolve as they do there.
/// # mod speleodb {
/// #     #[derive(Debug)]
/// #     pub struct SpeleoDbError;
/// # }
/// # mod core {
/// # pub mod state {
/// #     pub struct AppState {
/// #         speleodb_lock_ops: Ops,
/// #     }
/// #     struct Ops(std::sync::Mutex<()>);
/// #     impl Ops {
/// #         fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
/// #             self.0.lock().unwrap()
/// #         }
/// #     }
/// #     impl AppState {
/// #         pub fn stand_in() -> Self {
/// #             Self { speleodb_lock_ops: Ops(std::sync::Mutex::new(())) }
/// #         }
/// #         fn speleodb_active_token(
/// #             &self,
/// #         ) -> Result<(String, String), crate::speleodb::SpeleoDbError> {
/// #             Ok(("https://speleodb.example".into(), "token".into()))
/// #         }
/// #         fn speleodb_lock_holder_checked(
/// #             &self,
/// #             _: &str,
/// #             _: &str,
/// #             _: bool,
/// #             _: &str,
/// #         ) -> Result<Option<String>, crate::speleodb::SpeleoDbError> {
/// #             Ok(None)
/// #         }
/// #     }
/// #     pub mod speleodb_session {
/// #         use super::AppState;
/// #         mod lock_ops_span {
/// #             include!("speleodb_session/lock_ops_span.rs");
/// #         }
/// #         pub fn one_span(state: &AppState) {
/// let span = state.speleodb_lock_ops_guard();
/// let account = span.read_account();
/// drop(span); // ends the span while `account` is alive
/// assert!(account.get().is_ok());
/// #         }
/// #     }
/// # }
/// # }
/// # fn main() {
/// #     crate::core::state::speleodb_session::one_span(&crate::core::state::AppState::stand_in());
/// # }
/// ```
///
/// ```
/// # // The REAL module below (`include!`), over a stand-in `AppState` that has
/// # // only what the module reads. Paths mirror the crate's, so its
/// # // `pub(in crate::core::state)` items resolve as they do there.
/// # mod speleodb {
/// #     #[derive(Debug)]
/// #     pub struct SpeleoDbError;
/// # }
/// # mod core {
/// # pub mod state {
/// #     pub struct AppState {
/// #         speleodb_lock_ops: Ops,
/// #     }
/// #     struct Ops(std::sync::Mutex<()>);
/// #     impl Ops {
/// #         fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
/// #             self.0.lock().unwrap()
/// #         }
/// #     }
/// #     impl AppState {
/// #         pub fn stand_in() -> Self {
/// #             Self { speleodb_lock_ops: Ops(std::sync::Mutex::new(())) }
/// #         }
/// #         fn speleodb_active_token(
/// #             &self,
/// #         ) -> Result<(String, String), crate::speleodb::SpeleoDbError> {
/// #             Ok(("https://speleodb.example".into(), "token".into()))
/// #         }
/// #         fn speleodb_lock_holder_checked(
/// #             &self,
/// #             _: &str,
/// #             _: &str,
/// #             _: bool,
/// #             _: &str,
/// #         ) -> Result<Option<String>, crate::speleodb::SpeleoDbError> {
/// #             Ok(None)
/// #         }
/// #     }
/// #     pub mod speleodb_session {
/// #         use super::AppState;
/// #         mod lock_ops_span {
/// #             include!("speleodb_session/lock_ops_span.rs");
/// #         }
/// #         pub fn one_span(state: &AppState) {
/// let span = state.speleodb_lock_ops_guard();
/// let account = span.read_account();
/// assert!(account.get().is_ok());
/// drop(span);
/// #         }
/// #     }
/// # }
/// # }
/// # fn main() {
/// #     crate::core::state::speleodb_session::one_span(&crate::core::state::AppState::stand_in());
/// # }
/// ```
mod lock_ops_span;

pub(super) use lock_ops_span::{InSpan, LockOpsHeld};

/// [#576 A8 NF-A8-BYPASS] Why a reclaim-checked release may be decided where it is:
/// [`AppState::speleodb_release_unless_reclaimed_locked`] takes one. The deferred
/// release's is the stand-down's [`stand_down::StoodDown`]; the releases with no gate of
/// their own to shut (the session-moved unwind, a captured release, the janitor) each
/// hold a function-local marker no other flow can name. Implementing it anywhere else
/// is the bypass it exists to stop.
pub(super) trait ReclaimRelease {}

/// [#576 A8] A deferred release's stand-down as a type: the gate closes BEFORE the
/// reclaim decision, inside one lock-ops span, because the decision takes the proof
/// only the stand-down makes.
mod stand_down {
    use super::{AppState, LockOpsHeld, ReclaimRelease};

    /// Proof the releasing session has been stood down from the lock.
    pub(super) struct StoodDown {
        _gate_shut: (),
    }

    impl ReclaimRelease for StoodDown {}

    impl AppState {
        /// Shut the gate for the session that decided the release - if it is still
        /// the session on screen (`entry_gen` unchanged); a successor's gate is not
        /// this release's to touch. Under `project`, inside the caller's span.
        pub(super) fn speleodb_stand_down(
            &self,
            _lock_ops: &LockOpsHeld<'_>,
            entry_gen: u64,
        ) -> StoodDown {
            let _proj_guard = self.project.lock();
            if self.speleodb_session_generation() == entry_gen {
                self.set_edit_enabled(false);
            }
            StoodDown { _gate_shut: () }
        }
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

    /// [#576 NF-W4C-3-EDIT] Shut the edit gate for an Ariane adoption's hand-over,
    /// in the current session generation (see `AppState::speleodb_adoption_gate`).
    /// Called inside the adoption's `command_lock` span, so no edit can land between
    /// the span and the gate.
    pub(super) fn speleodb_shut_gate_for_adoption(&self) {
        let generation = self.speleodb_session_generation();
        self.speleodb_adoption_gate
            .store(generation.wrapping_add(1), Ordering::SeqCst);
    }

    /// Whether an adoption's hand-over holds the edit gate shut in the current
    /// session generation.
    pub(super) fn speleodb_adoption_holds_gate(&self) -> bool {
        let generation = self.speleodb_session_generation();
        self.speleodb_adoption_gate.load(Ordering::SeqCst) == generation.wrapping_add(1)
    }

    /// Lift the adoption's hold on the edit gate: the adoption has returned.
    pub(super) fn speleodb_open_gate_after_adoption(&self) {
        self.speleodb_adoption_gate.store(0, Ordering::SeqCst);
    }

    /// The generation and the open project's validated link, read together
    /// under the `project` mutex so they describe ONE world. Start Editing's
    /// entry point: capturing them separately is D6 from the abandoned attempt
    /// (a teardown between the two reads pairs the old world's generation with
    /// the new world's link, and the eventual refusal then unwinds the lock of
    /// the project the user actually has open).
    ///
    /// [#576 A8] Inside the lock-ops span, by its proof: every fact Start Editing
    /// decides on is read after the span opens.
    pub(super) fn speleodb_session_snapshot(
        &self,
        _lock_ops: &LockOpsHeld<'_>,
    ) -> Result<(u64, crate::models::SpeleoLink), SpeleoErrorDto> {
        let _proj_guard = self.project.lock();
        let generation = self.speleodb_session_generation();
        let link = self.speleodb_open_link().map_err(local_dto)?;
        super::speleodb::validate_project_id(&link.project_id).map_err(local_dto)?;
        Ok((generation, link))
    }

    /// Start Editing's commit: open the edit gate, mint the claim (this flow's
    /// acquisition when `acquired_here`, otherwise an adopted lock's resumed claim,
    /// #576 SDB-ACCT-D1), and clear force-edit - IF the session the caller
    /// started from is still the session on screen. All under one `project`
    /// acquisition, which is what makes the check and the commit atomic against
    /// every bump site (rule 1).
    ///
    /// `entry_generation`:
    /// - `Some(gen)` - the UpToDate arm: refuse if ANY session boundary passed
    ///   since the entry snapshot.
    /// - `None` - the fast-forward arm, whose own in-place reload IS a session
    ///   boundary (close + reopen), so a generation captured at entry is
    ///   guaranteed stale by the time it commits. Identity does the work
    ///   instead: the open project must still be linked to `project_id`. The
    ///   residual this accepts is benign by construction - if a teardown AND a
    ///   re-open of the SAME project raced the reload, the commit lands on a
    ///   world where that project is open at its canonical path and this
    ///   process genuinely holds its mutex (the acquire happened and nothing
    ///   released it), so gate + record + lock still agree.
    ///
    ///   What makes identity ENOUGH is not identity: it is that the one boundary
    ///   identity cannot see is excluded from the caller's span. Every other
    ///   boundary moves the project slot (close, delete, switch), so the link
    ///   comparison below catches it. LOGOUT does not - it ends the session and
    ///   leaves the slot exactly where it was - and a logout landing after the
    ///   reload therefore passed this check and reopened the edit gate for a
    ///   signed-out user, on a remote project whose mutex this process still held
    ///   (round 6). It cannot land there any more: logout takes
    ///   `speleodb_lock_ops`, which Start Editing holds from before its first
    ///   read through this commit, so it runs strictly before the flow (whose
    ///   token read then fails) or strictly after it. Anything that gives a
    ///   session boundary a way past that guard puts this arm back where it was.
    ///
    /// On refusal nothing has been committed: the gate stays as the successor
    /// world set it, and no record is minted. The caller decides what to do
    /// with the server lock it acquired - see
    /// [`AppState::speleodb_unwind_release`].
    ///
    /// [#576 A8] Takes the span's proof: "Start Editing holds `speleodb_lock_ops` from
    /// before its first read through this commit" - and the update fast-forward's
    /// restore with it - is the type's, so a commit outside the span does not compile.
    pub(super) fn speleodb_commit_editing(
        &self,
        _lock_ops: &LockOpsHeld<'_>,
        entry_generation: Option<u64>,
        project_id: &str,
        acquired_here: bool,
    ) -> Result<(), SpeleoDbError> {
        // [#576 NF-W4C-2] `command_lock` first (it sits below `speleodb_lock_ops`,
        // which every caller holds, and above `project`): the commit's link write
        // (the re-verify stamp below) must not land inside an autosave, which copies
        // the link at its start and renames a fresh file over the live one. No
        // caller holds `command_lock` here, and nothing below makes a network call.
        let _commands = self.hold_commands();
        let _proj_guard = self.project.lock();
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
        self.set_edit_enabled(true);
        // AFTER the gate opens, never before - a status poll landing in between
        // would see a shut gate and clear the record again
        // (`speleodb_forget_lock_acquired_if_gate_shut`).
        if acquired_here {
            // Recorded as THIS flow's acquisition, so the stale-context unwind
            // hands back the lock this operation took rather than whichever of
            // the account's claims it happened to find first.
            self.speleodb_note_acquisition_by_this_flow(project_id);
        } else {
            // [#576 SDB-ACCT-D1, SDB-QUIT-D1] A lock the ACCOUNT already held - another
            // Stalagmight session, another machine, the SpeleoDB website - is the
            // user's, and editing under it is an edit session like any other: it is
            // claimed for this session, so Done, a close, a switch, a logout, a
            // replacing sign-in and the quit hand it back as they hand back one this
            // session took. ADOPTED, not acquired: the claim is marked resumed, so it
            // licenses the release but not the authorship shortcut (#576 RV4-R-S3),
            // and it is not this flow's acquisition, so the stale-context unwind
            // still never hands it back. A claim this session already holds on the
            // project is kept as it is: a second Start Editing over a lock taken here
            // finds it "ours" too, and must not downgrade that claim to an adopted one.
            if !self.speleodb_lock_acquired_here(project_id) {
                self.speleodb_note_resumed_lock(project_id);
            }
        }
        // #213: we now genuinely hold the lock, so clear force-edit - the
        // "reconnect" transition for a force-editor, a no-op otherwise.
        self.set_force_editing(false);
        // #202: a fresh-verdict commit (UpToDate) confirmed BASE == tip as of the
        // acquire, so start the lazy re-verify window here (best-effort; a failed
        // stamp just makes the first edit re-verify sooner). The fast-forward arm
        // passes `None` and does NOT stamp here - its reload already re-stamped
        // the link, `last_verify_at` included.
        if entry_generation.is_some() {
            let _ = self.speleodb_mutate_link(|l| {
                l.last_verify_at = Some(chrono::Utc::now().to_rfc3339());
            });
        }
        Ok(())
    }

    /// Forget the acquisition record IF the edit gate is (still) shut. The poll's
    /// catch-all for the release paths that do not run through a release verb:
    /// close, project switch, logout, take-remote, the 401 auto-drop, the
    /// lost-lock re-verify.
    ///
    /// Re-reads the gate HERE, under the session lock, instead of trusting the
    /// caller's earlier snapshot - which a fast-forward finishing underneath an
    /// in-flight poll makes stale. That fast-forward reopens the gate and
    /// restores a record for a lock we genuinely still hold; acting on the stale
    /// snapshot would erase it and cost the authorship shortcut on our own lock.
    /// Comparing project ids instead does NOT close this: the poll and the
    /// fast-forward are on the same project, so the ids match and the clear
    /// fires anyway. Which is exactly why this is NOT the same helper as
    /// `speleodb_forget_lock_acquired_for`, whose race an id comparison DOES
    /// close because its interloper is a DIFFERENT project. Same field, two
    /// different legitimacy tests - see there before unifying them.
    ///
    /// Sound because every writer opens the gate BEFORE recording (the commit
    /// above, and `speleodb_update`'s fast-forward arm) and needs this same
    /// mutex to record: either the writer released the mutex before we took it -
    /// its gate store visible across that edge, so we see an open gate and
    /// decline - or it has not recorded yet, and its write lands after our clear
    /// and stands.
    pub(super) fn speleodb_forget_lock_acquired_if_gate_shut(&self) {
        // The OPEN project's id, resolved BEFORE the session lock is taken:
        // `speleodb_open_link` takes the database mutex, and the order in this
        // tree is database-then-session (`speleodb_held_lock_release_params`).
        // No open link, no claim of ours this gate can be about - the janitor
        // owns that world.
        let Ok(link) = self.speleodb_open_link() else {
            return;
        };
        let mut session = self.speleodb.lock();
        if !self.edit_enabled() {
            // The OPEN project's claim, held by the account signed in now, and
            // nothing else. The gate is one fact about one project: this
            // account's claims on OTHER projects can be live pointers to mutexes
            // still held (a janitor release that failed transiently leaves
            // exactly one), and another account's are never this session's to
            // drop. Clearing by account alone discarded both, so opening any
            // second project read-only was enough to strand an earlier lock.
            let (endpoint, email) = (session.endpoint.clone(), session.email.clone());
            session.locks_acquired_here.retain(|claim| {
                claim.project_id != link.project_id
                    || !claim.taken_by(endpoint.as_deref(), email.as_deref())
            });
        }
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
    /// self-deadlock.
    pub(super) fn speleodb_unwind_release(
        &self,
        lock_ops: &LockOpsHeld<'_>,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
        entry_gen: u64,
    ) {
        if self.speleodb_session_generation() == entry_gen {
            self.speleodb_release_best_effort(endpoint, token, insecure_http, project_id);
            return;
        }
        /// [#576 A8 NF-A8-BYPASS] This unwind's reclaim licence: the session moved, so
        /// the gate is the successor's, not this release's to shut.
        struct SessionMovedUnwind;
        impl ReclaimRelease for SessionMovedUnwind {}
        let params = LockReleaseParams {
            endpoint: endpoint.to_owned(),
            token: token.to_owned(),
            insecure_http,
            project_id: project_id.to_owned(),
        };
        if let Err(e) =
            self.speleodb_release_unless_reclaimed_locked(lock_ops, &SessionMovedUnwind, &params)
        {
            tracing::warn!(
                error = %e,
                project_id,
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
        /// [#576 A8 NF-A8-BYPASS] A captured release's reclaim licence: its session
        /// is gone (closed or deleted), so there is no gate of its own to shut.
        struct CapturedRelease;
        impl ReclaimRelease for CapturedRelease {}
        let lock_ops = self.speleodb_lock_ops_guard();
        self.speleodb_release_unless_reclaimed_locked(&lock_ops, &CapturedRelease, params)
    }

    /// The status poll's inherited-lock gate reopen: enable editing on the open
    /// remote project because the SERVER says this account already holds its
    /// mutex - re-asked and committed inside the lock-ops span, so the answer
    /// cannot go stale between the asking and the gate.
    ///
    /// The poll's own `get_project` cannot license this. It is one read in a
    /// sequence of several, and a captured or janitor release for this very
    /// project can complete between that read and the gate: the release's
    /// reclaim check looks for a claim record (none - the poll never minted one)
    /// and a live edit session (none - the gate is still SHUT, which is exactly
    /// why the poll is here), so it rightly POSTs and frees the mutex. Opening
    /// the gate from the earlier read then leaves a session editing a remote
    /// project with no server lock behind it and no record saying so - the one
    /// state the whole lock protocol is built to prevent.
    ///
    /// So the holder is read AGAIN under `speleodb_lock_ops`, and the gate opens
    /// in the same span. A release either finished first (the re-read finds the
    /// mutex free or someone else's, and the gate stays shut - the user can
    /// Start Editing to take it properly) or has not started (it will find the
    /// open gate and stand down, per
    /// [`Self::speleodb_live_session_accounts_for`]).
    ///
    /// Commits through [`Self::speleodb_commit_editing`] with `acquired_here`
    /// false: an inherited lock is an edit session, never an acquisition. A
    /// failed re-read declines to reopen - "cannot say" must not enable editing.
    ///
    /// [#576 SDB-ACCT-D1, SDB-QUIT-D1] However the account came to hold the lock
    /// (another Stalagmight session, another machine, the SpeleoDB website), it is
    /// the user's: the reopen ADOPTS it, and the commit records the adopted lock's
    /// claim (resumed: the release, not the authorship shortcut), so the quit and
    /// every other release verb hand it back. It used to open the gate with no
    /// claim, and the silent releases, which read the claim, left the lock held.
    pub(super) fn speleodb_reopen_gate_for_inherited_lock(
        &self,
        link: &crate::models::SpeleoLink,
    ) -> bool {
        // The guard comes FIRST, before the account is read. Everything after it
        // - the account, the holder, the gate - describes one moment, and a
        // LOGIN is what can move that moment: it swaps the whole session in one
        // write, and it takes this same guard (see `speleodb_login`). Reading
        // the account before the guard let a sign-in as somebody else land in
        // between, so this went on to confirm the PREVIOUS account's lock with
        // the previous account's token and opened the edit gate for the new one -
        // free to mutate a project whose mutex their credentials do not hold.
        let lock_ops = self.speleodb_lock_ops_guard();
        // One read, so the holder comparison below cannot pair one account's
        // endpoint with another's email (`speleodb_active_account`).
        let Ok((endpoint, token, me)) = self.speleodb_active_account() else {
            return false;
        };
        let insecure_http = self.speleodb_insecure_http();
        let holder = match self.speleodb_lock_holder_checked(
            &endpoint,
            &token,
            insecure_http,
            &link.project_id,
        ) {
            Ok(holder) => holder,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    project_id = %link.project_id,
                    purpose = "confirm we still hold the lock before reopening the gate",
                    "speleodb: could not re-read the lock holder; leaving the project read-only"
                );
                return false;
            }
        };
        if !super::speleodb::lock_held_by_me(holder.as_deref(), Some(me.as_str())) {
            return false;
        }
        if self
            .speleodb_commit_editing(&lock_ops, None, &link.project_id, false)
            .is_err()
        {
            return false;
        }
        // [#576 SDB-POLL-D1] When this session's own acquire lost its reply (and the
        // re-read after it failed), the re-read above says it landed: the claim the
        // acquire would have recorded replaces the adopted one, with the gate open,
        // in this span.
        self.speleodb_claim_unconfirmed_acquire(&lock_ops, &link.project_id);
        true
    }

    /// [#576 B35(e)-D2] The other half of the poll's gate rule: CLOSE the edit gate
    /// when the server says the lock this session is editing under is gone or held
    /// by another account. The poll used to only ever open the gate, so a session
    /// whose lock had been freed and taken by a colleague kept editing - for up to
    /// the 20-minute re-verify window - edits that could not be uploaded without
    /// taking the lock again.
    ///
    /// Decided exactly like [`Self::speleodb_reopen_gate_for_inherited_lock`]: the
    /// poll's own `get_project` is one read among several and only decides whether
    /// to ASK, so the holder is read again inside the lock-ops span and the gate
    /// closes in that same span. A read that fails, or finds the lock ours again,
    /// leaves the session alone - "cannot say" must not end an edit session either.
    ///
    /// Un-pushed edits are untouched (the dirty bit is the work); the user re-enters
    /// through Start editing, which takes the lock properly and fences the copy.
    pub(super) fn speleodb_close_gate_for_lost_lock(&self, link: &crate::models::SpeleoLink) {
        // Guard first, for the reason the reopen above gives: a login or logout
        // must not land between the account read and the gate.
        let _lock_ops = self.speleodb_lock_ops_guard();
        let Ok((endpoint, token, me)) = self.speleodb_active_account() else {
            return;
        };
        let insecure_http = self.speleodb_insecure_http();
        match self.speleodb_lock_holder_checked(&endpoint, &token, insecure_http, &link.project_id)
        {
            Ok(holder) if !super::speleodb::lock_held_by_me(holder.as_deref(), Some(&me)) => {
                if self.speleodb_shut_gate_over_lost_lock(&link.project_id) {
                    tracing::info!(
                        project_id = %link.project_id,
                        holder = ?holder,
                        purpose = "stop editing under a lock this session no longer holds",
                        "speleodb: the write lock is gone or held by another account; \
                         editing switched off"
                    );
                }
            }
            Ok(_) => {}
            Err(e) => tracing::debug!(
                error = %e,
                project_id = %link.project_id,
                purpose = "confirm the lock is lost before switching editing off",
                "speleodb: could not re-read the lock holder; leaving the edit session open"
            ),
        }
    }

    /// [#576 B35(e)] Shut the edit gate of a lock-holding session on `project_id`
    /// because the server says the lock is not ours, and drop this account's claim
    /// on it. Returns whether it closed anything.
    ///
    /// Under the `project` mutex, so the gate, the force-edit flag and the open
    /// project's identity describe one world: a force-editor holds no lock by
    /// design and is left alone, and a project that changed underneath the caller
    /// is not the one whose lock was read. The claim goes with the gate: it records
    /// a lock this process took and no longer holds, and leaving it would license
    /// the authorship shortcut over somebody else's commit.
    pub(super) fn speleodb_shut_gate_over_lost_lock(&self, project_id: &str) -> bool {
        {
            let _proj_guard = self.project.lock();
            if !self.edit_enabled() || self.force_editing() {
                return false;
            }
            if !self
                .speleodb_open_link()
                .is_ok_and(|open| open.project_id == project_id)
            {
                return false;
            }
            self.set_edit_enabled(false);
        }
        self.speleodb_forget_lock_acquired_for(project_id);
        true
    }

    /// Whether a LIVE edit session currently accounts for `project_id`'s server
    /// mutex: the open project is linked to it and the edit gate is open without
    /// force-edit (a force-editor holds no lock by definition). Link and gate
    /// are read under the `project` mutex so they describe one world.
    ///
    /// This is the reclaim check's second eye. A successor session that
    /// INHERITED the account's lock never minted an acquisition record - its
    /// probe saw the mutex already held (the capturing session's release had
    /// not POSTed yet), and recording an inheritance as an acquisition is the
    /// footgun the probe exists to prevent - so a reclaim check that reads only
    /// `lock_acquired_here` frees the mutex out from under exactly the
    /// successors the stand-down is for. The gate + link ARE that successor's
    /// footprint: its commit opened the gate on a project linked to this id,
    /// under the same lock-ops span that keeps this answer stable.
    ///
    /// `endpoint` is the DEPLOYMENT the captured release is for, and it is half
    /// the question. A project id identifies a project on one server; a cloned
    /// or restored deployment can hold the same UUID, and a session editing THAT
    /// project accounts for nothing on the server whose mutex is about to be
    /// left held. The link carries its own endpoint, so the comparison is
    /// available and the id alone was never enough.
    pub(super) fn speleodb_live_session_accounts_for(
        &self,
        project_id: &str,
        endpoint: &str,
    ) -> bool {
        let _proj_guard = self.project.lock();
        if !self.edit_enabled() || self.force_editing() {
            return false;
        }
        self.speleodb_open_link().is_ok_and(|link| {
            link.project_id == project_id
                && normalize_endpoint(&link.endpoint) == normalize_endpoint(endpoint)
        })
    }

    /// The body of [`Self::speleodb_release_unless_reclaimed`], for callers that
    /// already hold the lock-ops mutex. Never touches `lock_acquired_here`: a
    /// record present here is by definition a successor's (the capturing
    /// session's own record was cleared when the release was captured), and a
    /// record absent needs no clearing.
    ///
    /// Two stand-down tests, one per successor shape: the claim record (a
    /// successor whose acquire was real - the lock was freed out-of-band, or a
    /// re-record path outside the lock-ops span minted it) and the live edit
    /// session (a successor that inherited and so, deliberately, has no
    /// record). Either one means the lock is accounted for.
    ///
    /// Both are asked about the DEPLOYMENT this release is for, not just the
    /// project id. `params` names one server, and a project UUID is only unique
    /// on one: a cloned or restored deployment holding the same id had a session
    /// there answering for a mutex on a server it has never touched, so the
    /// release was never sent and that server's lock stayed held - over a file
    /// the delete may already have destroyed.
    ///
    /// [#576 A8 NF-A8-BYPASS] Takes the proof of why this release may be decided here
    /// (`_why`, a [`ReclaimRelease`]): the deferred release can only reach it through
    /// the stand-down's [`stand_down::StoodDown`], so a decision written above the
    /// gate - or around it - does not compile.
    pub(super) fn speleodb_release_unless_reclaimed_locked(
        &self,
        _lock_ops: &LockOpsHeld<'_>,
        _why: &impl ReclaimRelease,
        params: &LockReleaseParams,
    ) -> Result<(), SpeleoDbError> {
        if self.speleodb_claim_on_deployment(&params.endpoint, &params.project_id)
            || self.speleodb_live_session_accounts_for(&params.project_id, &params.endpoint)
        {
            tracing::info!(
                project_id = %params.project_id,
                purpose = "avoid freeing a successor session's mutex",
                "speleodb: standing down a captured release; a live session has \
                 reclaimed this project's lock"
            );
            return Ok(());
        }
        client::release_lock(
            &params.endpoint,
            &params.token,
            params.insecure_http,
            &params.project_id,
        )
        .map(|_| ())
    }

    /// A deferred release, from the moment it can finally be decided: stand the
    /// releasing session down from the lock, then hand it back - all inside ONE
    /// lock-ops span.
    ///
    /// The stand-down is the gate, and it belongs here rather than at the point
    /// the release was spawned. The status poll reopens a shut gate whenever the
    /// server says this account still holds the mutex
    /// ([`Self::speleodb_reopen_gate_for_inherited_lock`]), and between spawning
    /// a release and running it that is exactly the state a caller-side shut
    /// would leave behind - the poll takes the guard first, resurrects the gate,
    /// and the reclaim check below then stands down for a session the switch or
    /// the exit is about to destroy. Inside this span there is no such window:
    /// the gate closes and the decision is made without anything in between.
    ///
    /// `entry_gen` is the session generation read when the release was decided.
    /// Unchanged, the gate still belongs to the session that decided to hand the
    /// lock back and closing it states a fact. Changed, that session is gone and
    /// the gate now belongs to whatever replaced it - not this release's to
    /// touch, and the reclaim check judges the new world on its own terms.
    ///
    /// `claim` is what the caller took out of the claim list, returned on a
    /// failed POST so the mutex still has a local pointer for the janitor.
    pub(super) fn speleodb_stand_down_and_release(
        &self,
        params: &LockReleaseParams,
        entry_gen: u64,
        claim: Option<LockClaim>,
    ) -> Result<(), SpeleoDbError> {
        let lock_ops = self.speleodb_lock_ops_guard();
        // [#576 A8] The gate, then the decision - by type: the decision takes the
        // stand-down's proof.
        let stood_down = self.speleodb_stand_down(&lock_ops, entry_gen);
        let outcome = self.speleodb_release_unless_reclaimed_locked(&lock_ops, &stood_down, params);
        if outcome.is_err() {
            // The mutex is still held and now has no local pointer: put the
            // claim back so the no-project janitor can retry it, exactly as the
            // janitor restores its own.
            self.speleodb_restore_lock_claim(claim);
        }
        outcome
    }

    /// Mark this account's claim on `project_id` as deliberately held back, so
    /// the janitor leaves it alone (see [`LockClaim::kept`]).
    ///
    /// Two callers, one meaning: the close dialog's "Keep lock", and the
    /// un-pushed-edits veto declining a silent release. Both are decisions -
    /// the user's, or the data-safety rule's - and the janitor exists for the
    /// claims nobody ever decided about. Without the mark the two are the same
    /// state once the project closes (a claim, no project open), so the next
    /// status poll handed the mutex back and the decision was silently undone.
    /// `why` records which decision it was (see [`LockClaim::kept`]).
    pub(super) fn speleodb_keep_lock_deliberately(&self, project_id: &str, why: KeptFor) {
        let mut session = self.speleodb.lock();
        let (endpoint, email) = (session.endpoint.clone(), session.email.clone());
        for claim in &mut session.locks_acquired_here {
            if claim.project_id == project_id
                && claim.taken_by(endpoint.as_deref(), email.as_deref())
            {
                claim.kept = Some(why);
            }
        }
    }

    /// [#576 SDB-ACCT-D1] Take out this account's claim on `project_id` (a lock on
    /// `endpoint`'s deployment) when the un-pushed-edits veto is what kept it, with
    /// what its release needs: the signed-in account's credentials, read in the
    /// same session acquisition as the claim is matched and removed.
    ///
    /// For a landed push recorded in a file the session has switched away from or
    /// closed (`AppState::speleodb_record_landed_upload`): the silent release kept
    /// the lock because that copy was still dirty, the landing has left it clean,
    /// and the lock is now released the way that release would have released a
    /// clean copy. The claim comes back with its mark cleared, so a failed release
    /// that restores it leaves a claim the janitor retries, as after a switch over
    /// a clean copy. `None` - nothing taken - when no such claim exists: none at
    /// all, one the user kept on purpose ("Keep lock"), another account's or
    /// another deployment's, or nobody signed in.
    pub(super) fn speleodb_take_claim_kept_for_unpushed_edits(
        &self,
        project_id: &str,
        endpoint: &str,
    ) -> Option<(LockReleaseParams, LockClaim)> {
        let (live_endpoint, token, email) = self.speleodb_active_account().ok()?;
        let insecure_http = self.speleodb_insecure_http();
        let mut session = self.speleodb.lock();
        // The account read above must still be the one signed in: a login landing
        // between the two reads would pair its token with another account's claim.
        let still_signed_in = session
            .email
            .as_deref()
            .is_some_and(|signed_in| signed_in.eq_ignore_ascii_case(&email))
            && session.endpoint.as_deref().is_some_and(|signed_in| {
                normalize_endpoint(signed_in) == normalize_endpoint(&live_endpoint)
            });
        if !still_signed_in {
            return None;
        }
        let position = session.locks_acquired_here.iter().position(|claim| {
            claim.project_id == project_id
                && claim.kept == Some(KeptFor::UnpushedEdits)
                && claim.on_deployment(endpoint)
                && claim.taken_by(Some(&live_endpoint), Some(&email))
        })?;
        let claim = session.locks_acquired_here.remove(position);
        // The unwind slot must not hand back a lock this release is ending (the
        // same rule as `speleodb_forget_lock_acquired_inner`).
        if session
            .acquired_by_this_flow
            .as_ref()
            .is_some_and(|flow| flow.is_same_lock(&claim))
        {
            session.acquired_by_this_flow = None;
        }
        Some((
            LockReleaseParams {
                endpoint: live_endpoint,
                token,
                insecure_http,
                project_id: project_id.to_owned(),
            },
            LockClaim {
                kept: None,
                ..claim
            },
        ))
    }

    /// Put a claim back after a clear whose reason turned out not to happen.
    ///
    /// `AppState::delete_project` clears at snapshot time, under the same
    /// `project` guard that decided the file's identity, and only afterwards
    /// tries to destroy the file. When THAT fails nothing has been destroyed:
    /// the lock is still ours, still guarding a project still on disk, and the
    /// claim is the only local pointer to it - the session is closed by then, so
    /// no poll catch-all will ever see the mutex again.
    ///
    /// Declines if the same lock has been claimed meanwhile: a successor's entry
    /// is newer and describes the same mutex, so two would say one thing twice.
    pub(super) fn speleodb_restore_lock_claim(&self, claim: Option<LockClaim>) {
        let Some(claim) = claim else {
            return;
        };
        let mut session = self.speleodb.lock();
        if !session
            .locks_acquired_here
            .iter()
            .any(|live| live.is_same_lock(&claim))
        {
            session.locks_acquired_here.push(claim);
        }
    }

    /// The stranded-claim janitor: with NO project open, a surviving
    /// `lock_acquired_here` record can only belong to a session that no longer
    /// exists (records are minted under the `project` mutex with a project
    /// open), so clear it and hand its lock back. Called from the
    /// `NoRemoteProject` branch of `speleodb_link_status` - the one state the
    /// old catch-all (`speleodb_forget_lock_acquired_if_gate_shut`) could never
    /// reach, because that branch returned before it (F7's leftover damage).
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
    /// record stays so the first poll after a sign-in retries, and a failed
    /// POST puts it back (below) so the next poll of this branch retries too.
    /// A take-first shape lost the record to a transient network blip and
    /// turned "retry on the next poll" into a promise nothing could keep.
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
            let proj_guard = self.project.lock();
            if proj_guard.is_some() {
                return;
            }
            {
                // Unusable ids first, whoever they belong to: one that can never
                // be joined into a URL can never be released either, so clearing
                // is its only safe end and nobody's later sign-in helps.
                let mut session = self.speleodb.lock();
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
            // retained claim sit at the head of the list and answer every poll -
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
                // this janitor, and the poll after a sign-in walks this branch
                // again WITH a token.
                return;
            };
            let me = Some(me);
            let mut session = self.speleodb.lock();
            // One claim per visit - the poll comes back for the rest.
            let Some(position) = session.locks_acquired_here.iter().position(|claim| {
                // Deliberately kept locks are not this cleanup's business: the
                // user's "Keep lock", or the un-pushed-edits veto, already
                // decided that this mutex stays held. The janitor is for claims
                // whose session died without deciding anything.
                !claim.is_kept() && claim.taken_by(Some(&endpoint), me.as_deref())
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
        /// [#576 A8 NF-A8-BYPASS] The janitor's reclaim licence: no project is open, so
        /// there is no gate to shut.
        struct StrandedClaim;
        impl ReclaimRelease for StrandedClaim {}
        let lock_ops = self.speleodb_lock_ops_guard();
        if let Err(e) =
            self.speleodb_release_unless_reclaimed_locked(&lock_ops, &StrandedClaim, &params)
        {
            tracing::warn!(
                error = %e,
                project_id = %params.project_id,
                purpose = "clean up after a torn-down edit session",
                "speleodb: stranded-claim release failed; the next poll retries"
            );
            // Restore for the retry - the WHOLE claim, account included, or the
            // retry would be the account-blind pairing this cleanup just
            // refused to make. Only into the same no-project-open world the
            // decision read: with a project open the field belongs to the live
            // world (its own verbs and catch-alls own it), and this janitor
            // never runs there anyway.
            let proj_guard = self.project.lock();
            if proj_guard.is_none() {
                {
                    let mut session = self.speleodb.lock();
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
    /// The in-memory token store keeps it hermetic - no OS keychain probe, and
    /// `endpoint.is_some()` short-circuits the rehydration.
    fn sign_in_unreachable(state: &AppState) {
        sign_in_as(state, UNREACHABLE, "diver@example.com");
    }

    /// [`sign_in_unreachable`] for a named account on a named deployment, for
    /// the tests about a claim outliving the account that made it.
    fn sign_in_as(state: &AppState, endpoint: &str, email: &str) {
        let mut session = state.speleodb.lock();
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
            .speleodb
            .lock()
            .locks_acquired_here
            .iter()
            .map(|claim| claim.project_id.clone())
            .collect()
    }

    /// Sign out, WITHOUT touching the claim - what a logout that could not run
    /// its release leaves behind, and the state the janitor is written for.
    fn sign_out(state: &AppState) {
        let mut session = state.speleodb.lock();
        session.endpoint = None;
        session.email = None;
        session.token_store = crate::speleodb::auth::TokenStore::from_stores(
            Box::new(crate::speleodb::auth::InMemoryStore::new()),
            None,
        );
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
            .speleodb_session_snapshot(&state.speleodb_lock_ops_guard())
            .expect("a linked open project has a snapshot");
        assert_eq!(entry_gen, state.speleodb_session_generation());
        assert_eq!(link.project_id, ID);

        state.close_project();
        assert!(
            state
                .speleodb_session_snapshot(&state.speleodb_lock_ops_guard())
                .is_err(),
            "no open project, no snapshot"
        );
    }

    #[test]
    fn a_commit_in_an_unmoved_session_opens_the_gate_and_mints_the_claim() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        open_linked(&state, temp.path(), "a.stalx", ID);
        let (entry_gen, link) = state
            .speleodb_session_snapshot(&state.speleodb_lock_ops_guard())
            .expect("snapshot");

        state
            .speleodb_commit_editing(
                &state.speleodb_lock_ops_guard(),
                Some(entry_gen),
                &link.project_id,
                true,
            )
            .expect("nothing moved, so the commit lands");

        assert!(state.edit_enabled(), "the gate is open");
        assert!(!state.force_editing(), "force-edit is cleared");
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
    fn a_commit_without_acquired_here_opens_the_gate_without_a_claim() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        open_linked(&state, temp.path(), "a.stalx", ID);
        let (entry_gen, link) = state
            .speleodb_session_snapshot(&state.speleodb_lock_ops_guard())
            .expect("snapshot");

        state
            .speleodb_commit_editing(
                &state.speleodb_lock_ops_guard(),
                Some(entry_gen),
                &link.project_id,
                false,
            )
            .expect("commit lands");

        assert!(state.edit_enabled());
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "an inherited lock is an edit session, not an acquisition"
        );
    }

    #[test]
    fn a_commit_after_a_teardown_is_refused_with_nothing_committed() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        open_linked(&state, temp.path(), "a.stalx", ID);
        let (entry_gen, link) = state
            .speleodb_session_snapshot(&state.speleodb_lock_ops_guard())
            .expect("snapshot");

        state.close_project();

        let refused = state
            .speleodb_commit_editing(
                &state.speleodb_lock_ops_guard(),
                Some(entry_gen),
                &link.project_id,
                true,
            )
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
        let (entry_gen, link) = state
            .speleodb_session_snapshot(&state.speleodb_lock_ops_guard())
            .expect("snapshot");

        state.close_project();
        open_linked(&state, temp.path(), "b.stalx", ID);

        let refused = state
            .speleodb_commit_editing(
                &state.speleodb_lock_ops_guard(),
                Some(entry_gen),
                &link.project_id,
                true,
            )
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
            .speleodb_commit_editing(&state.speleodb_lock_ops_guard(), None, ID, true)
            .expect("the open project is still linked to this id");
        assert!(state.edit_enabled());
        assert!(state.speleodb_lock_acquired_here(ID));

        state.close_project();
        open_linked(&state, temp.path(), "c.stalx", OTHER_ID);
        let refused = state
            .speleodb_commit_editing(&state.speleodb_lock_ops_guard(), None, ID, true)
            .expect_err("a different project is open");
        assert!(matches!(refused, SpeleoDbError::SessionMoved));

        state.close_project();
        let refused = state
            .speleodb_commit_editing(&state.speleodb_lock_ops_guard(), None, ID, true)
            .expect_err("no project is open at all");
        assert!(matches!(refused, SpeleoDbError::SessionMoved));
    }

    #[test]
    fn the_unwind_in_an_unmoved_session_clears_the_record_as_before() {
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        open_linked(&state, temp.path(), "a.stalx", ID);
        let entry_gen = state.speleodb_session_generation();
        state.speleodb_note_lock_acquired(Some(ID));

        // The POST fails fast (nothing listens); best-effort is the contract.
        state.speleodb_unwind_release(
            &state.speleodb_lock_ops_guard(),
            "http://127.0.0.1:1/",
            "token",
            true,
            ID,
            entry_gen,
        );

        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "same session: the release ends OUR claim, exactly as it always did"
        );
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

        state.speleodb_unwind_release(
            &state.speleodb_lock_ops_guard(),
            "http://127.0.0.1:1/",
            "token",
            true,
            ID,
            entry_gen,
        );

        assert!(
            state.speleodb_lock_acquired_here(ID),
            "session moved: the record is the successor's, not ours to blank (5.5)"
        );
    }

    /// A signed-in session pointing at a dead endpoint: token resolution
    /// succeeds locally, every POST fails fast (nothing listens on port 1).
    /// The in-memory token store keeps this hermetic - no OS keychain probe
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
        // it so the first poll after a sign-in can retry (round 5).
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
        // lock was stranded until the server's own timeout.
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
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
            .speleodb_commit_editing(&state.speleodb_lock_ops_guard(), None, ID, true)
            .expect("the first flow's commit");

        // A later flow that inherits: it opens its span, commits without an
        // acquisition, and must leave the unwind empty.
        state.speleodb_begin_acquisition_flow(&state.speleodb_lock_ops_guard());
        state
            .speleodb_commit_editing(&state.speleodb_lock_ops_guard(), None, ID, false)
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
            .speleodb_commit_editing(&state.speleodb_lock_ops_guard(), None, ID, false)
            .expect("the successor's commit");

        // The captured release is for the same id on ANOTHER deployment.
        let elsewhere = super::LockReleaseParams {
            endpoint: "http://127.0.0.1:2/".to_owned(),
            token: "tok".to_owned(),
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
            insecure_http: true,
            project_id: ID.to_owned(),
        };
        state
            .speleodb_release_unless_reclaimed(&here)
            .expect("stood down for the live session on this deployment");
    }

    #[test]
    fn opening_a_second_project_read_only_keeps_the_first_ones_stranded_claim() {
        // Round 9 (P2): the poll's catch-all clear ran on every claim the
        // account held, so the first status poll after opening ANY project
        // read-only discarded a claim retained for a different project - the
        // only pointer to a mutex a failed janitor release had left held.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        state.speleodb_note_lock_acquired(Some(OTHER_ID));

        // A second project, opened read-only: its gate is shut, which is what
        // the catch-all keys on.
        open_linked(&state, temp.path(), "a.stalx", ID);
        state.speleodb_forget_lock_acquired_if_gate_shut();

        assert_eq!(
            retained_projects(&state),
            vec![OTHER_ID.to_owned()],
            "the shut gate says nothing about a lock on another project"
        );

        // And it still clears the OPEN project's claim, which is its job.
        state.speleodb_note_lock_acquired(Some(ID));
        state.speleodb_forget_lock_acquired_if_gate_shut();
        assert_eq!(
            retained_projects(&state),
            vec![OTHER_ID.to_owned()],
            "the open project's claim is over when its gate is shut"
        );
    }

    #[test]
    fn the_janitor_picks_the_claim_the_live_account_can_actually_release() {
        // Round 8 (P2): another account's retained claim sitting at the head of
        // the list used to answer every poll - the janitor took the FIRST entry,
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
        // Round 14 (P1): "Keep lock" closes the project and holds the mutex on
        // purpose, and the un-pushed-edits veto does the same on the app's own
        // authority. Once the project is closed both look exactly like the state
        // the janitor exists for - a claim, no project open - so without a mark
        // the very next status poll handed the mutex back and the choice was
        // undone in silence, freeing a project whose work is still only local.
        //
        // The stub answers the release, so a janitor that ignored the mark would
        // be visible as a claim that disappeared.
        let (base, _listener) = release_stub(ID);
        let state = AppState::new().expect("state");
        sign_in_as(&state, &base, "diver@example.com");
        state.speleodb_note_lock_acquired(Some(ID));
        state.speleodb_keep_lock_deliberately(ID, super::KeptFor::UserChoice);

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
        // poll rather than being lost.
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
             poll has nothing to retry from (round 5)"
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
            "the poll's visit to the no-project branch ran the janitor"
        );
    }

    #[test]
    fn a_captured_release_stands_down_for_an_inherited_successor_session() {
        // The round-4 P1: a successor that started editing while the account
        // still held the lock (the capturing session's release had not POSTed)
        // INHERITED it - `acquired_here` false, so no record was minted. The
        // reclaim check must still see that session, or the deferred release
        // frees the mutex out from under it.
        let temp = tempfile::TempDir::new().expect("temp");
        let state = AppState::new().expect("state");
        open_linked(&state, temp.path(), "a.stalx", ID);
        state
            .speleodb_commit_editing(&state.speleodb_lock_ops_guard(), None, ID, false)
            .expect("the inherited successor's commit");
        assert!(
            !state.speleodb_lock_acquired_here(ID),
            "fixture: an inherited session has no acquisition record"
        );

        let params = super::LockReleaseParams {
            endpoint: UNREACHABLE.to_owned(),
            token: "tok".to_owned(),
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
        let params = super::LockReleaseParams {
            endpoint: UNREACHABLE.to_owned(),
            token: "tok".to_owned(),
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
            .speleodb_commit_editing(&state.speleodb_lock_ops_guard(), None, OTHER_ID, false)
            .expect("the other project's session");
        assert!(state.speleodb_release_unless_reclaimed(&params).is_err());
    }

    // ---- tripwires -------------------------------------------------------

    /// The production region of a sibling source file of this module, read at test
    /// time and cut by the shared `models::source_scan::prod_region` (#576 A8).
    fn state_source(name: &str) -> String {
        let path = format!("{}/src/core/state/{name}", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        crate::models::source_scan::prod_region(&src, "fn ").to_owned()
    }

    /// [#576 NF-W9E1-3] The modules `mod_rs` declares for the production build:
    /// every top-level `mod <name>;` whose attributes carry no test cfg
    /// (`cfg(test)`, `cfg(any(test, ..))`, `cfg(all(test, ..))`). A file no
    /// declaration names is never compiled, so it can write no slot; one declared
    /// under a test cfg is test code (the shared test dump, the test-fixture module
    /// whose setters write the slot freely, every `*_tests.rs`).
    fn production_modules(mod_rs: &str) -> Vec<String> {
        let mut modules = Vec::new();
        let mut attrs = String::new();
        let mut depth = 0usize;
        for line in mod_rs.lines() {
            let trimmed = line.trim();
            if depth > 0 || trimmed.starts_with("#[") {
                // An attribute, possibly over several lines (`#[expect(\n ..\n)]`).
                attrs.push_str(trimmed);
                attrs.push('\n');
                depth += trimmed.matches('[').count();
                depth = depth.saturating_sub(trimmed.matches(']').count());
                continue;
            }
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            let declared = line
                .strip_prefix("pub ")
                .or_else(|| line.strip_prefix("pub(super) "))
                .or_else(|| line.strip_prefix("pub(crate) "))
                .unwrap_or(line)
                .strip_prefix("mod ")
                .and_then(|rest| rest.strip_suffix(';'));
            if let Some(name) = declared {
                let test_only = ["cfg(test)", "cfg(any(test", "cfg(all(test"]
                    .iter()
                    .any(|gate| attrs.contains(gate));
                if !test_only {
                    modules.push(name.trim().to_owned());
                }
            }
            attrs.clear();
        }
        modules
    }

    #[test]
    fn production_modules_are_the_declarations_without_a_test_cfg() {
        let mod_rs = "mod app_state;\n\
             pub mod dive_plan;\n\
             // a comment between items\n\
             #[cfg(test)]\n\
             mod tests;\n\
             #[cfg(test)]\n\
             #[expect(\n\
                 clippy::expect_used,\n\
                 reason = \"test module\"\n\
             )]\n\
             mod sdb_endpoint_tests;\n\
             #[cfg(any(test, feature = \"test-support\"))]\n\
             mod tests_support;\n\
             #[expect(clippy::too_many_lines, reason = \"production\")]\n\
             mod session_commit;\n\
             pub use app_state::AppState;\n";
        assert_eq!(
            production_modules(mod_rs),
            ["app_state", "dive_plan", "session_commit"],
            "test-gated declarations (one-line or multi-line attributes) are left out"
        );
    }

    #[test]
    fn bump_sites_are_exactly_the_known_set() {
        // D7's lesson from the abandoned attempt: an incomplete bump-site
        // inventory is invisible until the missed site re-opens the bug. Every
        // file that commits into or clears the project slot must either bump or
        // be on the classified DO-NOT-BUMP list below.
        //
        // [#576 NF-W9E1-3] Over the modules `mod.rs` declares for the production
        // build, not every `.rs` in the directory: a stray file (a throwaway, a
        // draft) is never compiled and failed this scan with a slot-write verdict
        // about code that does not exist.
        let dir = format!("{}/src/core/state", env!("CARGO_MANIFEST_DIR"));
        let mod_rs = std::fs::read_to_string(format!("{dir}/mod.rs")).expect("read mod.rs");
        let modules = production_modules(&mod_rs);
        for known in ["app_state", "session_commit", "speleodb_logout"] {
            assert!(
                modules.iter().any(|m| m == known),
                "RULE (the scan must see the slot writers it classifies): `{known}` is not \
                 among the production modules read off mod.rs: {modules:?}"
            );
        }
        let mut bumping: Vec<String> = Vec::new();
        let mut writing: Vec<String> = Vec::new();
        for module in modules {
            let name = format!("{module}.rs");
            // THIS module: the scan needles appear in this very test's source.
            if name == "speleodb_session.rs" {
                continue;
            }
            let src = std::fs::read_to_string(format!("{dir}/{name}"))
                .unwrap_or_else(|e| panic!("read {name}: {e}"));
            // Production code only: this tree keeps test modules at the bottom of
            // the file, so truncating at the first test gate is enough for a
            // tripwire (fixtures write the slot freely and legitimately).
            let src =
                &src[..crate::models::source_scan::test_module_start(&src).unwrap_or(src.len())];
            if src.contains("self.bump_speleodb_session()") {
                bumping.push(name.clone());
            }
            // The slot-commit spellings in this tree: the shared project-slot
            // commit (`slot.set(project)`, #576 A5 - every load, create and close),
            // a direct load/create commit (`.set(Some(project`), the rollback
            // restore (`.set(Some(restored`), and a direct close clear
            // (`.set(None)`) - the slot is a `ProjectSlot` since #576 A9 - plus the
            // pre-A9 assignment forms. Space-prefixed where a `== Some(project_id)`
            // comparison could otherwise read as an assignment.
            if src.contains("slot.set(project)")
                || src.contains(".set(Some(project")
                || src.contains(".set(Some(restored")
                || src.contains(".set(None)")
                || src.contains("*slot = project")
                || src.contains(" = Some(project")
                || src.contains(" = Some(restored")
                || src.contains("*project = None")
            {
                writing.push(name);
            }
        }
        bumping.sort();
        assert_eq!(
            bumping,
            vec!["session_commit.rs", "speleodb_logout.rs"],
            "RULE (sanctioned scan, #576 A8-D2 - every slot writer bumps the session or \
             is classified exempt): the bump-site inventory moved; re-derive the \
             session-boundary classification in `speleodb_session` before updating this list"
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

    /// The fast-forward arm commits on identity alone, and identity cannot see a logout
    /// (the project slot never moves). What keeps that sound is the lock-ops span:
    /// logout is a lock operation like the acquire and the captured releases, so it
    /// cannot run inside a Start Editing span (round 6).
    ///
    /// [#576 A8] guard -> gates -> credential forget, with the guard still held at the
    /// forget, is the type's job now: both halves take the span's `LockOpsHeld` proof,
    /// so a gate drop or a forget outside the span does not compile. What stays a
    /// sanctioned source scan (docs/testing-recipes.md, "Source scans") is the gate
    /// half's site rules, which no type states.
    #[test]
    fn the_logout_gate_half_inherits_its_callers_span() {
        use crate::models::source_scan::fn_body;
        let src = state_source("speleodb_logout.rs");
        let gate_body = fn_body(&src, "fn speleodb_logout_drop_gates<'s>(");
        let release_body = fn_body(&src, "fn speleodb_logout_release_and_drop_gates(");
        assert!(
            !gate_body.contains("self.speleodb_lock_ops_guard()")
                && !release_body.contains("self.speleodb_lock_ops_guard()"),
            "RULE (lock-ops is not reentrant): the gate half must inherit its caller's \
             span, not open a narrower one of its own"
        );
        assert!(
            gate_body.contains("self.bump_speleodb_session()"),
            "RULE (logout is a session boundary): the gate half bumps the session"
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
                    let _ = state.speleodb_logout(true);
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

    /// Round 10 (P1): the project-switch and app-exit releases run on a detached
    /// thread, which is exactly the delay a successor needs - the switch is what lets
    /// the user reopen that project, and a Start Editing on it INHERITS the still-held
    /// mutex and mints no claim. An unguarded POST landing afterwards frees the lock
    /// under a live edit session, so every release that can outlive the moment it was
    /// decided goes through the reclaim check.
    ///
    /// [#576 A8] The stand-down's order (the gate, then the decision, in one span) is
    /// the type's now (`stand_down::StoodDown`). What stays is a sanctioned site
    /// inventory (docs/testing-recipes.md "Source scans") of the spawn site itself:
    /// which release it routes to, that its one raw POST is the no-managed-state
    /// fallback, and that it leaves the gate to the span - statements about one
    /// function's calls no type expresses.
    #[test]
    fn the_detached_release_routes_through_the_reclaim_checked_span() {
        use crate::models::source_scan::fn_body;
        let src = state_source("speleodb_lock_release.rs");
        let body = fn_body(&src, "fn speleodb_spawn_lock_release(");
        assert!(
            body.contains("speleodb_stand_down_and_release(&p, entry_gen, claim)"),
            "RULE (every deferred release is reclaim-checked): the detached release goes \
             through the stand-down-and-release span"
        );
        assert_eq!(
            body.matches("client::release_lock(").count(),
            1,
            "RULE (every deferred release is reclaim-checked): the only bare POST is the \
             fallback for when there is no app handle to reach managed state through"
        );
        assert!(
            !body.contains("self.set_edit_enabled("),
            "RULE (the gate moves inside the release's own span, round 12): shutting it \
             at the spawn leaves a shut gate over a mutex the server still says we hold"
        );
    }
}
