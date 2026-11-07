//! WHERE the SpeleoDB API key is kept, once the OS keychain has already failed.
//!
//! Split out of `core::state::speleodb` (frozen at the file-size ratchet) as a
//! sibling `impl AppState` block, next to `speleodb_logout` - which owns the other
//! half of the same preference, the persisted refusal flag.
//!
//! Both verbs here are the SAME operation with the decline flag set differently
//! beforehand: clear or set `keychain_declined`, then re-store the token the
//! session is already holding ([`crate::speleodb::auth::TokenStore::restore`]).
//! That is the point of asking at login rather than in Settings - at the moment
//! the keychain fails the key is in memory, so honouring the answer costs no
//! re-login and no re-typed password.
//!
//! Neither verb PROBES the keychain, but neither leaves it strictly alone either.
//! `speleodb_set_token_storage` sets the decline flag first, so the store/load
//! path never reaches the keyring - yet a SESSION-ONLY answer still issues one
//! direct DELETE ([`crate::speleodb::auth::TokenStore::restore`] sweeps both
//! persistent tiers when the token lands in memory), because "gone when the app
//! closes" has to be true on disk and an entry from an earlier working login would
//! otherwise sign the user straight back in. `speleodb_retry_keychain` reaches it
//! for the obvious reason: the user pressed "Try again". Both are explicit
//! user-initiated moments - the one context where a desktop unlock prompt is an
//! answer rather than a nag - and neither runs from a background path.

use super::speleodb::local_dto;
use super::AppState;
use crate::config::TokenFallback;
use crate::speleodb::auth::{account_key, TokenTier};
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto, SpeleoSessionStatus};

impl AppState {
    /// Record where the user wants the API key kept after this login's keychain
    /// failure, and move it there. `mode` is the new `token_fallback` preference:
    /// `SessionOnly` (gone when the app closes) or `Plaintext` (a `0o600` file that
    /// survives a restart).
    ///
    /// Also sets `keychain_declined`, because a choice made here is only meaningful
    /// if we then stop asking: while it is set nothing is stored to or read from
    /// the keychain - not a login write, not a startup read, not a status probe -
    /// which is what stops the dismissed unlock prompt returning on every launch.
    /// (The one exception is the session-only sweep described in the module doc,
    /// which DELETES rather than asks.)
    ///
    /// Returns the refreshed status so the frontend renders where the key actually
    /// landed rather than where it asked for it to go: the plaintext write can
    /// itself fail, and then the honest answer is session-only. `status.storage` is
    /// what the storage prompt reports, so its toast names the tier that took the
    /// key rather than the button that was pressed.
    ///
    /// Errors - including a session-only answer whose sweep left a persistent copy
    /// behind (see [`Self::speleodb_move_token`]), and a `config.toml` that would
    /// not take the choice - leave the preference unwritten, so a failure never
    /// looks settled.
    pub fn speleodb_set_token_storage(
        &self,
        mode: TokenFallback,
    ) -> Result<SpeleoSessionStatus, SpeleoErrorDto> {
        // The sentinel never reaches the session or the config file; an
        // unrecognized mode falls to the safe tier rather than the on-disk one.
        let fallback = if mode == TokenFallback::Plaintext {
            TokenFallback::Plaintext
        } else {
            TokenFallback::SessionOnly
        };
        self.speleodb_move_token(true, fallback == TokenFallback::Plaintext)?;
        // Persisted only after the move succeeded, so a "not signed in" refusal
        // leaves no settled-looking preference behind for the next launch.
        //
        // And REPORTED when it will not persist, down the same failed-move path
        // rather than a channel of its own: the key really did move, but the ANSWER
        // did not land, so the next launch reaches for the keychain again and raises
        // the very unlock prompt the choice exists to stop. Returning success there
        // closed the prompt and told the user their choice had been recorded. The
        // frontend already keeps this prompt open (and re-raises it on a dismissal
        // that landed mid-move) for every `Err` this verb returns, so the retry the
        // user needs is the one they already have.
        self.speleodb_persist_storage_choice(fallback)
            .map_err(|e| {
                local_dto(SpeleoDbError::Io(format!(
                    "Your API key was moved, but the choice could not be saved ({e}), \
                     so the app may ask again the next time you open it"
                )))
            })?;
        self.speleodb_session_status()
    }

