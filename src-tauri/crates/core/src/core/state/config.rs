use anyhow::{anyhow, bail, Result};

use crate::config::{Config, ConfigPatch, ConfigSaveReport, TokenFallback};

use super::AppState;

/// What [`AppState::speleodb_disable_keychain`] was able to establish about the
/// keychain entry it was asked to evict. Three outcomes rather than a bool,
/// because the two that are not "gone" mean opposite things to the user: one is
/// the ordinary path, the other is a live bearer token left on their machine.
enum KeychainEviction {
    /// The keyring answered, and holds nothing for this account any more. The
    /// caller may record `keychain_has_token = false`.
    Emptied,
    /// Nothing was attempted: there is no persisted account to key an entry by, so
    /// there was nothing to delete and nothing was learned. Claims nothing and
    /// warns about nothing - this is the common path for anyone turning the switch
    /// on before they have ever signed in, and a warning that fires there is one
    /// the user learns to ignore.
    NotAttempted,
    /// The delete failed over a keychain that may still be holding the entry.
    /// Carries the reason, for the parenthetical in the warning.
    MayRemain(String),
}

/// Everything [`AppState::speleodb_disable_keychain`] settled, which is TWO
/// questions rather than one: what became of the entry the keyring already held,
/// and where the key this session is still holding landed once the keyring was
/// closed to it. Either half can strand a readable credential on the machine, and
/// only the first half was ever reported.
struct KeychainOff {
    /// What the eviction of the keyring entry established.
    evicted: KeychainEviction,
    /// A persistent copy that survived the session-only sweep, carrying the reason
    /// for the parenthetical in the warning. `None` whenever nothing landed
    /// session-only, nothing survived, or [`Self::evicted`] is already telling the
    /// user about the same credential in more specific words.
    persistent_residue: Option<String>,
}

impl AppState {
    /// Get config
    ///
    /// A POISONED config mutex is RECOVERED rather than answered with
    /// `Config::default()`: the Settings dialog renders whatever comes back and
    /// posts it straight to [`Self::update_config`], so handing out defaults would
    /// turn one unrelated panic into a wholesale reset of the user's settings.
    pub fn get_config(&self) -> Config {
        self.config.lock().clone()
    }

    /// Update config from a whole-`Config` payload.
    ///
    /// Returns a [`ConfigSaveReport`] rather than `()`: the save is a yes/no in
    /// every respect but one, and that one - evicting the keyring entry when the
    /// "never use the OS keychain" switch goes on - can fail over settings that
    /// were nevertheless written. See the eviction branch below for why that is
    /// reported instead of refused.
    pub fn update_config(&self, config: Config) -> Result<ConfigSaveReport> {
        let before = SessionSettings::of(&self.get_config());
        let report = {
            let _guard = self.config_write_lock.lock();
            self.update_config_locked(config)?
        };
        self.sync_config_side_effects_if_changed(&before);
        Ok(report)
    }

    /// [#576 D27-32(c)] Apply ONLY `patches` - the settings a save changed - onto
    /// the live config, under the config write lock, then normalize and save it
    /// (`update_config_locked`, the credential backstop included).
    ///
    /// The Settings dialog posted the whole `Config` from the snapshot it took when
    /// it opened, so every value the backend recorded in between - the panel layout
    /// and widths, the recent projects, the table views - was silently reverted by
    /// a save that never touched them. A patch carries only the user's own changes,
    /// so those survive by construction.
    ///
    /// Refused whole, with nothing written, when a path names no setting or a value
    /// has the wrong shape for its setting.
    pub fn patch_config(&self, patches: &[ConfigPatch]) -> Result<ConfigSaveReport> {
        self.modify_config(|config| apply_config_patches(config, patches))
    }

    /// Read-modify-write the config under ONE lock, for the partial-update
    /// commands.
    ///
    /// This is the whole of the fix for the lost-update race described on
    /// `AppState::config_write_lock`: `get_config()` then `update_config()` is two
    /// critical sections with a gap in the middle, and a concurrent writer that
    /// lands in the gap has its write overwritten by this caller's stale clone.
    /// Going through here closes the gap, so every partial update is applied to
    /// whatever is current at the moment it runs.
    ///
    /// `mutate` may reject the payload (`Err`), in which case NOTHING is written -
    /// the validation the partial commands do on modes and panel names happens
    /// inside the critical section, on the config that is actually about to be
    /// saved.
    pub fn modify_config<F>(&self, mutate: F) -> Result<ConfigSaveReport>
    where
        F: FnOnce(&mut Config) -> Result<()>,
    {
        let (report, before) = {
            let _guard = self.config_write_lock.lock();
            let mut config = self.get_config();
            let before = SessionSettings::of(&config);
            mutate(&mut config)?;
            (self.update_config_locked(config)?, before)
        };
        self.sync_config_side_effects_if_changed(&before);
        Ok(report)
    }

    /// [#576 A18(c)/(g)/(h)] Bring the running session in line with the config in
    /// force: the undo history limit (and the depth bound it feeds) and the live
    /// tile cache. The ONE place a config change reaches them, for a Settings save,
    /// a partial save and Reload Config alike - each used to apply its own subset,
    /// so the undo limit and the cache size and retention changed only at the next
    /// launch, and a reload never touched the cache at all.
    ///
    /// Reads the LIVE config, so it is idempotent and order-free: two saves racing
    /// each other both leave the session matching whichever config won.
    ///
    /// Takes `undo_stack`, then `database` and `undo_stack` (`sync_undo_depth_limit`),
    /// then `tile_cache` - call it with none of those, and not `config_write_lock`,
    /// held.
    pub(super) fn sync_config_side_effects(&self) {
        let config = self.get_config();
        {
            let mut undo = self.undo_stack.lock();
            if undo.history_limit() != config.general.undo_history_limit {
                // [#303] `set_history_limit` re-arms only the COUNT bound; a Full-mode
                // SpeleoDB project stays on the byte budget, so a settings save cannot
                // silently truncate the history its next push is meant to carry.
                undo.set_history_limit(config.general.undo_history_limit);
            }
        }
        // ...and the BOUND itself, which `set_history_limit` deliberately will not
        // touch: the global `speleodb.undo_mode` can change in the same save.
        self.sync_undo_depth_limit();
        self.sync_tile_cache(&config);
    }

    /// [`Self::sync_config_side_effects`], but only when a setting it applies
    /// differs from `before` - the partial writes (panel widths, table views) run
    /// often and change none of them.
    fn sync_config_side_effects_if_changed(&self, before: &SessionSettings) {
        if SessionSettings::of(&self.get_config()) != *before {
            self.sync_config_side_effects();
        }
    }

