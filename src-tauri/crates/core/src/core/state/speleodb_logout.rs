//! Logging OUT of SpeleoDB, and the two RESIDUES a logout has to clear.
//!
//! Split out of `core::state::speleodb` because logout is not one action but three
//! that must all happen and are each independently fallible, and the rules for what
//! to do when one fails are subtle enough to deserve their own place:
//!
//! 1. the transient EDIT GATE (and the force-edit gate), plus a best-effort release
//!    of a held write lock - see [`AppState::speleodb_release_held_lock_best_effort`];
//! 2. the stored CREDENTIAL, across every [`crate::speleodb::auth::SecretStore`] tier;
//! 3. the persisted account EMAIL - the *rehydrate seed*.
//!
//! (2) and (3) are one hazard, not two. A credential that outlives a failed delete is
//! only dangerous because `speleodb_ensure_rehydrated` keys the keychain lookup by the
//! saved email: leave the seed and the next launch finds the credential and silently
//! re-authenticates a user who asked to log out. So both are reported, and a run that
//! fails both reports both.
//!
//! The logout ITSELF always happens - the user asked for it, and a flaky keychain must
//! never leave them logged in. The `Err` is a WARNING the frontend surfaces so a retry
//! can be offered, never a refusal.

use super::speleodb::local_dto;
use super::AppState;
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

impl AppState {
    /// Record whether the OS keychain is believed to hold a token, and FOR WHOM,
    /// **without touching the persisted account email**.
    ///
    /// `held_for` is the whole belief in one value: `Some(account_key(endpoint,
    /// email))` when an entry is believed to exist for that account, `None` for no
    /// belief at all. One argument rather than a flag and a name, so no caller can
    /// hand the two config fields values that disagree - the incoherent pair is not
    /// expressible from here.
    ///
    /// For the two callers that change what we believe and nothing else: the 401
    /// auto-drop, and the storage-choice move (the login-time prompt's answer and
    /// the Settings "Try again"). LOGIN and LOGOUT do not come through here - they
    /// change the account too, and the account and the belief stamped with it have
    /// to land in ONE save (see [`Self::speleodb_write_account_record`]). Neither
    /// does the startup write-back, which lowers under a compare-and-set
    /// ([`Self::speleodb_lower_keychain_belief_after_proof`]).
    ///
    /// This is what lets startup skip the keyring entirely: the next launch reads
    /// this instead of opening the keychain to find out, which on Linux is the
    /// unlock prompt itself. Since #260 it is also the discriminator behind
    /// `TokenStore::unreadable_keychain_may_hold` - a locked keyring and a box with
    /// no secret service fail every op identically, and a recorded store is the only
    /// thing that tells them apart - so the two meanings have to be the SAME fact,
    /// and they are: what the keyring holds.
    ///
    /// Whether an entry exists therefore comes from `TokenStore::keychain_has_token`,
    /// which raises on evidence and falls only on proof (a delete the keyring
    /// accepted, a probe or read that answered `Ok(None)`). It is NOT "where this
    /// session's store landed": a login the keyring refuses falls back to memory or
    /// plaintext WITHOUT removing an entry an earlier login left there, so deriving
    /// it that way wrote `false` over a live bearer token's only record - after which
    /// the next launch skipped the keyring and turning the keyring off reported the
    /// failed eviction as an empty one.
    ///
    /// The DESTRUCTIVE callers pass `None` instead of what the store believes, and
    /// are the deliberate exceptions, for reasons of their own rather than one shared
    /// one. The 401 auto-drop (`AppState::speleodb_forget`) is the one that comes
    /// through here: it leaves the rehydrate seed, but the credential it just swept is
    /// one the server has already rejected - re-finding a dead token costs a
    /// launch-time unlock prompt and buys nothing. Logout's `None` goes through the
    /// account writer, because it clears the seed in the same breath, so nothing is
    /// left that could key an entry by it. See both call sites.
    ///
    /// Best-effort like its siblings: a lost write costs at most one prompt, or one
    /// sign-in. Best-effort is not write-ONCE, though - the value is PUT BACK when
    /// the save fails, the same way `speleodb_persist_storage_choice` does and for
    /// the same reason. The early return below compares against the in-memory value,
    /// so a mutation left standing over a file that never took it makes every later
    /// call with the same belief a no-op, and a transient failure (a full disk, a
    /// config file a backup sweep has open) permanent until the next launch. What
    /// that costs here is not a re-asked prompt but a lost RECORD: the launch after
    /// it reads `false` off disk and skips a keyring entry that really exists.
    pub(super) fn speleodb_persist_keychain_belief(&self, held_for: Option<String>) {
        let mut config = self.config.lock();
        // Compared as the PAIR, so a belief that keeps its answer but changes whose
        // account it is about is still a write. Guarding on the bool alone let the
        // stamp go stale under it, which is the same "no name on it" state this
        // field exists to end.
        if config.speleodb.keychain_has_token == held_for.is_some()
            && config.speleodb.keychain_token_account == held_for
        {
            return;
        }
        let previous = (
            config.speleodb.keychain_has_token,
            config.speleodb.keychain_token_account.clone(),
        );
        config.speleodb.keychain_has_token = held_for.is_some();
        config.speleodb.keychain_token_account.clone_from(&held_for);
        if let Err(e) = config.save() {
            config.speleodb.keychain_has_token = previous.0;
            config.speleodb.keychain_token_account = previous.1;
            tracing::warn!(
                error = %e,
                has_token = held_for.is_some(),
                "failed to persist whether the OS keychain holds a SpeleoDB token"
            );
        }
    }

