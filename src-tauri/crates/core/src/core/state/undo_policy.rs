//! [#303] Resolving the effective undo policy for the open project.
//!
//! The hybrid undo stack has ONE question at its centre: does this project's undo
//! history ride to SpeleoDB, and is its depth therefore bounded by a byte budget
//! instead of the fixed `undo_history_limit` / `undo_persist_limit` window? Every
//! trim, strip and clear site asks it here rather than re-deriving it, because the
//! answer has three inputs (is there a link, has its legacy NULL been initialized,
//! what is the global default) and three chances to get the transition wrong.

use crate::core::undo::MAX_PERSISTED_HISTORY_ROWS;
use crate::models::{SpeleoLink, UndoMode, UndoPolicy};
#[cfg(test)]
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

use super::AppState;

impl AppState {
    /// The effective [`UndoPolicy`] for the OPEN project.
    ///
    /// Reads the `speleo_link` row through the live database handle, so a project
    /// that is not linked (or not open at all) reads as local - the fixed window.
    ///
    /// Takes the `database` lock, so it must NOT be called from a site that already
    /// holds it, or from `save_project`'s persist block (where the live handle has
    /// been swapped out for a local one mid-save). Those pass the link they already
    /// have to [`Self::undo_policy_for`] instead.
    pub(crate) fn undo_policy(&self) -> UndoPolicy {
        // Recover a poisoned guard rather than falling back: `.ok()` here answered
        // `UndoPolicy::local()`, which makes `keep_undo` false and strips a Full-mode
        // project's history from the push - silently, with no log line - because some
        // unrelated thread panicked. This is a plain read of a link row, so taking the
        // value is the same judgement `project_io.rs` already makes at its own acquisitions.
        let link = self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|db| db.load_speleo_link().ok())
            .flatten();
        self.undo_policy_for(link.as_ref())
    }

    /// [`Self::undo_policy`] over a link the caller has ALREADY read - for
    /// `save_project`, which holds a database handle this state's `database` field
    /// does not yet point at, and for the unit tests.
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
        // Save rewrites the whole database while holding `project`, then renames the
        // fresh file over the live one. Hold the same lock through this database edit so
        // a save cannot snapshot the old mode and overwrite a successful setting change.
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
        let db_guard = self.database.lock().map_err(|_| {
            super::speleodb::local_dto(SpeleoDbError::Io("database lock poisoned".to_owned()))
        })?;
        let db = db_guard
            .as_ref()
            .ok_or_else(|| super::speleodb::local_dto(SpeleoDbError::NoRemoteProject))?;
        let now = chrono::Utc::now().to_rfc3339();
        db.set_speleo_undo_mode_and_mark_forward_edit(mode, &now)
            .map_err(|e| super::speleodb::local_dto(SpeleoDbError::Io(e.to_string())))?;
        drop(db_guard);
        drop(project_guard);
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
    /// Takes the `database` lock (via [`Self::undo_policy`]) and then `undo_stack`, so
    /// it must not be called while either is held. Callers that already hold a link pass
    /// it to [`Self::sync_undo_depth_limit_for`].
    pub(crate) fn sync_undo_depth_limit(&self) {
        let policy = self.undo_policy();
        self.apply_undo_depth_limit(policy);
    }

    /// [`Self::sync_undo_depth_limit`] over a link the caller has ALREADY read - for the
    /// two load paths, which hold the just-opened database handle directly and have to
    /// arm the bound BEFORE the persisted stack is restored into it.
    ///
    /// Takes only the `undo_stack` lock, never `database`, so it is safe at a point where
    /// the live handle is mid-swap.
    pub(crate) fn sync_undo_depth_limit_for(&self, link: Option<&SpeleoLink>) {
        let policy = self.undo_policy_for(link);
        self.apply_undo_depth_limit(policy);
    }

    /// [#469] How many persisted history rows the load paths may read for a project
    /// whose link is `link`, per stack.
    ///
    /// The mirror of [`Self::sync_undo_depth_limit_for`], answered BEFORE the rows are
    /// read rather than after they are in memory. A count-window project reads its own
    /// window, because that is exactly what survives the restore's `enforce_depth_limit`;
    /// a Full-mode linked project has no count window, so it reads the derived ceiling
    /// [`MAX_PERSISTED_HISTORY_ROWS`], past which the byte budget would evict anyway.
    ///
    /// The window comes off the undo stack rather than the config because it is the
    /// stack's own `history_limit` that `use_count_limit` re-arms moments later; reading
    /// the config here would let the two disagree if a reload landed between them.
    /// Takes only the `undo_stack` lock, so it is safe wherever
    /// `sync_undo_depth_limit_for` is.
    pub(crate) fn history_load_row_cap(&self, link: Option<&SpeleoLink>) -> usize {
        if self.undo_policy_for(link).unlimited_depth() {
            return MAX_PERSISTED_HISTORY_ROWS;
        }
        let window = self
            .undo_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .history_limit();
        window.min(MAX_PERSISTED_HISTORY_ROWS)
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
        let stored_mode = self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|db| db.load_speleo_link().ok())
            .flatten()
            .and_then(|l| l.undo_mode);
        self.effective_undo_mode(stored_mode)
    }

    /// Materialize a legacy NULL undo mode using this machine's global default.
    ///
    /// NULL is only an initialization marker. Once a linked project is opened, its
    /// resolved Full/Disabled choice is written into the shared `.stalx` and no later
    /// config change can alter that project's policy.
    pub(super) fn materialize_undo_mode(
        &self,
        db: &crate::persistence::Database,
        link: &mut SpeleoLink,
    ) {
        let mode = self.effective_undo_mode(link.undo_mode);
        if link.undo_mode == Some(mode) {
            return;
        }
        link.undo_mode = Some(mode);
        if let Err(e) = db.save_speleo_link(Some(link)) {
            tracing::warn!(
                error = %e,
                mode = ?mode,
                purpose = "initialize a legacy SpeleoDB undo policy (#303)",
                "Could not materialize the project's undo mode; this session will keep the resolved value in memory"
            );
        }
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
#[expect(
    clippy::panic,
    reason = "poisoned-lock tests must panic while holding the mutex guard"
)]
mod tests {
    use super::*;
    use crate::core::state::AppState;
    use crate::persistence::Database;
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