    /// The live tile cache, made to match `config.cache` (#576 A18(g)/(h)):
    /// dropped when caching is off; opened when it is on and there is none - which
    /// also retries a cache whose open failed at startup; and otherwise
    /// reconfigured in place, so a smaller size cap or a shorter retention is in
    /// force at once (`TileCache::reconfigure` evicts to it). The provider's own
    /// `Cache-Control` still bounds every tile, as before.
    fn sync_tile_cache(&self, config: &Config) {
        let mut guard = self.tile_cache.lock();
        if !config.cache.tile_cache_enabled {
            if guard.take().is_some() {
                // Existing cache data remains on disk.
                tracing::info!(purpose = "disable offline tiles", "Tile cache disabled");
            }
            return;
        }
        match guard.as_mut() {
            Some(cache) => match cache.reconfigure(
                config.cache.tile_cache_max_mb,
                config.cache.tile_ttl_days,
                &config.cache.provider_ttl_days(),
            ) {
                Ok(true) => tracing::info!(
                    max_mb = config.cache.tile_cache_max_mb,
                    ttl_days = config.cache.tile_ttl_days,
                    purpose = "apply the tile cache settings",
                    "Tile cache settings applied"
                ),
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    error = %e,
                    purpose = "apply the tile cache settings",
                    "Tile cache settings applied, but trimming the cache to them failed"
                ),
            },
            None => match Self::init_tile_cache(config) {
                Ok(cache) => {
                    tracing::info!(purpose = "enable offline tiles", "Tile cache enabled");
                    *guard = Some(cache);
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    purpose = "enable offline tiles",
                    "Failed to initialize tile cache"
                ),
            },
        }
    }

    /// The body of [`Self::update_config`], with `config_write_lock` already held.
    /// Never call this directly - the lock is what makes a partial update safe.
    fn update_config_locked(&self, mut config: Config) -> Result<ConfigSaveReport> {
        tracing::info!(purpose = "apply user settings", "Updating configuration");
        // Preserve backend-only fields the frontend never receives. `webkit_memory`
        // is `#[serde(skip_serializing)]` + `#[ts(skip)]`, so a settings-save payload
        // always carries the serde default. This restore is load-bearing: the save
        // below re-appends the in-memory `[webkit_memory]` table to `config.toml`
        // (see `Config::save_to_path`), so without putting the real value back here
        // first, saving any setting would persist defaults and silently reset a
        // user's hand-tuned knobs.
        //
        // The credential fields below are preserved for a different reason: they ARE
        // serialized, but the Settings form does not own them. Who is signed in and
        // where their API key is kept are settled by the login itself, the
        // login-time storage prompt and the "Try again" button, all of which write
        // `config.toml` through the backend while the Settings dialog stays mounted
        // over its snapshot of the config (`SpeleoStorageChoiceDialog` is a
        // top-layer overlay precisely so Settings is not unmounted, and the account
        // panel's "Log in now" runs without closing anything). Saving any unrelated
        // setting afterwards posted the stale values back and silently undid what
        // the backend had just recorded - re-arming the launch-time keyring nag, or
        // worse: a successful "Try again" moves the key into the keychain and
        // deletes the plaintext copy, so restoring a stale `keychain_declined = true`
        // made the next launch skip the keychain read and find nothing, signing the
        // user out with no way to explain it.
        //
        // `email` belongs in that list for exactly the same reason and costs the
        // same when it is missing: it is the rehydrate seed - `speleodb_ensure_
        // rehydrated` keys the credential lookup by it - so posting back the stale
        // (usually null) snapshot from before an inline login left the next launch
        // unable to find the token of the account that is signed in, forcing another
        // sign-in.
        //
        // A POISONED config mutex is RECOVERED (`into_inner`) rather than skipped,
        // for that last reason: skipping restores nothing, so the form's serde
        // defaults land on top of the backend-owned fields and sign the user out
        // with no way to explain it. That is the unsafe direction, and it is the
        // same call the cache replace makes with the project lock
        // (`project_io::close_project_if_open_at_held`) - these are plain values with no
        // half-written invariant to protect.
        let previously_disabled = {
            let existing = self.config.lock();
            config.webkit_memory = existing.webkit_memory.clone();
            config.speleodb.email.clone_from(&existing.speleodb.email);
            config.speleodb.token_fallback = existing.speleodb.token_fallback;
            config.speleodb.keychain_declined = existing.speleodb.keychain_declined;
            // Backend-owned for the same reason: it records what the keychain
            // actually holds, which only the credential paths know. `keychain_disabled`
            // is deliberately NOT in this list - that one IS a Settings toggle.
            //
            // Both halves of the belief, always together. Restoring the flag while
            // letting the form's (absent) stamp land would turn a stamped belief back
            // into an unstamped one on every settings save - which is read as "scoped
            // by the email alone", the exact same-address-different-server hole the
            // stamp closes, re-opened by a save that changed nothing.
            config.speleodb.keychain_has_token = existing.speleodb.keychain_has_token;
            config
                .speleodb
                .keychain_token_account
                .clone_from(&existing.speleodb.keychain_token_account);
            existing.speleodb.keychain_disabled
        };
        // Repair the payload the same way the load path repairs a file. Without
        // this, the write path was the ONE door into `config.toml` with no
        // normalization behind it: a whole-`Config` payload could persist a NaN
        // sigma, an out-of-range column width or a blank locale that
        // `Config::normalize` would have fixed, and the value then survived until
        // the next launch read it back and quietly repaired it - so the running
        // session and its own config file disagreed. The partial commands already
        // clamp their own fields; this generalises it to every field.
        //
        // AFTER the restore above, not before: normalization touches `speleodb`,
        // and it has to see the backend-owned values that were just put back
        // rather than the form's serde defaults.
        //
        // `_except_shortcuts`, NOT the full `normalize`: an empty binding list
        // means "missing" in a file and "the user cleared it" in a payload, and
        // only the load path may read it the first way. See the method's own doc
        // comment - getting this wrong silently restores a default binding over a
        // shortcut the user just cleared.
        config.normalize_except_shortcuts();
        let mut report = ConfigSaveReport::default();
        // The keyring switch has to bite NOW, not at the next launch: a user turning
        // it on has just been asked one unlock prompt too many, and the live session
        // would otherwise keep reaching for the keychain until they restart.
        let keychain_disabled = config.speleodb.keychain_disabled;
        if keychain_disabled && !previously_disabled {
            // Turning it ON is not a gate flip. Looking away from the keychain does
            // not empty it, and NOTHING else ever will: the disable is re-applied on
            // every subsequent login, so `TokenStore::store` never reaches the keyring
            // to overwrite the entry, and Settings' "Try again" clears only
            // `keychain_declined`. What a previous successful login left there would
            // stay as a working bearer token belonging to a user who has just said
            // "do not use the OS keychain" - and logout swept nothing and reported
            // success over it.
            //
            // Deliberately BEFORE the save below, so a save that fails leaves the
            // keyring emptied while the file still says it is enabled. That is the
            // recoverable direction (the next launch opens the keyring, finds
            // nothing, and asks for a sign-in); saving first would leave the file
            // saying "off" over an entry that survived.
            let KeychainOff {
                evicted,
                persistent_residue,
            } = self.speleodb_disable_keychain(&config.speleodb);
            match evicted {
                // Both halves, in the save this function is already making: a belief
                // and the account it names are only ever written together.
                KeychainEviction::Emptied => {
                    config.speleodb.keychain_has_token = false;
                    config.speleodb.keychain_token_account = None;
                }
                // Nothing was checked, so nothing is claimed and nothing is said.
                KeychainEviction::NotAttempted => {}
                // REPORTED, not refused, and the settings still save. The
                // evict-before-save ordering above exists precisely so a partial
                // failure lands in the recoverable direction, and refusing here
                // would land in the other one twice over: the eviction has already
                // run, so the user would be told "nothing was saved" over a keyring
                // we may well have just emptied, and the rest of their settings
                // would be abandoned along with it. Worse, the switch's own help
                // text points it at "your keyring is locked or unavailable" - the
                // very box whose delete fails - so refusing would leave exactly the
                // users it exists for unable to turn the keyring off at all.
                //
                // What must NOT happen is recording a removal that did not happen:
                // `keychain_has_token` says what the keyring actually holds, and
                // clearing it here would send the next launch past an entry that is
                // still in there. So it is left alone, and the user is told.
                KeychainEviction::MayRemain(reason) => {
                    tracing::warn!(
                        reason = %reason,
                        purpose = "turn the OS keyring off",
                        "could not remove the SpeleoDB token from the OS keyring; it may still be \
                         stored on this machine"
                    );
                    report.keychain_residue = Some(reason);
                }
            }
            // The OTHER half of the same move, and it goes wrong the same way. The
            // key was re-landed on the session-only tier, which is a promise about
            // disk - nothing persists - and a copy survived the sweep that was
            // supposed to make it true. The settings then save `token_fallback =
            // session_only` over a `0o600` file that `TokenStore::load` reads back
            // with no keyring, no unlock and no flag to get past, so the next launch
            // signs the user in from a credential the saved configuration says is
            // not there. Reported, not refused, for the reasons the eviction arm
            // above gives: the move has already happened, and abandoning the rest of
            // the user's settings over it helps nobody.
            report.persistent_residue = persistent_residue;
        } else {
            let mut session = self.speleodb.lock();
            session.token_store.set_keychain_disabled(keychain_disabled);
        }
        if let Err(e) = config.save() {
            tracing::error!(
                reason = %e,
                purpose = "persist user settings",
                "Failed to save configuration"
            );
            return Err(e);
        }

        // The session-side effects (undo limit, tile cache) are applied by the
        // callers once `config_write_lock` is released - see
        // `sync_config_side_effects` (#576 A18).

        // Recovered rather than skipped for the third time in this function: the
        // settings are already on disk by now, so dropping the in-memory update
        // would leave the running app disagreeing with its own config file until
        // the next launch.
        *self.config.lock() = config;
        Ok(report)
    }

    /// Apply a NEW standing "never use the OS keyring" instruction to the live
    /// session: take the key OUT of the keychain, THEN close the gate, then re-land
    /// the key wherever the closed gate allows.
    ///
    /// The order is the whole point. `TokenStore::evict_from_keychain` is the one
    /// call that bypasses the off-gate, and it has to run before the flag is set or
    /// the gate blocks it; the trailing `restore` is what stops `active_tier` (and
    /// so the Settings credential line) claiming a keychain we just emptied.
    ///
    /// The eviction is NOT gated on `keychain_has_token`. That flag is what startup
    /// reads to avoid opening the keyring, but it defaults to `false` for every
    /// config written before it existed - so trusting it here would silently skip
    /// the eviction for exactly the upgraded users most likely to have an entry.
    /// The cost of being wrong the other way is one delete against a keyring that
    /// holds nothing, which is `Ok` (a missing entry maps to success).
    ///
    /// Reports which of [`KeychainEviction`]'s three outcomes the delete reached,
    /// AND whether the re-landing that follows it left a persistent copy behind.
    /// A refused delete does NOT answer `Emptied`: the entry may well still be
    /// there, so the caller leaves `keychain_has_token` set (a later re-enable
    /// should still know the keyring is worth looking in) and tells the user.
    ///
    /// The two non-`Emptied` answers are kept apart because only one of them is
    /// worth interrupting anyone over. `NotAttempted` is the ordinary case - no
    /// account persisted, nothing to key an entry by - and reporting it would fire
    /// the warning for every user who turns this switch on before signing in.
    /// `MayRemain` is the one that matters: a working bearer token left in the
    /// keyring of someone who just asked for the keyring not to be used.
    /// `TokenStore::evict_from_keychain` owns the line between them, with the same
    /// three-way probe `TokenStore::clear` and the session-only sweep use - a
    /// missing entry is already `Ok`, and a keychain this session has learned is
    /// unusable AND has no record of holding anything of ours cannot have stranded
    /// anything.
    ///
    /// The persisted belief is handed down for that last clause, not to gate the
    /// eviction (see above - it is not trusted in the "no belief" direction). A
    /// keyring that is merely LOCKED fails every op exactly like a box with no secret
    /// service, so both record `Unavailable`; a previous login's successful store is
    /// the only thing that tells them apart, and without it the locked keyring - the
    /// likeliest reason anyone reaches for this switch - was reported emptied over a
    /// live bearer token. The live store learns it at rehydrate, but a session
    /// established by a login this run never rehydrates, so the value is re-asserted
    /// here where the config already has it.
    ///
    /// The whole `[speleodb]` block rather than the flag, because the flag alone does
    /// not say WHOSE entry it is about: it is re-asserted only once the session's
    /// account is known, and only if the belief is stamped with that account (see
    /// `SpeleoDbConfig::keychain_belief_applies_to`). Re-asserting it unscoped told
    /// this session that a second server's - or a previous user's - entry was one of
    /// ours, which is a residue warning nothing keyed by the signed-in account could
    /// ever act on.
    fn speleodb_disable_keychain(&self, speleodb: &crate::config::SpeleoDbConfig) -> KeychainOff {
        // Seeds `endpoint` / `email` from config when the session is still cold -
        // Settings is reachable without having touched SpeleoDB this run, and
        // without an account there is no key to evict by. Called with NO lock held:
        // `speleodb_ensure_rehydrated` documents the config-then-session order, and
        // inverting it here would be the deadlock.
        self.speleodb_ensure_rehydrated();

        let mut session = self.speleodb.lock();
        let (Some(endpoint), Some(email)) = (session.endpoint.clone(), session.email.clone())
        else {
            session.token_store.set_keychain_disabled(true);
            tracing::debug!(
                purpose = "no persisted account to key a keychain entry by",
                "turned the OS keyring off without an eviction"
            );
            // We verified nothing, so we claim nothing: leave the recorded belief
            // alone rather than assert an emptiness we did not check. And say
            // nothing to the user - there was no credential here to strand, and no
            // account key to sweep one by either.
            return KeychainOff {
                evicted: KeychainEviction::NotAttempted,
                persistent_residue: None,
            };
        };

        // Evidence only, and only ever raised: a belief that is absent - or stamped
        // with somebody else - means "the config has no record about THIS account",
        // never "the keyring is empty", so it leaves whatever this session has learned
        // first-hand alone. Asked here rather than before the account check above,
        // because the account is what makes the question answerable.
        session
            .token_store
            .note_keychain_has_token(speleodb.keychain_belief_applies_to(&endpoint, &email));
        let evicted = match session.token_store.evict_from_keychain(&endpoint, &email) {
            Ok(()) => KeychainEviction::Emptied,
            Err(e) => KeychainEviction::MayRemain(e.to_string()),
        };
        session.token_store.set_keychain_disabled(true);
        // The key is still in hand, so honour the instruction now: re-store it at
        // whatever tier the closed gate allows (the `token_fallback` preference
        // still decides between a `0o600` file and session-only memory).
        //
        // A survivor of the session-only sweep is REPORTED here rather than failing
        // the settings save, unlike `speleodb_move_token` which refuses on it. The
        // two verbs make different promises: answering the storage prompt with "this
        // session only" IS the promise that nothing persists, so a surviving copy
        // falsifies the very thing being reported, whereas turning the keyring off
        // promises only that the keyring stops being used - where the key goes
        // instead is `token_fallback`, which this save did not change. Failing here
        // would also abandon the rest of the user's settings over a residue, having
        // already emptied the keyring.
        //
        // It is not swallowed either, which is what it was. `Err` from `restore` is
        // narrow by construction: the key DID land session-only, and a persistent
        // tier's delete failed over a store that has NOT been proven empty
        // (`sweep_tier` returns `Ok` for a delete the tier accepted, for a probe that
        // answered `Ok(None)`, and for the keychain-less box that cannot have held
        // anything). So it means one thing - a readable copy may still be on this
        // machine - and logging it left the save reporting unqualified success over
        // exactly that.
        let residue = match session.token_store.restore(
            &endpoint,
            &email,
            speleodb.token_fallback == TokenFallback::Plaintext,
        ) {
            Ok(landed) => {
                tracing::info!(
                    ?landed,
                    emptied = matches!(evicted, KeychainEviction::Emptied),
                    "OS keyring turned off; the SpeleoDB key was moved out of it"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    emptied = matches!(evicted, KeychainEviction::Emptied),
                    "OS keyring turned off, but an older copy of the SpeleoDB key could not be \
                     removed from this machine"
                );
                Some(e.to_string())
            }
        };
        // Said ONCE per credential, not twice. A keyring that refused the eviction
        // refuses the sweep's second delete against that same entry, and
        // `TokenStore::sweep_persistent_tiers` returns the keychain's error ahead of
        // the plaintext one - so on the `MayRemain` path this string IS the failure
        // the eviction has already reported, in words that additionally name the
        // keyring and tell the user where to look. Nothing is lost by dropping it:
        // even when a plaintext file stranded too, the sweep has already discarded
        // that error in favour of the keychain's, so there was never a second reason
        // to show. A second, vaguer warning about the same key is the false alarm
        // this file's three-way probes exist to avoid.
        let persistent_residue = if matches!(evicted, KeychainEviction::MayRemain(_)) {
            None
        } else {
            residue
        };
        KeychainOff {
            evicted,
            persistent_residue,
        }
    }

    /// Reload config from disk without saving
    /// Returns the newly loaded config
    /// Also syncs config-derived state (recent_projects, and through
    /// `sync_config_side_effects` the undo limit and the tile cache, #576 A18(h))
    pub fn reload_config(&self) -> Result<Config> {
        let new_config = Config::load()?;

        // Under the config write lock, like every other write of the live config, so
        // a partial save racing the reload cannot land between the two.
        {
            let _guard = self.config_write_lock.lock();
            {
                let mut cfg = self.config.lock();
                *cfg = new_config.clone();
            }
        }

        // Sync recent_projects from config
        self.recent_projects
            .lock()
            .clone_from(&new_config.general.recent_projects);

        // Always, not only on a change: a reload is also how a cache whose open
        // failed at startup gets another try. No lock is held across it.
        self.sync_config_side_effects();

        Ok(new_config)
    }
}