    /// Lower a belief the startup rehydrate just DISPROVED - but only if the fact it
    /// read is still the fact on disk, and only if nothing has since raised the
    /// belief for real.
    ///
    /// `read` is the `[speleodb]` config block `AppState::speleodb_ensure_rehydrated`
    /// took its snapshot from. That snapshot is stale by the time we get here on
    /// purpose: rehydration releases the SESSION lock before writing, because the
    /// writer takes the CONFIG lock and the documented order is config-then-session.
    /// SpeleoDB commands run concurrently on `spawn_blocking` threads and the read
    /// that produced the proof can sit on a desktop unlock prompt, so "a login
    /// lands in that window" is not a thought experiment. An unconditional write
    /// then overwrote a fact newer than the one it was correcting, and the launch
    /// after that skipped a keyring entry the login had genuinely just put there -
    /// reporting the user signed out with their token still in the keyring, and a
    /// `clear` that would no longer sweep it.
    ///
    /// So this is a compare-and-set, on TWO things, because either alone has a hole:
    ///
    /// - The persisted pair (account, belief) must still be the one we read. This is
    ///   what catches a login as SOMEBODY ELSE: it writes its own account and its own
    ///   belief, and lowering over that would strand the new account's entry.
    /// - The LIVE store must still believe nothing. This is what catches a login as
    ///   the SAME account, which the value comparison cannot see: it re-asserts the
    ///   config's own `true`, so a successful keychain write leaves the persisted
    ///   pair byte-identical to the snapshot while the keyring now really does hold
    ///   an entry. `TokenStore::keychain_has_token` is the authoritative copy, and it
    ///   was raised by that write.
    ///
    /// The session lock is taken INSIDE the config lock, which is the documented
    /// order rather than an exception to it, and it is what makes this atomic: a
    /// login cannot persist anything while we hold the config lock, and cannot raise
    /// the store's belief while we hold the session lock. A login that finishes
    /// storing between the two writes back its `true` after ours, which is the
    /// correct outcome. Safe to nest here specifically because the only caller
    /// already acquires the session lock twice on its own, so no caller of it can be
    /// holding one.
    ///
    /// A poisoned session lock declines to write. "Could not tell" must not collapse
    /// into either answer, and of the two available it collapses into keeping the
    /// belief: one unlock prompt, self-corrected by the read it pays for, against
    /// stranding a token.
    pub(super) fn speleodb_lower_keychain_belief_after_proof(
        &self,
        read: &crate::config::SpeleoDbConfig,
    ) {
        let mut config = self.config.lock();
        if config.speleodb.email != read.email
            || config.speleodb.keychain_has_token != read.keychain_has_token
            || config.speleodb.keychain_token_account != read.keychain_token_account
        {
            return; // written since we read it; that write knows something we don't
        }
        let session = self.speleodb.lock();
        let raised_since = session.token_store.keychain_has_token();
        drop(session);
        if raised_since {
            return;
        }
        config.speleodb.keychain_has_token = false;
        config.speleodb.keychain_token_account = None;
        // NOT put back on a failed save, unlike the writer above, and for the reason
        // that one gives for putting it back: there is no later call in this process
        // whose guard the leftover value could short-circuit (rehydration runs once).
        // What it does instead is give the correction a second chance to reach disk
        // on the next config save of any kind - and the value left standing is one a
        // keychain read PROVED, so it is honest in memory whatever the file says.
        if let Err(e) = config.save() {
            tracing::warn!(
                error = %e,
                "failed to persist that the OS keychain no longer holds a SpeleoDB token"
            );
        }
    }

