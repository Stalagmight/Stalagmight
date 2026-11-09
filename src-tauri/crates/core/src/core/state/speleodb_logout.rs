//! End SpeleoDB sessions and remove the persisted token.
use super::speleodb::local_dto;
use super::AppState;
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

/// Delete only this account's obsolete app-owned plaintext file. Never read or
/// import its contents, inspect other accounts, or access a system keychain.
fn remove_legacy_token(
    config_path: &std::path::Path,
    endpoint: &str,
    email: &str,
) -> Result<(), SpeleoErrorDto> {
    let filename: String = crate::speleodb::auth::account_key(endpoint, email)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let root = config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    match std::fs::remove_file(root.join("speleodb").join("tokens").join(filename)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(local_dto(SpeleoDbError::Io("Signed out, but an obsolete plaintext SpeleoDB credential could not be removed. Try signing out again.".into()))),
    }
}

impl AppState {
    /// Clear the token while holding the same lock used by settings saves.
    pub(super) fn speleodb_clear_saved_token(&self) -> Result<(), SpeleoErrorDto> {
        let mut config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        config.speleodb.token.0 = None;
        config.speleodb.preserve_unreadable_file = false;
        config.save_speleodb().map_err(|_| local_dto(SpeleoDbError::Io(
            "Signed out, but the saved SpeleoDB token could not be removed. Try signing out again.".to_owned()
        )))?;
        if let (Some(endpoint), Some(email)) = (&config.speleodb.endpoint, &config.speleodb.email) {
            let path = crate::config::Config::config_path().map_err(|_| {
                local_dto(SpeleoDbError::Io(
                    "Could not locate obsolete SpeleoDB credentials".into(),
                ))
            })?;
            remove_legacy_token(&path, endpoint, email)?;
        }
        Ok(())
    }

    pub fn speleodb_logout(&self) -> Result<(), SpeleoErrorDto> {
        let _lock_ops = self.speleodb_lock_ops_guard();
        self.speleodb_logout_drop_gates();
        self.speleodb
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .forget();
        self.speleodb_clear_saved_token()
    }

    pub(super) fn speleodb_logout_drop_gates(&self) {
        // A session boundary even though the project slot never moves (footgun
        // 5.8): the token an in-flight Start Editing would commit a claim over is
        // being forgotten, so the commit guard must see this. Bumped under a brief
        // `project` acquisition per the `speleodb_session` rule; taken and dropped
        // before the gates/lock work below, none of which needs it. Bumped even on
        // the read-only early return: the boundary is the LOGOUT, not the release.
        {
            let _proj_guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bump_speleodb_session();
        }
        if !self.edit_enabled() {
            return;
        }
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

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "synthetic legacy credential cleanup tests"
)]
mod tests {
    use super::*;
    #[test]
    fn logout_removes_only_the_current_accounts_obsolete_plaintext_file() {
        crate::test_helpers::with_isolated_config(|| {
            let state = AppState::new().expect("state");
            let path = crate::config::Config::config_path().expect("isolated config");
            let root = path.parent().expect("parent").join("speleodb/tokens");
            std::fs::create_dir_all(&root).expect("directory");
            let old = root.join("https___example.com_diver_example.com");
            let other = root.join("other-account");
            std::fs::write(&old, b"synthetic-old-token").expect("legacy file");
            std::fs::write(&other, b"synthetic-other-token").expect("other account");
            {
                let mut config = state.config.lock().expect("config");
                config.speleodb.endpoint = Some("https://example.com".into());
                config.speleodb.email = Some("diver@example.com".into());
                config.speleodb.token.0 = Some("synthetic-current-token".into());
            }
            state.speleodb_logout().expect("logout");
            assert!(!old.exists());
            assert!(other.exists());
            assert!(state.get_config().speleodb.token.0.is_none());
        });
    }
    #[test]
    fn legacy_cleanup_failure_is_reported_without_recursive_deletion() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("config.toml");
        let target = dir
            .path()
            .join("speleodb/tokens/https___example.com_diver_example.com");
        std::fs::create_dir_all(&target).expect("block removal");
        assert!(remove_legacy_token(&path, "https://example.com", "diver@example.com").is_err());
        assert!(target.is_dir());
    }
}
