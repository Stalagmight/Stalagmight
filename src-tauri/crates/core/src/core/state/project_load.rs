use super::session_commit::{CommandsHeld, SessionCommit, SessionHistory};
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
    ///
    /// The body of [`Self::open_project`] and [`Self::open_project_held`] [#576
    /// NF-W4C-3]: `held` is a `command_lock` span the caller already holds, and `None`
    /// takes the lock around the commit only, after the file read, as the open always
    /// has. First in this impl because the source tripwires that pin the open's
    /// history cap and its shared commit (`undo_policy`, `speleodb_session` tests)
    /// read the first open-project definition in this file.
    fn open_project_with(&self, held: Option<&CommandsHeld<'_>>, path: PathBuf) -> CommandResult {
        tracing::info!(
            path = ?path,
            purpose = "resume editing",
            "Opening project file"
        );
        // `try_exists()` rather than `exists()`: a metadata-access denial (e.g.
        // macOS folder-permission loss after an OS/app update) must not collapse to
        // a misleading "not found" - `exists()` would do exactly that. Parity with
        // `do_load_project_full` (#169 / #92 / #104). A denial carries
        // `FS_PERMISSION_DENIED`, matching the save path's `save_failure`.
        // NOTE: `open_project` is the legacy command - its frontend consumer
        // (`openProject`, projectStore.ts) does NOT strip the marker or run
        // `isPermissionError` (the load/save paths do). So this tag is currently
        // forward-compatible hardening, not live folder-access guidance; the
        // production UI opens via `loadProject` → `load_project_full`, whose
        // structured `LoadError::permission` is handled there.
        match path.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(
                    path = ?path,
                    purpose = "resume editing",
                    reason = "file not found",
                    "Project open failed"
                );
                return CommandResult::failure(format!("File not found: {}", path.display()));
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
                    return CommandResult::refused(crate::core::CommandError::new(
                        crate::models::CommandErrorCode::FsPermissionDenied,
                        format!("Cannot access {}: {e}", path.display()),
                    ));
                }
                tracing::warn!(
                    path = ?path,
                    purpose = "resume editing",
                    reason = %e,
                    "Project open failed (metadata access error)"
                );
                return CommandResult::failure(format!("File not found: {}", path.display()));
            }
        }

        // Use open_existing which validates the file is a valid Stalagmight project
        let db = match Database::open_existing(&path) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!(
                    path = ?path,
                    purpose = "resume editing",
                    reason = %e,
                    "Failed to open project database"
                );
                return CommandResult::failure(format!("{e}"));
            }
        };
        // [#576 B6-D1] A file a newer build saved is viewed, never written: see
        // the same step in `do_load_project_full`.
        let newer_schema = db.newer_schema_version();
        let db = match newer_schema {
            None => db,
            Some(_) => match db.into_read_only(&path) {
                Ok(db) => db,
                Err(e) => {
                    tracing::error!(
                        path = ?path,
                        purpose = "resume editing",
                        reason = %e,
                        "Failed to reopen a newer-version project read-only"
                    );
                    return CommandResult::failure(format!("{e}"));
                }
            },
        };

        let (mut project, load_report) = match db.load_project_with_report() {
            Ok(loaded) => loaded,
            Err(e) => {
                tracing::error!(
                    path = ?path,
                    purpose = "resume editing",
                    reason = %e,
                    "Failed to load project data"
                );
                return CommandResult::failure(format!("Failed to load project data: {e}"));
            }
        };

        project.file_path = Some(path.clone());
        // [#576 B15] Stored settings the load could not read: the project opens
        // with defaults in their place, and every save is refused while they are
        // unread (committed with the project below).
        let unread_settings: Vec<String> = load_report
            .unreadable_settings
            .iter()
            .map(|key| (*key).to_owned())
            .collect();

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
        // [#576 B6-D1] A write - skipped on a newer file, which is never written.
        let backfilled = match newer_schema {
            None => db.backfill_resolved_warning_uuids(&live_shot_uuids),
            Some(_) => Ok(0),
        };
        if let Err(e) = backfilled {
            tracing::warn!(
                error = %e,
                purpose = "upgrade legacy ignores to UUID keying",
                "open_project: failed to backfill resolved-warning UUIDs; legacy rows remain position-keyed"
            );
        }
        let healed_offsets = project.redrive_offsets_after_load();
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
        let mut speleo_link = db.load_speleo_link().ok().flatten();
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
        // [#576 B6-D1] A newer file's history is not read at all: see the same step
        // in `do_load_project_full`.
        let undo_entries = match newer_schema {
            Some(_) => Vec::new(),
            None => {
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
                }
            }
        };
        let redo_entries = match newer_schema {
            Some(_) => Vec::new(),
            None => {
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
                }
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

        // [#576 A5] ALL commits under ONE `project` acquisition, through the commit
        // every load, create and close shares - for the same reason as
        // `do_load_project_full`'s finalization (round 5): the delete's
        // check-and-teardown serializes on that mutex, and a slot commit in its own
        // span let a delete of this path land between the slot and the handles.
        let path_display = format!("{}", path.display());
        // [#303] The depth bound the entries are restored under, resolved from the link
        // read off the file's own handle rather than through `undo_policy()`, which takes
        // the `database` lock this commit is mid-swap on.
        let policy = self.undo_policy_for(speleo_link.as_ref());
        // [#576 A1] Under `command_lock`, taken only now that the file has been read,
        // so the commit cannot land inside an edit of the previous project - or under
        // the caller's span when it already holds one [#576 NF-W4C-3].
        {
            let own_commands;
            let commands = if let Some(commands) = held {
                commands
            } else {
                own_commands = self.hold_commands();
                &own_commands
            };
            self.commit_open_session(
                commands,
                SessionCommit {
                    project: Some(project),
                    database: Some(db),
                    // Opened an existing project from disk - its initial save is not deferred.
                    initial_save_deferred: false,
                    // [Track B] (#191) Read-only edit gate: remote => edit disabled, local-only
                    // => enabled (see do_load_project_full). Off the link read above, which came
                    // from the same handle before `db` moves.
                    edit_enabled: speleo_link.is_none(),
                    history_discarded,
                    // [#576 B6-D1] The read-only state of a file a newer build saved.
                    newer_file_schema: newer_schema,
                    unreadable_project_settings: unread_settings,
                    // Settings will be loaded from the DB by the frontend.
                    display_settings: Vec::new(),
                    history: SessionHistory {
                        policy,
                        inherited_through: speleo_undo_watermark,
                        undo: undo_entries,
                        redo: redo_entries,
                    },
                    recent: Some(path),
                },
            );
        }

        tracing::info!(
            path = %path_display,
            purpose = "resume editing",
            "Project opened"
        );
        CommandResult::success()
    }

    /// The legacy open (see `open_project_with` above for what it does and does not
    /// guard), taking `command_lock` for its commit only.
    pub(super) fn open_project(&self, path: PathBuf) -> CommandResult {
        self.open_project_with(None, path)
    }

    /// [`Self::open_project`] inside a `command_lock` span the caller already holds
    /// [#576 NF-W4C-3] - the Ariane adoption's, whose reopen at the cache path (and
    /// whose failure-path reopen of the project it replaced) are steps of that one
    /// span. The file read runs inside the span too, which is the point there.
    pub(super) fn open_project_held(
        &self,
        commands: &CommandsHeld<'_>,
        path: PathBuf,
    ) -> CommandResult {
        self.open_project_with(Some(commands), path)
    }

    /// Load a project with full progress reporting.
    /// This is the main implementation used by load_project_full command.
    ///
    /// All computation happens locally, then state is committed at the end.
    /// Cancellation is honored up to finalization; once finalization begins, it is
    /// non-cancellable to avoid partial AppState updates.
    ///
    /// [#576 RV4-R-M3-OPEN] `upgrade_refusal` is for a file in a place nothing may
    /// be written (the app's own folders, a system folder): it opens as anywhere
    /// else, but an older-format file - one the open would upgrade in place and
    /// copy beside itself first - is refused with the code `UPGRADE_REFUSED` and its
    /// `older_format` message, untouched; an unstamped file the open would complete
    /// in place, with its `unstamped` message (#576 RV6b-R3). `None` everywhere a
    /// user keeps projects.
    ///
    pub fn do_load_project_full_with(
        &self,
        path: PathBuf,
        upgrade_refusal: Option<&crate::persistence::UpgradeRefusal>,
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
        {
            let proj_guard = self.project.lock();
            if let Some(project) = proj_guard.as_ref() {
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
        let opened = match upgrade_refusal {
            None => Database::open_existing(&path),
            Some(refusal) => Database::open_existing_refusing_upgrade(&path, refusal),
        };
        let db = match opened {
            Ok(db) => db,
            Err(e) => {
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
                // [#576 B1-D1] A file from before the first public release: its own
                // code, and the message alone (it says what to do), not a generic
                // database failure. The file was not touched. [#576 C4] Recognised by
                // its type, not by a prefix on the message.
                if let Some(pre_release) = e.downcast_ref::<crate::persistence::PreReleaseFile>() {
                    early_return_err!(LoadError::with_details(
                        crate::models::CommandErrorCode::PreReleaseFile,
                        pre_release.to_string(),
                        path.display().to_string()
                    ));
                }
                // [#576 RV4-R-M3-OPEN] An older file in a place nothing may be
                // written: the same shape - its own code, the message alone (it says
                // to copy the project somewhere writable), the file untouched.
                // [#576 C4] Recognised by its type, not by a prefix on the message.
                if let Some(refused) = e.downcast_ref::<crate::persistence::UpgradeRefused>() {
                    early_return_err!(LoadError::with_details(
                        crate::models::CommandErrorCode::UpgradeRefused,
                        refused.to_string(),
                        path.display().to_string()
                    ));
                }
                // Detect corruption-specific errors and return CORRUPT code
                // so the frontend can offer recovery options
                if msg.contains("integrity check failed") || msg.contains("corrupted") {
                    early_return_err!(LoadError::corrupt(msg, &path.display().to_string()));
                }
                early_return_err!(LoadError::new(
                    crate::models::CommandErrorCode::Database,
                    msg
                ));
            }
        };
        // [#576 B6-D1] A file a NEWER build saved opens read-only, and is never
        // written: `open_existing` ran no migration and no undo purge on it, the
        // load-time backfills below are skipped, its history is not restored, and
        // the session holds it through a READ-ONLY connection so nothing else that
        // reaches the handle (display settings are deliberately ungated) can write
        // it either. Committed below as `newer_file_schema`, which closes the edit
        // gate and refuses every save; the frontend learns it from the result.
        let newer_schema = db.newer_schema_version();
        let db = match newer_schema {
            None => db,
            Some(_) => match db.into_read_only(&path) {
                Ok(db) => db,
                Err(e) => early_return_err!(LoadError::new(
                    crate::models::CommandErrorCode::Database,
                    e.to_string()
                )),
            },
        };
        check_cancelled!();
        progress.complete_phase(LoadingPhase::OpeningDatabase);
        // [#576 RV4-R-M3-D2] A file where nothing may be written (the app's own
        // folders, a system folder): the load writes nothing to it either. The
        // repairs below that would write - the legacy ignore-UUID backfill, the
        // unreadable-kind repair, the kind-classification persist and the undo-mode
        // materialization - are kept in memory only, as for a file a newer build
        // saved. Only the LOAD's own writes are refused: the session is an ordinary
        // writable one (RV4-R-M3-OPEN), so the next save persists them - an in-place
        // save (Ctrl+S, an autosave, the save on close) into this same file
        // included - and a display toggle writes this file at once, with no save.
        let protected = upgrade_refusal.is_some();

        // === PHASE 1: Loading project data (20%) ===
        progress.start_phase(LoadingPhase::LoadingProjectData);
        let (mut project, load_report) = match db.load_project_with_report() {
            Ok(loaded) => loaded,
            Err(e) => {
                early_return_err!(LoadError::new(
                    crate::models::CommandErrorCode::Database,
                    e.to_string()
                ));
            }
        };
        project.file_path = Some(path.clone());
        // [#576 B15] Stored settings the load could not read: the project opens
        // with defaults in their place, and every save is refused while they are
        // unread (committed with the project below).
        let unread_settings: Vec<String> = load_report
            .unreadable_settings
            .iter()
            .map(|key| (*key).to_owned())
            .collect();
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
        // [#576 B6-D1] A write - skipped on a newer file, which is never written.
        // [#576 RV4-R-M3-D2] And where nothing may be written: the legacy rows stay
        // keyed by position, which still suppresses their warnings this session
        // (the position fallback), and a save re-keys them by UUID.
        let backfilled = match newer_schema {
            None if !protected => db.backfill_resolved_warning_uuids(&live_shot_uuids),
            _ => Ok(0),
        };
        match backfilled {
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
        let healed_offsets = project.redrive_offsets_after_load();
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
        //
        // [#576 B6-D1] Not on a newer file. Its unknown kinds are most likely kinds
        // the newer build added; they are shown fail-closed as Reference all the
        // same, but rewriting them on disk is exactly the write such a file must
        // never get, and asking the user to re-pick each one would only lead to an
        // edit the read-only project refuses.
        let scanned = match newer_schema {
            None => db.unrecognised_shot_kinds(),
            Some(_) => Ok(Vec::new()),
        };
        match scanned {
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
                    //
                    // [#576 RV4-R-M3-D2] Not where nothing may be written: the
                    // substitution stays session-only there, and the user is still
                    // asked.
                    let repaired = if protected {
                        Ok(0)
                    } else {
                        db.repair_unrecognised_shot_kinds()
                    };
                    match repaired {
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

        // [#576 B6-D1] Never on a newer file: the pass writes its verdicts through.
        if newer_schema.is_none() && db.is_kind_classification_pending() {
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
            // Addressed by (section_id, position in the loaded section), which
            // `persist_backfilled_shot_kinds` resolves to the row through the
            // loader's own `ORDER BY sequence_order` - `Shot::id` is a
            // synthesised in-memory value and would hit the wrong rows. Virtual
            // shots only: `Normal` is what
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
            // The enumerate() index IS that position: nothing between the load
            // and here adds, removes or reorders a shot. It used to be
            // `shot.id % FEATURE_ID_STRIDE`, back when the loader numbered
            // shots by their stored rank; a rank gap in a legacy file made the
            // two differ, and a miss here is permanent because the flag is
            // cleared with it (B35(f), #576).
            //
            // [#576 RV4-R-M3-D2] Where nothing may be written the verdicts stay in
            // memory and the flag stays set, so the pass runs again on the next
            // open - exactly what a failed persist below leads to.
            let verdicts: Vec<_> = project
                .sections
                .iter()
                .flat_map(|section| {
                    section
                        .shots
                        .iter()
                        .enumerate()
                        .filter(|(_, shot)| shot.kind.is_virtual())
                        .map(|(position, shot)| (section.id, position as u32, shot.kind))
                })
                .collect();
            if protected {
                tracing::info!(
                    reclassified,
                    purpose = "load a project where nothing may be written (#576 RV4-R-M3-D2)",
                    "Kept the classified shot kinds in memory only"
                );
            } else if let Err(e) = db.persist_backfilled_shot_kinds(&verdicts) {
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
        //
        // [#576 B35(b) review] A link that cannot be READ still opens as "no link" - the
        // project is usable as a local one - but it is not silent: every save that would
        // rewrite this file is refused (the rebuilt file would lose the row), so the
        // load result says so and the frontend tells the user when the project opens.
        let (mut speleo_link, speleo_link_unreadable) = match db.load_speleo_link() {
            Ok(link) => (link, false),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    purpose = "preserve SpeleoDB hand-off link on save",
                    "The speleo_link table could not be read; opening as a local project \
                     whose in-place saves are refused"
                );
                (None, true)
            }
        };
        if let Some(link) = &mut speleo_link {
            if protected {
                // [#576 RV4-R-M3-D2, RV6b-R2] The legacy NULL mode is resolved for
                // this session only: writing it would rewrite the link row of a
                // file where nothing may be written.
                link.undo_mode = Some(self.effective_undo_mode(link.undo_mode));
            } else {
                self.materialize_undo_mode(&db, link);
            }
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
        // [#576 B6-D1] A newer file's history is not read at all. It stays in the
        // file for the build that wrote it, but no migration arm here has vetted
        // its payloads - the hazard the open-time purge used to cover (a forged
        // future stamp over pre-v17 entries replays into a wiped section name) -
        // and the project is read-only, so there is nothing for it to undo. Not
        // read rather than read-and-dropped: a format this build does not know can
        // fail to parse, and that must not raise the discarded-history banner.
        let undo_entries = match newer_schema {
            Some(_) => Vec::new(),
            None => {
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
                }
            }
        };
        let redo_entries = match newer_schema {
            Some(_) => Vec::new(),
            None => {
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
                }
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
        //
        // [#576 A1] From here to the end of the commit under `command_lock`, taken only
        // now that the file has been read and solved: an edit of the outgoing project
        // finishes before the old link is read and the slot is swapped, and none can
        // start in between. The detached release takes `project` only briefly and
        // never `speleodb_lock_ops` on this thread, so it is safe under it.
        let commands = self.hold_commands();
        // [#576 RV4-R-C2] Re-checked now that the lock is held, still before any state
        // write: the wait for an in-flight command can be long, and a newer load that
        // started meanwhile has cancelled this one. Committing anyway let the older
        // project land last (the mutex is not fair), leaving the backend on a project
        // the frontend had moved past.
        check_cancelled!();
        // [#576 FZ-FZ-INT-R1] The "already open" guard again, now that no commit can
        // land: it ran when this load STARTED, and a load of this same file that
        // started earlier can have committed since. Committing anyway installed a
        // second session of the open file - what this load read before that commit,
        // with a database handle on the file as it was then. A save since renames a
        // fresh file over the path, so every later write of the session went into the
        // deleted file and the next save dropped it (and the release below let go of
        // the session's own SpeleoDB lock). Answered as a load that started after the
        // commit is.
        let reopening_the_open_file = self
            .project
            .lock()
            .as_ref()
            .is_some_and(|open| open.file_path.as_ref() == Some(&path));
        if reopening_the_open_file {
            early_return_err!(LoadError::already_open());
        }
        self.speleodb_release_held_lock_detached();

        // [#576 A5] ALL commits under ONE `project` acquisition, through the commit
        // every load, create and close shares. The delete's check-and-teardown
        // (`delete_project`) takes this same mutex and reasons "strictly before the
        // load, or strictly after it"; when the slot was committed in its own span and
        // the database / gates / undo / recents in later ones, a delete of this very
        // path landing after the slot commit was NEITHER - it matched the freshly-
        // installed project, tore the half-built session down, and this function then
        // went on installing its handles into an empty world and returned success over
        // a project the backend no longer had (round 5).
        //
        // [Track B] (#191) Read-only edit gate: a remote SpeleoDB project (one carrying
        // a speleo_link row) opened WITHOUT a write lock is read-only; local-only =>
        // edit enabled. Off the link read above, taken from the same handle before
        // `db` is moved into the handle slot.
        let is_remote = speleo_link.is_some();
        // [#303] As in the non-staged load: the depth bound the entries are restored
        // under, from the link read off this file's own handle.
        let policy = self.undo_policy_for(speleo_link.as_ref());
        self.commit_open_session(
            &commands,
            SessionCommit {
                project: Some(project.clone()),
                database: Some(db),
                // Loaded an existing project from disk - its initial save is not deferred.
                initial_save_deferred: false,
                edit_enabled: !is_remote,
                // [#473] A cancelled load never reaches this finalize section, so it cannot
                // leave its verdict on the project the user actually opened.
                history_discarded,
                // [#576 B6-D1] The value handed to the frontend below, so the two gates
                // cannot disagree about this load (the #270 shape).
                newer_file_schema: newer_schema,
                unreadable_project_settings: unread_settings.clone(),
                display_settings: Vec::new(),
                history: SessionHistory {
                    policy,
                    inherited_through: speleo_undo_watermark,
                    undo: undo_entries,
                    redo: redo_entries,
                },
                recent: Some(path.clone()),
            },
        );
        drop(commands);

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
            // [#576 B6-D1] Likewise the newer-version read-only state.
            newer_schema_version: newer_schema,
            // [#576 B35(b) review] Read off the same link read the edit gate used.
            speleo_link_unreadable,
            // [#576 B15] ...and the settings whose unread state refuses saves.
            unreadable_project_settings: unread_settings,
            // [#576 B35(h)] ...and what the load reset or dropped.
            load_repairs: load_report.repairs,
        })
    }

    /// [`Self::do_load_project_full_with`] for a file where a user keeps projects:
    /// an older-format file is upgraded in place as usual.
    pub fn do_load_project_full(
        &self,
        path: PathBuf,
    ) -> Result<crate::core::loading::LoadProjectFullResult, crate::core::loading::LoadError> {
        self.do_load_project_full_with(path, None)
    }
}