    /// Persist whether the user has declined the OS keychain after it failed at
    /// login.
    ///
    /// This has to survive a restart to do its job at all: the whole complaint is
    /// that dismissing the desktop unlock prompt once did not stop it coming back
    /// on the next launch. Best-effort in the same sense as the email and
    /// keychain-has-token writers - a failed write means the next launch asks
    /// again, which is annoying but never unsafe. A poisoned config mutex is
    /// recovered rather than treated as "cannot write", mirroring the email path.
    ///
    /// Only ever written `false` by a path that has PROVED the keychain took the
    /// key (login gates it on `TokenTier::Keychain`; so does the Settings "Try
    /// again"). Revoking the refusal on a retry that fell back to another tier
    /// brought the launch-time unlock prompt straight back (#259).
    ///
    /// The value is PUT BACK on a failed save, for the reason spelled out on
    /// [`Self::speleodb_persist_keychain_belief`]: the early return here compares
    /// against the in-memory value, so a mutation left standing over a file that
    /// never took it turns every later call with the same answer into a no-op. A
    /// keychain that starts working again would then never get its `false` to disk,
    /// and the launch after it would still skip a keyring that now works.
    pub(super) fn speleodb_persist_keychain_declined(&self, declined: bool) {
        let mut config = self.config.lock();
        if config.speleodb.keychain_declined == declined {
            return; // no write, no disk churn on every login
        }
        let previous = config.speleodb.keychain_declined;
        config.speleodb.keychain_declined = declined;
        if let Err(e) = config.save() {
            config.speleodb.keychain_declined = previous;
            tracing::warn!(
                error = %e,
                declined,
                "failed to persist the SpeleoDB keychain preference; the next launch may ask again"
            );
        }
    }