    /// Give the OS keychain another go (the Settings "Try again" affordance): lift
    /// the refusal on the live store and re-store the held token, which now reaches
    /// the keychain again. A user whose keyring was locked when they signed in,
    /// and who has since unlocked it, gets a durable key without signing out.
    ///
    /// The PERSISTED refusal is lifted only if the keychain actually takes the key
    /// - see the gate below. The live one is lifted either way, which is what makes
    /// a second press meaningful.
    ///
    /// The refreshed status is the whole answer: if the keychain is still broken
    /// the re-store falls back exactly as login does and the report says
    /// `unavailable`, so the button is safe to press twice.
    pub fn speleodb_retry_keychain(&self) -> Result<SpeleoSessionStatus, SpeleoErrorDto> {
        // The plaintext preference is unchanged by a retry - it only decides where
        // the token lands if the keychain refuses AGAIN.
        let allow_plaintext = self.speleodb_plaintext_allowed();
        let tier = self.speleodb_move_token(false, allow_plaintext)?;
        // Revoke the persisted refusal ONLY once the keychain has actually taken the
        // key - the same gate the login path is held to, for the same reason (#259).
        // `speleodb_move_token` reports `Ok` for ANY landing tier, so writing `false`
        // unconditionally revoked the refusal on a retry that had just fallen back to
        // plaintext or memory: the keyring was still broken, nothing had proved
        // otherwise, and the next launch rehydrated through it and re-raised the
        // unlock prompt the flag exists to stop.
        //
        // The live store's flag stays cleared either way, exactly as it does after a
        // failed login attempt: that is what lets this session report `unavailable`
        // (a broken keyring) rather than `declined`, which is the honest answer to a
        // button the user just pressed - and it is what keeps a further press from
        // being a no-op.
        if tier == TokenTier::Keychain {
            self.speleodb_persist_keychain_declined(false);
        }
        self.speleodb_session_status()
    }

    /// The shared half of both verbs: apply `declined` to the live token store,
    /// re-persist the held token under it, and report WHICH TIER it landed on.
    ///
    /// The tier is the return value rather than a discarded detail because `Ok`
    /// alone says almost nothing: the fallback chain means a still-broken keychain
    /// resolves to plaintext or memory and still reports success. A caller deciding
    /// whether the keychain has proved itself has to ask the tier.
    ///
    /// Refuses when no token is held. That is not pedantry - the two callers are a
    /// post-login prompt and a Settings button, and both can be reached after the
    /// session ended underneath them (a 401 auto-drop, a logout in another window).
    /// Silently succeeding would report a storage tier for a key that no longer
    /// exists, and would persist a preference the user never got the benefit of.
    ///
    /// Refuses too when the key landed in session-only memory but a persistent copy
    /// of it survived the sweep. The tier reported would be perfectly accurate and
    /// completely misleading: "Memory" over a cleartext file that the next cold
    /// `TokenStore::load` reads straight back is the promise "the key disappears
    /// when the app closes" made about a machine where it demonstrably does not.
    fn speleodb_move_token(
        &self,
        declined: bool,
        allow_plaintext: bool,
    ) -> Result<TokenTier, SpeleoErrorDto> {
        // A cold session that has only ever been rehydrated lazily still holds its
        // token behind this call, so run it before reading the session's identity.
        self.speleodb_ensure_rehydrated();

        let mut session = self.speleodb.lock();
        let (Some(endpoint), Some(email)) = (session.endpoint.clone(), session.email.clone())
        else {
            return Err(local_dto(SpeleoDbError::NotAuthenticated));
        };
        // Flip the flag around the restore, and put it BACK if the restore fails.
        // Leaving the mutation in place on the failure path left the live store
        // saying "the keychain is fine to use" while `config.speleodb
        // .keychain_declined` still said the opposite - the disagreement the login
        // path's "the two must not disagree across a restart" note guards against,
        // and enough to let a later keychain access in this process raise the
        // unlock prompt the persisted refusal forbids. Reachable whenever the
        // session ended underneath the panel (a 401 auto-drop, a logout in another
        // window) between the identity check above and the restore.
        let previously_declined = session.token_store.keychain_declined();
        session.token_store.set_keychain_declined(declined);
        match session
            .token_store
            .restore(&endpoint, &email, allow_plaintext)
        {
            Ok(Some(tier)) => {
                // What the KEYRING is believed to hold, read off the store - not
                // `tier == TokenTier::Keychain`, which answers where THIS move
                // landed. The two diverge on precisely the machine both verbs exist
                // for: answering the prompt with "write it to a file" stores to
                // plaintext without going near the keyring, so an entry an earlier
                // working login left there survives untouched, and deriving the flag
                // from the landing tier wrote `false` over the only record of it. A
                // "Try again" that the keyring refuses again is the same story. Read
                // BEFORE the session lock is released, and persisted after: the
                // config lock is taken inside the writer, and the documented order is
                // config-then-session.
                //
                // Stamped with the account this move was keyed by, so the record can
                // never be read as one about a different account (or the same address
                // on a different server) later.
                let has_token = session.token_store.keychain_has_token();
                drop(session);
                self.speleodb_persist_keychain_belief(
                    has_token.then(|| account_key(&endpoint, &email)),
                );
                Ok(tier)
            }
            // Nothing was settled, by either route, so the live flag goes back the
            // way it came in both - a settled-looking store over an unsettled config
            // is the disagreement described above.
            Ok(None) => {
                session
                    .token_store
                    .set_keychain_declined(previously_declined);
                Err(local_dto(SpeleoDbError::NotAuthenticated))
            }
            // The key DID move, but a persistent copy of it survived the
            // session-only sweep, so "gone when the app closes" is not true on this
            // machine. Reporting it is the whole point: the caller must not persist
            // the choice, and the user has to be told, because the surviving copy is
            // what signs them straight back in on the next launch.
            Err(e) => {
                session
                    .token_store
                    .set_keychain_declined(previously_declined);
                Err(local_dto(SpeleoDbError::Io(format!(
                    "Your API key was moved, but an older copy could not be removed \
                     from this computer ({e}), so it may sign you back in the next \
                     time you open the app"
                ))))
            }
        }
    }

