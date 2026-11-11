use super::AppState;
use crate::core::CommandResult;
use crate::persistence::Database;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

impl AppState {
    /// Legacy open, deliberately WITHOUT the canonical-storage guard that
    /// `do_load_project_full` runs (`foreign_link`): this function has no
    /// `#[tauri::command]` wrapper and is not in `generate_handler!`, so it is
    /// unreachable from the webview. Its one production caller is the recovery
    /// reopen after a failed cache replace (`replace_cache_stalx_abandoning`),
    /// which always reopens the CANONICAL cache path - refusing there would
    /// strand the shell on a project the backend no longer has open. If this is
    /// ever exposed over IPC, the guard question returns; a tripwire test pins
    /// the non-exposure (`the_legacy_open_project_is_not_exposed_over_ipc`).
    pub(super) fn open_project_prepared<F>(
        &self,
        path: PathBuf,
        expected: Option<(u64, Vec<u8>)>,
        before_commit: F,
    ) -> std::result::Result<CommandResult, crate::speleodb::SpeleoErrorDto>
    where
        F: FnOnce() -> std::result::Result<(), crate::speleodb::SpeleoErrorDto>,
    {
        self.open_project_prepared_inner(path, expected, before_commit, None)
    }

    /// Fully prepare a downloaded project before installing its bytes and state.
    #[expect(
        clippy::result_large_err,
        reason = "preserve the shared structured IPC recovery payload"
    )]
    pub(super) fn open_project_download(
        &self,
        source: PathBuf,
        dest: PathBuf,
        expected: (u64, Vec<u8>),
        editing: bool,
    ) -> std::result::Result<CommandResult, crate::speleodb::SpeleoErrorDto> {
        self.open_project_prepared_inner(dest, Some(expected), || Ok(()), Some((source, editing)))
    }

    #[expect(
        clippy::result_large_err,
        reason = "preserve the shared structured IPC recovery payload"
    )]
    fn open_project_prepared_inner<F>(
        &self,
        path: PathBuf,
        expected: Option<(u64, Vec<u8>)>,
        before_commit: F,
        download: Option<(PathBuf, bool)>,
    ) -> std::result::Result<CommandResult, crate::speleodb::SpeleoErrorDto>
    where
        F: FnOnce() -> std::result::Result<(), crate::speleodb::SpeleoErrorDto>,
    {
        let source = download
            .as_ref()
            .map_or(path.as_path(), |(source, _)| source.as_path());
        tracing::info!(
            path = ?path,
            purpose = "resume editing",
            "Opening project file"
        );
        // `try_exists()` rather than `exists()`: a metadata-access denial (e.g.
        // macOS folder-permission loss after an OS/app update) must not collapse to
        // a misleading "not found" - `exists()` would do exactly that. Parity with
        // `do_load_project_full` (#169 / #92 / #104). A denial is tagged with
        // `PERMISSION_DENIED_CODE`, matching the save path's `save_failure`.
        // NOTE: `open_project` is the legacy command - its frontend consumer
        // (`openProject`, projectStore.ts) does NOT strip the marker or run
        // `isPermissionError` (the load/save paths do). So this tag is currently
        // forward-compatible hardening, not live folder-access guidance; the
        // production UI opens via `loadProject` → `load_project_full`, whose
        // structured `LoadError::permission` is handled there.
        match source.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(
                    path = ?path,
                    purpose = "resume editing",
                    reason = "file not found",
                    "Project open failed"
                );
                return Ok(CommandResult::failure(format!(
                    "File not found: {}",
                    path.display()
                )));
            }
            Err(e) => {
                let e = anyhow::Error::from(e);
                if crate::core::loading::is_permission_denied(&e) {
                    tracing::warn!(
                        path = ?path,
                        purpose = "resume editing",
                        reason = "permission denied",
                        "Project open failed"
                    );
                    return Ok(CommandResult::failure(format!(
                        "{}: Cannot access {}: {e}",
                        crate::core::loading::PERMISSION_DENIED_CODE,
                        path.display()
                    )));
                }
                tracing::warn!(
                    path = ?path,
                    purpose = "resume editing",
                    reason = %e,
                    "Project open failed (metadata access error)"
                );
                return Ok(CommandResult::failure(format!(
                    "File not found: {}",
                    path.display()
                )));
            }
        }

        // Use open_existing which validates the file is a valid Stalagmight project
        let db = match Database::open_existing(source) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!(
                    path = ?path,
                    purpose = "resume editing",
                    reason = %e,
                    "Failed to open project database"
                );
                return Ok(CommandResult::failure(format!("{e}")));
            }
        };

        let mut project = match db.load_project() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(
                    path = ?path,
                    purpose = "resume editing",
                    reason = %e,
                    "Failed to load project data"
                );
                return Ok(CommandResult::failure(format!(
                    "Failed to load project data: {e}"
                )));
            }
        };

        project.file_path = Some(path.clone());

        // Same legacy-seeding as do_load_project_full: a project with no stored
        // `validation` row gets the user's global thresholds, not the hardcoded
        // default.
        match db.load_project_validation() {
            Ok(None) => {
                project.validation = self.get_config().validation;
            }
            Ok(Some(_)) => {}
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    purpose = "seed validation thresholds for legacy project",
                    "open_project: could not determine stored validation thresholds; keeping loaded value"
                );
            }
        }

        // Assign IDs to any legacy closures that have id=0
        project.assign_closure_ids();

        // Canonicalize the same way the production load path (do_load_project_full)
        // does: a pre-v6 file installed here otherwise keeps positional Index cross-
        // reference anchors live, and since inserts/deletes no longer shift positional
        // refs, a later edit would silently move a merge/datum/comment to the wrong
        // station - and the integrity check can't see an in-range Index ref. Mint
        // UUIDs, then re-anchor (Index→AfterShot) and re-solve offsets, so the two
        // load entry points can't diverge. (This legacy IPC has no production caller
        // today, but it stays registered and invokable, so harden it.)
        // Upgrade legacy NULL-uuid resolved-warning rows to UUID keying. Build the
        // map BEFORE minting so it contains only shots whose UUID is already
        // PERSISTED in the file: backfilling a freshly-minted, not-yet-saved UUID
        // would write it to resolved_warnings on open, and if the user then closes
        // WITHOUT saving, the next open mints a DIFFERENT UUID and orphans the
        // ignore (issue #57 / Codex). Pre-v6 shots keep position-keyed ignores
        // until a save persists their UUIDs and the save carry-over re-keys them.
        // Shared with do_load_project_full via Project::shot_uuid_by_position so the
        // two load entry points can't diverge. Idempotent; non-fatal.
        let live_shot_uuids = project.shot_uuid_by_position();
        project.ensure_shot_uuids();
        if let Err(e) = db.backfill_resolved_warning_uuids(&live_shot_uuids) {
            tracing::warn!(
                error = %e,
                purpose = "upgrade legacy ignores to UUID keying",
                "open_project: failed to backfill resolved-warning UUIDs; legacy rows remain position-keyed"
            );
        }
        let (healed_offsets, renamed_shots) = project.redrive_offsets_after_load_reporting();
        if healed_offsets > 0 {
            tracing::warn!(
                healed_offsets,
                purpose = "self-heal on open",
                "open_project: corrected drifted section offsets on load"
            );
        }
        // The #27 one-shot backfill (see `do_load_project_full`) is deliberately
        // NOT replicated here. It is the one load-time step that WRITES, and
        // this path has no remote/read-only context to gate that on. Leaving the
        // flag set costs nothing: the kinds a migrated file opens with are the
        // migration's Jump seed either way, and the next open through the real
        // load path classifies and persists them. Classifying in memory here
        // without persisting would only make the two paths disagree.

        // [#303] The link that decides how deep the stack the persisted entries land in is
        // allowed to be. Read off the same handle they are, before it is swapped, and used
        // below BEFORE the restore - see the arming call for what a late one costs.
        //
        // [#469] Read BEFORE the entries rather than after, because the depth it resolves
        // is also the bound the LOAD runs under; see `history_load_row_cap`.
        let mut speleo_link = match db.load_speleo_link() {
            Ok(link) => link,
            Err(error) if expected.is_some() => {
                return Err(super::speleodb::local_dto(
                    crate::speleodb::SpeleoDbError::Io(error.to_string()),
                ))
            }
            Err(_) => None,
        };
        if let Some(link) = &mut speleo_link {
            self.materialize_undo_mode(&db, link);
        }
        let history_row_cap = self.history_load_row_cap(speleo_link.as_ref());

        // Load undo/redo stacks from database before replacing database reference
        //
        // [#473] Either stack failing is RECORDED, not just logged: an empty stack
        // substituted for the user's real history is a silent state change, and the
        // banner this flag feeds is what stops it being silent.
        let mut history_discarded = false;
        // [#473] ONE inflation budget for this open, spanning BOTH stacks. Letting each
        // loader default to its own would bound the open at 2x the constant, because the
        // undo strings are still resident while the redo stack inflates.
        let history_budget = Database::load_history_budget();
        let undo_entries =
            match db.load_undo_stack_with_timestamps_within(history_row_cap, &history_budget) {
                Ok(loaded) => loaded
                    .into_iter()
                    .map(|(_, cmd, undo, ts)| (cmd, undo, ts))
                    .collect::<Vec<_>>(),
                Err(e) => {
                    if expected.is_some() {
                        return Err(super::speleodb::local_dto(
                            crate::speleodb::SpeleoDbError::Io(e.to_string()),
                        ));
                    }
                    tracing::warn!(
                        error = %e,
                        purpose = "restore edit history",
                        "Failed to load undo stack from database; starting with empty history"
                    );
                    history_discarded = true;
                    Vec::new()
                }
            };
        let redo_entries =
            match db.load_redo_stack_with_timestamps_within(history_row_cap, &history_budget) {
                Ok(loaded) => loaded
                    .into_iter()
                    .map(|(_, cmd, undo, ts)| (cmd, undo, ts))
                    .collect::<Vec<_>>(),
                Err(e) => {
                    if expected.is_some() {
                        return Err(super::speleodb::local_dto(
                            crate::speleodb::SpeleoDbError::Io(e.to_string()),
                        ));
                    }
                    tracing::warn!(
                        error = %e,
                        purpose = "restore edit history",
                        "Failed to load redo stack from database; starting with empty redo history"
                    );
                    history_discarded = true;
                    Vec::new()
                }
            };
        // [#303] ...and the inherited-history watermark that goes with them. Read from the
        // SAME database handle, before it is swapped, so it describes these entries.
        // `UndoStack::next_stamp` needs it to keep a new entry above the cut the push
        // applies; without it, reopening an unwound project would lose the floor while
        // keeping the rows it protects against.
        let speleo_undo_watermark = db
            .load_speleo_undo_watermark()
            .ok()
            .flatten()
            .and_then(|ts| {
                chrono::DateTime::parse_from_rfc3339(&ts)
                    .ok()
                    .map(|dt| dt.with_timezone(&chrono::Utc))
            });

        let saved_link_matches = || {
            speleo_link.as_ref().is_some_and(|saved| {
                self.speleodb_open_link().is_ok_and(|live| {
                    saved.project_id == live.project_id
                        && crate::speleodb::auth::normalize_endpoint(&saved.endpoint)
                            == crate::speleodb::auth::normalize_endpoint(&live.endpoint)
                })
            })
        };
        // A discard is consent for the exact in-memory project captured before
        // reading its saved copy. Check before HTTP and again under the commit lock.
        if let Some((generation, bytes)) = &expected {
            let current = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.speleodb_session_generation() != *generation
                || !saved_link_matches()
                || serde_json::to_vec(&*current).ok().as_deref() != Some(bytes.as_slice())
            {
                return Err(super::speleodb::local_dto(
                    crate::speleodb::SpeleoDbError::SessionMoved,
                ));
            }
        }
        before_commit()?;

        // ALL commits under ONE `project` acquisition, exactly as
        // `do_load_project_full`'s finalization and for the same reason (round
        // 5): the delete's check-and-teardown serializes on this mutex, and a
        // slot commit in its own span let a delete of this path land between
        // the slot and the handles - tearing down a half-built session that
        // this function then finished building into an empty world.
        let mut proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((generation, bytes)) = &expected {
            if self.speleodb_session_generation() != *generation
                || !saved_link_matches()
                || serde_json::to_vec(&*proj_guard).ok().as_deref() != Some(bytes.as_slice())
            {
                return Err(super::speleodb::local_dto(
                    crate::speleodb::SpeleoDbError::SessionMoved,
                ));
            }
        }
        let db = if let Some((source, _)) = &download {
            if self
                .active_loading
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
            {
                return Err(super::speleodb::local_dto(
                    crate::speleodb::SpeleoDbError::SessionMoved,
                ));
            }
            self.install_prepared_database(source, &path, db, proj_guard.as_ref())
                .map_err(super::speleodb::local_dto)?
        } else {
            db
        };
        *proj_guard = Some(project);
        // [Track B] (#191) The edit gate, re-derived for what is now open (see
        // `speleodb_edit_gate`): a local project is editable, a linked one only
        // through a bound lock claim. A downloaded reload that keeps its edit
        // session (`editing`) carries the claim bound to the generation it was
        // prepared against - and only that: a reload never starts a session, so a
        // lock released before it (Take remote) leaves the project read-only.
        let carry_from = download
            .as_ref()
            .filter(|(_, editing)| *editing)
            .and(expected.as_ref())
            .map(|(generation, _)| *generation);
        {
            // A session boundary (a project committed into the slot), bumped under
            // the guard per the `speleodb_session` rule - and under the SpeleoDB
            // session lock, in ONE step with the gate it re-derives, so a reader
            // never sees the new generation with the old binding.
            let mut session = self
                .speleodb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bump_speleodb_session();
            // Off the link read above, which came from the same handle before `db`
            // moves.
            self.speleodb_commit_open_gate(&mut session, speleo_link.as_ref(), carry_from);
        }
        // An add's pending ignores (#674) belong to the project that just left.
        self.clear_pending_ignores();
        // [#473] Under the SAME acquisition that commits the project, so the flag can
        // only ever describe the project in the slot. Unconditional, so a clean load
        // clears the previous project's verdict.
        self.set_history_discarded(history_discarded);
        *self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(db);
        // Opened an existing project from disk - its initial save is not deferred.
        self.initial_save_deferred
            .store(false, std::sync::atomic::Ordering::Relaxed);

        // Clear display settings cache (settings will be loaded from DB by frontend)
        self.clear_display_settings_cache();
        self.invalidate_adjusted_cache();

        // [#303] Arm the depth bound for the project about to be restored BEFORE its
        // entries go in, not after. `load_from_persisted_with_timestamps` ends in
        // `enforce_depth_limit`, so whichever bound is in force at that instant is the one
        // that trims - and until this call it is still the PREVIOUS project's (or the
        // startup default, `Count(undo_history_limit)`). A Full-mode linked project holding
        // more than that many entries therefore lost the oldest ones on open, the next
        // full-rewrite save wrote the truncation to disk, and the next push shipped a
        // commit missing the history it was supposed to carry. Re-arming afterwards cannot
        // undo it: Count -> Bytes has nothing to put back.
        //
        // Resolved from the link read off the file's own handle rather than through
        // `undo_policy()`, which takes the `database` lock this path is mid-swap on.
        self.sync_undo_depth_limit_for(speleo_link.as_ref());

        self.load_renamed_shots.store(
            !renamed_shots.is_empty(),
            std::sync::atomic::Ordering::Relaxed,
        );

        // Restore persisted undo/redo stacks
        if let Ok(mut undo_stack) = self.undo_stack.lock() {
            // [#303] Unconditionally, and before the entries: a project with no history
            // can still carry a watermark, and a value left over from the PREVIOUS project
            // would otherwise floor this one's stamps at an unrelated instant.
            undo_stack.set_inherited_through(speleo_undo_watermark);
            if undo_entries.is_empty() && redo_entries.is_empty() {
                undo_stack.clear();
            } else {
                undo_stack.load_from_persisted_with_timestamps(undo_entries, redo_entries);
                // ...and against the project they will be replayed onto: first
                // translated through the shot UUIDs this load renamed (the
                // entries were recorded against the old ids), then a restored
                // entry that still cannot find its shot is dropped, since it
                // would fail and be pushed straight back, sealing off
                // everything under it (#413).
                if let Some(project) = proj_guard.as_ref() {
                    undo_stack.reconcile_with_loaded_project(project, &renamed_shots);
                }
                tracing::info!(
                    undo = undo_stack.len(),
                    purpose = "restore edit history",
                    "Restored undo/redo stacks from database"
                );
            }
        }

        // No second `sync_undo_depth_limit()` after the restore: it would re-resolve the
        // SAME policy from the SAME file and re-apply the SAME bound, and the one thing it
        // could still do differently is fail its `database` lock and fall back to the count
        // window - i.e. re-introduce the truncation the arming above exists to avoid.

        // Add to recent projects and persist to config
        let path_display = format!("{}", path.display());
        if let Ok(mut recent) = self.recent_projects.lock() {
            recent.retain(|p| p != &path);
            recent.insert(0, path);
            if let Ok(mut config) = self.config.lock() {
                recent.truncate(config.general.recent_projects_max);
                // Update config with recent projects and save
                config.general.recent_projects.clone_from(&recent);
                if let Err(e) = config.save() {
                    tracing::warn!(
                        error = %e,
                        purpose = "persist recent projects list",
                        "Failed to save config with recent projects"
                    );
                }
            }
        }

        drop(proj_guard);

        tracing::info!(
            path = %path_display,
            purpose = "resume editing",
            "Project opened"
        );
        Ok(CommandResult::success())
    }

    pub(super) fn open_project(&self, path: PathBuf) -> CommandResult {
        match self.open_project_prepared(path, None, || Ok(())) {
            Ok(result) => result,
            Err(error) => CommandResult::failure(error.message),
        }
    }

    /// Load a project with full progress reporting.
    /// This is the main implementation used by load_project_full command.
    ///
    /// All computation happens locally, then state is committed at the end.
    /// Cancellation is honored up to finalization; once finalization begins, it is
    /// non-cancellable to avoid partial AppState updates.
    pub fn do_load_project_full(
        &self,
        path: PathBuf,
    ) -> Result<crate::core::loading::LoadProjectFullResult, crate::core::loading::LoadError> {
        use crate::core::loading::{
            DisplaySettings, LoadError, LoadProjectFullResult, LoadingPhase, ProgressEmitter,
        };

        let sink = self
            .get_event_sink()
            .ok_or_else(|| LoadError::internal("EventSink not initialized"))?;

        tracing::info!(
            path = ?path,
            purpose = "load project for workspace",
            "Starting full project load"
        );

        // Check if the same project is already open (avoid unnecessary reload)
        if let Ok(proj_guard) = self.project.lock() {
            if let Some(ref project) = *proj_guard {
                if project.file_path.as_ref() == Some(&path) {
                    return Err(LoadError::already_open());
                }
            }
        }

        // Start new load - cancels any existing load
        // Store the token for identity-safe clearing later
        let cancel_token = self.start_new_load();
        let mut progress = ProgressEmitter::new_for_open(sink, Arc::clone(&cancel_token));

        // Helper macro for early return with identity-safe token clearing
        macro_rules! check_cancelled {
            () => {
                if progress.is_cancelled() {
                    self.clear_active_load(&cancel_token);
                    return Err(LoadError::cancelled());
                }
            };
        }

        macro_rules! early_return_err {
            ($err:expr) => {{
                self.clear_active_load(&cancel_token);
                return Err($err);
            }};
        }

        // === PHASE 0: Opening database (5%) ===
        progress.start_phase(LoadingPhase::OpeningDatabase);
        // `try_exists()` rather than `exists()`: a metadata-access denial
        // (macOS folder permission) must surface guidance, not NOT_FOUND -
        // `exists()` would collapse that error to `false`.
        match path.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                early_return_err!(LoadError::not_found(&path.display().to_string()))
            }
            Err(e) => {
                let e = anyhow::Error::from(e);
                if crate::core::loading::is_permission_denied(&e) {
                    early_return_err!(LoadError::permission(
                        e.to_string(),
                        &path.display().to_string()
                    ));
                }
                early_return_err!(LoadError::not_found(&path.display().to_string()));
            }
        }
        // Canonical-storage guard: a `.stalx` carrying a `speleo_link` row that
        // is not the cache copy for that project is a SECOND local copy of a
        // managed project, and opening it is what makes two divergent copies
        // possible. Refuse here, in Phase 0, for three reasons: it is before ANY
        // state mutation (the Finalizing block below is where the project / db /
        // edit gate / recents commit); it is before `open_existing`, which
        // MIGRATES the file and quick_checks every page, neither of which a
        // refused file should pay or suffer; and the structured refusal lets the
        // frontend offer the two real choices (open the canonical copy, or drop
        // the link) instead of a generic load error. It sits after the
        // already-open guard above, so a project already on screen is never
        // refused. See `speleodb_foreign` for the probe's degradation rules.
        //
        // The probe can also ACCEPT the file under a different spelling: the
        // cache copy reached through a symlink is the canonical copy to read,
        // but saving through that name would rename a fresh file over the link
        // entry and strand the real cached copy. The load continues with the
        // canonical cache path in that case, so `project.file_path` - and every
        // save that follows from it - names the real file.
        use super::speleodb_foreign::LinkedSpelling;
        let path = match self.foreign_link(&path) {
            LinkedSpelling::AsSpelled => path,
            LinkedSpelling::Canonical(canonical) => {
                tracing::info!(
                    spelled = ?path,
                    canonical = ?canonical,
                    purpose = "keep one local copy per SpeleoDB project",
                    "Opening the canonical cache copy behind a symlink to it"
                );
                canonical
            }
            LinkedSpelling::Foreign(managed) => {
                tracing::info!(
                    path = ?path,
                    project_id = %managed.project_id,
                    purpose = "keep one local copy per SpeleoDB project",
                    "Refused to open a non-canonical copy of a SpeleoDB project"
                );
                early_return_err!(LoadError::speleodb_managed(managed));
            }
        };
        // Local variable only - NO state mutation until Finalizing
        let db = match Database::open_existing(&path) {
            Ok(db) => db,
            Err(e) => {
                // A hostile schema is refused before anything reads the file (#608);
                // say so plainly rather than as a generic database error.
                if let Some(refused) = crate::core::loading::refused_schema(&e) {
                    tracing::warn!(
                        path = ?path,
                        what = refused.what,
                        purpose = "never open a file whose schema could run its own SQL",
                        "Refused to open a file with a hostile schema"
                    );
                    early_return_err!(LoadError::unsafe_file(refused, &path.display().to_string()));
                }
                // A newer Stalagmight's file is refused before anything writes to
                // it (#649); the remedy is an update, not a repair.
                if let Some(newer) = crate::core::loading::newer_schema(&e) {
                    tracing::warn!(
                        path = ?path,
                        found = newer.found,
                        supported = newer.supported,
                        purpose = "never rewrite a file a newer schema saved",
                        "Refused to open a project saved by a newer version"
                    );
                    early_return_err!(LoadError::newer_schema(newer, &path.display().to_string()));
                }
                let msg = e.to_string();
                // A permission denial (e.g. macOS forgot folder access after an
                // update) otherwise masquerades as corruption - check it first.
                // SQLite says only "unable to open database file" for a denial,
                // so that message is corroborated by re-opening the file
                // ourselves: it is a denial when the OS refuses US too, and an
                // ordinary database error otherwise (#429).
                if crate::core::loading::is_permission_denied(&e)
                    || (crate::core::loading::mentions_unopenable_database(&e)
                        && crate::core::loading::read_access_denied(&path))
                {
                    early_return_err!(LoadError::permission(msg, &path.display().to_string()));
                }
                // Detect corruption-specific errors and return CORRUPT code
                // so the frontend can offer recovery options
                if msg.contains("integrity check failed") || msg.contains("corrupted") {
                    early_return_err!(LoadError::corrupt(msg, &path.display().to_string()));
                }
                early_return_err!(LoadError::new("DATABASE", msg));
            }
        };
        check_cancelled!();
        progress.complete_phase(LoadingPhase::OpeningDatabase);

        // === PHASE 1: Loading project data (20%) ===
        progress.start_phase(LoadingPhase::LoadingProjectData);
        let mut project = match db.load_project() {
            Ok(p) => p,
            Err(e) => {
                early_return_err!(LoadError::new("DATABASE", e.to_string()));
            }
        };
        project.file_path = Some(path.clone());
        // Legacy projects (saved before #57) have no persisted `validation` row;
        // load_project() filled `project.validation` with the hardcoded default.
        // Override it with the user's GLOBAL thresholds (the "defaults for new
        // projects"), matching create_project/merge. The choice persists on the
        // next save (save_project writes the row), so a project is only treated
        // as legacy until its first save under this build.
        match db.load_project_validation() {
            Ok(None) => {
                project.validation = self.get_config().validation;
            }
            Ok(Some(_)) => {} // explicit row present - honor it as-is
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    purpose = "seed validation thresholds for legacy project",
                    "Could not determine whether project has stored validation thresholds; keeping loaded value"
                );
            }
        }
        // Upgrade legacy NULL-uuid resolved-warning rows to UUID keying. Build the
        // map BEFORE minting so it only contains shots whose UUID is already
        // PERSISTED: a freshly-minted, not-yet-saved UUID written into
        // resolved_warnings on open would be orphaned if the user closes without
        // saving (the next open mints a different UUID) - issue #57 / Codex. Pre-v6
        // shots keep position-keyed ignores until a save persists their UUIDs and
        // the carry-over re-keys them. Shared with open_project. Idempotent; non-fatal.
        let live_shot_uuids = project.shot_uuid_by_position();
        // Guarantee every shot has a stable UUID at runtime, not just after the
        // next save: pre-v6 files load shots with uuid=None, and station identity
        // anchors on the shot UUID. The
        // minted UUIDs persist on the next save (matching save_project's call).
        project.ensure_shot_uuids();
        match db.backfill_resolved_warning_uuids(&live_shot_uuids) {
            Ok(n) if n > 0 => tracing::info!(
                upgraded = n,
                purpose = "upgrade legacy ignores to UUID keying",
                "Backfilled resolved-warning UUIDs on load"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(
                error = %e,
                purpose = "upgrade legacy ignores to UUID keying",
                "Failed to backfill resolved-warning UUIDs; legacy rows remain position-keyed"
            ),
        }
        // Assign IDs to any legacy closures that have id=0
        project.assign_closure_ids();
        // Section offsets are derived state: re-solve them from the merge/datum
        // constraints so a file persisted with drifted offsets (e.g. from an
        // older redo bug) opens geometrically correct, before stations are
        // computed below. No-op for an already-consistent file.
        let (healed_offsets, renamed_shots) = project.redrive_offsets_after_load_reporting();
        if healed_offsets > 0 {
            tracing::warn!(
                healed = healed_offsets,
                purpose = "self-heal derived geometry on load",
                "Re-derived drifted section offsets while opening project"
            );
        }
        // One-shot #27 backfill. The v12->v13 migration seeded every legacy
        // `is_reference` row as Jump, which is only q2's guess; the real verdict
        // needs sections + merges + closures assembled, which is exactly what we
        // have now and the bare migration connection did not. Runs once, then
        // clears its own flag: classification is import-time only (q14), so a
        // later merge must never silently re-flip a kind.
        // Record any shot whose stored `kind` this build cannot read (#27). The
        // loader already substituted `Reference` for them via `from_u8` - the
        // fail-closed choice, so a corrupt or future byte cannot invent a
        // swimmable leg - but that substitution is a guess, not a verdict. Stash
        // them so the frontend can ask the user what each one really is.
        match db.unrecognised_shot_kinds() {
            Ok(found) => {
                if !found.is_empty() {
                    tracing::warn!(
                        count = found.len(),
                        purpose = "surface unreadable shot kinds for user resolution (#27)",
                        "Found shots with an unrecognised kind; defaulted them to Reference"
                    );
                    // Make the disk agree with the value already in force in
                    // memory. Otherwise the substitution is session-only: the
                    // project is never dirty (nothing was edited), so nothing
                    // saves it, and the prompt returns on every open. This
                    // writes the fail-closed value, not a choice taken from the
                    // user - picking anything else stays an ordinary undoable
                    // SetShotKind on top.
                    match db.repair_unrecognised_shot_kinds() {
                        Ok(n) if n > 0 => tracing::info!(
                            repaired = n,
                            purpose = "persist the fail-closed substitution (#27)",
                            "Rewrote unreadable shot kinds to Reference on disk"
                        ),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(
                            error = %e,
                            "Failed to persist the Reference substitution; it stays session-only"
                        ),
                    }
                }
                self.set_unrecognised_shot_kinds(progress.loading_id(), found);
            }
            Err(e) => {
                // Non-fatal: the shots are already safely Reference in memory.
                // Losing the prompt is better than failing the load.
                tracing::warn!(error = %e, "Failed to scan for unrecognised shot kinds");
                self.set_unrecognised_shot_kinds(progress.loading_id(), Vec::new());
            }
        }

        if db.is_kind_classification_pending() {
            let reclassified =
                project.classify_virtual_shots(&crate::models::ClassifyScope::WholeProject);
            tracing::info!(
                reclassified,
                purpose = "one-shot #27 Jump/Reference backfill after v12->v13 migration",
                "Classified virtual shots on first open after migration"
            );
            // Write the verdicts through and clear the flag together. The pass
            // only touched the in-memory project, so clearing the flag on its
            // own would burn the one retry on a result that dies with the
            // process if the user closes without saving - and the next open
            // would then keep the migration's Jump seed for good.
            //
            // Addressed by (section_id, sequence_order), which is what the rows
            // are keyed by on disk - `Shot::id` is a synthesised in-memory value
            // and would hit the wrong rows. Virtual shots only: `Normal` is what
            // the migration already wrote for every non-legacy-reference row, so
            // re-stamping them would be one UPDATE per shot in the project on a
            // load that is already paying for the schema migration.
            //
            // Runs for a remote project too, deliberately. Gating it on
            // `load_speleo_link()` looks safer - `shots.kind` rides in the push
            // snapshot, so writing on a lock-free open diverges the local .stalx
            // from BASE - but that predicate is true whether or not the user
            // holds the lock, so the flag would never clear and this pass would
            // re-run on EVERY open. Being project-wide, it cannot tell the
            // migration's Jump seed from a kind the user set by hand, so each
            // reopen would silently revert their choice and restore exactly the
            // unsafe shortcut #27 exists to remove. A one-time local write whose
            // values are what the classifier is meant to land - and which the
            // v12->v13 migration has already rewritten this file's schema
            // without a lock to produce - is the smaller hazard by a wide margin.
            //
            // `shot.id % FEATURE_ID_STRIDE`, not the enumerate() index: the
            // loader reads `ORDER BY section_id, sequence_order` and pushes
            // sequentially, so the index is the shot's RANK, while the id
            // carries the raw stored `sequence_order` (read.rs synthesises
            // `section_id * FEATURE_ID_STRIDE + seq` from the column). They
            // differ the moment a legacy file has a gap in the sequence, and
            // a miss here is permanent because the flag is cleared with it.
            let verdicts: Vec<_> = project
                .sections
                .iter()
                .flat_map(|section| {
                    section
                        .shots
                        .iter()
                        .filter(|shot| shot.kind.is_virtual())
                        .map(|shot| {
                            (
                                section.id,
                                shot.id % crate::models::FEATURE_ID_STRIDE,
                                shot.kind,
                            )
                        })
                })
                .collect();
            if let Err(e) = db.persist_backfilled_shot_kinds(&verdicts) {
                // Non-fatal: the flag is cleared in the same transaction as
                // the writes, so a failure here leaves both undone and the
                // (idempotent) backfill simply runs again on the next open.
                tracing::warn!(
                    error = %e,
                    purpose = "one-shot #27 Jump/Reference backfill after v12->v13 migration",
                    "Failed to persist backfilled shot kinds; will retry on next open"
                );
            }
        }

        progress.update_phase(LoadingPhase::LoadingProjectData, 1, 1);
        check_cancelled!();
        progress.complete_phase(LoadingPhase::LoadingProjectData);

        // === PHASE 2: Loading sections (15%) ===
        progress.start_phase(LoadingPhase::LoadingSections);
        let total_sections = project.sections.len() as u32;
        for (i, _section) in project.sections.iter().enumerate() {
            check_cancelled!();
            progress.update_phase(LoadingPhase::LoadingSections, i as u32 + 1, total_sections);
        }
        progress.complete_phase(LoadingPhase::LoadingSections);

        // === PHASE 3: Computing stations (20%) ===
        progress.start_phase(LoadingPhase::ComputingStations);
        // Pre-compute the expected station total so the UI can show a real
        // progress bar from the start of this phase. Otherwise the first
        // ~second of the phase (build_colocated_groups + station_depths)
        // shows an indeterminate spinner - the per-station callback only
        // fires once the main walk begins.
        let expected_station_total: u32 = project
            .sections
            .iter()
            .map(|s| s.shots.len() as u32 + 1)
            .sum();
        progress.update_phase(LoadingPhase::ComputingStations, 0, expected_station_total);
        let stations =
            project.compute_station_coordinates_with_progress(Some(&mut |current, total| {
                if !progress.is_cancelled() {
                    progress.update_phase(LoadingPhase::ComputingStations, current, total);
                }
            }));
        check_cancelled!();
        progress.complete_phase(LoadingPhase::ComputingStations);

        // === PHASE 4: Computing statistics (15%) - INDETERMINATE ===
        progress.start_phase(LoadingPhase::ComputingStatistics);
        // Reference ties are not swimmable, so penetration excludes them unless
        // the user opts them back in (#27 q13).
        let include_reference_legs = self.get_config().gas.include_reference_legs;
        // Heartbeat at start to indicate phase is active
        progress.heartbeat(LoadingPhase::ComputingStatistics);
        let mut stats = project.stats();
        // Calculate penetration distance with computed stations
        stats.penetration_distance =
            project.calculate_penetration_with(&stations, include_reference_legs);
        // Depth is the absolute station range, not a per-section delta walk (#481).
        let (min_depth, max_depth) = crate::models::Project::depth_range(&stations);
        stats.min_depth = min_depth;
        stats.max_depth = max_depth;
        // Heartbeat after computation to indicate work completed
        progress.heartbeat(LoadingPhase::ComputingStatistics);
        check_cancelled!();
        progress.complete_phase(LoadingPhase::ComputingStatistics);

        // === PHASE 5: Validating data (10%) ===
        // Compute warnings using the local db reference (before finalization)
        progress.start_phase(LoadingPhase::ValidatingData);
        let warnings = self.compute_project_warnings(&project, Some(&db));

        // Surface corruption present on disk (older versions / external edits /
        // a missed offset cascade): both dangling references and drifted merges.
        // The frontend re-checks after load and offers a repair; here we just
        // record it. (#17, merge-misalignment)
        let mut integrity = project.check_structural_integrity();
        integrity.extend(project.check_merge_alignment());
        if !integrity.is_empty() {
            tracing::warn!(
                count = integrity.len(),
                purpose = "flag corruption on load",
                "Loaded project has integrity violations"
            );
            for violation in &integrity {
                tracing::warn!(category = %violation.category, "{}", violation.message);
            }
        }

        progress.heartbeat(LoadingPhase::ValidatingData);
        check_cancelled!();
        progress.complete_phase(LoadingPhase::ValidatingData);

        // === PHASE 6: Loading settings (5%) ===
        progress.start_phase(LoadingPhase::LoadingSettings);
        let stored_settings = db.load_display_settings().unwrap_or_default();
        let display_settings = DisplaySettings::from_stored(&stored_settings);
        check_cancelled!();
        progress.complete_phase(LoadingPhase::LoadingSettings);

        // === PHASE 7: Finalizing (10%) - NON-CANCELLABLE STATE COMMIT ===
        progress.start_phase(LoadingPhase::Finalizing);
        // Allow cancellation only before any state mutation occurs.
        check_cancelled!();

        // [#303] The link that bounds the stack the persisted entries land in, off the
        // handle they are read from and before it is swapped. Used before the restore
        // below - see the arming call. This is the INCOMING file's link; the detached
        // release further down reads the OUTGOING project's through the still-live handle,
        // and the two are not interchangeable.
        //
        // [#469] Read BEFORE the entries rather than after, because the depth it resolves
        // is also the bound the LOAD runs under; see `history_load_row_cap`.
        let mut speleo_link = db.load_speleo_link().ok().flatten();
        if let Some(link) = &mut speleo_link {
            self.materialize_undo_mode(&db, link);
        }
        let history_row_cap = self.history_load_row_cap(speleo_link.as_ref());

        // Load undo/redo stacks from database BEFORE storing db
        //
        // [#473] Recorded as well as logged - see the same block in `open_project`.
        let mut history_discarded = false;
        // [#473] ONE inflation budget for this open, spanning BOTH stacks. Letting each
        // loader default to its own would bound the open at 2x the constant, because the
        // undo strings are still resident while the redo stack inflates.
        let history_budget = Database::load_history_budget();
        let undo_entries =
            match db.load_undo_stack_with_timestamps_within(history_row_cap, &history_budget) {
                Ok(loaded) => loaded
                    .into_iter()
                    .map(|(_, cmd, undo, ts)| (cmd, undo, ts))
                    .collect::<Vec<_>>(),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        purpose = "restore edit history",
                        "Failed to load undo stack from database; starting with empty history"
                    );
                    history_discarded = true;
                    Vec::new()
                }
            };
        let redo_entries =
            match db.load_redo_stack_with_timestamps_within(history_row_cap, &history_budget) {
                Ok(loaded) => loaded
                    .into_iter()
                    .map(|(_, cmd, undo, ts)| (cmd, undo, ts))
                    .collect::<Vec<_>>(),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        purpose = "restore edit history",
                        "Failed to load redo stack from database; starting with empty redo history"
                    );
                    history_discarded = true;
                    Vec::new()
                }
            };
        // [#303] ...and the inherited-history watermark that goes with them. Read from the
        // SAME database handle, before it is swapped, so it describes these entries.
        // `UndoStack::next_stamp` needs it to keep a new entry above the cut the push
        // applies; without it, reopening an unwound project would lose the floor while
        // keeping the rows it protects against.
        let speleo_undo_watermark = db
            .load_speleo_undo_watermark()
            .ok()
            .flatten()
            .and_then(|ts| {
                chrono::DateTime::parse_from_rfc3339(&ts)
                    .ok()
                    .map(|dt| dt.with_timezone(&chrono::Utc))
            });

        // All finalization commits use unwrap_or_else(PoisonError::into_inner) to ensure
        // data is written even if a previous panic poisoned a mutex. This is acceptable
        // during finalization because the alternative (silently skipping) leads to
        // inconsistent state that is harder to diagnose.

        // [Track B] (fix M1) Swapping to a DIFFERENT project while still holding the
        // OLD project's SpeleoDB write lock must release that lock, or it stays locked
        // for every other editor until the app quits - `close_project`/quit release,
        // but a direct open (Ctrl+O, Open-from-SpeleoDB, recents, discard-and-open)
        // routes here without a close. Done in this non-cancellable finalize section
        // (so a failed/cancelled load never releases), reading the OLD link BEFORE the
        // database/gate are swapped below, and detached so it can't stall opening the
        // new project on the old server. A same-project fast-forward reload doesn't
        // reach here (it `close_project`s first, clearing the link) and the "already
        // open" guard rejects reopening the same path, so neither releases our lock.
        self.speleodb_release_held_lock_detached();

        // ALL commits under ONE `project` acquisition. The delete's
        // check-and-teardown (`delete_project`) takes this same mutex and
        // reasons "strictly before the load, or strictly after it"; when the
        // slot was committed in its own span and the database / gates / undo /
        // recents in later ones, a delete of this very path landing after the
        // slot commit was NEITHER - it matched the freshly-installed project,
        // tore the half-built session down, and this function then went on
        // installing its handles into an empty world and returned success over
        // a project the backend no longer had (round 5). One guard makes the
        // finalization the atom that argument needs. The guard is held over
        // the recents/config write too - the delete prunes recents only after
        // its own teardown, so the row this adds must not land between the two.
        // Lock order is the documented one: `project` is outermost of
        // {project, database, undo_stack}, and recents/config are leaves.
        let mut proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Commit 1: Project state mutation
        *proj_guard = Some(project.clone());
        // [Track B] (#191) The edit gate, re-derived for what is now open (see
        // `speleodb_edit_gate`): a project carrying a speleo_link row is a
        // SpeleoDB project, editable only once Start Editing binds a lock claim to
        // this session; a local-only project is always editable. Off the link read
        // above, taken from the same handle before `db` is moved into the handle
        // slot. A fresh open carries no edit session: whatever claim this account
        // still holds on the project stays a pointer, never an open gate.
        let is_remote = speleo_link.is_some();
        {
            // A session boundary (a project committed into the slot), bumped under
            // the guard per the `speleodb_session` rule, in one step with the gate.
            let mut session = self
                .speleodb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bump_speleodb_session();
            self.speleodb_commit_open_gate(&mut session, speleo_link.as_ref(), None);
        }
        // An add's pending ignores (#674) belong to the project that just left.
        self.clear_pending_ignores();
        // [#473] Under the SAME acquisition that commits the project - a cancelled load
        // never reaches this finalize section, so it cannot leave its verdict on the
        // project the user actually opened.
        self.set_history_discarded(history_discarded);

        // Commit 2: Database handle mutation
        *self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(db);
        // Loaded an existing project from disk - its initial save is not deferred.
        self.initial_save_deferred
            .store(false, std::sync::atomic::Ordering::Relaxed);

        // Commit 3: Display settings cache clear
        self.clear_display_settings_cache();
        self.invalidate_adjusted_cache();

        // [#303] As in the non-staged load: arm the depth bound BEFORE the entries are
        // restored under it, from the link read off this file's own handle. Late, the
        // restore's own `enforce_depth_limit` would trim a Full-mode project's history
        // down to the count window and no later re-arm could put it back.
        self.sync_undo_depth_limit_for(speleo_link.as_ref());

        self.load_renamed_shots.store(
            !renamed_shots.is_empty(),
            std::sync::atomic::Ordering::Relaxed,
        );

        // Commit 4: Undo/redo stack restore
        {
            let mut undo_stack = self
                .undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // [#303] Unconditionally, and before the entries: a project with no history
            // can still carry a watermark, and a value left over from the PREVIOUS project
            // would otherwise floor this one's stamps at an unrelated instant.
            undo_stack.set_inherited_through(speleo_undo_watermark);
            if undo_entries.is_empty() && redo_entries.is_empty() {
                undo_stack.clear();
            } else {
                undo_stack.load_from_persisted_with_timestamps(undo_entries, redo_entries);
                // ...and against the project they will be replayed onto: first
                // translated through the shot UUIDs this load renamed, then an
                // entry that still cannot find its shot is dropped (#413).
                undo_stack.reconcile_with_loaded_project(&project, &renamed_shots);
                tracing::info!(
                    undo = undo_stack.len(),
                    purpose = "restore edit history",
                    "Restored undo/redo stacks from database"
                );
            }
        }

        // Commit 5: Recent projects update
        {
            let mut recent = self
                .recent_projects
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            recent.retain(|p| p != &path);
            recent.insert(0, path.clone());
            if let Ok(mut config) = self.config.lock() {
                recent.truncate(config.general.recent_projects_max);
                config.general.recent_projects.clone_from(&recent);
                if let Err(e) = config.save() {
                    tracing::warn!(
                        error = %e,
                        purpose = "persist recent projects list",
                        "Failed to save config with recent projects"
                    );
                }
            }
        }

        drop(proj_guard);

        progress.complete_phase(LoadingPhase::Finalizing);

        // Get loading_id before clearing token
        let loading_id = progress.loading_id();

        // Clear active load AFTER all mutations and computations complete
        self.clear_active_load(&cancel_token);

        tracing::info!(
            path = ?path,
            purpose = "load project for workspace",
            "Project loaded via load_project_full"
        );

        Ok(LoadProjectFullResult {
            loading_id,
            project,
            stations,
            stats,
            warnings,
            display_settings,
            // Same verdict the edit gate above was derived from, so the frontend
            // gate and the backend gate can never disagree about this load (#270).
            is_remote,
        })
    }
}