    /// Write - in ONE save - the persisted account email and the keyring belief
    /// stamped with it. The two writes LOGIN and LOGOUT used to make separately.
    ///
    /// The email is the *rehydrate seed*: `speleodb_ensure_rehydrated` keys the
    /// credential lookup by it, which is why `None` (the logout half, #270) matters -
    /// leaving it behind means a keychain entry that outlived a failed logout delete
    /// is found again on the next launch, silently re-authenticating a user who asked
    /// to log out.
    ///
    /// The **endpoint is deliberately NOT persisted here**: it is a user-facing
    /// Settings field owned by the config-staging flow, persisted only when the
    /// Settings dialog's Save runs (`updateConfig`). Writing it as a login side
    /// effect meant a "Log in now" test - or any login - leaked the endpoint into the
    /// saved config behind the dialog's back, surviving a Cancel/Undo, and also
    /// risked leaking the `DEV_SPELEODB_URL` dev box. `held_for` carries the endpoint
    /// INSIDE the belief's stamp, which is a different thing: nothing ever reads it
    /// as an endpoint, it cannot decide where the app connects, and the string is a
    /// copy of the name the OS keyring already stores the entry under.
    ///
    /// ONE save because the pair is only meaningful together. Written apart, a switch
    /// from account A - whose `true` is on disk - to account B whose keychain write
    /// fails saved B's email first, and if the belief save then failed, or the
    /// process exited between the two, disk kept the mismatched `(B, A's true)`
    /// forever. The next launch read A's evidence as B's, opened the keyring for an
    /// account it holds nothing for, and warned about a residue nothing keyed by B
    /// can remove - the exact pair of symptoms the stamp exists to prevent, recreated
    /// by writing the stamp and the thing it stamps at two different moments.
    ///
    /// Returns the reason the write did not land, so LOGOUT can report it. A
    /// silently-skipped clear is the whole hazard there: the caller was told logout
    /// succeeded while the seed that re-authenticates them is still on disk. LOGIN
    /// drops it - genuinely best-effort, since a config that could not be written
    /// just means the next launch will not silently rehydrate, the safe direction.
    ///
    /// No repeated-value guard, deliberately, so nothing here needs the roll-back its
    /// guarded siblings above make on a failed save: every call re-attempts the
    /// write, which is what makes the next one a real retry rather than a no-op. The
    /// values applied over a file that refused them are the SAFE direction besides -
    /// an in-memory seed this process will not rehydrate from, alongside a belief
    /// this session established first-hand.
    ///
    /// A POISONED config mutex is recovered (`into_inner`) rather than treated as
    /// "cannot write", exactly as the save pipeline does with the project lock
    /// (`project_io::close_project_if_open_at_held`): a poisoned lock means some other
    /// thread panicked mid-mutation, which is a reason to be careful with what is
    /// in there - not a reason to leave a live rehydrate seed behind. Skipping was
    /// the unsafe direction.
    pub(super) fn speleodb_write_account_record(
        &self,
        email: Option<String>,
        held_for: Option<String>,
    ) -> Result<(), String> {
        let mut config = self.config.lock();
        config.speleodb.email = email;
        config.speleodb.keychain_has_token = held_for.is_some();
        config.speleodb.keychain_token_account = held_for;
        config.save().map_err(|e| {
            tracing::warn!(error = %e, "failed to persist the SpeleoDB account record to config");
            e.to_string()
        })
    }

    /// Forget the local token (keychain + plaintext + in-memory), clear the session,
    /// and clear the persisted account email. The DRF token is not revoked
    /// server-side, but if we were mid-edit on a remote project we best-effort
    /// RELEASE the write lock (it needs the token, so before `forget`) so it isn't
    /// stranded, and drop the transient edit gate - a token-less session must not
    /// keep mutating a project it can never push.
    ///
    /// #270: returns `Err` when a *persistent* tier refused to give the credential
    /// up, OR when the persisted account email (the rehydrate seed) could not be
    /// cleared. The logout still happened (session dropped, gates dropped); the
    /// error means a residue that can resurrect the session survives on this
    /// machine, which the FE surfaces so the user can retry. Unlike the 401
    /// auto-drop (`AppState::speleodb_forget`), which stays best-effort by design.
    pub fn speleodb_logout(&self) -> Result<(), SpeleoErrorDto> {
        // ONE lock-ops span over the WHOLE logout - gates, lock release, AND the
        // credential sweep below - because logout is a SpeleoDB lock operation
        // (it ends an edit session and hands its mutex back) and the thing that
        // makes it one is not finished until the token is gone
        // (`core::state::speleodb_session`).
        //
        // Scoping the guard to the gate half alone left the gap this closes: a
        // Start Editing could take the guard the instant `..._drop_gates`
        // returned, read the token that had NOT been forgotten yet, and go on to
        // acquire and commit - reopening the edit gate for a session whose
        // credential this call then deleted. That flow's own commit cannot see
        // it either: the fast-forward arm commits on identity, and a logout
        // leaves the project slot exactly where it was. The user ends up signed
        // out with remote editing enabled over a server mutex that, with no
        // account left to stamp a claim with, nothing is even recording.
        //
        // Nothing inside takes this guard again: the gate half no longer does,
        // and the release path (`speleodb_release_held_lock_best_effort`) posts
        // directly and touches only the session record.
        let lock_ops = self.speleodb_lock_ops_guard();
        let gates_dropped = self.speleodb_logout_drop_gates(&lock_ops);

        // Drop the session AND the persisted rehydrate seed FIRST, then report. The
        // ordering is load-bearing: an early return on the credential failure would
        // leave the seed behind, which is the half that makes the credential findable.
        //
        // A POISONED session mutex is RECOVERED rather than turned into an early
        // return. The command's contract is that the session is always dropped (the
        // frontend clears its own session in a `finally` on that promise), and
        // returning here honoured neither half: the user stayed logged in backend-side
        // while the UI said otherwise. `forget_reporting` is a drop-everything
        // operation, so a half-written session is exactly what it wants to clear -
        // the same reasoning as the config mutex above and the save pipeline's
        // project lock.
        let cleared = self.speleodb_forget_session_in_span(gates_dropped);
        // ONE save for both halves. Whether the sweep reported a failure or not, this
        // session no longer has a keychain entry it can claim, and recording no belief
        // is the safe direction: a surviving entry costs a sign-in, never a silent
        // re-authentication. Written WITH the seed clear rather than after it, so a
        // process that dies mid-logout cannot leave the departed account's `true`
        // standing over an empty seed - or, worse, over the NEXT account's email.
        let seed_cleared = self.speleodb_write_account_record(None, None);
        logout_residue_error(cleared.err().map(|e| e.to_string()), seed_cleared.err())
    }

