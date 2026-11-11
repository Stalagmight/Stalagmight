use anyhow::Result;

use crate::config::Config;

use super::speleodb_lock_release::LockReleaseParams;
use super::AppState;

/// Which door a config write came through, which decides what it may change.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfigWrite {
    /// A whole-`Config` payload (the Settings dialog). The only door that can change
    /// the SpeleoDB instance, so the only one that may need the lock-ops span.
    /// `lock_ops_held` says whether the caller took it.
    Whole { lock_ops_held: bool },
    /// A partial update (`modify_config`). It never edits `[speleodb]`, so the
    /// stored section is kept as it is at write time. Its payload is a clone taken
    /// under `config_write_lock`, but a sign-in writes `config` without that lock,
    /// and the clone's endpoint can be one sign-in old. Kept from the live config,
    /// the section cannot read as an instance change, and a divider drag cannot sign
    /// the user out or wait behind a SpeleoDB network operation.
    Partial,
}

/// What a whole-config write must still do after `config_write_lock` is dropped.
#[must_use]
struct AfterConfigWrite {
    /// The old instance's write lock, captured while its gates came down. Released
    /// over the network only after the write lock is gone, so partial config
    /// writers (which are sync commands on the main thread) never wait on the POST.
    release: Option<LockReleaseParams>,
}

/// The outcome of one pass of a locked config write.
#[must_use]
enum Locked<T> {
    /// Written; what is left to do once the write lock is dropped.
    Done(AfterConfigWrite),
    /// Nothing was written: the SpeleoDB identity this write would replace moved
    /// (a sign-in or sign-out landed) after the caller decided it did not need the
    /// lock-ops span. Hands the payload back for a pass that takes it.
    NeedsLockOps(T),
}

/// The SpeleoDB identity a config names: instance, account and token.
fn speleodb_identity(config: &Config) -> (Option<String>, Option<String>, Option<String>) {
    (
        config
            .speleodb
            .endpoint
            .as_deref()
            .map(crate::speleodb::auth::normalize_endpoint),
        config.speleodb.email.clone(),
        config.speleodb.token.0.clone(),
    )
}

