//! [#303] Resolving the effective undo policy for the open project.
//!
//! The hybrid undo stack has ONE question at its centre: does this project's undo
//! history ride to SpeleoDB, and is its depth therefore bounded by a byte budget
//! instead of the fixed `undo_history_limit` / `undo_persist_limit` window? Every
//! trim, strip and clear site asks it here rather than re-deriving it, because the
//! answer has three inputs (is there a link, has its legacy NULL been initialized,
//! what is the global default) and three chances to get the transition wrong.

use crate::models::{SpeleoLink, UndoMode, UndoPolicy};
#[cfg(test)]
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

use super::AppState;

impl AppState {
    /// The effective [`UndoPolicy`] for the OPEN project.
    ///
    /// Reads the open project's in-memory link (`FileState`), so a project that is
    /// not linked (or not open at all) reads as local - the fixed window.
    ///
    /// Takes the `file` lock, so it must NOT be called from a site that already
    /// holds it. Such a site passes the link it already has to
    /// [`Self::undo_policy_for`] instead.
    pub(crate) fn undo_policy(&self) -> UndoPolicy {
        // `speleo_link` recovers a poisoned guard rather than falling back: `.ok()`
        // here once answered `UndoPolicy::local()`, which makes `keep_undo` false and
        // strips a Full-mode project's history from the push - silently, with no log
        // line - because some unrelated thread panicked.
        let link = self.speleo_link();
        self.undo_policy_for(link.as_ref())
    }

    /// [`Self::undo_policy`] over a link the caller has ALREADY read - for the
    /// load path, which arms the bound before the file state is committed, and for
    /// the unit tests.
    ///
    /// `None` means "no link", i.e. a local-only project, not a legacy NULL mode.
    pub(crate) fn undo_policy_for(&self, link: Option<&SpeleoLink>) -> UndoPolicy {
        let Some(link) = link else {
            return UndoPolicy::local();
        };
        UndoPolicy {
            linked: true,
            mode: self.effective_undo_mode(link.undo_mode),
        }
    }

    /// [#303] Set the open project's undo-history policy.
    ///
    /// EDIT-SESSION-REQUIRED (q17), and the reason is structural rather than cautious: this writes
    /// `speleo_link.undo_mode` in the `.stalx`, which rides to the server in the next
    /// push, so the value becomes every collaborator's. That makes it an edit of the
    /// project - it dirties the copy exactly like a survey edit, and takes effect for
    /// anyone else only once pushed - so it goes through the same gate every other edit
    /// verb does. A read-only session cannot silently change what the project ships.
    ///
    /// Re-arms the depth bound immediately, so switching to Disabled starts trimming to
    /// the count window without waiting for a reload.
    #[cfg(test)]
    pub fn speleodb_set_undo_mode(&self, mode: UndoMode) -> Result<(), SpeleoErrorDto> {
        // The checks run under `project`, so no open or close lands between them;
        // the change itself is a local-only durable save, which takes `project` on
        // its own, so the guard is dropped first.
        let project_guard = self.project.lock().map_err(|_| {
            super::speleodb::local_dto(SpeleoDbError::Io("project lock poisoned".to_owned()))
        })?;
        // Must be a remote project: a local one has no policy to set (the fixed window is
        // unconditional there), and no link row to write it into.
        let _link = self.speleodb_validated_link()?;
        if !self.edit_enabled() {
            return Err(super::speleodb::local_dto(SpeleoDbError::Io(
                "start editing before changing the undo-history setting".to_owned(),
            )));
        }
        drop(project_guard);
        // The mode, the dirty stamp and the Unwind chain clear are ONE change of
        // the file state and ONE durable save: the setting is a forward edit
        // (see `mark_remote_forward_edit`), so it invalidates an Unwind position,
        // and a half-write could leave the dirt excused by a stale pointer.
        self.with_file(|file| {
            let Some(mut link) = file.speleo_link() else {
                return;
            };
            link.undo_mode = Some(mode);
            link.has_unpushed_edits = true;
            link.last_edit_at = Some(chrono::Utc::now().to_rfc3339());
            file.set_speleo_link(Some(&link));
            file.extras.local.speleodb.unwound_from = None;
        });
        self.persist_local_state()
            .map_err(|e| super::speleodb::local_dto(SpeleoDbError::Io(e.to_string())))?;
        self.sync_undo_depth_limit();
        tracing::info!(
            mode = ?mode,
            purpose = "per-project undo-history policy (#303)",
            "Set the project's undo mode"
        );
        Ok(())
    }