    /// [#576 A8] Drop the session (token, keychain entry, plaintext) - inside the
    /// SAME lock-ops span the gate half ran in: it takes the gate half's
    /// [`logout_span::GatesDropped`], which borrows that span, so a forget after the span
    /// closed (or under a fresh one) does not compile. A forget outside it would let a
    /// Start Editing read the not-yet-forgotten token (#576 RV8a-1).
    fn speleodb_forget_session_in_span(
        &self,
        _gates_dropped: logout_span::GatesDropped<'_>,
    ) -> Result<(), crate::speleodb::auth::SecretStoreError> {
        self.speleodb.lock().forget_reporting()
    }

    /// The edit-session half of [`Self::speleodb_logout_drop_gates`].
    fn speleodb_logout_release_and_drop_gates(&self) {
        match self.speleodb_open_link() {
            // Remote edit session: best-effort RELEASE the lock (needs the token, so
            // before `forget`), then drop the gates. #213: a force-editor holds NO
            // lock, so the release is a no-op there but the gates still drop.
            //
            // Through the SHARED gate, which vetoes the release when the project still
            // has un-pushed edits (it also re-checks the force-edit case). Logging out
            // is not the same act as giving up the lock: unlike unlink / take-remote -
            // the two verbs that legitimately bypass the veto - a logout neither
            // deletes the link row nor discards the local edits, so the asymmetry the
            // veto exists for applies in full. A lock left held is recoverable (the
            // re-login scan raises the held-lock prompt, which offers to resume it),
            // whereas a lock freed over stranded edits lets a colleague lock and push,
            // leaving weeks of local survey reachable only through a divergence.
            Ok(_) => {
                self.speleodb_release_held_lock_best_effort();
                self.set_edit_enabled(false);
                self.set_force_editing(false);
            }
            // Local-only project (no hand-off link): no lock, nothing to push - keep
            // it editable.
            Err(SpeleoDbError::NoRemoteProject) => {}
            // Couldn't determine locality (e.g. a poisoned database Mutex): the
            // gate-drop is a SAFETY action, so it must not be skipped just because the
            // link load failed - a logged-out session must not keep editing a project
            // it may be unable to push. Fail safe to read-only. (#197 review; no
            // project_id available here, so no lock release.)
            Err(_) => {
                self.set_edit_enabled(false);
                self.set_force_editing(false);
            }
        }
    }
}

/// [#576 A8 RV8a-1] The logout's gate half as a proof that borrows the lock-ops span it
/// ran in: the credential forget takes it, so the two cannot be split across spans.
mod logout_span {
    use super::super::speleodb_session::LockOpsHeld;
    use super::AppState;