impl AppState {
    /// Get config
    ///
    /// A POISONED config mutex is RECOVERED rather than answered with
    /// `Config::default()`: the Settings dialog renders whatever comes back and
    /// posts it straight to [`Self::update_config`], so handing out defaults would
    /// turn one unrelated panic into a wholesale reset of the user's settings.
    pub fn get_config(&self) -> Config {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether saving `incoming` would move the app to a different SpeleoDB instance.
    fn speleodb_instance_changes(&self, incoming: &Config) -> bool {
        let normalize = |endpoint: &Option<String>| {
            endpoint
                .as_deref()
                .map(crate::speleodb::auth::normalize_endpoint)
        };
        normalize(&self.get_config().speleodb.endpoint) != normalize(&incoming.speleodb.endpoint)
    }

    /// Refuse a config write that moves the app to another SpeleoDB instance while a
    /// session is connected: changing servers is a Disconnect (the frontend's
    /// sign-out flow, which protects un-uploaded work and hands locks back) and a
    /// fresh Connect, never a settings write underneath the session. Also refuses an
    /// address the transport policy refuses, before it is saved. The instance-change
    /// handling below stays as defence in depth for a sign-in racing this check.
    ///
    /// Called only for a write that changes the instance, and never under the
    /// `config` lock (the session status reads it).
    fn speleodb_check_instance_change(&self, incoming: &Config) -> Result<()> {
        if self.speleodb_session_status().is_ok_and(|s| s.logged_in) {
            return Err(crate::speleodb::SpeleoDbError::ServerChangeWhileConnected.into());
        }
        Self::speleodb_check_endpoint(incoming)
    }

    /// Refuse a saved SpeleoDB address the transport policy refuses, under the
    /// `insecure_http` choice the same save carries (honoured only in a
    /// development build, `tls::INSECURE_HTTP_HONOURED`).
    fn speleodb_check_endpoint(incoming: &Config) -> Result<()> {
        if let Some(endpoint) = incoming.speleodb.endpoint.as_deref() {
            crate::speleodb::validate_endpoint(endpoint, incoming.speleodb.insecure_http)?;
        }
        Ok(())
    }

    /// Update config from a whole-`Config` payload. A change of SpeleoDB instance is
    /// refused while connected ([`Self::speleodb_check_instance_change`]). A save
    /// that changes the server or the insecure-HTTP choice must leave an address
    /// the transport policy accepts: turning the choice off under a saved
    /// `http://` hostname would otherwise save a server every call then refuses.
    pub fn update_config(&self, mut config: Config) -> Result<()> {
        // Clean the endpoint as entered (#675): a pasted address commonly carries
        // whitespace or zero-width characters around it. The cleaned value is
        // what is compared, validated and stored.
        config.speleodb.endpoint = clean_endpoint(config.speleodb.endpoint.as_deref());
        if self.speleodb_instance_changes(&config) {
            self.speleodb_check_instance_change(&config)?;
        } else if self.get_config().speleodb.insecure_http != config.speleodb.insecure_http {
            Self::speleodb_check_endpoint(&config)?;
        }
        self.write_whole_config(config)
    }

    /// The write behind [`Self::update_config`], after its connected-instance check.
    ///
    /// Takes the SpeleoDB lock-ops span ONLY when the payload changes the instance:
    /// that ends the session (a logout) and can release the open project's lock, so
    /// it must not land inside another lock operation's span. With the check in
    /// front, that is defence in depth for a sign-in that lands after it. Every other
    /// save is a plain local write and must not queue behind an upload or a
    /// download, which hold lock-ops for minutes. Lock-ops is OUTERMOST, so it is
    /// taken before `config_write_lock`. A sign-in that lands after the check and
    /// turns this into an instance change is caught under the `config` lock, before
    /// anything is written, and sends the save round again to take the span first.
    pub(super) fn write_whole_config(&self, mut config: Config) -> Result<()> {
        let mut lock_ops = None;
        loop {
            if lock_ops.is_none() && self.speleodb_instance_changes(&config) {
                lock_ops = Some(self.speleodb_lock_ops_guard());
            }
            let write = self
                .config_write_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let door = ConfigWrite::Whole {
                lock_ops_held: lock_ops.is_some(),
            };
            match self.update_config_locked(config, door)? {
                Locked::Done(after) => {
                    drop(write);
                    self.finish_config_write(after);
                    return Ok(());
                }
                Locked::NeedsLockOps(payload) => {
                    drop(write);
                    config = payload;
                    lock_ops = Some(self.speleodb_lock_ops_guard());
                }
            }
        }
    }

    /// The network half of an instance change, after `config_write_lock` is gone and
    /// still inside the caller's lock-ops span: hand the old instance's lock back.
    /// Best-effort, exactly like the logout release it is split from.
    fn finish_config_write(&self, after: AfterConfigWrite) {
        if let Some(params) = after.release {
            self.speleodb_release_best_effort(&params);
        }
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
    ///
    /// Never waits on SpeleoDB: the partial commands are synchronous, so Tauri runs
    /// them on the main thread, and a wait here froze the whole window for as long
    /// as an upload or download held the lock-ops span. They cannot change the
    /// instance (see [`ConfigWrite::Partial`]), which is the only reason a config
    /// write needs that span.
    pub fn modify_config<F>(&self, mutate: F) -> Result<()>
    where
        F: FnOnce(&mut Config) -> Result<()>,
    {
        let _guard = self
            .config_write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut config = self.get_config();
        mutate(&mut config)?;
        match self.update_config_locked(config, ConfigWrite::Partial)? {
            Locked::Done(after) => {
                debug_assert!(
                    after.release.is_none(),
                    "a partial config write never changes the SpeleoDB instance"
                );
                self.finish_config_write(after);
                Ok(())
            }
            // Unreachable by construction: a partial write keeps the stored section.
            Locked::NeedsLockOps(_) => Err(anyhow::anyhow!(
                "a partial settings write cannot change the SpeleoDB instance"
            )),
        }
    }

    /// The body of [`Self::update_config`], with `config_write_lock` already held.
    /// Never call this directly - the lock is what makes a partial update safe.
    ///
    /// Holds no lock across network I/O. An instance change returns the release it
    /// captured in [`AfterConfigWrite`] for the caller to send once the write lock is
    /// dropped, and one found without the lock-ops span returns the payload
    /// unwritten ([`Locked::NeedsLockOps`]).
    fn update_config_locked(
        &self,
        mut config: Config,
        door: ConfigWrite,
    ) -> Result<Locked<Config>> {
        tracing::info!(purpose = "apply user settings", "Updating configuration");
        // Hold the config lock through persistence so login cannot update the token
        // between this restore and the settings write.
        let mut existing = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if door == ConfigWrite::Partial {
            config.speleodb.clone_from(&existing.speleodb);
        }
        let instance_changed = {
            let normalize = |endpoint: &Option<String>| {
                endpoint
                    .as_deref()
                    .map(crate::speleodb::auth::normalize_endpoint)
            };
            normalize(&existing.speleodb.endpoint) != normalize(&config.speleodb.endpoint)
        };
        if instance_changed
            && door
                != (ConfigWrite::Whole {
                    lock_ops_held: true,
                })
        {
            // Never end a session outside the lock-ops span. Checked under the
            // `config` lock that sign-in also writes under, so the answer holds
            // until the save below.
            return Ok(Locked::NeedsLockOps(config));
        }
        config.webkit_memory = existing.webkit_memory.clone();
        if instance_changed {
            // The account and its token belong to the old server. Settings saves a
            // new server before Connect signs in to it, so keeping either would
            // leave the old server's credentials paired with the new one when that
            // sign-in fails: a new server starts with no account until a sign-in
            // saves one.
            config.speleodb.email = None;
            config.speleodb.token.0 = None;
        } else {
            config.speleodb.email.clone_from(&existing.speleodb.email);
            config.speleodb.token.clone_from(&existing.speleodb.token);
        }
        config.speleodb.preserve_unreadable_file = existing.speleodb.preserve_unreadable_file
            && serde_json::to_value(&existing.speleodb).ok()
                == serde_json::to_value(&config.speleodb).ok();
        let old_cache_enabled = existing.cache.tile_cache_enabled;
        config.normalize_except_shortcuts();
        let new_cache_enabled = config.cache.tile_cache_enabled;

        if let Err(e) = config.save() {
            tracing::error!(
                reason = %e,
                purpose = "persist user settings",
                "Failed to save configuration"
            );
            return Err(e);
        }

        let mut release = None;
        if instance_changed {
            // A failed save must leave the current session and lock untouched.
            // The caller's lock-ops span and the write guard still exclude
            // login/settings writers. The release POST itself is deferred to the
            // caller, after the write guard is dropped.
            drop(existing);
            release = self.speleodb_logout_drop_gates_deferring_release();
            existing = self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *existing = config.clone();
        drop(existing);
        if instance_changed {
            self.speleodb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .forget();
        }

        // Sync tile cache if cache setting changed
        if old_cache_enabled != new_cache_enabled {
            if let Ok(mut cache_guard) = self.tile_cache.lock() {
                if new_cache_enabled {
                    // Enable cache - create new cache
                    match Self::init_tile_cache(&config) {
                        Ok(cache) => {
                            tracing::info!(purpose = "enable offline tiles", "Tile cache enabled");
                            *cache_guard = Some(cache);
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                purpose = "enable offline tiles",
                                "Failed to initialize tile cache"
                            );
                        }
                    }
                } else {
                    // Disable cache - set to None (existing cache data remains on disk)
                    tracing::info!(purpose = "disable offline tiles", "Tile cache disabled");
                    *cache_guard = None;
                }
            }
        }

        Ok(Locked::Done(AfterConfigWrite { release }))
    }

    /// Reload settings from disk.
    ///
    /// Like [`Self::update_config`], takes the lock-ops span only when the file on
    /// disk names a different account or instance, and takes it before
    /// `config_write_lock`. That is only known once the file is read, so a reload
    /// that discovers a change drops the write lock, takes lock-ops, and reads again.
    /// So does one that finds the live identity moved (a sign-in or sign-out landed)
    /// between its read and its write: publishing the file then would put back the
    /// credentials the sign-out just removed.
    pub fn reload_config(&self) -> Result<(Config, bool)> {
        let mut lock_ops = None;
        loop {
            let write = self
                .config_write_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut new_config = Config::load()?;
            // A hand-edited file gets the same cleaning as a settings save (#675).
            new_config.speleodb.endpoint = clean_endpoint(new_config.speleodb.endpoint.as_deref());
            let previous = self.get_config();
            if speleodb_identity(&previous).0 != speleodb_identity(&new_config).0 {
                self.speleodb_check_instance_change(&new_config)?;
            }
            let auth_changed = speleodb_identity(&previous) != speleodb_identity(&new_config);
            if auth_changed && lock_ops.is_none() {
                drop(write);
                lock_ops = Some(self.speleodb_lock_ops_guard());
                continue;
            }
            match self.reload_config_locked(
                &new_config,
                &previous,
                auth_changed,
                lock_ops.is_some(),
            ) {
                Locked::Done(after) => {
                    drop(write);
                    self.finish_config_write(after);
                    return Ok((new_config, auth_changed));
                }
                Locked::NeedsLockOps(()) => {
                    drop(write);
                    lock_ops = Some(self.speleodb_lock_ops_guard());
                }
            }
        }
    }

    /// The body of [`Self::reload_config`], with `config_write_lock` (and, on an
    /// identity change, lock-ops) held. Returns the old instance's lock release for
    /// the caller to send after the write lock is dropped, or, without lock-ops,
    /// [`Locked::NeedsLockOps`] when the live identity no longer matches `previous`.
    fn reload_config_locked(
        &self,
        new_config: &Config,
        previous: &Config,
        auth_changed: bool,
        lock_ops_held: bool,
    ) -> Locked<()> {
        let release = if auth_changed {
            self.speleodb_logout_drop_gates_deferring_release()
        } else {
            None
        };

        // Publish the new source before allowing lazy credential restoration.
        // Keep config locked so a parallel session read cannot restore the old file.
        {
            let mut cfg = self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !lock_ops_held && speleodb_identity(&cfg) != speleodb_identity(previous) {
                // Sign-in and sign-out write `config` under this lock, so a check
                // here sees every one that landed after `previous` was read.
                return Locked::NeedsLockOps(());
            }
            *cfg = new_config.clone();
            if auth_changed {
                self.speleodb
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .reload_credentials();
            }
        }

        // Sync recent_projects from config
        if let Ok(mut recent) = self.recent_projects.lock() {
            recent.clone_from(&new_config.general.recent_projects);
        }

        // Sync undo_stack history limit from config.
        //
        // [#303] `set_history_limit` records the new count but only re-arms the COUNT
        // bound; a Full-mode SpeleoDB project stays on the byte budget, so a settings
        // save cannot silently truncate the history its next push is meant to carry.
        // The recorded count still takes effect the moment the project goes back onto a
        // count window (closed, or switched to Disabled).
        if let Ok(mut undo) = self.undo_stack.lock() {
            undo.set_history_limit(new_config.general.undo_history_limit);
        }
        // ...and the BOUND itself, which `set_history_limit` deliberately will not touch.
        // The global `speleodb.undo_mode` can change in the same save. An already-open
        // linked project has an explicit value and therefore stays unchanged; local/no
        // project state remains on the count window. `sync_undo_depth_limit`'s own doc
        // lists config reload as a site it must be called from, so keep the re-arm here.
        // Sequenced after the block above so no lock is held across it.
        self.sync_undo_depth_limit();
        Locked::Done(AfterConfigWrite { release })
    }
}

/// The settings-side wrapper around [`crate::speleodb::clean_endpoint_input`]:
/// a field that cleans to nothing is "no endpoint configured".
pub(super) fn clean_endpoint(endpoint: Option<&str>) -> Option<String> {
    endpoint.and_then(crate::speleodb::clean_endpoint_input)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "synthetic session test setup")]
mod tests {
    use super::*;
    use crate::speleodb::auth::TokenStore;