    /// Read the `token_fallback` preference under the config lock. Taken BEFORE the
    /// session lock is acquired: `speleodb_ensure_rehydrated` documents the
    /// config-then-session lock order, and inverting it here would be the deadlock.
    fn speleodb_plaintext_allowed(&self) -> bool {
        self.config.lock().speleodb.token_fallback == TokenFallback::Plaintext
    }

    /// Persist the pair the login-time choice settles: where the key goes, and the
    /// fact that the question has been answered. One lock and one save rather than
    /// two, so a crash between them can't leave a refusal recorded with no
    /// destination (or the reverse).
    ///
    /// Reports the reason the write did not land, because this pair is the ONLY
    /// durable form the answer has: the whole complaint the prompt exists to answer
    /// is that the keyring is reached for again on the next launch, and that
    /// decision is made from `config.toml` and nothing else. Unlike its email /
    /// decline siblings this is not best-effort - the caller turns a failure into
    /// the same refusal a failed move gets, so the question stays open.
    ///
    /// The in-memory values are PUT BACK on a failed save, mirroring
    /// [`Self::speleodb_move_token`]'s treatment of the live decline flag. Leaving
    /// them applied over a file that never took them made the very next attempt a
    /// no-op: the early return below compares against exactly those values, so a
    /// retry in this process found the choice already "made" and never touched the
    /// disk again. The live token store is deliberately NOT rolled back with them -
    /// the key HAS moved and the session must keep honouring that, and a fresh login
    /// is the one thing that re-opens the keychain to it anyway.
    fn speleodb_persist_storage_choice(&self, fallback: TokenFallback) -> Result<(), String> {
        let mut config = self.config.lock();
        if config.speleodb.token_fallback == fallback && config.speleodb.keychain_declined {
            return Ok(()); // no write, no disk churn on a repeated choice
        }
        let previous = (
            config.speleodb.token_fallback,
            config.speleodb.keychain_declined,
        );
        config.speleodb.token_fallback = fallback;
        config.speleodb.keychain_declined = true;
        if let Err(e) = config.save() {
            config.speleodb.token_fallback = previous.0;
            config.speleodb.keychain_declined = previous.1;
            tracing::warn!(
                error = %e,
                ?fallback,
                "failed to persist the SpeleoDB credential-storage choice; the next launch may ask again"
            );
            return Err(e.to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::AppState;
    use crate::config::TokenFallback;
    use crate::speleodb::auth::{
        account_key, InMemoryStore, PlaintextStore, SecretStore, SecretStoreError, TokenStore,
    };
    use crate::speleodb::{KeychainReport, TokenStorage};

    const EP: &str = "https://sdb.example";
    const EMAIL: &str = "u@x";

    /// A keychain that refuses every op - the locked / headless keyring that makes
    /// "Try again" a button worth having, and worth pressing twice.
    struct FailingSecretStore;

    impl SecretStore for FailingSecretStore {
        fn kind(&self) -> &'static str {
            "failing"
        }
        fn store(&self, _: &str, _: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError::for_test("no secret service"))
        }
        fn load(&self, _: &str) -> Result<Option<String>, SecretStoreError> {
            Err(SecretStoreError::for_test("no secret service"))
        }
        fn delete(&self, _: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError::for_test("no secret service"))
        }
    }