    /// Proof the gate half ran inside the span it borrows. Made only by
    /// [`AppState::speleodb_logout_drop_gates`] below.
    pub(super) struct GatesDropped<'s> {
        _span: &'s LockOpsHeld<'s>,
    }

    impl AppState {
        /// The gate half of [`AppState::speleodb_logout`]: surrender the write lock if one is
        /// held, and drop the edit / force-edit gates.
        ///
        /// A held write lock exists only on a remote EDIT session (`edit_enabled` + a
        /// remote link); a read-only open or a local project holds none. The gates are
        /// dropped independently of whether the token still resolves - a logged-out
        /// session must not keep editing a project it can never push.
        ///
        /// **The caller must hold `speleodb_lock_ops`**, and hold it until the
        /// credential is gone. Logout is a SpeleoDB lock operation - it ends an edit
        /// session and hands its mutex back - and being unsynchronized is what let it
        /// land INSIDE a Start Editing, between that flow's token read and its
        /// commit. The guard lives in [`AppState::speleodb_logout`] rather than here
        /// because the span has to outlast this function: see there for what a
        /// gate-half-only span left open.
        ///
        /// [#576 A8 RV8a-1] Returns the proof the credential forget takes, borrowing the
        /// caller's span - so the forget runs in this span, not merely in a span.
        /// [#576 RV9a-3] The proof's only producer: it lives in this module, whose field
        /// nothing outside can fill, so no gate-less proof can be made for a fresh span.
        pub(super) fn speleodb_logout_drop_gates<'s>(
            &self,
            lock_ops: &'s LockOpsHeld<'s>,
        ) -> GatesDropped<'s> {
            // A session boundary even though the project slot never moves (footgun
            // 5.8): the token an in-flight Start Editing would commit a claim over is
            // being forgotten, so the commit guard must see this. Bumped under a brief
            // `project` acquisition per the `speleodb_session` rule; taken and dropped
            // before the gates/lock work below, none of which needs it. Bumped even on
            // the read-only early return: the boundary is the LOGOUT, not the release.
            {
                let _proj_guard = self.project.lock();
                self.bump_speleodb_session();
            }
            if self.edit_enabled() {
                self.speleodb_logout_release_and_drop_gates();
            }
            GatesDropped { _span: lock_ops }
        }
    }
}