    /// [#303] Re-arm the undo stack's depth bound from the current policy.
    ///
    /// Must be called wherever the policy can change: a project opening or closing, a
    /// mode flip, a config reload. Full mode on a linked project goes onto the byte
    /// budget; everything else goes back onto the configured entry-count window.
    ///
    /// Takes the `file` lock (via [`Self::undo_policy`]) and then `undo_stack`, so
    /// it must not be called while either is held. Callers that already hold a link pass
    /// it to [`Self::sync_undo_depth_limit_for`].
    pub(crate) fn sync_undo_depth_limit(&self) {
        let policy = self.undo_policy();
        self.apply_undo_depth_limit(policy);
    }

    /// [`Self::sync_undo_depth_limit`] over a link the caller has ALREADY read - for the
    /// load path, which holds the just-read file state and has to arm the bound BEFORE
    /// the persisted stack is restored into it.
    ///
    /// Takes only the `undo_stack` lock, never `file`, so it is safe at a point where
    /// the file state is mid-swap.
    pub(crate) fn sync_undo_depth_limit_for(&self, link: Option<&SpeleoLink>) {
        let policy = self.undo_policy_for(link);
        self.apply_undo_depth_limit(policy);
    }

    fn apply_undo_depth_limit(&self, policy: UndoPolicy) {
        // Same reasoning: skipping the arm left the stack on its previous bound, and the
        // `enforce_depth_limit` at the end of a load then truncates to it - the loss the
        // load path says no later re-arm can undo.
        let mut undo = self
            .undo_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if policy.unlimited_depth() {
            undo.set_byte_budget(crate::core::undo::UNDO_BYTE_BUDGET_BYTES);
        } else {
            undo.use_count_limit();
        }
    }

    /// The global default. Recover a poisoned guard so a panic cannot replace an
    /// explicit `Disabled` setting with [`UndoMode::default`] (`Full`).
    pub(super) fn global_undo_mode(&self) -> UndoMode {
        self.config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .speleodb
            .undo_mode
            .resolved()
    }

    /// An explicit project value, or the global default for a legacy NULL that has
    /// not yet been initialized. Resolved so no consumer sees [`UndoMode::Unknown`].
    pub(super) fn effective_undo_mode(&self, override_mode: Option<UndoMode>) -> UndoMode {
        match override_mode {
            Some(mode) => mode.resolved(),
            None => self.global_undo_mode(),
        }
    }

    /// The effective mode for the open project, for the status/IPC surface.
    ///
    /// Deliberately NOT `undo_policy().mode`: that reports `Full` for an unlinked
    /// project, because a local project is exempt from the hybrid rules rather than
    /// following them, and rendering that as the control's value would show a
    /// setting the global default disagrees with. This answers the narrower question
    /// the control asks - which value is in force for this project's undo history.
    pub(crate) fn undo_mode_status(&self) -> UndoMode {
        let stored_mode = self.speleo_link().and_then(|l| l.undo_mode);
        self.effective_undo_mode(stored_mode)
    }

    /// Materialize a legacy NULL undo mode using this machine's global default.
    ///
    /// NULL is only an initialization marker. Once a linked project is opened, its
    /// resolved Full/Disabled choice is written into the shared `.stalx` (`speleodb`)
    /// and no later config change can alter that project's policy.
    pub(super) fn materialize_undo_mode(&self, link: &mut SpeleoLink) -> bool {
        let mode = self.effective_undo_mode(link.undo_mode);
        if link.undo_mode == Some(mode) {
            return false;
        }
        link.undo_mode = Some(mode);
        true
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
#[expect(
    clippy::panic,
    reason = "poisoned-lock tests must panic while holding the mutex guard"
)]
mod tests {
    use super::super::speleodb_test_files::{link_on_disk, write_v2};
    use super::*;
    use crate::core::state::AppState;
    use crate::test_helpers::with_isolated_config;
    use tempfile::TempDir;

    /// `AppState::new()` loads the global config, so every test here runs under the
    /// config isolation guard - otherwise it reads (and races on) the developer's
    /// real `config.toml`, which is what `with_isolated_config` exists to prevent.
    fn state() -> AppState {
        AppState::new().expect("construct AppState")
    }

    fn link_with(mode: Option<UndoMode>) -> SpeleoLink {
        SpeleoLink {
            endpoint: "https://example.invalid".to_owned(),
            project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            project_name: "P".to_owned(),
            base_commit_sha: None,
            has_unpushed_edits: false,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: mode,
        }
    }