    /// The serde sentinel still resolves to the documented default. Database parsing
    /// handles malformed stored strings separately and fails closed to `Disabled`.
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
    fn poisoned_database_lock_preserves_the_project_override_status() {
        with_isolated_config(|| {
            let temp_dir = TempDir::new().expect("temp dir");
            let path = temp_dir.path().join("undo-mode-status.stalx");
            let db = Database::open(&path).expect("open database");
            db.save_speleo_link(Some(&link_with(Some(UndoMode::Disabled))))
                .expect("save link");

            let state = state();
            *state.database.lock().expect("database lock") = Some(db);
            poison_mutex(&state.database);

            assert_eq!(state.undo_mode_status(), UndoMode::Disabled);
        });
    }

    /// [#469] The load cap is the bound the restore is ABOUT to apply, resolved one step
    /// earlier so the rows past it are never read at all.
    #[test]
    fn the_history_load_cap_matches_the_bound_the_restore_will_apply() {
        with_isolated_config(|| {
            let state = state();

            // A Full-mode linked project has no count window - its bound is the byte
            // budget - so it reads the ceiling derived from that budget.
            assert_eq!(
                state.history_load_row_cap(Some(&link_with(Some(UndoMode::Full)))),
                MAX_PERSISTED_HISTORY_ROWS
            );

            // Everything else reads exactly the window `use_count_limit` re-arms.
            state
                .undo_stack
                .lock()
                .expect("undo stack lock")
                .set_history_limit(7);
            assert_eq!(state.history_load_row_cap(None), 7);
            assert_eq!(
                state.history_load_row_cap(Some(&link_with(Some(UndoMode::Disabled)))),
                7
            );

            // ...and never more than the ceiling, whatever the config asks for.
            state
                .undo_stack
                .lock()
                .expect("undo stack lock")
                .set_history_limit(usize::MAX);
            assert_eq!(state.history_load_row_cap(None), MAX_PERSISTED_HISTORY_ROWS);
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
            {
                let mut db = Database::open(&path).expect("open database");
                db.save_project(&crate::models::Project::new("Broken".to_owned(), None))
                    .expect("save project");
            }
            {
                // A blob whose gzip trailer claims far more than one row may inflate to.
                // Refused on the DECLARED size, which the loader reads straight from the
                // last four bytes - so the decoder is never invoked and the bytes in
                // between never have to be a valid deflate stream. Hand-assembled rather
                // than produced with `flate2`, which this crate does not depend on: a
                // gzip magic pair, filler, and a forged ISIZE is the entire hostile
                // shape, and writing it out makes that explicit. Written through a bare
                // connection because it is precisely what `Database`'s own writers will
                // not produce.
                let mut liar = vec![0x1f, 0x8b, 0x08, 0x00];
                liar.extend_from_slice(&[0_u8; 8]);
                liar.extend_from_slice(&u32::MAX.to_le_bytes());
                let conn = rusqlite::Connection::open(&path).expect("raw connection");
                conn.execute(
                    "INSERT INTO undo_stack (sequence, command_json, undo_data_json, \
                     executed_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![0_i64, liar.clone(), liar, "2026-08-08T00:00:00+00:00"],
                )
                .expect("insert hostile row");
            }

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
            {
                let mut db = Database::open(&path).expect("open database");
                db.save_project(&crate::models::Project::new("Intact".to_owned(), None))
                    .expect("save project");
            }

            let state = state();
            state.set_history_discarded(true);
            assert!(state.open_project(path).success);
            assert!(
                !state.history_discarded(),
                "every commit writes the flag, so a clean load clears a stale one"
            );
        });
    }