    fn signed_in_state() -> AppState {
        let state = AppState::new().unwrap();
        {
            let mut config = state.config.lock().unwrap();
            config.speleodb.endpoint = Some("https://sdb.example.com".to_owned());
            config.speleodb.email = Some("diver@example.com".to_owned());
            config.speleodb.token.0 = Some("synthetic-test-token".to_owned());
            config.save_speleodb().unwrap();
        }
        {
            let mut session = state.speleodb.lock().unwrap();
            session.endpoint = Some("https://sdb.example.com".to_owned());
            session.email = Some("diver@example.com".to_owned());
            session.token_store = TokenStore::from_in_memory_token("synthetic-test-token");
        }
        state
    }

    #[test]
    fn unrelated_settings_save_preserves_backend_credentials() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let mut stale = state.get_config();
            stale.speleodb.email = None;
            stale.speleodb.token.0 = None;
            state.update_config(stale).unwrap();
            let loaded = Config::load().unwrap();
            assert_eq!(
                loaded.speleodb.token.0.as_deref(),
                Some("synthetic-test-token")
            );
            assert_eq!(loaded.speleodb.email.as_deref(), Some("diver@example.com"));
            assert!(!toml::to_string(&loaded.speleodb)
                .unwrap()
                .contains("allow_force_edit"));
            assert!(state.speleodb_session_status().unwrap().logged_in);
        });
    }

    #[test]
    fn changing_instance_requires_disconnect_without_mutating_credentials() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let mut settings = state.get_config();
            settings.speleodb.endpoint = Some("https://other.example.com".to_owned());
            assert!(state.update_config(settings).is_err());
            assert!(state.speleodb_session_status().unwrap().logged_in);
            let loaded = Config::load().unwrap();
            assert_eq!(
                loaded.speleodb.endpoint.as_deref(),
                Some("https://sdb.example.com")
            );
            assert!(loaded.speleodb.token.0.is_some());
        });
    }

    /// Settings validates the saved address whenever a save changes the server OR
    /// the insecure-HTTP choice: turning the choice off under a plain-http
    /// hostname is refused, not saved as a server every call would then refuse.
    #[test]
    fn turning_insecure_http_off_under_a_plain_http_server_is_refused() {
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().unwrap();
            {
                let mut config = state.config.lock().unwrap();
                config.speleodb.endpoint = Some("http://sdb.example.com".to_owned());
                config.speleodb.insecure_http = true;
            }
            let mut incoming = state.get_config();
            incoming.speleodb.insecure_http = false;
            assert!(state.update_config(incoming).is_err());
            assert!(
                state.get_config().speleodb.insecure_http,
                "nothing was saved"
            );

            // Over https the choice changes nothing the policy refuses.
            state.config.lock().unwrap().speleodb.endpoint =
                Some("https://sdb.example.com".to_owned());
            let mut incoming = state.get_config();
            incoming.speleodb.insecure_http = false;
            state.update_config(incoming).unwrap();
            assert!(!state.get_config().speleodb.insecure_http);
        });
    }

    #[test]
    fn equivalent_instance_spelling_keeps_the_token() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let mut settings = state.get_config();
            settings.speleodb.endpoint = Some("https://SDB.example.com:443/".to_owned());
            state.update_config(settings).unwrap();
            assert!(state.speleodb_session_status().unwrap().logged_in);
            assert!(Config::load().unwrap().speleodb.token.0.is_some());
        });
    }

    /// What copy-paste actually puts around an address (#675): spaces, a tab, a
    /// newline, a no-break space, a zero-width space and a BOM.
    const PASTED_SPELEODB: [&str; 6] = [
        "https://www.speleodb.org ",
        " https://www.speleodb.org",
        "\thttps://www.speleodb.org\n",
        "\u{00A0}https://www.speleodb.org\u{00A0}",
        "https://www.speleodb.org\u{200B}",
        "\u{FEFF}https://www.speleodb.org",
    ];

    #[test]
    fn pasted_endpoint_saves_cleaned() {
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().unwrap();
            for pasted in PASTED_SPELEODB {
                let mut settings = state.get_config();
                settings.speleodb.endpoint = Some(pasted.to_owned());
                let saved = state.update_config(settings);
                assert!(saved.is_ok(), "{pasted:?} refused: {saved:?}");
                assert_eq!(
                    Config::load().unwrap().speleodb.endpoint.as_deref(),
                    Some("https://www.speleodb.org"),
                    "{pasted:?}"
                );
                assert_eq!(
                    state.get_config().speleodb.endpoint.as_deref(),
                    Some("https://www.speleodb.org"),
                    "{pasted:?}"
                );
            }
        });
    }

    #[test]
    fn blank_endpoint_saves_as_none_and_interior_space_is_still_refused() {
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().unwrap();
            let mut settings = state.get_config();
            settings.speleodb.endpoint = Some(" \u{200B} ".to_owned());
            state.update_config(settings).unwrap();
            assert_eq!(Config::load().unwrap().speleodb.endpoint, None);

            let mut settings = state.get_config();
            settings.speleodb.endpoint = Some("https://www.speleo db.org".to_owned());
            let err = state.update_config(settings).unwrap_err().to_string();
            assert!(err.contains("Not a usable SpeleoDB address"), "{err}");
            assert_eq!(Config::load().unwrap().speleodb.endpoint, None);
        });
    }

    #[test]
    fn pasted_spelling_of_the_connected_server_keeps_the_session() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let mut settings = state.get_config();
            settings.speleodb.endpoint = Some("\u{200B}https://sdb.example.com/ ".to_owned());
            state.update_config(settings).unwrap();
            assert!(state.speleodb_session_status().unwrap().logged_in);
            let loaded = Config::load().unwrap();
            assert_eq!(
                loaded.speleodb.endpoint.as_deref(),
                Some("https://sdb.example.com/")
            );
            assert!(loaded.speleodb.token.0.is_some());
        });
    }

    #[test]
    fn reload_cleans_a_hand_edited_endpoint() {
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().unwrap();
            let mut edited = state.get_config();
            edited.speleodb.endpoint = Some(" https://www.speleodb.org\u{200B}".to_owned());
            edited.save().unwrap();
            state.reload_config().unwrap();
            assert_eq!(
                state.get_config().speleodb.endpoint.as_deref(),
                Some("https://www.speleodb.org")
            );
        });
    }

    #[test]
    fn startup_cleans_a_hand_edited_endpoint() {
        crate::test_helpers::with_isolated_config(|| {
            let mut edited = Config::default();
            edited.speleodb.endpoint = Some("\u{FEFF}https://www.speleodb.org \u{200B}".to_owned());
            edited.save().unwrap();
            let state = AppState::new().unwrap();
            assert_eq!(
                state.get_config().speleodb.endpoint.as_deref(),
                Some("https://www.speleodb.org")
            );
        });
    }

    #[test]
    fn sign_out_removes_token_and_keeps_connection_preferences() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            state.speleodb_logout().unwrap();
            assert!(!state.speleodb_session_status().unwrap().logged_in);
            let loaded = Config::load().unwrap();
            assert!(loaded.speleodb.token.0.is_none());
            assert_eq!(
                loaded.speleodb.endpoint.as_deref(),
                Some("https://sdb.example.com")
            );
        });
    }

    #[test]
    fn sign_out_reports_unwritable_file_but_ends_the_live_session() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let dir = crate::test_helpers::current_config_dir().unwrap();
            let file = dir.join("speleodb/speleodb.conf");
            std::fs::remove_file(&file).unwrap();
            std::fs::create_dir(&file).unwrap();
            assert!(state.speleodb_logout().is_err());
            assert!(!state.speleodb_session_status().unwrap().logged_in);
        });
    }
    #[test]
    fn failed_instance_save_keeps_previous_live_and_saved_credentials() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let dir = crate::test_helpers::current_config_dir().unwrap();
            let credentials = dir.join("speleodb/speleodb.conf");
            let previous = std::fs::read(&credentials).unwrap();
            std::fs::create_dir(dir.join("config.toml")).unwrap();
            let mut incoming = state.get_config();
            incoming.speleodb.endpoint = Some("https://other.example.com".to_owned());
            let generation = state.speleodb_session_generation();
            assert!(state.update_config(incoming).is_err());
            assert_eq!(state.speleodb_session_generation(), generation);
            assert_eq!(std::fs::read(&credentials).unwrap(), previous);
            assert_eq!(
                state.get_config().speleodb.endpoint.as_deref(),
                Some("https://sdb.example.com")
            );
            assert!(state.speleodb_session_status().unwrap().logged_in);
        });
    }

    #[test]
    fn failed_credential_save_does_not_change_general_preferences() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            state.get_config().save().unwrap();
            let dir = crate::test_helpers::current_config_dir().unwrap();
            let credentials = dir.join("speleodb/speleodb.conf");
            let before = std::fs::read(dir.join("config.toml")).unwrap();
            std::fs::remove_file(&credentials).unwrap();
            std::fs::create_dir(&credentials).unwrap();
            let mut incoming = state.get_config();
            incoming.general.undo_history_limit = 987;
            assert!(state.update_config(incoming).is_err());
            assert_eq!(std::fs::read(dir.join("config.toml")).unwrap(), before);
            assert!(state.speleodb_session_status().unwrap().logged_in);
        });
    }

    #[test]
    fn a_whole_save_racing_a_sign_in_to_another_instance_asks_for_lock_ops_and_writes_nothing() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let stale = state.get_config();
            let before = Config::load().unwrap();
            // A sign-in to another instance lands after the caller decided this
            // save needed no lock-ops span.
            state.config.lock().unwrap().speleodb.endpoint =
                Some("https://signed-in-elsewhere.example.com".to_owned());
            let outcome = state
                .update_config_locked(
                    stale,
                    ConfigWrite::Whole {
                        lock_ops_held: false,
                    },
                )
                .unwrap();
            assert!(matches!(outcome, Locked::NeedsLockOps(_)));
            assert_eq!(
                state.get_config().speleodb.endpoint.as_deref(),
                Some("https://signed-in-elsewhere.example.com"),
                "nothing written to memory"
            );
            assert_eq!(
                toml::to_string(&Config::load().unwrap()).unwrap(),
                toml::to_string(&before).unwrap(),
                "nothing written to disk"
            );
            assert!(state.speleodb_session_status().unwrap().logged_in);
        });
    }

    #[test]
    fn a_reload_racing_a_sign_out_asks_for_lock_ops_and_publishes_nothing() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let previous = state.get_config();
            let from_disk = Config::load().unwrap();
            // A sign-out lands between the reload's read and its write.
            state.config.lock().unwrap().speleodb.token.0 = None;
            let outcome = state.reload_config_locked(&from_disk, &previous, false, false);
            assert!(matches!(outcome, Locked::NeedsLockOps(())));
            assert!(
                state.get_config().speleodb.token.0.is_none(),
                "the sign-out must not be undone by a reload of the older file"
            );
        });
    }

    #[test]
    fn reload_only_replaces_session_when_saved_identity_changes() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let mut changed = state.get_config();
            changed.general.undo_history_limit = 987;
            changed.save().unwrap();
            let generation = state.speleodb_session_generation();
            assert!(!state.reload_config().unwrap().1);
            assert_eq!(state.speleodb_session_generation(), generation);
            assert!(state.speleodb_session_status().unwrap().logged_in);
            for (endpoint, email, token) in [
                (
                    "https://sdb.example.com",
                    "diver@example.com",
                    "synthetic-test-token",
                ),
                (
                    "https://sdb.example.com",
                    "another@example.com",
                    "synthetic-test-token",
                ),
                (
                    "https://sdb.example.com",
                    "another@example.com",
                    "replacement-token",
                ),
            ] {
                let mut changed = state.get_config();
                let expected_change = changed.speleodb.email.as_deref() != Some(email)
                    || changed.speleodb.token.0.as_deref() != Some(token);
                changed.speleodb.endpoint = Some(endpoint.to_owned());
                changed.speleodb.email = Some(email.to_owned());
                changed.speleodb.token.0 = Some(token.to_owned());
                changed.save().unwrap();
                assert_eq!(state.reload_config().unwrap().1, expected_change);
                let actual = state.speleodb_active_account().unwrap();
                assert_eq!(
                    actual,
                    (endpoint.to_owned(), token.to_owned(), email.to_owned())
                );
            }
        });
    }
    #[test]
    fn a_server_change_while_connected_is_refused_in_its_own_words() {
        // Not the generic "filesystem error: ..." an Io error reads as.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let refused = state
                .speleodb_login(
                    Some("https://other.example.com".into()),
                    None,
                    None,
                    None,
                    Some("<your_token_here>".into()),
                )
                .unwrap_err(); // a connected session is bound to its server
            assert_eq!(
                refused.message,
                "Disconnect from SpeleoDB before changing servers"
            );
            assert_eq!(
                refused.action,
                crate::speleodb::SpeleoDbAction::Fatal,
                "not retryable"
            );
            let mut settings = state.get_config();
            settings.speleodb.endpoint = Some("https://other.example.com".into());
            let refused = state.update_config(settings).unwrap_err();
            assert_eq!(
                refused.to_string(),
                "Disconnect from SpeleoDB before changing servers"
            );
        });
    }

    #[test]
    fn a_new_server_saved_before_connect_keeps_no_credential_of_the_old_one() {
        // Connect in Settings saves the staged server first, then signs in. When
        // that sign-in fails, the old server's account must not stay saved as the
        // new server's, and nothing of the failed sign-in may be saved either.
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            state.speleodb_logout().unwrap();
            assert_eq!(
                state.get_config().speleodb.email.as_deref(),
                Some("diver@example.com"),
                "signed out, the account is still remembered for its server"
            );
            let mut staged = state.get_config();
            staged.speleodb.endpoint = Some("https://other.example.com".to_owned());
            state.update_config(staged).unwrap();
            let refused = state.speleodb_login(
                Some("https://other.example.com".into()),
                None,
                Some("other@example.com".into()),
                None,
                None,
            );
            assert_eq!(
                refused.unwrap_err().message,
                "an email and a password are required to connect"
            );
            for saved in [state.get_config(), Config::load().unwrap()] {
                assert_eq!(
                    saved.speleodb.endpoint.as_deref(),
                    Some("https://other.example.com"),
                    "the server is a preference, saved by Connect"
                );
                assert_eq!(saved.speleodb.email, None, "no account for it yet");
                assert_eq!(saved.speleodb.token.0, None);
            }
            assert!(!state.speleodb_session_status().unwrap().logged_in);
        });
    }

    #[test]
    fn direct_login_and_reload_cannot_switch_a_connected_instance() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            assert!(state
                .speleodb_login(
                    Some("https://other.example.com".into()),
                    None,
                    None,
                    None,
                    Some("synthetic-token".into())
                )
                .is_err());
            let mut changed = state.get_config();
            changed.speleodb.endpoint = Some("https://other.example.com".into());
            changed.save().unwrap();
            assert!(state.reload_config().is_err());
            assert_eq!(
                state.speleodb_session_status().unwrap().endpoint.as_deref(),
                Some("https://sdb.example.com")
            );
        });
    }
}