    fn poison_mutex<T>(mutex: &std::sync::Mutex<T>) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().expect("lock before poisoning");
            panic!("poison mutex for undo-policy coverage");
        }));
    }

    /// Spec item 27, first half: an explicit project value beats the global default in BOTH
    /// directions, so a Disabled project stays Disabled under a Full default and a
    /// Full project stays Full under a Disabled default.
    #[test]
    fn explicit_undo_mode_wins_over_the_global_default() {
        with_isolated_config(|| {
            let state = state();

            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Full;
            let policy = state.undo_policy_for(Some(&link_with(Some(UndoMode::Disabled))));
            assert_eq!(policy.mode, UndoMode::Disabled);
            assert!(!policy.pushes_history());

            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Disabled;
            let policy = state.undo_policy_for(Some(&link_with(Some(UndoMode::Full))));
            assert_eq!(policy.mode, UndoMode::Full);
            assert!(policy.pushes_history());
        });
    }

    /// Spec item 27, second half: a legacy NULL uses the global default once, then the
    /// materialized project value no longer follows later config changes.
    #[test]
    fn legacy_null_is_initialized_once() {
        with_isolated_config(|| {
            let state = state();
            let mut link = link_with(None);

            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Disabled;
            link.undo_mode = Some(state.effective_undo_mode(link.undo_mode));

            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Full;
            assert_eq!(state.undo_policy_for(Some(&link)).mode, UndoMode::Disabled);
        });
    }

    #[test]
    fn poisoned_config_lock_preserves_a_disabled_global_default() {
        with_isolated_config(|| {
            let state = state();
            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Disabled;
            poison_mutex(&state.config);

            let policy = state.undo_policy_for(Some(&link_with(None)));
            assert_eq!(policy.mode, UndoMode::Disabled);
            assert!(!policy.pushes_history());
        });
    }

    /// q4: no link, no hybrid. A local project keeps the fixed window whatever the
    /// global default says, because there is no commit for its history to chain
    /// through.
    #[test]
    fn a_project_with_no_link_is_local_whatever_the_default() {
        with_isolated_config(|| {
            let state = state();
            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Full;

            let policy = state.undo_policy_for(None);
            assert!(!policy.linked);
            assert!(!policy.unlimited_depth());
            assert!(!policy.pushes_history());
        });
    }

    /// The serde sentinel still resolves to the documented default. The file readers
    /// handle malformed stored strings separately and fail closed to `Disabled`.
    #[test]
    fn an_unknown_override_resolves_to_full() {
        with_isolated_config(|| {
            let state = state();
            let policy = state.undo_policy_for(Some(&link_with(Some(UndoMode::Unknown))));
            assert_eq!(policy.mode, UndoMode::Full);
            assert!(policy.pushes_history());
        });
    }

    /// Spec item 26: the per-project undo-mode write needs the edit gate, and refuses a
    /// project with no link outright.
    ///
    /// Not caution: the value lands in the `.stalx` and rides to the server in the next
    /// push, so it becomes every collaborator's policy. That makes it an edit of the
    /// project, and a read-only session must not be able to change what the project
    /// ships.
    #[test]
    fn setting_the_undo_mode_requires_an_edit_session() {
        with_isolated_config(|| {
            let state = state();

            // No project open: there is no link row to write the override into.
            let err = state
                .speleodb_set_undo_mode(UndoMode::Disabled)
                .expect_err("a local / absent project has no per-project policy to set");
            assert!(
                !err.message.is_empty(),
                "the refusal must carry a message the FE can surface, got {err:?}"
            );

            // ...and with the edit gate shut (a remote project opened read-only) it
            // refuses too. Asserted through the same absent-link path here - the gate
            // check sits behind `speleodb_validated_link`, and a live remote link needs
            // the cache dir the env-gated live tests initialise - so what this pins is
            // that no arm of the verb succeeds without an edit session.
            state.set_edit_enabled(false);
            assert!(
                state.speleodb_set_undo_mode(UndoMode::Full).is_err(),
                "read-only sessions cannot change what the project ships"
            );
        });
    }

    /// In an edit session the setting is an edit of the project: it dirties the
    /// link and reaches the file at once (the local-only durable save), so it
    /// rides the next push and survives a crash.
    #[test]
    fn setting_the_undo_mode_is_durable_at_once_and_dirties_the_link() {
        with_isolated_config(|| {
            let temp_dir = TempDir::new().expect("temp dir");
            let path = temp_dir.path().join("linked.stalx");
            write_v2(&path, "Linked", Some(&link_with(Some(UndoMode::Full))));
            let state = state();
            assert!(state.open_project(path.clone()).success);
            state.set_edit_enabled(true);

            state
                .speleodb_set_undo_mode(UndoMode::Disabled)
                .expect("an edit session may change the policy");

            let on_disk = link_on_disk(&path).expect("still linked");
            assert_eq!(on_disk.undo_mode, Some(UndoMode::Disabled));
            assert!(
                on_disk.has_unpushed_edits,
                "the change is an un-pushed edit"
            );
            assert_eq!(state.undo_policy().mode, UndoMode::Disabled);
        });
    }

    /// With no linked project open, status reports the application default.
    #[test]
    fn undo_mode_status_reports_the_global_value_without_a_project() {
        with_isolated_config(|| {
            let state = state();
            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Disabled;

            // No project open at all: the GLOBAL default, and not overridden. Not
            // `UndoPolicy::local()`'s Full - that answers "is this project exempt
            // from the hybrid rules", which is a different question from "what
            // value is in force", and conflating them showed the control a value
            // the global setting disagreed with.
            assert_eq!(state.undo_mode_status(), UndoMode::Disabled);
        });
    }

    #[test]
    fn poisoned_file_lock_preserves_the_project_override_status() {
        with_isolated_config(|| {
            let state = state();
            state.set_speleo_link_for_tests(Some(&link_with(Some(UndoMode::Disabled))));
            poison_mutex(&state.file);

            assert_eq!(state.undo_mode_status(), UndoMode::Disabled);
        });
    }

    /// [#473] A load that CANNOT read the history sets the flag the banner renders from.
    ///
    /// The link the whole feature hangs on, and until this test nothing asserted it: the
    /// state tests covered create-clears and close-clears, the persistence tests covered
    /// the loader returning `Err`, and the Playwright specs drive a mock. Nothing joined
    /// "the loader failed" to "the user is told".
    ///
    /// Goes through `open_project` because `do_load_project_full` needs an `AppHandle`
    /// and has no unit entry point; the structural assertions below are what keep the two
    /// paths from diverging.
    #[test]
    fn a_history_that_cannot_be_read_sets_the_discarded_flag() {
        with_isolated_config(|| {
            let temp_dir = TempDir::new().expect("temp dir");
            let path = temp_dir.path().join("broken-history.stalx");
            write_v2(&path, "Broken", None);
            // An entry that is not an undo entry at all: the file is readable, its
            // history is not (tier 3: dropped whole, never making the project
            // unopenable).
            let bytes = std::fs::read(&path).expect("bytes");
            let text = String::from_utf8(bytes).expect("utf-8");
            let text = text.replace(
                "\"end\":true}",
                "\"history\":{\"undo\":[{\"v\":1,\"t\":\"2026-08-08T00:00:00Z\",\"k\":42}]},\n\"end\":true}",
            );
            std::fs::write(&path, text).expect("write the hostile history");

            let state = state();
            assert!(state.open_project(path).success, "the project still opens");
            assert!(
                state.history_discarded(),
                "an unreadable history must be recorded, or the banner never appears"
            );
        });
    }

    /// ...and a load that CAN read it clears a flag the previous project left set.
    #[test]
    fn a_readable_history_clears_the_discarded_flag() {
        with_isolated_config(|| {
            let temp_dir = TempDir::new().expect("temp dir");
            let path = temp_dir.path().join("intact-history.stalx");
            write_v2(&path, "Intact", None);

            let state = state();
            state.set_history_discarded(true);
            assert!(state.open_project(path).success);
            assert!(
                !state.history_discarded(),
                "every commit writes the flag, so a clean load clears a stale one"
            );
        });
    }

    #[test]
    fn opening_materializes_a_legacy_null_from_the_global_default() {
        with_isolated_config(|| {
            let temp_dir = TempDir::new().expect("temp dir");
            let path = temp_dir.path().join("legacy-null.stalx");
            write_v2(&path, "Legacy", Some(&link_with(None)));

            let state = state();
            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Disabled;
            assert!(state.open_project(path.clone()).success);
            let link = state.speleo_link().expect("link exists");
            assert_eq!(link.undo_mode, Some(UndoMode::Disabled));
            assert_eq!(
                link_on_disk(&path).and_then(|link| link.undo_mode),
                Some(UndoMode::Disabled),
                "and the materialized value is durable at once"
            );
        });
    }
}