    /// A tier that IS holding a copy of the key and will not give it up - a
    /// read-only or full filesystem under the plaintext root, or a keyring entry
    /// the OS refuses to delete.
    ///
    /// [`FailingSecretStore`] cannot express this and is the wrong fixture for it:
    /// a store nothing can be written to is a store nothing can survive in, which
    /// is exactly the case the sweep must NOT report.
    struct DeleteFailsStore(InMemoryStore);

    impl DeleteFailsStore {
        fn holding(account_key: &str, token: &str) -> Self {
            let inner = InMemoryStore::new();
            inner.store(account_key, token).expect("seed the copy");
            Self(inner)
        }
    }

    impl SecretStore for DeleteFailsStore {
        fn kind(&self) -> &'static str {
            "delete-fails"
        }
        fn store(&self, k: &str, v: &str) -> Result<(), SecretStoreError> {
            self.0.store(k, v)
        }
        fn load(&self, k: &str) -> Result<Option<String>, SecretStoreError> {
            self.0.load(k)
        }
        fn delete(&self, _: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError::for_test("delete refused"))
        }
    }

    /// A state whose session already holds a token, over a WORKING fake keychain
    /// and a plaintext tier in `plaintext_root`. The working keychain is the point:
    /// it makes "we did not touch it" observable as a tier that is not Keychain.
    fn holding(plaintext_root: Option<std::path::PathBuf>) -> AppState {
        holding_over(Box::new(InMemoryStore::new()), plaintext_root)
    }

    /// [`holding`] with the keychain backend chosen by the caller, so a test can
    /// ask what happens when the retry does NOT reach the keyring.
    fn holding_over(
        keychain: Box<dyn SecretStore>,
        plaintext_root: Option<std::path::PathBuf>,
    ) -> AppState {
        holding_over_tiers(
            keychain,
            plaintext_root.map(|r| Box::new(PlaintextStore::new(r)) as Box<dyn SecretStore>),
        )
    }

    /// [`holding`] with BOTH backends chosen by the caller, so a test can ask what
    /// happens when the session-only sweep cannot remove an older copy.
    ///
    /// `rehydrate_attempted` is left alone - a seeded `endpoint` already
    /// short-circuits `speleodb_ensure_rehydrated`, so the fixture never reaches
    /// the config/keychain read.
    fn holding_over_tiers(
        keychain: Box<dyn SecretStore>,
        plaintext: Option<Box<dyn SecretStore>>,
    ) -> AppState {
        let state = AppState::new().expect("state");
        let mut session = state.speleodb.lock();
        session.endpoint = Some(EP.to_owned());
        session.email = Some(EMAIL.to_owned());
        session.token_store = TokenStore::from_stores_holding("tok-1", keychain, plaintext);
        drop(session);
        state
    }

    /// The persisted `(token_fallback, keychain_declined)` pair.
    fn persisted(state: &AppState) -> (TokenFallback, bool) {
        let config = state.config.lock();
        (
            config.speleodb.token_fallback,
            config.speleodb.keychain_declined,
        )
    }

    /// Tell the live store what a previous launch's successful keychain login
    /// recorded, exactly as `SpeleoDbSession::rehydrate` mirrors in the persisted
    /// belief. Both halves are set, because both are read: the store's copy answers
    /// the sweeps, the config's is what the next launch and the Settings keyring
    /// switch see. The persisted half is stamped with the account it is about, the
    /// way every writer of it stamps it.
    fn a_previous_login_filled_the_keyring(state: &AppState) {
        state
            .speleodb
            .lock()
            .token_store
            .note_keychain_has_token(true);
        state.speleodb_persist_keychain_belief(Some(account_key(EP, EMAIL)));
    }

    /// Whether the config still records a keyring entry for this account.
    fn believed_in_keyring(state: &AppState) -> bool {
        state.config.lock().speleodb.keychain_has_token
    }

    #[test]
    fn choosing_a_file_does_not_erase_an_earlier_keyring_entry() {
        // `config.speleodb.keychain_has_token` answers "what is in the keyring". It
        // used to be written here as `tier == TokenTier::Keychain` - "where did THIS
        // move land" - and answering a prompt with "save it to a file" lands on
        // plaintext WITHOUT going near the keyring, so an entry a previous working
        // login left there survives untouched while the only record of it is
        // overwritten with `false`.
        //
        // That record is load-bearing twice over: it is what lets startup skip the
        // keyring, and it is the only thing separating a merely LOCKED keyring from a
        // box with no secret service - both fail every op, both record `Unavailable`.
        // Losing it is what let a later "never use the OS keychain" classify its
        // failed eviction as an empty one and report success over a live bearer token
        // (see `a_login_the_keyring_refuses_does_not_erase_an_earlier_logins_entry`
        // for that end of it).
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            // A keyring that will not answer today, over an entry Monday put there.
            let state = holding_over(Box::new(FailingSecretStore), Some(dir.path().to_path_buf()));
            a_previous_login_filled_the_keyring(&state);

            let status = state
                .speleodb_set_token_storage(TokenFallback::Plaintext)
                .expect("moved");

            assert_eq!(status.storage, Some(TokenStorage::Plaintext));
            assert!(
                believed_in_keyring(&state),
                "the keyring was never touched by this move, so Monday's entry is still \
                 in it and the record of it must survive the answer"
            );
        });
    }

    #[test]
    fn a_retry_the_keychain_refuses_does_not_erase_the_entry() {
        // The same write, reached through the other verb the finding names. "Try
        // again" against a keyring that is still broken falls back exactly as a login
        // does, so `tier == TokenTier::Keychain` was false and the entry's record went
        // with it - on the one button whose entire audience is people whose keyring is
        // in that state.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = holding_over(Box::new(FailingSecretStore), Some(dir.path().to_path_buf()));
            a_previous_login_filled_the_keyring(&state);
            state
                .speleodb_set_token_storage(TokenFallback::Plaintext)
                .expect("declined first");

            let status = state.speleodb_retry_keychain().expect("retried");

            assert_eq!(
                status.storage,
                Some(TokenStorage::Plaintext),
                "the keyring is still broken, so the key went back to the file"
            );
            assert!(
                believed_in_keyring(&state),
                "a retry that never reached the keyring has proved nothing about what \
                 is in it"
            );
        });
    }

    #[test]
    fn a_session_only_choice_that_empties_the_keyring_lowers_the_belief() {
        // The other direction, and the one that keeps the fix above from becoming
        // "never write `false`". A session-only answer SWEEPS both persistent tiers,
        // and a keyring that accepts the delete has PROVED it holds nothing for this
        // account - so the record has to follow it down, or the next launch opens a
        // keyring for nothing and the switch warns about a credential that is gone.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = holding(Some(dir.path().to_path_buf()));
            a_previous_login_filled_the_keyring(&state);

            state
                .speleodb_set_token_storage(TokenFallback::SessionOnly)
                .expect("moved");

            assert!(
                !believed_in_keyring(&state),
                "the sweep emptied it, which is proof - not the absence of evidence"
            );
        });
    }

    #[test]
    fn a_choice_the_config_would_not_take_is_not_reported_as_settled() {
        // The pair written here IS the answer: nothing else survives the process, and
        // whether the next launch reaches for the keychain is decided from
        // `config.toml` and nowhere else. A save that failed was logged and the verb
        // still returned a healthy session status, so the prompt closed and told the
        // user their choice had landed - and the next launch raised the unlock prompt
        // the choice exists to stop.
        //
        // The second half is worse and is why the values are put back: they were
        // applied in memory BEFORE the save, so the repeated-choice early return
        // matched them on the very next attempt and the disk write was never tried
        // again in this process. Fixing only the return value would have handed the
        // frontend a retry that could not work.
        //
        // Failure is injected by squatting the config FILE's path with a DIRECTORY, so
        // the write cannot land no matter how the save is spelled (the same idiom as
        // `logout_reports_a_seed_that_could_not_be_cleared`).
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = holding(Some(dir.path().to_path_buf()));
            let config_dir =
                crate::test_helpers::current_config_dir().expect("this thread is isolated");
            let squatted = config_dir.join("config.toml");
            std::fs::create_dir_all(&squatted).expect("squat the config file path");

            let err = state
                .speleodb_set_token_storage(TokenFallback::Plaintext)
                .expect_err("a choice that did not reach disk is not settled");

            assert!(
                err.message.contains("could not be saved"),
                "the user has to be told the choice did not stick: {}",
                err.message
            );
            assert_eq!(
                persisted(&state),
                (TokenFallback::SessionOnly, false),
                "and the in-memory pair goes back, or the retry below is a no-op"
            );

            // The retry the frontend re-raises the prompt for, on a machine where
            // whatever blocked the write has since cleared.
            std::fs::remove_dir(&squatted).expect("unblock the config file");
            state
                .speleodb_set_token_storage(TokenFallback::Plaintext)
                .expect("the second attempt has somewhere to write");

            let saved = crate::config::Config::load().expect("read the config back off disk");
            assert_eq!(
                (
                    saved.speleodb.token_fallback,
                    saved.speleodb.keychain_declined
                ),
                (TokenFallback::Plaintext, true),
                "the retry has to actually re-attempt the disk write, not short-circuit \
                 on the values the failed attempt left behind"
            );
        });
    }

    #[test]
    fn choosing_a_file_moves_the_key_there_and_settles_the_question() {
        // Both halves matter. Moving the key is what the user asked for; recording
        // the refusal is what stops the keychain being touched again on the next
        // launch, which is the complaint the whole flow exists to answer.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = holding(Some(dir.path().to_path_buf()));

            let status = state
                .speleodb_set_token_storage(TokenFallback::Plaintext)
                .expect("the session holds a token, so it can be moved");

            assert_eq!(status.storage, Some(TokenStorage::Plaintext));
            assert_eq!(status.keychain, KeychainReport::Declined);
            assert!(status.logged_in, "moving the key does not sign anyone out");
            assert_eq!(
                persisted(&state),
                (TokenFallback::Plaintext, true),
                "the choice has to survive a restart to do its job"
            );
        });
    }

    #[test]
    fn choosing_session_only_keeps_the_key_off_disk() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = holding(Some(dir.path().to_path_buf()));

            let status = state
                .speleodb_set_token_storage(TokenFallback::SessionOnly)
                .expect("moved");

            assert_eq!(status.storage, Some(TokenStorage::Memory));
            assert_eq!(persisted(&state), (TokenFallback::SessionOnly, true));
            // Nothing was written to the plaintext root - "this session only" is a
            // promise about the filesystem, not a label.
            assert!(
                std::fs::read_dir(dir.path()).is_ok_and(|mut d| d.next().is_none()),
                "a session-only choice must leave no token file behind"
            );
        });
    }

    #[test]
    fn a_session_only_choice_that_left_a_copy_behind_is_not_settled() {
        // The prompt says the key will be gone when the app closes. When the sweep
        // cannot remove an older copy that promise is false ON THIS MACHINE, and the
        // tier reported ("memory") is accurate and completely misleading: the next
        // cold `TokenStore::load` falls back to the surviving file and signs the user
        // straight back in. The verb used to persist the choice and report success
        // over exactly that, because the failed delete was only `warn!`-ed.
        crate::test_helpers::with_isolated_config(|| {
            let state = holding_over_tiers(
                Box::new(InMemoryStore::new()),
                Some(Box::new(DeleteFailsStore::holding(
                    &account_key(EP, EMAIL),
                    "yesterdays-copy",
                ))),
            );

            let err = state
                .speleodb_set_token_storage(TokenFallback::SessionOnly)
                .expect_err("a promise the filesystem did not keep is not a success");

            assert!(
                err.message.contains("could not be removed"),
                "the user has to be told WHAT survived, not just that something did: {}",
                err.message
            );
            assert_eq!(
                persisted(&state),
                (TokenFallback::SessionOnly, false),
                "and the question stays open - recording the refusal would settle an \
                 answer that did not land, and stop the keychain being offered again"
            );
        });
    }

    #[test]
    fn the_sentinel_mode_falls_to_the_safe_tier() {
        // `TokenFallback::Unknown` is the serde catch-all; an unrecognized mode from
        // the webview must never be resolved towards the ON-DISK tier.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = holding(Some(dir.path().to_path_buf()));

            let status = state
                .speleodb_set_token_storage(TokenFallback::Unknown)
                .expect("moved");

            assert_eq!(status.storage, Some(TokenStorage::Memory));
            assert_eq!(persisted(&state), (TokenFallback::SessionOnly, true));
        });
    }

    #[test]
    fn retrying_the_keychain_clears_the_refusal_and_re_stores() {
        // The "Try again" affordance for someone who has since unlocked their
        // keyring: no sign-out, no re-typed password, and the refusal is lifted so
        // the next launch rehydrates from the keychain again.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = holding(Some(dir.path().to_path_buf()));
            state
                .speleodb_set_token_storage(TokenFallback::Plaintext)
                .expect("declined first");

            let status = state.speleodb_retry_keychain().expect("retried");

            assert_eq!(status.storage, Some(TokenStorage::Keychain));
            assert_eq!(status.keychain, KeychainReport::Available);
            let (fallback, declined) = persisted(&state);
            assert!(!declined, "the refusal is lifted");
            assert_eq!(
                fallback,
                TokenFallback::Plaintext,
                "the retry does not undo where the key goes if the keychain fails again"
            );
        });
    }

    #[test]
    fn a_retry_that_never_reached_the_keychain_keeps_the_refusal() {
        // The retry falls back exactly as login does, so `speleodb_move_token`
        // reports `Ok` even when the keyring is still broken and the key just went
        // back into the plaintext file. Revoking the persisted refusal on that
        // outcome undid an answer nothing had disproved, and the next launch
        // rehydrated through the keychain and re-raised the unlock prompt the flag
        // exists to stop - the same hazard the login path is gated against (#259).
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = holding_over(Box::new(FailingSecretStore), Some(dir.path().to_path_buf()));
            state
                .speleodb_set_token_storage(TokenFallback::Plaintext)
                .expect("declined first");
            assert_eq!(persisted(&state), (TokenFallback::Plaintext, true));

            let status = state.speleodb_retry_keychain().expect("retried");

            assert_eq!(
                status.storage,
                Some(TokenStorage::Plaintext),
                "the keyring is still broken, so the key went back to the file"
            );
            assert_eq!(
                status.keychain,
                KeychainReport::Unavailable,
                "and the panel says why, rather than claiming the user declined"
            );
            let (_, declined) = persisted(&state);
            assert!(
                declined,
                "a retry that never reached the keychain must not revoke the refusal"
            );
        });
    }

    #[test]
    fn a_failed_move_puts_the_live_decline_flag_back() {
        // The session can end underneath the panel between the identity check and
        // the restore. Leaving the flag flipped there left the live store saying
        // "the keychain is fine to use" while the persisted preference still said
        // the opposite - and that disagreement is enough to let a later keychain
        // access raise the unlock prompt the refusal forbids.
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            {
                let mut session = state.speleodb.lock();
                session.endpoint = Some("https://sdb.example".to_owned());
                session.email = Some("u@x".to_owned());
                // Identity present, but NO token held - `restore` returns None.
                session.token_store = TokenStore::from_stores(Box::new(InMemoryStore::new()), None);
                session.token_store.set_keychain_declined(true);
            }

            assert!(state.speleodb_retry_keychain().is_err(), "no token to move");

            let session = state.speleodb.lock();
            assert!(
                session.token_store.keychain_declined(),
                "a failed move must not leave the refusal silently lifted"
            );
        });
    }

    #[test]
    fn neither_verb_settles_anything_without_a_session() {
        // Both are reachable after the session died underneath them (a 401 auto-drop,
        // a logout in another window). Persisting a preference for a key that no
        // longer exists would mean the user is silently opted out of the keychain
        // without ever having been given the benefit of the choice.
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            assert!(state
                .speleodb_set_token_storage(TokenFallback::Plaintext)
                .is_err());
            assert!(state.speleodb_retry_keychain().is_err());
            assert_eq!(persisted(&state), (TokenFallback::SessionOnly, false));
        });
    }
}
