use anyhow::Result;

use crate::config::Config;

use super::AppState;

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

    /// Update config from a whole-`Config` payload.
    ///
    pub fn update_config(&self, config: Config) -> Result<()> {
        let _guard = self
            .config_write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.update_config_locked(config)
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
        self.update_config_locked(config)
    }

    /// The body of [`Self::update_config`], with `config_write_lock` already held.
    /// Never call this directly - the lock is what makes a partial update safe.
    fn update_config_locked(&self, mut config: Config) -> Result<()> {
        tracing::info!(purpose = "apply user settings", "Updating configuration");
        let _lock_ops = self.speleodb_lock_ops_guard();
        let instance_changed = {
            let existing = self.get_config();
            let normalize = |endpoint: &Option<String>| {
                endpoint
                    .as_deref()
                    .map(crate::speleodb::auth::normalize_endpoint)
            };
            normalize(&existing.speleodb.endpoint) != normalize(&config.speleodb.endpoint)
        };
        // Hold the config lock through persistence so login cannot update the token
        // between this restore and the settings write.
        let mut existing = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        config.webkit_memory = existing.webkit_memory.clone();
        config.speleodb.email.clone_from(&existing.speleodb.email);
        if instance_changed {
            config.speleodb.token.0 = None;
        } else {
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

        if instance_changed {
            // A failed save must leave the current session and lock untouched.
            // The outer lock-ops/write guards still exclude login/settings writers.
            drop(existing);
            self.speleodb_logout_drop_gates();
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

        Ok(())
    }

    /// Reload settings from disk.
    pub fn reload_config(&self) -> Result<(Config, bool)> {
        let _write = self
            .config_write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _lock_ops = self.speleodb_lock_ops_guard();
        let new_config = Config::load()?;
        let previous = self.get_config();
        let normalize = |endpoint: &Option<String>| {
            endpoint
                .as_deref()
                .map(crate::speleodb::auth::normalize_endpoint)
        };
        let auth_changed = normalize(&previous.speleodb.endpoint)
            != normalize(&new_config.speleodb.endpoint)
            || previous.speleodb.email != new_config.speleodb.email
            || previous.speleodb.token.0 != new_config.speleodb.token.0;
        if auth_changed {
            self.speleodb_logout_drop_gates();
        }

        // Publish the new source before allowing lazy credential restoration.
        // Keep config locked so a parallel session read cannot restore the old file.
        {
            let mut cfg = self
                .config
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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

        Ok((new_config, auth_changed))
    }
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
            stale.speleodb.poll_interval_seconds = 90;
            state.update_config(stale).unwrap();
            let loaded = Config::load().unwrap();
            assert_eq!(
                loaded.speleodb.token.0.as_deref(),
                Some("synthetic-test-token")
            );
            assert_eq!(loaded.speleodb.email.as_deref(), Some("diver@example.com"));
            assert_eq!(loaded.speleodb.poll_interval_seconds, 90);
            assert!(state.speleodb_session_status().unwrap().logged_in);
        });
    }

    #[test]
    fn changing_instance_clears_saved_and_live_credentials() {
        crate::test_helpers::with_isolated_config(|| {
            let state = signed_in_state();
            let mut settings = state.get_config();
            settings.speleodb.endpoint = Some("https://other.example.com".to_owned());
            state.update_config(settings).unwrap();
            assert!(!state.speleodb_session_status().unwrap().logged_in);
            let loaded = Config::load().unwrap();
            assert_eq!(
                loaded.speleodb.endpoint.as_deref(),
                Some("https://other.example.com")
            );
            assert!(loaded.speleodb.token.0.is_none());
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
                    "https://other.example.com",
                    "diver@example.com",
                    "synthetic-test-token",
                ),
                (
                    "https://other.example.com",
                    "another@example.com",
                    "synthetic-test-token",
                ),
                (
                    "https://other.example.com",
                    "another@example.com",
                    "replacement-token",
                ),
            ] {
                let mut changed = state.get_config();
                changed.speleodb.endpoint = Some(endpoint.to_owned());
                changed.speleodb.email = Some(email.to_owned());
                changed.speleodb.token.0 = Some(token.to_owned());
                changed.save().unwrap();
                assert!(state.reload_config().unwrap().1);
                let actual = state.speleodb_active_account().unwrap();
                assert_eq!(
                    actual,
                    (endpoint.to_owned(), token.to_owned(), email.to_owned())
                );
            }
        });
    }
}