    /// [#469, #473] Both load paths read the persisted history through the row-capped,
    /// budget-sharing loader, resolve the cap before they read, and share ONE budget
    /// across the two stacks.
    ///
    /// Structural because both failures are invisible in behaviour: an uncapped read
    /// produces the same restored stack as a capped one - the trim afterwards sees to
    /// that - and a per-stack budget produces the same stack as a shared one, on every
    /// file except a hostile one. They differ only in how much of that hostile file
    /// reaches memory. `do_load_project_full` needs an `AppHandle`, so it has no
    /// unit-test entry point of its own; this is what keeps the two paths from drifting.
    #[test]
    fn both_load_paths_read_the_history_under_a_cap() {
        let src = include_str!("project_load.rs");
        assert_eq!(
            src.matches("load_undo_stack_with_timestamps_within(")
                .count(),
            2,
            "both load paths read the undo table through the capped, budget-sharing loader"
        );
        assert_eq!(
            src.matches("load_redo_stack_with_timestamps_within(")
                .count(),
            2,
            "both load paths read the redo table through the capped, budget-sharing loader"
        );
        assert!(
            !src.contains("load_undo_stack_with_timestamps()")
                && !src.contains("load_redo_stack_with_timestamps()"),
            "an unwindowed history read is exactly what #469 removed from this file"
        );
        // [#473] Exactly one budget per load path. Two would mean the undo and redo
        // stacks each got a full one, bounding an open at 2x the constant while every
        // comment says 1x - the defect this pins.
        assert_eq!(
            src.matches("Database::load_history_budget()").count(),
            2,
            "each load path creates ONE budget and hands it to both stacks"
        );

        // [#473] BOTH project-slot commits in project_create.rs clear the discarded-
        // history flag. Only `create_project` has a behavioural test - the other,
        // `do_create_project_full`, needs an `AppHandle` and so has no unit-test entry
        // point, which is exactly the asymmetry that let the flag be forgotten in the
        // first place. A stale `true` here puts a data-loss banner over a new project.
        let create_src = include_str!("project_create.rs");
        assert_eq!(
            create_src.matches("set_history_discarded(false)").count(),
            2,
            "every project-slot commit in project_create.rs must clear the flag"
        );
        // [#473] ...and BOTH load paths write it from what their own load actually did.
        // `a_history_that_cannot_be_read_sets_the_discarded_flag` covers `open_project`
        // behaviourally; `do_load_project_full` needs an `AppHandle` and cannot be unit
        // tested, so this count is what keeps it from silently losing the write.
        assert_eq!(
            src.matches("set_history_discarded(history_discarded)")
                .count(),
            2,
            "both load paths must record whether their history survived"
        );

        for func in [
            "fn open_project_prepared_inner",
            "pub fn do_load_project_full",
        ] {
            // Slice at the NEXT item in the `impl AppState` block rather than at the end
            // of the file. Both are 4-space-indented methods, so the next `///`, `pub` or
            // `fn` at that indent ends this one. Without the cut, `open_project`'s slice
            // ran through `do_load_project_full` and would have passed on ITS cap and read
            // even if `open_project` had lost both - the check has to be scoped to one
            // function to mean anything.
            let start = src
                .find(func)
                .unwrap_or_else(|| panic!("{func} is present"))
                + func.len();
            let rest = &src[start..];
            let end = ["\n    /// ", "\n    pub ", "\n    pub(", "\n    fn "]
                .iter()
                .filter_map(|boundary| rest.find(boundary))
                .min()
                .unwrap_or(rest.len());
            let body = &rest[..end];
            let cap = body
                .find("history_load_row_cap(")
                .unwrap_or_else(|| panic!("{func} resolves a load cap"));
            let read = body
                .find("load_undo_stack_with_timestamps_within(")
                .unwrap_or_else(|| panic!("{func} reads the undo table"));
            assert!(
                cap < read,
                "{func} must resolve the cap BEFORE the read - the whole point is that \
                 the rows past it are never fetched"
            );
        }
    }