/// Assemble logout's warning from whichever residues survived, or `Ok(())` when
/// none did.
///
/// BOTH are reported, and a run that fails both must not hide one behind the other:
/// a surviving credential is a live bearer token on this machine, and a surviving
/// email is the seed `speleodb_ensure_rehydrated` keys the keychain lookup by - so a
/// credential that outlived a failed delete gets found again on the next launch and
/// silently re-authenticates the user who asked to log out.
///
/// A free function over the two failure strings so the assembly is unit-testable
/// without a keychain or an unwritable config.
fn logout_residue_error(
    credential: Option<String>,
    seed: Option<String>,
) -> Result<(), SpeleoErrorDto> {
    let mut problems: Vec<String> = Vec::new();
    if let Some(e) = credential {
        problems.push(format!(
            "the stored SpeleoDB credential could not be removed from this machine ({e})"
        ));
    }
    if let Some(e) = seed {
        problems.push(format!(
            "the saved account could not be cleared, so this machine may sign in \
             again on the next launch ({e})"
        ));
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(local_dto(SpeleoDbError::Io(format!(
        "Logged out, but {}",
        problems.join("; and ")
    ))))
}

// Nothing below this line is production code. The `speleodb*` source scans
// (`tests_support::speleodb_prod_sources`) take everything before the FIRST
// `#[cfg(test)]` as the production region, so a test-only item above this point
// silently empties this file's half of every scan built on it.
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::{logout_residue_error, AppState};

    /// Make every `config.save()` fail, by squatting the config FILE's path with a
    /// DIRECTORY - the write cannot land no matter how the save is spelled. Returns
    /// the squatted path so the test can unblock it and retry. (Same idiom as
    /// `logout_reports_a_seed_that_could_not_be_cleared`.)
    fn squat_the_config_file() -> std::path::PathBuf {
        let squatted = crate::test_helpers::current_config_dir()
            .expect("this thread is isolated")
            .join("config.toml");
        std::fs::create_dir_all(&squatted).expect("squat the config file path");
        squatted
    }

    /// Diagnosis for the post-unblock asserts, which failed on CI and never
    /// locally (#522).
    ///
    /// After the squat is lifted, the retry can miss its disk write two ways -
    /// the retry short-circuited on the in-memory guard (the real bug these
    /// tests catch), or the unblocked save failed again for an environmental
    /// reason. Save and load cannot diverge on the PATH: both go through
    /// `Config::config_path()`, which resolves to this thread's own config dir.
    ///
    /// An empty config dir alone cannot tell those apart. What discriminates
    /// them is the file `Config::load` reads: absent means the retry attempted
    /// the write and it failed again, while present narrows it to a write that
    /// never happened OR a file that is not the one this test wrote - #522 was
    /// the second, another test's `config.toml` renamed over this one between
    /// the save and the read-back, which per-thread config resolution now
    /// prevents. So this prints the load path AND that file's contents (whose
    /// `recent_projects` name the writer), not just a dir listing. Total by
    /// construction - it must never panic and replace the assertion message it
    /// is meant to enrich.
    fn config_state_for_diagnosis() -> String {
        use std::fmt::Write;
        let mut out = String::new();

        match crate::config::Config::config_path() {
            Ok(path) => {
                let _ = write!(out, "Config::load reads {}", path.display());
                match std::fs::read_to_string(&path) {
                    Ok(body) => {
                        let _ = write!(out, " (present, {} bytes):\n{body}", body.len());
                    }
                    Err(e) => {
                        let _ = write!(out, " (unreadable: {e})");
                    }
                }
            }
            Err(e) => {
                let _ = write!(out, "Config::config_path() errored: {e}");
            }
        }

        let Some(dir) = crate::test_helpers::current_config_dir() else {
            out.push_str("; this thread resolves the config for real");
            return out;
        };
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                let contents: Vec<String> = entries
                    .map(|entry| match entry {
                        Ok(e) => {
                            let name = e.file_name().to_string_lossy().into_owned();
                            match e.metadata() {
                                Ok(m) if m.is_file() => format!("{name} ({}B)", m.len()),
                                Ok(_) => format!("{name} (non-file)"),
                                Err(err) => format!("{name} (metadata: {err})"),
                            }
                        }
                        Err(err) => format!("<entry error: {err}>"),
                    })
                    .collect();
                let _ = write!(out, "; {} holds [{}]", dir.display(), contents.join(", "));
            }
            Err(e) => {
                let _ = write!(out, "; {} unreadable: {e}", dir.display());
            }
        }
        out
    }

    /// The diagnostic runs only when an assertion fails, so nothing else would
    /// notice it silently breaking. This exercises both of its discriminating
    /// branches - a present config file and an absent one - so a regression in
    /// the helper reds here rather than during the next flake it is meant to
    /// explain.
    #[test]
    fn config_state_for_diagnosis_discriminates_present_from_absent() {
        crate::test_helpers::with_isolated_config(|| {
            // Absent: no config.toml written yet (cause-2 shape).
            let absent = config_state_for_diagnosis();
            assert!(
                absent.contains("config.toml"),
                "names the load path even when the file is absent: {absent}"
            );
            assert!(
                absent.contains("unreadable") || absent.contains("holds ["),
                "reports the absent file and the dir listing: {absent}"
            );

            // Present: a real save lands a config.toml (cause-1 shape).
            let state = AppState::new().expect("state");
            state.speleodb_persist_keychain_belief(Some(crate::speleodb::auth::account_key(
                "https://sdb.example",
                "u@x",
            )));
            let present = config_state_for_diagnosis();
            assert!(
                present.contains("present"),
                "a written config.toml is reported present with its contents: {present}"
            );
        });
    }

    #[test]
    fn a_keychain_record_that_could_not_be_saved_is_retried_not_swallowed() {
        // Both writers here mutate the in-memory config, then save, then guard the
        // NEXT call on that same in-memory value. Leaving the mutation applied over a
        // file that never took it made the very next call a no-op: the guard matched,
        // so a transient failure - a full disk, a config file locked by a backup
        // sweep - became permanent until the process restarted.
        //
        // What that costs here is not a re-asked prompt but a lost RECORD: the launch
        // after it reads `false` off disk and skips a keyring entry that really
        // exists, which is the entire failure `keychain_has_token` was introduced to
        // prevent.
        crate::test_helpers::with_isolated_config(|| {
            let squatted = squat_the_config_file();
            let state = AppState::new().expect("state");

            let held = Some(crate::speleodb::auth::account_key(
                "https://sdb.example",
                "u@x",
            ));
            state.speleodb_persist_keychain_belief(held.clone());
            assert!(
                !state.get_config().speleodb.keychain_has_token,
                "the value goes back when the save fails, or the retry below is a no-op"
            );

            // The retry, on a machine where whatever blocked the write has cleared.
            std::fs::remove_dir(&squatted).expect("unblock the config file");
            state.speleodb_persist_keychain_belief(held.clone());

            let saved = crate::config::Config::load().expect("read the config back off disk");
            // FIRST, because it decides which question the assertion below is
            // asking. This test opens no project, so its own config carries no
            // recent projects; a document that does is somebody else's file and
            // the finding is the isolation, not the retry (#522).
            assert!(
                saved.general.recent_projects.is_empty(),
                "the config read back must be the one this test wrote - {}",
                config_state_for_diagnosis()
            );
            assert!(
                saved.speleodb.keychain_has_token,
                "the retry has to re-attempt the disk write, not short-circuit on the \
                 value the failed attempt left behind - {}",
                config_state_for_diagnosis()
            );
            assert_eq!(
                saved.speleodb.keychain_token_account, held,
                "and the belief lands with the account it is about, not on its own"
            );
        });
    }

    #[test]
    fn a_keychain_refusal_that_could_not_be_saved_is_retried_not_swallowed() {
        // The sibling writer, the same shape, and the same fix. Its stake is the
        // launch-time unlock prompt in both directions: a `true` that never lands
        // brings the prompt back, and a `false` that never lands - written only once
        // the keychain has actually taken the key - leaves the next launch skipping a
        // keyring that has started working again.
        crate::test_helpers::with_isolated_config(|| {
            let squatted = squat_the_config_file();
            let state = AppState::new().expect("state");

            state.speleodb_persist_keychain_declined(true);
            assert!(
                !state.get_config().speleodb.keychain_declined,
                "the value goes back when the save fails, or the retry below is a no-op"
            );

            std::fs::remove_dir(&squatted).expect("unblock the config file");
            state.speleodb_persist_keychain_declined(true);

            let saved = crate::config::Config::load().expect("read the config back off disk");
            // Same order and the same reason as its sibling above.
            assert!(
                saved.general.recent_projects.is_empty(),
                "the config read back must be the one this test wrote - {}",
                config_state_for_diagnosis()
            );
            assert!(
                saved.speleodb.keychain_declined,
                "the retry has to re-attempt the disk write, not short-circuit on the \
                 value the failed attempt left behind - {}",
                config_state_for_diagnosis()
            );
        });
    }

    #[test]
    fn a_clean_logout_reports_nothing() {
        assert!(logout_residue_error(None, None).is_ok());
    }

    #[test]
    fn each_residue_is_named_and_neither_hides_the_other() {
        // The seed half used to have no error channel at all: the clear was skipped
        // silently on a poisoned config mutex and logout reported success, leaving the
        // session to re-authenticate itself on the next launch.
        let credential = logout_residue_error(Some("keychain refused".to_owned()), None)
            .expect_err("a surviving credential is reported");
        assert!(credential.message.contains("could not be removed"));
        assert!(
            !credential.message.contains("sign in again"),
            "and does not claim the seed also survived: {}",
            credential.message
        );

        let seed = logout_residue_error(None, Some("read-only config".to_owned()))
            .expect_err("a surviving rehydrate seed is reported on its own");
        assert!(seed.message.contains("sign in again"));

        // Both: one message, both facts. Reporting only the first would hide the half
        // that makes the other one dangerous.
        let both = logout_residue_error(
            Some("keychain refused".to_owned()),
            Some("read-only config".to_owned()),
        )
        .expect_err("both residues are reported");
        assert!(
            both.message.contains("could not be removed") && both.message.contains("sign in again"),
            "both residues must survive into the one warning: {}",
            both.message
        );
    }
}