/// The settings [`AppState::sync_config_side_effects`] applies to the running
/// session, to tell whether a save changed any of them.
#[derive(PartialEq)]
struct SessionSettings {
    undo_history_limit: usize,
    undo_mode: crate::models::UndoMode,
    cache: crate::config::CacheConfig,
}

impl SessionSettings {
    fn of(config: &Config) -> Self {
        Self {
            undo_history_limit: config.general.undo_history_limit,
            undo_mode: config.speleodb.undo_mode,
            cache: config.cache.clone(),
        }
    }
}

/// Apply `patches` to `config` (#576 D27-32(c)): each `path` names a setting by its
/// field names from the top, and its `value` replaces that setting. Refused whole
/// - `config` untouched - when a path names no setting or a value does not fit it.
fn apply_config_patches(config: &mut Config, patches: &[ConfigPatch]) -> Result<()> {
    let mut tree = serde_json::to_value(&*config)?;
    for patch in patches {
        let Some((key, parents)) = patch.path.split_last() else {
            bail!("A settings change names no setting");
        };
        // A missing (or empty optional) table on the way is created, so a new map
        // entry can arrive as its settings one by one; a name the config does not
        // have is caught by the check below, after the round trip drops it.
        let mut node = &mut tree;
        for segment in parents {
            let object = node
                .as_object_mut()
                .ok_or_else(|| anyhow!("Unknown setting '{}'", patch.path.join(".")))?;
            node = object
                .entry(segment.clone())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if node.is_null() {
                *node = serde_json::Value::Object(serde_json::Map::new());
            }
        }
        let object = node
            .as_object_mut()
            .ok_or_else(|| anyhow!("Unknown setting '{}'", patch.path.join(".")))?;
        object.insert(key.clone(), patch.value.clone());
    }
    let patched: Config = serde_json::from_value(tree)
        .map_err(|e| anyhow!("A changed setting has a value that does not fit it: {e}"))?;
    // A name the config does not have is dropped by the read above rather than
    // refused, so check that every patched setting is still there. (A `null` that
    // clears an optional setting may be dropped too - that is the clearing.)
    let check = serde_json::to_value(&patched)?;
    for patch in patches {
        let found = patch
            .path
            .iter()
            .try_fold(&check, |node, segment| node.get(segment));
        if found.is_none() && !patch.value.is_null() {
            bail!("Unknown setting '{}'", patch.path.join("."));
        }
    }
    *config = patched;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::AppState;
    use crate::config::TokenFallback;
    use crate::speleodb::auth::{
        account_key, InMemoryStore, KeychainState, PlaintextStore, SecretStore, SecretStoreError,
        TokenStore, TokenTier,
    };
    use std::path::Path;

    const EP: &str = "https://sdb.example";
    const EMAIL: &str = "u@x";

    /// A keychain that cannot be written to - the headless / locked-keyring box.
    /// Local to this module for the same reason its siblings are: the real fakes in
    /// `auth.rs` are private to that file's test module.
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

    /// A keychain that hands the entry back but will not delete it - the locked or
    /// read-only keyring whose credential provably SURVIVES the eviction, as
    /// opposed to [`FailingSecretStore`], which cannot even be asked.
    struct DeleteRefusingStore(InMemoryStore);

    impl SecretStore for DeleteRefusingStore {
        fn kind(&self) -> &'static str {
            "refusing"
        }
        fn store(&self, key: &str, token: &str) -> Result<(), SecretStoreError> {
            self.0.store(key, token)
        }
        fn load(&self, key: &str) -> Result<Option<String>, SecretStoreError> {
            self.0.load(key)
        }
        fn delete(&self, _: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError::for_test("the keyring is locked"))
        }
    }

    /// A plaintext tier that hands its file back but will not delete it - the
    /// read-only token directory, or the file another process is holding open.
    /// [`DeleteRefusingStore`]'s sibling, kept separate rather than generalised
    /// because the reason the user is shown has to name the right part of their
    /// machine, and because this one delegates to a REAL [`PlaintextStore`] so a
    /// second handle on the same tempdir can see what survived.
    struct DeleteRefusingPlaintext(PlaintextStore);

    impl SecretStore for DeleteRefusingPlaintext {
        fn kind(&self) -> &'static str {
            "plaintext"
        }
        fn store(&self, key: &str, token: &str) -> Result<(), SecretStoreError> {
            self.0.store(key, token)
        }
        fn load(&self, key: &str) -> Result<Option<String>, SecretStoreError> {
            self.0.load(key)
        }
        fn delete(&self, _: &str) -> Result<(), SecretStoreError> {
            Err(SecretStoreError::for_test("the token file is read-only"))
        }
    }

    /// A state whose session is signed in over the injected keychain backend.
    /// `endpoint` is seeded, which short-circuits `speleodb_ensure_rehydrated`, so
    /// nothing in these tests reaches a real keyring.
    fn signed_in_over(keychain: Box<dyn SecretStore>) -> AppState {
        signed_in_over_tiers(keychain, None)
    }

    /// [`signed_in_over`] with the plaintext tier wired too - production's default
    /// shape, and the one the session-only sweep has two stores to answer for.
    fn signed_in_over_tiers(
        keychain: Box<dyn SecretStore>,
        plaintext: Option<Box<dyn SecretStore>>,
    ) -> AppState {
        let state = AppState::new().expect("state");
        let mut session = state.speleodb.lock();
        session.endpoint = Some(EP.to_owned());
        session.email = Some(EMAIL.to_owned());
        session.token_store = TokenStore::from_stores(keychain, plaintext);
        drop(session);
        state
    }

    /// A `PlaintextStore` standing in for the OS keychain, so the entry it holds is
    /// observable from OUTSIDE the `TokenStore` that owns the backend.
    fn observable_keychain(root: &Path) -> Box<dyn SecretStore> {
        Box::new(PlaintextStore::new(root.to_path_buf()))
    }

    /// The Settings payload: what `get_config` hands the form, with the keyring
    /// switch set as the user left it.
    fn settings_save(state: &AppState, keychain_disabled: bool) -> crate::config::Config {
        let mut config = state.get_config();
        config.speleodb.keychain_disabled = keychain_disabled;
        config
    }

    #[test]
    fn disabling_the_keychain_removes_the_entry_it_already_holds() {
        // Flipping the switch used to only make the live session look away. Nothing
        // ever removed what an earlier successful login had already written: the
        // disable is re-applied on every subsequent login so `store` never overwrites
        // the entry, and "Try again" clears the OTHER flag. The result was a working
        // bearer token left in the OS keychain of a user who had just turned the OS
        // keychain off - and logout then swept nothing and reported success.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = signed_in_over(observable_keychain(dir.path()));
            {
                let mut session = state.speleodb.lock();
                // An earlier working login: the key is durably in the keychain.
                assert_eq!(
                    session.token_store.store(EP, EMAIL, "tok", false),
                    TokenTier::Keychain
                );
                drop(session);
            }
            // What that login persists, through the writer it uses: the account and
            // the belief stamped with it, in one record.
            state
                .speleodb_write_account_record(Some(EMAIL.to_owned()), Some(account_key(EP, EMAIL)))
                .expect("what the earlier login persisted");
            let probe = PlaintextStore::new(dir.path().to_path_buf());
            assert!(probe.load(&account_key(EP, EMAIL)).unwrap().is_some());

            let report = state
                .update_config(settings_save(&state, true))
                .expect("settings saved");

            assert_eq!(
                report.keychain_residue, None,
                "an eviction that worked must not warn about a residue there isn't"
            );
            assert_eq!(
                probe.load(&account_key(EP, EMAIL)).unwrap(),
                None,
                "turning the keyring off must take the key OUT of it, not just look away"
            );
            let session = state.speleodb.lock();
            assert!(
                !session.token_store.keychain_backed(),
                "and the credential line must stop claiming a keychain we just emptied"
            );
            drop(session);
            assert!(
                !state.get_config().speleodb.keychain_has_token,
                "the recorded belief has to follow, or the next launch opens the keyring for nothing"
            );
        });
    }

    #[test]
    fn turning_the_keyring_off_reports_a_key_the_keyring_would_not_give_up() {
        // The eviction is the only thing that makes the switch honest, and a locked
        // or read-only keyring can refuse it. The save reported unqualified success
        // over that refusal: Settings then told the user their API key would not be
        // stored in the OS keychain while the previous login's live bearer token was
        // still sitting in it, with the failure visible only in the backend log.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(DeleteRefusingStore(InMemoryStore::new())));
            {
                let mut session = state.speleodb.lock();
                assert_eq!(
                    session.token_store.store(EP, EMAIL, "tok", false),
                    TokenTier::Keychain
                );
                drop(session);
            }
            // What that login persists, through the writer it uses: the account and
            // the belief stamped with it, in one record.
            state
                .speleodb_write_account_record(Some(EMAIL.to_owned()), Some(account_key(EP, EMAIL)))
                .expect("what the earlier login persisted");

            let report = state
                .update_config(settings_save(&state, true))
                .expect("the settings still save - the residue is reported, not refused");

            let residue = report
                .keychain_residue
                .expect("a credential the keyring refused to give up must reach the user");
            assert!(
                residue.contains("locked"),
                "the reason belongs in the warning so it is actionable: {residue}"
            );
            let saved = state.get_config();
            assert!(
                saved.speleodb.keychain_disabled,
                "refusing the toggle would strand the locked-keyring box the switch exists for"
            );
            assert!(
                saved.speleodb.keychain_has_token,
                "the recorded belief must not claim a removal that did not happen"
            );
        });
    }

    #[test]
    fn turning_the_keyring_off_reports_a_plaintext_copy_the_sweep_could_not_remove() {
        // The OTHER half of the same move, and it reached the log and nowhere else.
        // Turning the keyring off re-lands the key at whatever tier the closed gate
        // allows; under the default `session_only` preference that is memory, and
        // the sweep that MAKES that landing true has to remove the `0o600` file an
        // earlier storage choice left behind. When it cannot, the settings still
        // saved `token_fallback = session_only` - so the configuration claims the
        // key is gone when the app closes while `TokenStore::load` reads that file
        // straight back on the next launch (no keyring, no unlock and no flag in the
        // way) and signs the user in from a credential the save said was not there.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = signed_in_over_tiers(
                Box::new(InMemoryStore::new()),
                Some(Box::new(DeleteRefusingPlaintext(PlaintextStore::new(
                    dir.path().to_path_buf(),
                )))),
            );
            // The older copy: a previous `token_fallback = plaintext` choice wrote
            // it, and nothing has removed it since (the self-heal only runs on a
            // keychain store that succeeds, and it fails against this tier too).
            let probe = PlaintextStore::new(dir.path().to_path_buf());
            probe.store(&account_key(EP, EMAIL), "tok").unwrap();
            {
                let mut session = state.speleodb.lock();
                // A working login this run, so the key is in hand to be re-landed.
                assert_eq!(
                    session.token_store.store(EP, EMAIL, "tok", false),
                    TokenTier::Keychain
                );
            }

            let report = state
                .update_config(settings_save(&state, true))
                .expect("the settings still save - the residue is reported, not refused");

            assert_eq!(
                report.keychain_residue, None,
                "the keyring gave its entry up, so there is nothing to say about the keyring"
            );
            let residue = report.persistent_residue.expect(
                "a copy the sweep could not remove is what signs the user back in next launch",
            );
            assert!(
                residue.contains("read-only"),
                "the reason belongs in the warning so it is actionable: {residue}"
            );
            assert_eq!(
                probe.load(&account_key(EP, EMAIL)).unwrap(),
                Some("tok".to_owned()),
                "the premise of the warning: the copy really did survive the sweep"
            );
            assert_eq!(
                state.get_config().speleodb.token_fallback,
                TokenFallback::SessionOnly,
                "and the saved configuration is the thing the survivor contradicts"
            );
        });
    }

    #[test]
    fn a_plaintext_copy_the_sweep_removed_raises_no_alarm() {
        // DO NOT REGRESS. The ordinary path for anyone who ever answered the storage
        // prompt with "write it to a file": the sweep deletes that file and PROVES it
        // gone, so there is nothing left to sign them back in and nothing to say.
        // Warning on every sweep that had a file to remove would fire here, which is
        // most of the times this switch is thrown on a box that has used the escape
        // hatch - and a warning that fires on the ordinary path is one the user
        // learns to ignore.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = signed_in_over_tiers(
                Box::new(InMemoryStore::new()),
                Some(Box::new(PlaintextStore::new(dir.path().to_path_buf()))),
            );
            let probe = PlaintextStore::new(dir.path().to_path_buf());
            probe.store(&account_key(EP, EMAIL), "tok").unwrap();
            {
                let mut session = state.speleodb.lock();
                assert_eq!(
                    session.token_store.store(EP, EMAIL, "tok", false),
                    TokenTier::Keychain
                );
            }

            let report = state
                .update_config(settings_save(&state, true))
                .expect("settings saved");

            assert_eq!(
                report.persistent_residue, None,
                "a copy that is provably gone has stranded nothing to warn about"
            );
            assert_eq!(report.keychain_residue, None);
            assert_eq!(
                probe.load(&account_key(EP, EMAIL)).unwrap(),
                None,
                "the premise of the silence: the sweep really did remove it"
            );
        });
    }

    #[test]
    fn a_cold_session_turns_the_keyring_off_without_a_false_alarm() {
        // DO NOT REGRESS. The launch after a "this session only" answer: the account
        // is still in the config (it is the rehydrate seed) but the token is not - it
        // was never persisted, which is exactly what the user asked for. There is
        // nothing to re-land, so nothing is swept and nothing can have survived.
        // `restore` says so with `Ok(None)`, and reading that as unsettled would warn
        // every signed-out user who turns the switch on about a credential this
        // machine does not have.
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = signed_in_over_tiers(
                Box::new(InMemoryStore::new()),
                Some(Box::new(DeleteRefusingPlaintext(PlaintextStore::new(
                    dir.path().to_path_buf(),
                )))),
            );

            let report = state
                .update_config(settings_save(&state, true))
                .expect("settings saved");

            assert_eq!(
                report.persistent_residue, None,
                "no key was moved, so no copy of one can have been left behind"
            );
            assert_eq!(report.keychain_residue, None);
        });
    }

    #[test]
    fn a_keyring_that_refused_the_eviction_is_not_reported_twice() {
        // DO NOT REGRESS. The sweep re-attempts the delete the eviction just lost
        // against the SAME entry, and loses it the same way - and
        // `sweep_persistent_tiers` returns the keychain's error ahead of the
        // plaintext one, so the second report would be the first one again with the
        // keyring's name taken out of it. One credential, one warning: the more
        // specific one, which tells the user where to go look.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(DeleteRefusingStore(InMemoryStore::new())));
            {
                let mut session = state.speleodb.lock();
                assert_eq!(
                    session.token_store.store(EP, EMAIL, "tok", false),
                    TokenTier::Keychain
                );
                drop(session);
            }
            // What that login persists, through the writer it uses: the account and
            // the belief stamped with it, in one record.
            state
                .speleodb_write_account_record(Some(EMAIL.to_owned()), Some(account_key(EP, EMAIL)))
                .expect("what the earlier login persisted");

            let report = state
                .update_config(settings_save(&state, true))
                .expect("the settings still save - the residue is reported, not refused");

            assert!(
                report.keychain_residue.is_some(),
                "the keyring refused, and that is the report worth making"
            );
            assert_eq!(
                report.persistent_residue, None,
                "saying it twice, the second time more vaguely, is the false alarm"
            );
        });
    }

    #[test]
    fn an_unreadable_keyring_is_reported_rather_than_assumed_empty() {
        // The keyring that worked at some earlier login and has since stopped
        // answering: the delete fails and the probe fails with it. "Cannot be read"
        // is a fact about right now; the entry is still there on the next launch.
        // Only `Ok(None)` proves absence - the same line `TokenStore::clear` and the
        // session-only sweep draw.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(FailingSecretStore));
            // Through the writer that earlier login would have used, rather than
            // hand-setting the flag: the belief and the account it is ABOUT are one
            // record, and a belief with nobody's name on it is not a state a login can
            // leave behind (it is what a config written before the stamp existed looks
            // like, which is a different case with its own test).
            state
                .speleodb_write_account_record(Some(EMAIL.to_owned()), Some(account_key(EP, EMAIL)))
                .expect("what the earlier login persisted");

            let report = state
                .update_config(settings_save(&state, true))
                .expect("settings saved");

            assert!(
                report.keychain_residue.is_some(),
                "a keyring that will not answer has not proved it is empty"
            );
            assert!(
                state.get_config().speleodb.keychain_has_token,
                "an indeterminate eviction must leave the recorded belief alone"
            );
        });
    }

    #[test]
    fn a_keychainless_box_turns_the_keyring_off_without_a_false_alarm() {
        // DO NOT REGRESS. The configuration this switch EXISTS for - its help text
        // says so: headless Linux with no secret service, where every keychain op
        // dies at `Entry::new` before it can be asked about the entry, so the delete
        // fails and nothing can have been stranded. Warning on every raw delete
        // failure would fire here, on the most common path there is for this toggle,
        // and a warning that always fires is one the user learns to ignore.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(FailingSecretStore));
            {
                let mut session = state.speleodb.lock();
                // A login this session that the keychain refused: THAT is what
                // records `Unavailable`, and it is half the evidence separating a
                // keychain-less box from a keyring that has merely gone quiet.
                let tier = session.token_store.store(EP, EMAIL, "tok", false);
                assert_eq!(tier, TokenTier::Memory);
                assert_eq!(
                    session.token_store.keychain_state(),
                    KeychainState::Unavailable
                );
                // The other half, written by the SAME call the login makes and with
                // the same argument, rather than hand-set: a keychain-less box cannot
                // reach `keychain_has_token = true`, because the store only raises it
                // where a keychain read or write actually succeeded, and none can
                // here. Hand-setting it would have been a state production cannot
                // produce, and would have made this test demand silence over the one
                // piece of evidence that a credential really is stranded.
                let believed = session.token_store.keychain_has_token();
                drop(session);
                state.speleodb_persist_keychain_belief(believed.then(|| account_key(EP, EMAIL)));
                assert!(!state.get_config().speleodb.keychain_has_token);
            }

            let report = state
                .update_config(settings_save(&state, true))
                .expect("settings saved");

            assert_eq!(
                report.keychain_residue, None,
                "a keychain that cannot hold anything has stranded nothing to warn about"
            );
            assert!(
                !state.get_config().speleodb.keychain_has_token,
                "and the next launch has no reason to open a keyring that holds nothing"
            );
        });
    }

    #[test]
    fn a_locked_keyring_that_a_previous_login_filled_is_not_reported_as_emptied() {
        // The other side of the line the test above draws, and the case with the
        // strongest evidence of a stranded credential there is. A previous launch's
        // login DID put a token in the keyring and recorded it; this launch the
        // keyring is locked, so the startup rehydrate read fails and records
        // `Unavailable` - the identical verdict a box with no secret service records.
        // Reading that verdict alone as "nothing can be in there" made the eviction
        // report success over a live bearer token and clear the one flag that would
        // have sent a later launch looking for it.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(FailingSecretStore));
            {
                // What the previous launch's successful keychain store persisted,
                // stamped with the account it is about.
                state.speleodb_persist_keychain_belief(Some(account_key(EP, EMAIL)));
                let mut session = state.speleodb.lock();
                // Which `SpeleoDbSession::rehydrate` mirrors into the live store
                // before wiring the load that the locked keyring then refuses.
                session.token_store.note_keychain_has_token(true);
                assert_eq!(session.token_store.load(EP, EMAIL), None);
                assert_eq!(
                    session.token_store.keychain_state(),
                    KeychainState::Unavailable
                );
            }

            let report = state
                .update_config(settings_save(&state, true))
                .expect("the settings still save - the residue is reported, not refused");

            assert!(
                report.keychain_residue.is_some(),
                "a keyring we know took a token and now will not answer has not proved it is empty"
            );
            assert!(
                state.get_config().speleodb.keychain_has_token,
                "and the belief must not be cleared over an entry that may still be in there - \
                 clearing it sends the next launch straight past it"
            );
        });
    }

    #[test]
    fn a_keychainless_box_that_just_restarted_turns_the_keyring_off_without_a_false_alarm() {
        // The keychain-less box of `a_keychainless_box_turns_the_keyring_off_without
        // _a_false_alarm` again, but reached the way a RESTART reaches it. The sign-in
        // that test makes is what records `Unavailable`; a launch under a persisted
        // "this session only" decline records nothing at all, because rehydration
        // applies the decline BEFORE the load and the load then skips the keyring
        // entirely. Suppressing on `Unavailable` alone left that launch sitting at
        // `NotAttempted` and warned it about a credential a keychain-less box cannot
        // hold - the false alarm this whole line exists to avoid, fired at the user
        // who had already answered the storage question once.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(FailingSecretStore));
            {
                let mut config = state.config.lock();
                // What that earlier session-only answer persisted, and what
                // `SpeleoDbSession::rehydrate` reads back on the next launch.
                config.speleodb.keychain_declined = true;
                assert!(
                    !config.speleodb.keychain_has_token,
                    "a keychain-less box cannot reach `true`: the flag mirrors \
                     `TokenStore::keychain_has_token`, which rises only where a keychain \
                     read or write actually succeeded, and none can here"
                );
                drop(config);
                let mut session = state.speleodb.lock();
                // The live store as that rehydrate leaves it: the decline applied, and
                // NO keychain access behind it - no login this run to record one.
                session.token_store.set_keychain_declined(true);
                session.token_store.note_keychain_has_token(false);
                assert_eq!(
                    session.token_store.keychain_state(),
                    KeychainState::Declined,
                    "the user chose this, and nothing underneath it has been learned"
                );
            }

            let report = state
                .update_config(settings_save(&state, true))
                .expect("settings saved");

            assert_eq!(
                report.keychain_residue, None,
                "a keychain that cannot hold anything has stranded nothing to warn about, \
                 restart or no restart"
            );
            assert!(
                !state.get_config().speleodb.keychain_has_token,
                "and the next launch still has no reason to open a keyring that holds nothing"
            );
        });
    }

    #[test]
    fn turning_the_keyring_off_with_no_account_raises_no_alarm() {
        // Nobody signed in, so there is no account to key an entry by and the
        // eviction never runs. The pre-fix code answered this exactly as it
        // answered a refused delete - `false` - which is why "report on every
        // `false`" would cry wolf at every user who turns the switch on before they
        // have ever touched SpeleoDB.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(InMemoryStore::new()));
            {
                // `endpoint` stays seeded so nothing reaches a real keyring; the
                // email is what makes the account key, and there isn't one.
                let mut session = state.speleodb.lock();
                session.email = None;
                drop(session);
            }
            // A recorded belief, in the shape a real login leaves it - the point being
            // that no SESSION account means no key to evict by, whatever is recorded.
            state
                .speleodb_write_account_record(Some(EMAIL.to_owned()), Some(account_key(EP, EMAIL)))
                .expect("what the earlier login persisted");

            let report = state
                .update_config(settings_save(&state, true))
                .expect("settings saved");

            assert_eq!(
                report.keychain_residue, None,
                "there was nothing to delete, so there is nothing to warn about"
            );
            assert!(
                state.get_config().speleodb.keychain_has_token,
                "and nothing was checked, so nothing is claimed either way"
            );
        });
    }

    #[test]
    fn the_keyring_switch_reaches_the_live_session_immediately() {
        // The point of applying it here rather than at the next launch: the user has
        // just been asked one unlock prompt too many, so the running session must
        // stop reaching for the keychain now.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(InMemoryStore::new()));

            state
                .update_config(settings_save(&state, true))
                .expect("settings saved");

            let mut session = state.speleodb.lock();
            assert_eq!(
                session.token_store.keychain_state(),
                // `Disabled`, not `Declined`: the switch is a standing policy the
                // retry verb cannot revoke, and reporting it as a decline is what
                // made Settings offer a "Try again" that could not do anything.
                KeychainState::Disabled,
                "the live store has to know before the next restart"
            );
            assert_eq!(
                session.token_store.store(EP, EMAIL, "tok", false),
                TokenTier::Memory,
                "a working keychain is still not used once the switch is on"
            );
        });
    }

    #[test]
    fn a_settings_save_keeps_the_backend_owned_credential_fields() {
        // The switch is the only credential field the form owns. Its three siblings
        // are settled by the login-time storage prompt and "Try again" - both of
        // which write `config.toml` behind a Settings dialog that is still mounted
        // over its own stale snapshot - so a save has to put them back.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(InMemoryStore::new()));
            {
                let mut config = state.config.lock();
                config.speleodb.token_fallback = TokenFallback::Plaintext;
                config.speleodb.keychain_declined = true;
                config.speleodb.keychain_has_token = true;
                config.speleodb.keychain_token_account = Some(account_key(EP, EMAIL));
                config.speleodb.keychain_disabled = true;
            }

            // A form payload carrying the serde defaults for everything it does not
            // own, and the switch turned back off.
            let mut posted = crate::config::Config::default();
            posted.speleodb.keychain_disabled = false;
            state.update_config(posted).expect("settings saved");

            let saved = state.get_config();
            assert_eq!(saved.speleodb.token_fallback, TokenFallback::Plaintext);
            assert!(saved.speleodb.keychain_declined);
            assert!(saved.speleodb.keychain_has_token);
            assert_eq!(
                saved.speleodb.keychain_token_account,
                Some(account_key(EP, EMAIL)),
                "and the belief keeps the account it is about: put back WITHOUT its stamp \
                 it reads as an unstamped one, which is scoped by the email alone - the \
                 same-address-different-server hole, re-opened by a save that changed nothing"
            );
            assert!(
                !saved.speleodb.keychain_disabled,
                "the switch itself IS the form's to set, in both directions"
            );
        });
    }

    #[test]
    fn a_settings_save_keeps_an_email_persisted_underneath_it() {
        // The account panel's "Log in now" signs in WITHOUT closing Settings, so the
        // backend writes the account email while the form still holds the snapshot it
        // opened with - one where nobody was signed in. Saving any unrelated setting
        // afterwards posted that snapshot back and cleared the email, which is the
        // rehydrate seed: `speleodb_ensure_rehydrated` keys the credential lookup by
        // it, so the next launch could not find the token of the account that was
        // signed in and asked for another login instead.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(InMemoryStore::new()));
            // The form's copy, taken before the inline login resolved the account.
            let staged = state.get_config();
            assert_eq!(staged.speleodb.email, None);
            state
                .speleodb_write_account_record(Some(EMAIL.to_owned()), None)
                .expect("the inline login persists the account");

            state.update_config(staged).expect("settings saved");

            assert_eq!(
                state.get_config().speleodb.email.as_deref(),
                Some(EMAIL),
                "a save behind an inline login must not clear the rehydrate seed"
            );
        });
    }

    #[test]
    fn an_unrelated_settings_save_keeps_the_credential_line() {
        // Keychain fails at login, the user answers the storage prompt with "Decide
        // later" (which deliberately persists nothing), then saves any unrelated
        // setting. Re-applying the UNCHANGED `keychain_disabled` re-ran
        // `refresh_keychain_state`, which reset the line to "not attempted" and took
        // the "Try again" button with it - it is offered only for `declined` /
        // `unavailable` - until the user signed in again.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_over(Box::new(FailingSecretStore));
            {
                let mut session = state.speleodb.lock();
                assert_eq!(
                    session.token_store.store(EP, EMAIL, "tok", false),
                    TokenTier::Memory
                );
                assert_eq!(
                    session.token_store.keychain_state(),
                    KeychainState::Unavailable
                );
            }

            let mut posted = settings_save(&state, false);
            posted.general.undo_history_limit = 42;
            state.update_config(posted).expect("settings saved");

            let session = state.speleodb.lock();
            assert_eq!(
                session.token_store.keychain_state(),
                KeychainState::Unavailable,
                "saving an unrelated setting must not forget that the keychain is broken"
            );
        });
    }
}