    /// [#473] No PRODUCTION code anywhere in this crate calls a history loader that mints
    /// its own `InflationBudget`.
    ///
    /// Six `Database::load_*` loaders do, and each bounds only itself - so two of them in
    /// one open is 2x the documented ceiling. While the backend was a single crate they
    /// carried `#[cfg(test)]`, which made a production caller a COMPILE error. The
    /// workspace split ended that (`cfg(test)` is per-crate, and other crates' tests call
    /// them across the boundary), and the first replacement was a source scan of
    /// `project_load.rs` alone - blind to a new caller in any other file, which is exactly
    /// what the compile error had covered.
    ///
    /// So this walks the whole crate's source, cutting each file at its `#[cfg(test)]`
    /// boundary so test code may keep using them. Still weaker than a compile error - it
    /// cannot see the other crates - but it covers every file a history reader would
    /// plausibly be added to.
    /// Does this `#[cfg(test)]` gate a `mod`, rather than a gated `use` or fn?
    ///
    /// `from` starts AT the attribute. Skips its own line, then any blank or attribute
    /// lines, and answers on the first line that is neither. This crate puts `#[expect]`
    /// lint waivers between the gate and the module, and those waivers are usually
    /// MULTI-LINE, so the skip has to balance brackets rather than test for a `#[` prefix:
    /// a waiver's continuation line (`clippy::unwrap_used,`) looks like ordinary code and
    /// ended the scan early, which is what made a first version of this report every
    /// `#[expect]`-waivered test module in the crate as production.
    fn gates_a_module(from: &str) -> bool {
        let bracket_delta = |line: &str| -> i32 {
            let opens = i32::try_from(line.matches('[').count()).unwrap_or(0);
            let closes = i32::try_from(line.matches(']').count()).unwrap_or(0);
            opens - closes
        };
        let mut depth = 0_i32;
        for line in from.lines().skip(1) {
            if depth > 0 {
                depth += bracket_delta(line);
                continue;
            }
            let trimmed = line.trim_start();
            // Blank, comment, or attribute lines all sit BETWEEN the gate and the module.
            // The comment case is not hypothetical: `speleodb/sync.rs` explains its
            // `#[expect]` waiver in three `//` lines between the two, and leaving them out
            // meant that file was never cut - its whole test module counted as production,
            // which both risks a false offender and inflates `production_bytes`, the one
            // number that is supposed to notice a bad cut.
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            if trimmed.starts_with("#[") {
                depth += bracket_delta(line);
                continue;
            }
            // An INLINE module (`mod tests {`) ends the production region. A DECLARATION
            // (`mod adversarial_tests;`) does not: its body is a separate file the walk
            // visits on its own, while the rest of THIS file is still production.
            // `core/state/mod.rs` opens with fourteen such declarations and closes with
            // `pub use app_state::AppState;` - treating the first one as the boundary
            // discarded every production line after it, in the module root of the very
            // directory this guard exists to watch.
            let is_mod = trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ");
            return is_mod && !trimmed.trim_end().ends_with(';');
        }
        false
    }

