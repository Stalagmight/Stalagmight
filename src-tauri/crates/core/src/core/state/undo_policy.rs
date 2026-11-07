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
    pub fn speleodb_set_undo_mode(&self, mode: UndoMode) -> Result<(), SpeleoErrorDto> {
        // [#576 RVF-c-C1] `command_lock` first (it sits above `project`), held through
        // the write: a mode made for another project than the open one is refused with
        // PROJECT_CHANGED before anything is read or written, and no switch can land
        // between the check and the link row.
        let commands = self.hold_commands();
        self.speleodb_refuse_other_project("Setting the undo-history mode")?;
        // Save rewrites the whole database while holding `project`, then renames the
        // fresh file over the live one. Hold the same lock through this database edit so
        // a save cannot snapshot the old mode and overwrite a successful setting change.
        let project_guard = self.project.lock();
        // Must be a remote project: a local one has no policy to set (the fixed window is
        // unconditional there), and no link row to write it into.
        let _link = self.speleodb_validated_link()?;
        // [#576 A8] The write below is a method of the gate's proof, so it cannot be
        // moved ahead of the check (a policy written first would ride the next push).
        let gate = self.speleodb_undo_mode_gate()?;
        let db_guard = self.database.lock();
        let db = db_guard
            .as_ref()
            .ok_or_else(|| super::speleodb::local_dto(SpeleoDbError::NoRemoteProject))?;
        let now = chrono::Utc::now().to_rfc3339();
        gate.write(db, mode, &now)?;
        drop(db_guard);
        drop(project_guard);
        drop(commands);
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
    /// it must not be called while either is held. A project-slot commit arms the bound
    /// itself, from the policy its own link resolves (`commit_open_session`, through
    /// [`arm_undo_depth_limit`]).
    pub(crate) fn sync_undo_depth_limit(&self) {
        let policy = self.undo_policy();
        // Same reasoning as the commit's: skipping the arm left the stack on its previous
        // bound, and the next `enforce_depth_limit` then truncates to it.
        let mut undo = self.undo_stack.lock();
        arm_undo_depth_limit(&mut undo, policy);
    }

    /// [#469] How many persisted history rows the load paths may read for a project
    /// whose link is `link`, per stack.
    ///
    /// The mirror of the bound the commit arms ([`arm_undo_depth_limit`]), answered
    /// BEFORE the rows are read rather than after they are in memory. A count-window
    /// project reads its own window, because that is exactly what survives the restore's
    /// `enforce_depth_limit`; a Full-mode linked project has no count window, so it reads
    /// the derived ceiling [`MAX_PERSISTED_HISTORY_ROWS`], past which the byte budget would
    /// evict anyway.
    ///
    /// The window comes off the undo stack rather than the config because it is the
    /// stack's own `history_limit` that `use_count_limit` re-arms moments later; reading
    /// the config here would let the two disagree if a reload landed between them.
    /// Takes only the `undo_stack` lock, never `database`, so it is safe at a point where
    /// the live handle is mid-swap.
    pub(crate) fn history_load_row_cap(&self, link: Option<&SpeleoLink>) -> usize {
        if self.undo_policy_for(link).unlimited_depth() {
            return MAX_PERSISTED_HISTORY_ROWS;
        }
        let window = self.undo_stack.lock().history_limit();
        window.min(MAX_PERSISTED_HISTORY_ROWS)
    }

    /// The global default. Recover a poisoned guard so a panic cannot replace an
    /// explicit `Disabled` setting with [`UndoMode::default`] (`Full`).
    pub(super) fn global_undo_mode(&self) -> UndoMode {
        self.config.lock().speleodb.undo_mode.resolved()
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

/// [#303] Put `stack` on the depth bound `policy` calls for: the byte budget for Full
/// mode on a linked project, the configured entry-count window for everything else.
/// Idempotent; trims immediately if the stack already exceeds the new bound.
pub(super) fn arm_undo_depth_limit(stack: &mut crate::core::UndoStack, policy: UndoPolicy) {
    if policy.unlimited_depth() {
        stack.set_byte_budget(crate::core::undo::UNDO_BYTE_BUDGET_BYTES);
    } else {
        stack.use_count_limit();
    }
}

/// [#576 A8] The undo-mode setter's edit gate as a type: the policy write is a method
/// of [`EditSessionOpen`], made only by the check, so writing the policy before
/// checking the gate does not compile.
mod undo_mode_gate {
    use super::AppState;
    use crate::models::UndoMode;
    use crate::persistence::Database;
    use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

    /// Proof that the edit gate was open when the setter checked it.
    pub(super) struct EditSessionOpen {
        _checked: (),
    }

    impl AppState {
        /// Changing what the next push ships needs an edit session.
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn speleodb_undo_mode_gate(&self) -> Result<EditSessionOpen, SpeleoErrorDto> {
            if !self.edit_enabled() {
                return Err(super::super::speleodb::local_dto(SpeleoDbError::refused(
                    "start editing before changing the undo-history setting".to_owned(),
                )));
            }
            Ok(EditSessionOpen { _checked: () })
        }
    }

    impl EditSessionOpen {
        /// The policy, the dirty stamp and the chain clear, in one transaction.
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn write(
            &self,
            db: &Database,
            mode: UndoMode,
            now: &str,
        ) -> Result<(), SpeleoErrorDto> {
            db.set_speleo_undo_mode_and_mark_forward_edit(mode, now)
                .map_err(|e| super::super::speleodb::local_dto(SpeleoDbError::Io(e.to_string())))
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

    fn poison_mutex<T>(mutex: &crate::models::poison_policy::RecoveringMutex<T>) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock();
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

            state.config.lock().speleodb.undo_mode = UndoMode::Full;
            let policy = state.undo_policy_for(Some(&link_with(Some(UndoMode::Disabled))));
            assert_eq!(policy.mode, UndoMode::Disabled);
            assert!(!policy.pushes_history());

            state.config.lock().speleodb.undo_mode = UndoMode::Disabled;
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

            state.config.lock().speleodb.undo_mode = UndoMode::Disabled;
            link.undo_mode = Some(state.effective_undo_mode(link.undo_mode));

            state.config.lock().speleodb.undo_mode = UndoMode::Full;
            assert_eq!(state.undo_policy_for(Some(&link)).mode, UndoMode::Disabled);
        });
    }

    #[test]
    fn poisoned_config_lock_preserves_a_disabled_global_default() {
        with_isolated_config(|| {
            let state = state();
            state.config.lock().speleodb.undo_mode = UndoMode::Disabled;
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
            state.config.lock().speleodb.undo_mode = UndoMode::Full;

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
            state.config.lock().speleodb.undo_mode = UndoMode::Disabled;

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
            *state.database.lock() = Some(db);
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
            state.undo_stack.lock().set_history_limit(7);
            assert_eq!(state.history_load_row_cap(None), 7);
            assert_eq!(
                state.history_load_row_cap(Some(&link_with(Some(UndoMode::Disabled)))),
                7
            );

            // ...and never more than the ceiling, whatever the config asks for.
            state.undo_stack.lock().set_history_limit(usize::MAX);
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
    /// [#576 A8] Through BOTH load paths: `do_load_project_full` needs only an event
    /// sink, so it is driven here like `open_project` rather than pinned by its text.
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

            for load in LOAD_PATHS {
                let state = state();
                assert!(
                    load.run(&state, &path),
                    "{}: the project still opens",
                    load.name
                );
                assert!(
                    state.history_discarded(),
                    "{}: an unreadable history must be recorded, or the banner never appears",
                    load.name
                );
            }
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

    /// One production load path, as these tests drive it.
    struct LoadPath {
        name: &'static str,
        run: fn(&AppState, &std::path::Path) -> bool,
    }

    impl LoadPath {
        fn run(&self, state: &AppState, path: &std::path::Path) -> bool {
            (self.run)(state, path)
        }
    }

    /// The plain open and the progress-reporting full load. The full load refuses to
    /// run without an event sink, and needs nothing else (#576 A8).
    const LOAD_PATHS: [LoadPath; 2] = [
        LoadPath {
            name: "open_project",
            run: |state, path| state.open_project(path.to_path_buf()).success,
        },
        LoadPath {
            name: "do_load_project_full",
            run: |state, path| {
                state.set_event_sink(std::sync::Arc::new(crate::core::events::NullEventSink));
                state.do_load_project_full(path.to_path_buf()).is_ok()
            },
        },
    ];

    /// A history row whose gzip trailer declares more than any budget admits: refused
    /// on the declared size alone, so a load that FETCHES it discards the history.
    fn insert_hostile_history_row(path: &std::path::Path, table: &str, sequence: i64) {
        let mut liar = vec![0x1f, 0x8b, 0x08, 0x00];
        liar.extend_from_slice(&[0_u8; 8]);
        liar.extend_from_slice(&u32::MAX.to_le_bytes());
        let conn = rusqlite::Connection::open(path).expect("raw connection");
        conn.execute(
            &format!(
                "INSERT INTO {table} (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (?1, ?2, ?3, ?4)"
            ),
            rusqlite::params![sequence, liar.clone(), liar, "2026-08-08T00:00:00+00:00"],
        )
        .expect("insert hostile row");
    }

    /// [#469] Both load paths read the history through the row cap, resolved BEFORE the
    /// read, so the rows past it are never fetched at all - not fetched and then trimmed.
    ///
    /// [#576 A8] Behavioural, where it used to be a source scan of `project_load.rs` on
    /// the grounds that the difference is invisible. It is visible through the
    /// inflation budget: the OLDEST row of each stack declares a size no budget admits,
    /// so the history survives a load only if that row is never read. A cap of two
    /// leaves it out; the cap of three in the second pass is the positive control that
    /// the row is really there and really refused once fetched.
    #[test]
    fn both_load_paths_leave_the_rows_past_the_cap_unread() {
        with_isolated_config(|| {
            let temp_dir = TempDir::new().expect("temp dir");
            let path = temp_dir.path().join("deep-history.stalx");
            {
                let mut db = Database::open(&path).expect("open database");
                db.save_project(&crate::models::Project::new("Deep".to_owned(), None))
                    .expect("save project");
            }
            {
                // Two well-formed rows per stack (sequences 0 and 1, written the way the
                // app writes them), the newest two...
                let db = Database::open_existing(&path).expect("reopen database");
                db.save_undo_stack(vec![
                    (r#"{"c":1}"#.to_owned(), r#"{"u":1}"#.to_owned()),
                    (r#"{"c":2}"#.to_owned(), r#"{"u":2}"#.to_owned()),
                ])
                .expect("seed undo");
                db.save_redo_stack_with_timestamps(vec![
                    (r#"{"c":3}"#.to_owned(), r#"{"u":3}"#.to_owned(), None),
                    (r#"{"c":4}"#.to_owned(), r#"{"u":4}"#.to_owned(), None),
                ])
                .expect("seed redo");
            }
            // ...and one hostile row per stack, OLDER than both: past a cap of two.
            for table in ["undo_stack", "redo_stack"] {
                insert_hostile_history_row(&path, table, -1);
            }

            for load in LOAD_PATHS {
                for (cap, fetches_the_hostile_row) in [(2, false), (3, true)] {
                    let state = state();
                    state.undo_stack.lock().set_history_limit(cap);
                    assert!(load.run(&state, &path), "{}: the project opens", load.name);
                    assert_eq!(
                        state.history_discarded(),
                        fetches_the_hostile_row,
                        "{} with a row cap of {cap}: the row past the cap must never be \
                         read (a load that reads the whole table and trims afterwards has \
                         already paid for, and refused on, the row it then drops)",
                        load.name
                    );
                }
            }
        });
    }

    /// [#473] Each load path mints ONE inflation budget and hands it to both stacks.
    ///
    /// A sanctioned source scan (#576 A8-D2, docs/testing-recipes.md "Source scans"):
    /// a per-stack budget changes only how much of a hostile file reaches memory, which
    /// a test can observe only with a history big enough to fill the budget for real.
    #[test]
    fn each_load_path_mints_one_history_budget_for_both_stacks() {
        use crate::models::source_scan::{fn_body, prod_region};
        let src = prod_region(
            include_str!("project_load.rs"),
            "pub fn do_load_project_full_with(",
        );
        for signature in ["fn open_project_with(", "pub fn do_load_project_full_with("] {
            assert_eq!(
                fn_body(src, signature)
                    .matches("Database::load_history_budget()")
                    .count(),
                1,
                "RULE (#473, one history budget per open): `{signature}` must mint exactly \
                 one inflation budget and share it between the undo and redo stacks - a \
                 budget per stack doubles the ceiling every doc states"
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
    ///
    /// [#576 A8] The cut is the shared `models::source_scan::test_module_start`.
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
                let production = crate::models::source_scan::test_module_start(&text)
                    .map_or(text.as_str(), |at| &text[..at]);
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
            "RULE (sanctioned scan, #576 A8-D2 - one history budget per open, #473): \
             production code must share the open's budget through the `_within` loaders \
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
            state.config.lock().speleodb.undo_mode = UndoMode::Disabled;
            assert!(state.open_project(path).success);
            let db_guard = state.database.lock();
            let db = db_guard.as_ref().expect("open database installed");
            let link = db
                .load_speleo_link()
                .expect("load link")
                .expect("link exists");

            assert_eq!(link.undo_mode, Some(UndoMode::Disabled));
        });
    }
}
