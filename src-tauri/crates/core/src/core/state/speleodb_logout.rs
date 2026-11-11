//! End SpeleoDB sessions and remove the persisted token.
use super::speleodb::local_dto;
use super::speleodb_lock_release::LockReleaseParams;
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
        Err(_) => Err(local_dto(SpeleoDbError::Io("Disconnected, but an obsolete plaintext SpeleoDB credential could not be removed. Try disconnecting again.".into()))),
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
            "Disconnected, but the saved SpeleoDB token could not be removed. Try disconnecting again.".to_owned()
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
        // Attempt both cleanups even when credential persistence fails.
        let token_result = self.speleodb_clear_saved_token();
        let recent_result = self
            .speleodb_forget_recent_projects(super::speleodb_local_state::cache_root().as_deref());
        token_result.and(recent_result)
    }

    /// Forget only confirmed SpeleoDB references. Metadata reads never modify the
    /// project files; unreadable/missing entries are retained rather than guessed.
    ///
    /// Two kinds of reference are KEPT:
    ///
    /// - A cached copy (`<cache>/speleodb/<id>/project.stalx`, the one place a managed
    ///   project lives). Signed out, the home screen shows every cached copy - list and
    ///   map - and opens it read-only; the map is built from recents, so forgetting the
    ///   row would take the copy off the map for exactly the user it is shown to.
    /// - A reference whose copy still holds un-uploaded edits: the recents row is the
    ///   way back to that work, and dropping it on sign-out left the changes on disk
    ///   with nothing in the app that could reach them. The user's discard removes such
    ///   a copy (and its row) before signing out; anything still dirty here is work
    ///   they chose to come back to.
    ///
    /// `cache_root` is passed in so tests can use a temporary cache; `None` (the app
    /// dirs are not resolved) recognises no cached copy.
    fn speleodb_forget_recent_projects(
        &self,
        cache_root: Option<&std::path::Path>,
    ) -> Result<(), SpeleoErrorDto> {
        let mut recent = self
            .recent_projects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recent.retain(|path| {
            let Ok(Some(link)) = crate::persistence::Database::open_for_metadata(path)
                .and_then(|db| db.load_speleo_link())
            else {
                return true;
            };
            link.has_unpushed_edits
                || cache_root.is_some_and(|root| {
                    super::speleodb_local_state::is_the_cached_copy(root, &link.project_id, path)
                })
        });
        let mut config = self
            .config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        config.general.recent_projects.clone_from(&recent);
        config.save().map_err(|_| local_dto(SpeleoDbError::Io(
            "Disconnected, but SpeleoDB project references could not be saved. Try disconnecting again.".into()
        )))
    }

    pub(super) fn speleodb_logout_drop_gates(&self) {
        // A session boundary even though the project slot never moves (footgun
        // 5.8): the token an in-flight Start Editing would commit a claim over is
        // being forgotten, so the commit guard must see this. Bumped under a brief
        // `project` acquisition per the `speleodb_session` rule; taken and dropped
        // before the lock work below, none of which needs it.
        //
        // The bump is also what shuts the gate: it leaves the edit session's claim
        // bound to a generation that is over (`speleodb_edit_gate`). An open LOCAL
        // project is unmanaged and stays editable without being asked about.
        {
            let _proj_guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bump_speleodb_session();
        }
        // The open project's lock, if this account holds a claim on it - bound to
        // the session that just ended or not: decided from the claim, never from
        // the gate the bump has just shut. Silent (the un-pushed-edits veto keeps
        // it) and holder-checked (lock#4); a no-op without a claim or a link.
        self.speleodb_release_held_lock_best_effort();
    }
    /// [`Self::speleodb_logout_drop_gates`] for a caller that holds a lock the
    /// release POST must not be made under: a settings save that changes the
    /// SpeleoDB instance holds `config_write_lock`, which every synchronous partial
    /// config command (panel sizes, column widths) also takes on the main thread.
    ///
    /// Everything but the network call happens here, in the original order - the
    /// session boundary (which shuts the gate) and the silent-release veto - and
    /// the release is returned for the caller to send once that lock is dropped,
    /// still inside its lock-ops span, so no lock operation can land between this
    /// and the POST. Nothing is edited under a lock that is about to be handed
    /// back: the gate is shut from the bump on.
    ///
    /// The claim ends when the release is sent: it is matched to the account the
    /// params were captured for, not the live session the instance switch is
    /// about to forget, so a failed release can still put it back
    /// (`speleodb_forget_claim_released_by`).
    pub(super) fn speleodb_logout_drop_gates_deferring_release(&self) -> Option<LockReleaseParams> {
        {
            let _proj_guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bump_speleodb_session();
        }
        self.speleodb_held_lock_release_params()
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
    fn logout_forgets_only_clean_speleodb_references_and_preserves_files() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let state = AppState::new().expect("state");
            let local = dir.path().join("local.stalx");
            let missing = dir.path().join("missing.stalx");
            let unreadable = dir.path().join("unreadable.stalx");
            std::fs::write(&unreadable, b"synthetic invalid database").expect("write");
            let mut paths = vec![local.clone(), missing, unreadable];
            // remote-a still holds un-uploaded edits, remote-b is clean: only the
            // clean reference may be forgotten.
            for (name, endpoint, dirty) in [
                ("local", None, false),
                ("remote-a", Some("https://a.example/"), true),
                ("remote-b", Some("https://b.example/"), false),
            ] {
                let path = dir.path().join(format!("{name}.stalx"));
                let mut db = crate::persistence::Database::open(&path).expect("db");
                db.save_project(&crate::models::Project::new(name.into(), None))
                    .expect("project");
                if let Some(endpoint) = endpoint {
                    db.save_speleo_link(Some(&crate::models::SpeleoLink {
                        endpoint: endpoint.into(),
                        project_id: uuid::Uuid::new_v4().to_string(),
                        project_name: name.into(),
                        base_commit_sha: None,
                        has_unpushed_edits: dirty,
                        last_edit_at: None,
                        last_verify_at: None,
                        undo_mode: None,
                    }))
                    .expect("link");
                    paths.push(path);
                }
            }
            let preserved: Vec<_> = paths.iter().map(|path| std::fs::read(path).ok()).collect();
            *state.recent_projects.lock().expect("recents") = paths.clone();
            state.config.lock().expect("config").general.recent_projects = paths.clone();
            let kept: Vec<_> = paths[..4].to_vec();
            assert!(state.open_project(local).success);
            state.speleodb_logout().expect("logout");
            assert_eq!(*state.recent_projects.lock().expect("recents"), kept);
            assert_eq!(state.get_config().general.recent_projects, kept);
            assert_eq!(
                crate::config::Config::load()
                    .expect("reload")
                    .general
                    .recent_projects,
                kept
            );
            assert!(state.get_project_state().is_some());
            assert!(state.edit_enabled());
            for (path, before) in paths.iter().zip(preserved) {
                assert_eq!(std::fs::read(path).ok(), before, "{}", path.display());
            }
            state.speleodb_logout().expect("repeated logout");
            assert_eq!(*state.recent_projects.lock().expect("recents"), kept);
        });
    }

    /// Signed out, the home screen lists every cached copy on the list AND the map,
    /// and the map is built from recents: a clean cached copy keeps its row. A clean
    /// linked file outside the cache is still forgotten.
    #[test]
    fn logout_keeps_the_recents_rows_of_cached_copies() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let root = dir.path().join("speleodb");
            let id = uuid::Uuid::new_v4().to_string();
            let cached = super::super::speleodb_local_state::cache_path_in(&root, &id);
            let stray = dir.path().join("stray-linked-copy.stalx");
            std::fs::create_dir_all(cached.parent().expect("parent")).expect("cache dir");
            for path in [&cached, &stray] {
                let mut db = crate::persistence::Database::open(path).expect("db");
                db.save_project(&crate::models::Project::new("Synthetic".into(), None))
                    .expect("project");
                db.save_speleo_link(Some(&crate::models::SpeleoLink {
                    endpoint: "https://a.example/".into(),
                    project_id: id.clone(),
                    project_name: "Synthetic".into(),
                    base_commit_sha: None,
                    has_unpushed_edits: false,
                    last_edit_at: None,
                    last_verify_at: None,
                    undo_mode: None,
                }))
                .expect("link");
            }
            let state = AppState::new().expect("state");
            let paths = vec![cached.clone(), stray];
            *state.recent_projects.lock().expect("recents") = paths.clone();
            state.config.lock().expect("config").general.recent_projects = paths;
            state
                .speleodb_forget_recent_projects(Some(&root))
                .expect("forget");
            assert_eq!(
                *state.recent_projects.lock().expect("recents"),
                vec![cached.clone()]
            );
            assert_eq!(state.get_config().general.recent_projects, vec![cached]);
        });
    }

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