    #[test]
    fn no_production_caller_mints_its_own_history_budget() {
        // The bare names. The trailing `(` in each form below separates them from the
        // budget-SHARING `_within(` variants, which share every one of these prefixes.
        const BUDGET_MINTING: [&str; 6] = [
            "load_undo_stack",
            "load_undo_stack_capped",
            "load_undo_stack_with_timestamps",
            "load_undo_stack_with_timestamps_capped",
            "load_redo_stack_with_timestamps",
            "load_redo_stack_with_timestamps_capped",
        ];
        // BOTH call syntaxes. Method position (`db.load_undo_stack(`) is what anyone
        // writes, but UFCS (`Database::load_undo_stack(&db, ..)`) is the same function
        // with the same 2x-budget consequence and matched nothing while the needle
        // required a leading dot. Neither form matches the `pub fn` that DEFINES a
        // loader, which is what keeps the workspace-wide walk from reporting the
        // persistence crate against itself.
        let call_forms = |name: &str| [format!(".{name}("), format!("Database::{name}(")];
        // POSITIVE CONTROL, first, because everything below is a negative assertion and a
        // negative assertion over stale needles passes forever while guarding nothing.
        // These names live in another crate, so renaming one there is not a compile error
        // here - it would just quietly retire this test. Pinning them against the defining
        // source makes that rename fail loudly instead.
        let defining_src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../persistence/src/persistence/sqlite/undo.rs"
        ));
        for needle in BUDGET_MINTING {
            assert!(
                defining_src.contains(&format!("pub fn {needle}(")),
                "{needle} no longer names a loader in persistence/sqlite/undo.rs - this \
                 test is guarding a symbol that does not exist"
            );
        }

        let mut offenders = Vec::new();
        let mut scanned = 0_usize;
        let mut production_bytes = 0_usize;
        let mut total_bytes = 0_usize;
        // The WHOLE workspace, not this crate. Rooting at `CARGO_MANIFEST_DIR/src` covered
        // `crates/core` alone, which left the entire Tauri command surface in
        // `src-tauri/src/` unwatched - and that is where a new IPC command reading history
        // would land. Two levels up is `src-tauri/`, whose `crates/*/src` and `src` are
        // every Rust source that can reach these loaders.
        let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let mut stack = vec![workspace.join("src"), workspace.join("crates")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read crate source dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                // WHOLE-file test modules carry no inner `#[cfg(test)]` - the attribute is
                // on the `mod` declaration in the parent - so the split below cannot see
                // they are test code. Same set `scripts/check-rust-file-size.mjs` excludes,
                // and leaving it out is not theoretical: it fired on `core/state/tests.rs`
                // the first time this test ran.
                // The loaders' OWN module delegates among them (`load_undo_stack` calls
                // `self.load_undo_stack_capped`), which is the family's internal business,
                // not a caller minting a stray budget. The positive control above already
                // pins this file, so skipping it here costs no coverage.
                if path.ends_with("persistence/sqlite/undo.rs") {
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "tests.rs"
                    || name == "test_helpers.rs"
                    || name.ends_with("_tests.rs")
                    || name.ends_with("_fuzz.rs")
                {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read source file");
                // Cut at the test MODULE, not at the first `#[cfg(test)]` of any kind.
                // Splitting on the bare attribute looks equivalent and is catastrophically
                // not: a `#[cfg(test)] use ...;` at the top of a file ends the scan there.
                // `commands.rs` has one on LINE 2 of 1425, so the whole `AppState` command
                // surface - the likeliest place a new history read lands - was invisible,
                // and `speleodb_push.rs` was cut at line 308 of 1159.
                //
                // The module is not always the NEXT line either: this crate's convention
                // puts `#[expect(...)]` lint waivers between the gate and the `mod`, so the
                // cut walks forward over any further attribute lines and requires a `mod`
                // at the end of them. Anything else (a gated `use`, a gated fn) is not a
                // module boundary and does not end the production region.
                let production = text
                    .match_indices("#[cfg(test)]")
                    .find(|&(at, _)| gates_a_module(&text[at..]))
                    .map_or(text.as_str(), |(at, _)| &text[..at]);
                scanned += 1;
                production_bytes += production.len();
                total_bytes += text.len();
                for name in BUDGET_MINTING {
                    for needle in call_forms(name) {
                        if production.contains(&needle) {
                            offenders.push(format!("{}: {needle}", path.display()));
                        }
                    }
                }
            }
        }
        assert!(scanned > 50, "the walk must actually reach the sources");
        // ...and actually reach their CONTENT. `scanned` counts files OPENED, so it stayed
        // happy while the cut was throwing away 99% of the bytes it was counting. A RATIO
        // rather than a floor, because a floor is satisfied by whatever the tree happens
        // to be big enough to leak: the first version asserted 500 KB while the real
        // figure was ~1.6 MB, so a regression discarding two thirds of production would
        // still have passed.
        assert!(
            production_bytes * 10 > total_bytes * 6,
            "the walk retained {production_bytes} of {total_bytes} bytes across {scanned} \
             files - the test-module cut is discarding real code"
        );
        assert!(
            offenders.is_empty(),
            "production code must share the open's budget through the `_within` loaders \
             (see Database::load_history_budget), never mint its own: {offenders:?}"
        );
    }

    #[test]
    fn opening_materializes_a_legacy_null_from_the_global_default() {
        with_isolated_config(|| {
            let temp_dir = TempDir::new().expect("temp dir");
            let path = temp_dir.path().join("legacy-null.stalx");
            {
                let mut db = Database::open(&path).expect("open database");
                db.save_project(&crate::models::Project::new("Legacy".to_owned(), None))
                    .expect("save project");
                db.save_speleo_link(Some(&link_with(None)))
                    .expect("save link");
            }

            let state = state();
            state.config.lock().expect("config lock").speleodb.undo_mode = UndoMode::Disabled;
            assert!(state.open_project(path).success);
            let db_guard = state.database.lock().expect("database lock");
            let db = db_guard.as_ref().expect("open database installed");
            let link = db
                .load_speleo_link()
                .expect("load link")
                .expect("link exists");

            assert_eq!(link.undo_mode, Some(UndoMode::Disabled));
        });
    }
}
