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
        self.clear_collaborator_cache();
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
    /// Each file's link is read through the shared probe
    /// (`speleodb_local_state::probe_link`: a version 2 header, or a legacy file's
    /// link row); only a file that READ as linked can be forgotten. The OPEN
    /// project's file is also kept while its in-memory link holds un-uploaded
    /// edits: a legacy file not upgraded yet keeps a link change in memory until
    /// its first save, so its file on disk can still say clean.
    ///
    /// `cache_root` is passed in so tests can use a temporary cache; `None` (the app
    /// dirs are not resolved) recognises no cached copy.
    fn speleodb_forget_recent_projects(
        &self,
        cache_root: Option<&std::path::Path>,
    ) -> Result<(), SpeleoErrorDto> {
        use super::speleodb_local_state::{probe_link, FileLink, LegacyRead};
        // Read before `recent_projects` is taken: it nests inside nothing.
        let open_dirty = self
            .speleo_link()
            .filter(|link| link.has_unpushed_edits)
            .and_then(|_| self.open_project_path());
        let mut recent = self
            .recent_projects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recent.retain(|path| {
            if open_dirty
                .as_deref()
                .is_some_and(|open| super::project_io::same_file(open, path))
            {
                return true;
            }
            let FileLink::Read(Some(link)) = probe_link(path, LegacyRead::ReadOnly) else {
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
    use super::super::synthetic_stalx_tests::{
        legacy_linked_project, link, write_legacy_link_only, write_v2,
    };
    use super::*;

    #[test]
    fn logout_forgets_only_clean_speleodb_references_and_preserves_files() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let state = AppState::new().expect("state");
            let local = dir.path().join("local.stalx");
            write_v2(&local, "local", None);
            let missing = dir.path().join("missing.stalx");
            let unreadable = dir.path().join("unreadable.stalx");
            std::fs::write(&unreadable, b"synthetic invalid project").expect("write");
            // A version 2 file whose header stops short: present, unreadable.
            let truncated = dir.path().join("truncated.stalx");
            std::fs::write(
                &truncated,
                br#"{"format":"stalx","version":2,"solver":1,"req"#,
            )
            .expect("write");
            let mut paths = vec![local.clone(), missing, unreadable, truncated];
            // The dirty ones still hold un-uploaded edits, the clean ones do not:
            // only a clean reference may be forgotten, in either format.
            let remote = |name: &str, dirty: bool| {
                link(
                    "https://a.example/",
                    &uuid::Uuid::new_v4().to_string(),
                    name,
                    None,
                    dirty,
                )
            };
            let v2_dirty = dir.path().join("remote-a.stalx");
            write_v2(&v2_dirty, "remote-a", Some(&remote("remote-a", true)));
            let legacy_dirty = dir.path().join("legacy-a.stalx");
            write_legacy_link_only(&legacy_dirty, &remote("legacy-a", true));
            let v2_clean = dir.path().join("remote-b.stalx");
            write_v2(&v2_clean, "remote-b", Some(&remote("remote-b", false)));
            let legacy_clean = dir.path().join("legacy-b.stalx");
            write_legacy_link_only(&legacy_clean, &remote("legacy-b", false));
            paths.extend([v2_dirty, legacy_dirty, v2_clean, legacy_clean]);
            let preserved: Vec<_> = paths.iter().map(|path| std::fs::read(path).ok()).collect();
            *state.recent_projects.lock().expect("recents") = paths.clone();
            state.config.lock().expect("config").general.recent_projects = paths.clone();
            let kept: Vec<_> = paths[..6].to_vec();
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

    /// A legacy file is upgraded only by its first save, so until then a link
    /// change (the dirty bit an edit sets) lives in memory and the SQLite file
    /// still says clean. The open project's row must be kept on the strength of
    /// its live link, or signing out would drop the way back to unsaved work.
    #[test]
    fn logout_keeps_the_open_projects_row_while_its_live_link_is_dirty() {
        crate::test_helpers::with_isolated_config(|| {
            let dir = tempfile::tempdir().expect("temp");
            let id = uuid::Uuid::new_v4().to_string();
            let path = legacy_linked_project(
                &dir.path().join("legacy.stalx"),
                Some(&link("https://a.example/", &id, "Legacy", None, false)),
            );
            let before = std::fs::read(&path).expect("bytes");
            let state = AppState::new().expect("state");
            assert!(
                state.open_project(path.clone()).success,
                "open the legacy copy"
            );
            state
                .mutate_speleo_link(|link| {
                    if let Some(link) = link.as_mut() {
                        link.has_unpushed_edits = true;
                    }
                    true
                })
                .expect("the dirty bit is kept in memory");
            assert_eq!(
                std::fs::read(&path).expect("bytes"),
                before,
                "precondition: nothing reached the legacy file"
            );
            *state.recent_projects.lock().expect("recents") = vec![path.clone()];
            state.speleodb_forget_recent_projects(None).expect("forget");
            assert_eq!(*state.recent_projects.lock().expect("recents"), vec![path]);
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
            let clean = link("https://a.example/", &id, "Synthetic", None, false);
            for path in [&cached, &stray] {
                write_v2(path, "Synthetic", Some(&clean));
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
