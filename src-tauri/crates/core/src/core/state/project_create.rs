use super::command_epilogue::EpilogueSteps;
use super::session_commit::{CommandsHeld, SessionCommit};
use super::AppState;
use crate::core::CommandResult;
use crate::models::Project;
use crate::persistence::Database;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

impl AppState {
    /// Resolve the project file path from a name and optional directory.
    /// Sanitizes the name and ensures .stalx extension.
    ///
    /// # Arguments
    /// * `name` - Project name (will be sanitized for filesystem use)
    /// * `directory` - Optional directory to save in. If None, uses user's
    ///   documents `stalagmight` subdirectory.
    ///
    /// # Returns
    /// The full path to the project file (e.g., `/path/to/dir/ProjectName.stalx`)
    fn resolve_project_path(
        &self,
        name: &str,
        directory: Option<&PathBuf>,
    ) -> Result<PathBuf, String> {
        // Validate name is not empty
        if name.trim().is_empty() {
            return Err("Project name cannot be empty".to_string());
        }

        // Sanitize the project name for filesystem use
        let sanitized_name: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();

        // Remove leading/trailing whitespace and ensure not empty after sanitization
        let sanitized_name = sanitized_name.trim();
        if sanitized_name.is_empty() {
            return Err(
                "Project name must contain at least one alphanumeric character".to_string(),
            );
        }

        // Determine the directory to use
        let dir = if let Some(d) = directory {
            // Validate the provided directory is a directory (or doesn't exist yet)
            if d.exists() && !d.is_dir() {
                return Err(format!("Path is not a directory: {}", d.display()));
            }
            d.clone()
        } else {
            // Default to user's documents/stalagmight directory, with fallbacks.
            // 1. app_dirs (Tauri path resolver - works on Android)
            // 2. directories crate (desktop pre-setup fallback)
            // 3. $HOME/Documents
            let documents_dir = crate::app_dirs::document_dir()
                .map(std::path::Path::to_path_buf)
                .or_else(|| {
                    directories::UserDirs::new()
                        .and_then(|ud| ud.document_dir().map(std::path::Path::to_path_buf))
                })
                .or_else(|| {
                    std::env::var("HOME")
                        .ok()
                        .map(|h| PathBuf::from(h).join("Documents"))
                })
                .ok_or_else(|| "Could not determine documents directory".to_string())?;

            let app_dir = documents_dir.join("stalagmight");

            // Ensure the app default directory exists.
            if !app_dir.exists() {
                std::fs::create_dir_all(&app_dir)
                    .map_err(|e| format!("Failed to create default project directory: {e}"))?;
            }

            app_dir
        };

        // Build the full path with .stalx extension
        let filename = format!("{sanitized_name}.stalx");
        let full_path = dir.join(filename);

        Ok(full_path)
    }

    /// Return an available project path by appending `_N` before the extension when needed.
    ///
    /// Examples:
    /// - `Cave.stalx` -> `Cave_1.stalx` (if `Cave.stalx` exists)
    /// - `Cave_1.stalx` -> `Cave_2.stalx` (if both previous files exist)
    ///
    /// [#576 RV13-1] A link at a name is a name taken, even one that leads nowhere
    /// yet: `symlink_metadata`, not `exists()`/`metadata()`, which follow it - a
    /// dangling link was "free" and a link to an empty file a placeholder, and the
    /// create then made or wrote the link's target, wherever it points.
    pub(super) fn next_available_project_path(path: &PathBuf) -> PathBuf {
        let taken = |candidate: &std::path::Path| candidate.symlink_metadata().is_ok();
        if !taken(path) {
            return path.clone();
        }
        // Treat 0-byte files as non-existent: Android's SAF (ACTION_CREATE_DOCUMENT)
        // creates an empty placeholder at the chosen path before returning.
        if path.symlink_metadata().is_ok_and(|m| m.len() == 0) {
            return path.clone();
        }

        let parent = path.parent().map(std::path::Path::to_path_buf);
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "project".to_string());
        let extension = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_string())
            .filter(|ext| !ext.is_empty());

        let mut index: usize = 1;
        loop {
            let candidate_name = if let Some(ext) = &extension {
                format!("{stem}_{index}.{ext}")
            } else {
                format!("{stem}_{index}")
            };
            let candidate_path = if let Some(parent_dir) = &parent {
                parent_dir.join(&candidate_name)
            } else {
                PathBuf::from(&candidate_name)
            };

            if !taken(&candidate_path) {
                return candidate_path;
            }

            index += 1;
        }
    }

    /// Create a project in memory, with no file (#576 NF-AN3-5: test-only).
    ///
    /// Reached only through `execute(Command::CreateProject)`, which only the test
    /// suites issue - the app's File > New is [`Self::do_create_project_full`]. Gated
    /// out of release builds so nothing there can install a file-less project.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn create_project(&self, name: String, location: Option<String>) -> CommandResult {
        let mut project = Project::new(name, location);
        // Seed per-project validation thresholds from the global config - the
        // "defaults for new projects" (#57). Authoritative in the .stalx after.
        project.validation = self.get_config().validation;

        tracing::info!(
            project_name = %project.name,
            purpose = "start new project",
            "Project created in memory"
        );

        // [#576 A5] A project-slot commit like every other: each per-project cell
        // written under one `project` acquisition (this used to set three of the
        // gates after dropping the guard, and kept the previous project's database
        // handle and undo watermark). A project created from nothing has no file,
        // so no handle, nothing deferred and no Recent Projects row.
        let commands = self.hold_commands();
        self.commit_open_session(&commands, SessionCommit::new_local(project, None));

        CommandResult::success()
    }

    /// Update per-project settings (#57): the project name, the validation
    /// thresholds and (#576 NF-W3-2) the default north reference - the one a
    /// section declaring none inherits (`Project::default_azimuth_reference`,
    /// `None` = nothing recorded). Persisted into the .stalx on the next save;
    /// warnings recompute from the new thresholds on the next
    /// get_project_warnings, and geometry from the new reference on the next
    /// read (the cached closure-adjusted coordinates are dropped here).
    ///
    /// The Project Settings dialog sends all three on every Save, so `None` is
    /// "nothing recorded", not "leave it alone". A reference arriving off the
    /// wire is re-sanitised (`AzimuthReference::sanitised`) like every ingress.
    ///
    /// [#576 NF-W3-2-PLAN] The name and the thresholds are on the read-only
    /// workspace's allowlist; the north reference is NOT. It turns every section
    /// that declares none of its own - a survey edit, like the entrance elevation -
    /// so in Plan and Export a save that would CHANGE it is refused whole
    /// (`WORKSPACE_READ_ONLY`, nothing applied), and an unread one is not set
    /// (lifted) from there either: it stays unread until a Survey save sets it.
    ///
    /// Undoable via a snapshot pair (mirrors repair_open_project_integrity) so an
    /// unrelated undo can't silently revert a settings change: without its own
    /// undo entry, undoing an edit made BEFORE the settings change would restore a
    /// snapshot carrying the old name/validation.
    pub fn update_project_settings(
        &self,
        name: String,
        validation: crate::config::ValidationConfig,
        default_azimuth_reference: Option<crate::models::AzimuthReference>,
    ) -> CommandResult {
        let default_azimuth_reference =
            default_azimuth_reference.map(crate::models::AzimuthReference::sanitised);
        // Serialize against execute()/undo()/redo() so the before/after pair is
        // consistent (same rationale as repair_open_project_integrity).
        let _cmd_guard = self.command_lock.lock();
        // Read-only edit gate (#191, D-B5): a settings change mutates the project,
        // so it is gated on a read-only remote project (returns the EDIT_DISABLED
        // coded failure). [#576 C5] On the read-only-workspace allowlist: the name
        // and the validation thresholds stay editable in Plan/Export on purpose
        // (docs/app-modes.md says why). The north reference is not (NF-W3-2-PLAN):
        // its own workspace check is below, once it is known to change.
        if let Err(f) = self.guard_metadata_mutation("Update project settings") {
            return CommandResult::refused(f);
        }
        let survey_edits_allowed = !self.workspace_mode().is_read_only();
        let before_snapshot = self.capture_project_snapshot().ok();
        let frame_before = self.frame_datum_key();
        let reference_changed = {
            let mut guard = self.project.lock();
            let Some(project) = guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            // No-op when nothing actually changed: touch() would still bump
            // modified_at, marking the project dirty and pushing a misleading
            // "Update project settings" undo entry for a save that changed nothing
            // (Codex). Compare before mutating.
            if project.name == name
                && project.validation == validation
                && project.default_azimuth_reference == default_azimuth_reference
            {
                // [#576 B15 follow-up] Unchanged, but SET: the dialog sends the
                // thresholds and the north reference while they are unread so the
                // user can keep the shown ones, and that choice is what lets a save
                // write them. Setting the north reference is a survey edit, so not
                // from Plan or Export (NF-W3-2-PLAN).
                self.lift_unreadable_project_setting("validation");
                if survey_edits_allowed {
                    self.lift_unreadable_project_setting("default_azimuth_reference");
                }
                return CommandResult::success();
            }
            let reference_changed = project.default_azimuth_reference != default_azimuth_reference;
            // [#576 NF-W3-2-PLAN] A changed north reference is a survey edit: in
            // Plan or Export the whole save is refused, before anything is applied.
            if reference_changed {
                if let Err(f) = self.guard_workspace("Change the default north reference") {
                    return CommandResult::refused(f);
                }
            }
            // [#576 RV5-F4-2] Where each anchoring datum is, measured BEFORE the
            // reference turns the sections that inherit it.
            let datum_holds = if reference_changed {
                datum_holds(project)
            } else {
                Vec::new()
            };
            project.name = name;
            project.validation = validation;
            project.default_azimuth_reference = default_azimuth_reference;
            // Each inheriting section turned about its own station 0, so a datum on
            // a later station moved: the frame datum off local (0, 0), and every
            // other datum off its GPS-implied place. Hold each merge group's datum
            // where it was and re-place the group's partners around it, exactly as
            // a shot edit does (`realign_after_shot_edit`).
            for (section_id, held_at) in datum_holds {
                project.realign_after_shot_edit(section_id, Some(held_at));
            }
            // [#576 FZ-PERSIST-3] Then settle every other group with the solve a
            // reopen runs (`redrive_offsets_after_load`), claims measured first as
            // the reconcile does. The epilogue's reconcile cannot settle a group
            // that holds no datum: it re-solves only a merge at least
            // `MERGE_ALIGNMENT_INVARIANT_M` apart, and a small correction opens a
            // junction by less (0.04 degrees, 5.8 m out: 4 mm). That gap stayed
            // live and in the file, and the reopen closed it - a section moved by a
            // save and reopen. The same solve here leaves the reopen nothing to move.
            if reference_changed {
                project.remeasure_closure_alignment();
                project.resolve_offsets();
            }
            project.touch();
            reference_changed
        };
        // Every section that declares no reference of its own turns with the
        // default: the closure-adjusted coordinates cached for the old one are stale.
        if reference_changed {
            self.invalidate_adjusted_cache();
        }
        // [#576 B15 follow-up] The thresholds and the north reference are now the
        // user's, not defaults standing in for unread ones: they no longer refuse a
        // save (NF-W3-2 gave the north reference its control) - the reference only
        // where survey edits are allowed (NF-W3-2-PLAN).
        self.lift_unreadable_project_setting("validation");
        if survey_edits_allowed {
            self.lift_unreadable_project_setting("default_azimuth_reference");
        }
        // Record an undo entry (before -> after) so the change is on the stack, and
        // flip the dirty bit (#191, D-B16) - through the shared forward epilogue
        // (#576 A4). A name or threshold edit needs no derived-state step. A changed
        // north reference does (#576 RV4-R-C1): each inheriting section turned about
        // its own stored origin, so the offsets are re-derived (the reconcile) before
        // the after-snapshot, or sections merged at a junction come apart.
        let steps = if reference_changed {
            EpilogueSteps::GEOMETRY
        } else {
            EpilogueSteps::NONE
        };
        self.record_snapshot_edit_if_changed(
            frame_before,
            steps,
            crate::core::Command::UpdateProjectSettings,
            before_snapshot,
        );
        CommandResult::success()
    }

    /// Set (or clear) the project's entrance elevation in meters (#465).
    /// Persisted into the .stalx on the next save. `None` clears a previously
    /// recorded value back to "not recorded" - the only recovery path for the
    /// TML import's no-clobber rule (#431) locking in a wrong first value.
    ///
    /// Undoable via a snapshot pair, mirroring `update_project_settings`: its
    /// own undo entry keeps an unrelated undo from silently reverting this edit.
    pub fn set_surface_elevation(&self, elevation: Option<f64>) -> CommandResult {
        // Reject non-finite input at the boundary (NaN/Infinity have no useful
        // meaning as an elevation and would corrupt export/display downstream).
        if let Some(value) = elevation {
            if !value.is_finite() {
                return CommandResult::failure("Elevation must be a finite number".to_string());
            }
        }
        // Serialize against execute()/undo()/redo(), same rationale as
        // update_project_settings.
        let _cmd_guard = self.command_lock.lock();
        if let Err(f) = self.guard_mutation("Set entrance elevation") {
            return CommandResult::refused(f);
        }
        let before_snapshot = self.capture_project_snapshot().ok();
        {
            let mut guard = self.project.lock();
            let Some(project) = guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            // No-op guard: this compares raw METERS, so it only catches a caller
            // that resent the exact stored value. The UI's own draft-string
            // comparison (ProjectSettingsDialog) is what actually keeps an
            // untouched, unit-converted/display-rounded field from reaching here
            // as a "changed" call in the first place - any future caller that
            // skips that comparison inherits the same spurious-dirty risk this
            // guard alone cannot close.
            if project.surface_elevation == elevation {
                // [#576 B15 follow-up] Unchanged, but SET: see
                // `update_project_settings`.
                self.lift_unreadable_project_setting("surface_elevation");
                return CommandResult::success();
            }
            project.surface_elevation = elevation;
            project.touch();
        }
        // [#576 B15 follow-up] The elevation is now the user's: it no longer
        // refuses a save.
        self.lift_unreadable_project_setting("surface_elevation");
        // [#576 A4] Recorded through the shared forward epilogue, like the settings.
        self.record_snapshot_edit_if_changed(
            None,
            EpilogueSteps::NONE,
            crate::core::Command::SetSurfaceElevation,
            before_snapshot,
        );
        CommandResult::success()
    }

    /// Create a new project with full progress reporting.
    /// This is the main implementation used by create_project_full command.
    ///
    /// Phases:
    /// 1. CreatingDatabase (40%) - Resolve/create SQLite file path
    /// 2. InitializingProject (40%) - Create project record
    /// 3. SavingSettings (20%) - Save default display settings
    ///
    /// All phases are indeterminate since creation is typically fast.
    ///
    /// # Arguments
    /// * `name` - Project name (used for filename and project metadata)
    /// * `project_location` - Optional cave location metadata (free text)
    /// * `save_directory` - Optional directory to save the project in.
    ///   If omitted (and `save_path` is omitted), uses user's documents
    ///   `stalagmight` subdirectory.
    /// * `save_path` - Optional full file path for the new project. If provided,
    ///   this takes precedence over `save_directory`.
    /// * `defer_initial_save` - When true, reserve `project.file_path` but do not
    ///   create/write the project file yet. First save/autosave will create it.
    ///
    /// The body of [`Self::do_create_project_full`] and
    /// [`Self::do_create_project_full_held`] [#576 NF-W4C-3]: `held` is a
    /// `command_lock` span the caller already holds, and `None` takes the lock around
    /// the commit only, as the create always has. First in this impl because the
    /// source tripwire that pins the create's shared commit (`speleodb_session`
    /// tests) reads the first full-create definition in this file.
    ///
    /// `check_target` [#576 RV13-1]: see [`Self::do_create_project_full_checked`].
    fn do_create_project_full_with(
        &self,
        held: Option<&CommandsHeld<'_>>,
        check_target: Option<&dyn Fn(&std::path::Path) -> Result<(), String>>,
        name: String,
        project_location: Option<String>,
        save_directory: Option<PathBuf>,
        save_path: Option<PathBuf>,
        defer_initial_save: bool,
    ) -> Result<crate::core::loading::CreateProjectFullResult, crate::core::loading::LoadError>
    {
        use crate::core::loading::{
            CreateProjectFullResult, DisplaySettings, LoadError, LoadingPhase, ProgressEmitter,
        };

        let sink = self
            .get_event_sink()
            .ok_or_else(|| LoadError::internal("EventSink not initialized"))?;

        // [#576 RV14-1] The file this create opens is resolved, and the caller's
        // write-path check run on it, before the create cancels any open or load
        // in progress or emits a progress phase: a create refused here (or whose
        // name does not resolve) leaves both as they were.
        //
        // Resolve database path from explicit save path or from project name + optional save directory.
        let mut db_path = self.requested_create_path(&name, save_directory.as_ref(), save_path)?;

        // Avoid collisions by selecting the next available path instead of overwriting.
        let available_db_path = Self::next_available_project_path(&db_path);
        if available_db_path != db_path {
            tracing::warn!(
                requested_path = ?db_path,
                selected_path = ?available_db_path,
                purpose = "avoid overwriting existing project on create",
                "Project path already exists; using auto-renamed path"
            );
            db_path = available_db_path;
        }

        // [#576 RV13-1] The caller's write-path check, on the file this create
        // opens - the name resolved and any `_N` rename made - before anything is
        // created, the missing folders included.
        if let Some(check) = check_target {
            check(&db_path).map_err(LoadError::internal)?;
        }

        // Start new load - cancels any existing load
        let cancel_token = self.start_new_load();
        let mut progress = ProgressEmitter::new_for_create(sink, Arc::clone(&cancel_token));

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

        // === PHASE 0: Creating database (40%) ===
        progress.start_phase(LoadingPhase::CreatingDatabase);

        // Create parent directories if they don't exist
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    early_return_err!(LoadError::new(
                        crate::models::CommandErrorCode::Io,
                        format!("Failed to create directory: {e}")
                    ));
                }
            }
        }

        // Create the database only when immediate persistence is requested.
        let mut db = if defer_initial_save {
            None
        } else {
            match Database::open(&db_path) {
                Ok(db) => Some(db),
                Err(e) => {
                    early_return_err!(LoadError::new(
                        crate::models::CommandErrorCode::Database,
                        format!("Failed to create database: {e}")
                    ));
                }
            }
        };
        check_cancelled!();
        progress.complete_phase(LoadingPhase::CreatingDatabase);

        // === PHASE 1: Initializing project (40%) ===
        progress.start_phase(LoadingPhase::InitializingProject);
        let mut project = Project::new(name.clone(), project_location);
        // Seed per-project validation from the global config (the "defaults for
        // new projects", #57). This is the path the New Project UI actually uses.
        project.validation = self.get_config().validation;
        project.file_path = Some(db_path.clone());

        if let Some(ref mut database) = db {
            if let Err(e) = database.save_project(&project) {
                early_return_err!(LoadError::new(
                    crate::models::CommandErrorCode::Database,
                    format!("Failed to save project: {e}")
                ));
            }
        }
        check_cancelled!();
        progress.complete_phase(LoadingPhase::InitializingProject);

        // === PHASE 2: Saving settings (20%) ===
        progress.start_phase(LoadingPhase::SavingSettings);
        let display_settings = DisplaySettings::default();

        // Save default display settings to database
        // Convert display_settings to HashMap for storage
        let settings_map = vec![
            ("showStations", display_settings.show_stations.to_string()),
            (
                "showSectionNames",
                display_settings.show_section_names.to_string(),
            ),
            (
                "showStationLabels",
                display_settings.show_station_labels.to_string(),
            ),
            ("showShotData", display_settings.show_shot_data.to_string()),
            (
                "showReferenceShots",
                display_settings.show_reference_shots.to_string(),
            ),
            ("showJumps", display_settings.show_jumps.to_string()),
            (
                "showZeroDatum",
                display_settings.show_zero_datum.to_string(),
            ),
            ("showGrid", display_settings.show_grid.to_string()),
            ("showSatellite", display_settings.show_satellite.to_string()),
            (
                "showDepthGradient",
                display_settings.show_depth_gradient.to_string(),
            ),
            ("gridSpacing", display_settings.grid_spacing.to_string()),
        ];

        if let Some(ref mut database) = db {
            for (key, value) in &settings_map {
                if let Err(e) = database.save_display_setting(key, value) {
                    // Fail the command if display settings can't be saved - this indicates a real DB error
                    early_return_err!(LoadError::new(
                        crate::models::CommandErrorCode::Database,
                        format!("Failed to save display setting {key}: {e}")
                    ));
                }
            }
        }
        check_cancelled!();
        progress.complete_phase(LoadingPhase::SavingSettings);

        // === FINALIZE: Store in AppState (NON-CANCELLABLE STATE COMMIT) ===
        // Allow cancellation only before any state mutation occurs.
        check_cancelled!();

        // [#576 A5] Every per-project cell under ONE `project` acquisition, through the
        // commit every load, create and close shares (round 4: this project carries a
        // `file_path`, so `delete_project`'s check-and-teardown can match it, and a
        // delete landing between a slot commit and the handle commits tore down a
        // half-built session that this function then finished building into an empty
        // world). The shared commit is also what puts a new project on the local undo
        // window with no inherited-history watermark: File > New over a Full-mode
        // linked project used to keep its byte budget and watermark (#576 A18(f)).
        // [#576 A1] Under `command_lock`, so the commit cannot land inside an edit of
        // the previous project and inherit its undo entry - or under the caller's span
        // when it already holds one [#576 NF-W4C-3].
        {
            let own_commands;
            let commands = if let Some(commands) = held {
                commands
            } else {
                own_commands = self.hold_commands();
                // [#576 RV4-R-C2] Re-checked once the lock is held, still before any
                // state write: a newer load or create that started while this one
                // waited for an in-flight command has cancelled it.
                check_cancelled!();
                &own_commands
            };
            self.commit_open_session(
                commands,
                SessionCommit {
                    // A deferred new project has no file on disk yet, so its first import must
                    // durably persist it.
                    initial_save_deferred: defer_initial_save,
                    // ...and no database to read its display settings from: seed the cache
                    // with the defaults just chosen.
                    display_settings: if defer_initial_save {
                        settings_map
                            .iter()
                            .map(|(key, value)| ((*key).to_string(), value.clone()))
                            .collect()
                    } else {
                        Vec::new()
                    },
                    // Recent Projects only after a real file exists.
                    recent: (!defer_initial_save).then(|| db_path.clone()),
                    ..SessionCommit::new_local(project.clone(), db)
                },
            );
        }

        // Get loading_id before clearing token
        let loading_id = progress.loading_id();

        // Clear active load AFTER all mutations complete
        self.clear_active_load(&cancel_token);

        tracing::info!(
            path = ?db_path,
            name = %name,
            purpose = if defer_initial_save { "create new project in memory with reserved save path" } else { "create new project on disk" },
            "Project created via create_project_full"
        );

        Ok(CreateProjectFullResult {
            loading_id,
            project,
            display_settings,
        })
    }

    /// Create a new project with full progress reporting (see
    /// `do_create_project_full_with` above for the phases and arguments), taking
    /// `command_lock` for its commit only - File > New.
    #[expect(
        clippy::result_large_err,
        reason = "the LoadError every create path returns; this only delegates to the body \
                  above, and boxing the error type is a repo-wide change outside NF-W4C-3"
    )]
    pub fn do_create_project_full(
        &self,
        name: String,
        project_location: Option<String>,
        save_directory: Option<PathBuf>,
        save_path: Option<PathBuf>,
        defer_initial_save: bool,
    ) -> Result<crate::core::loading::CreateProjectFullResult, crate::core::loading::LoadError>
    {
        self.do_create_project_full_with(
            None,
            None,
            name,
            project_location,
            save_directory,
            save_path,
            defer_initial_save,
        )
    }

    /// [`Self::do_create_project_full`] with the caller's write-path check run on
    /// the file the create opens [#576 RV13-1]: `check_target` is handed that path
    /// once the name is resolved and any `_N` rename made, before anything is
    /// created; its `Err` refuses the create with that message. The
    /// `create_project_full` command's entry when the webview names a file or a
    /// folder - checking a name of its own instead (the folder's
    /// `project.stalx`) let a link at the name the create really opens through.
    #[expect(
        clippy::result_large_err,
        reason = "the LoadError every create path returns; this only delegates to the body \
                  above, and boxing the error type is a repo-wide change outside NF-W4C-3"
    )]
    pub fn do_create_project_full_checked(
        &self,
        check_target: &dyn Fn(&std::path::Path) -> Result<(), String>,
        name: String,
        project_location: Option<String>,
        save_directory: Option<PathBuf>,
        save_path: Option<PathBuf>,
        defer_initial_save: bool,
    ) -> Result<crate::core::loading::CreateProjectFullResult, crate::core::loading::LoadError>
    {
        self.do_create_project_full_with(
            None,
            Some(check_target),
            name,
            project_location,
            save_directory,
            save_path,
            defer_initial_save,
        )
    }

    /// [`Self::do_create_project_full`] inside a `command_lock` span the caller
    /// already holds [#576 NF-W4C-3] - the Ariane adoption's, whose create is the
    /// first step of that one span.
    #[expect(
        clippy::result_large_err,
        reason = "the LoadError every create path returns; this only delegates to the body \
                  above, and boxing the error type is a repo-wide change outside NF-W4C-3"
    )]
    pub(super) fn do_create_project_full_held(
        &self,
        commands: &CommandsHeld<'_>,
        name: String,
        project_location: Option<String>,
        save_directory: Option<PathBuf>,
        save_path: Option<PathBuf>,
        defer_initial_save: bool,
    ) -> Result<crate::core::loading::CreateProjectFullResult, crate::core::loading::LoadError>
    {
        self.do_create_project_full_with(
            Some(commands),
            None,
            name,
            project_location,
            save_directory,
            save_path,
            defer_initial_save,
        )
    }

    /// The file a create asks for, before any `_N` rename: `save_path` with the
    /// project extension, else `name` resolved in `save_directory` (or the default
    /// folder). A name that does not resolve is an `Io` refusal.
    #[expect(
        clippy::result_large_err,
        reason = "the LoadError every create path returns; boxing it is a repo-wide change"
    )]
    fn requested_create_path(
        &self,
        name: &str,
        save_directory: Option<&PathBuf>,
        save_path: Option<PathBuf>,
    ) -> Result<PathBuf, crate::core::loading::LoadError> {
        match save_path {
            Some(path) => Ok(crate::persistence::with_stalx_extension(path)),
            None => self
                .resolve_project_path(name, save_directory)
                .map_err(|e| {
                    crate::core::loading::LoadError::new(crate::models::CommandErrorCode::Io, e)
                }),
        }
    }

    /// [#576 RV15-2] The write-path check [`Self::do_create_project_full_checked`]
    /// would run, alone: `check_target` on the file that create would open (the
    /// name resolved, any `_N` rename made), with the error that create would
    /// return. Nothing is created, cancelled or emitted, so the webview can ask
    /// before it stops a load in progress or shows its loading state for a create
    /// that will be refused. The create checks again (the disk may change between
    /// the two), so this is a courtesy, not the guard.
    ///
    /// Only for a create that names `save_directory` or `save_path`: the default
    /// location is not checked, and resolving it would create the default folder,
    /// so with neither this answers `Ok` and touches nothing (#576 RV16-3).
    #[expect(
        clippy::result_large_err,
        reason = "the LoadError every create path returns; boxing it is a repo-wide change"
    )]
    pub fn check_create_target(
        &self,
        check_target: &dyn Fn(&std::path::Path) -> Result<(), String>,
        name: &str,
        save_directory: Option<&PathBuf>,
        save_path: Option<PathBuf>,
    ) -> Result<(), crate::core::loading::LoadError> {
        if save_directory.is_none() && save_path.is_none() {
            return Ok(());
        }
        let requested = self.requested_create_path(name, save_directory, save_path)?;
        check_target(&Self::next_available_project_path(&requested))
            .map_err(crate::core::loading::LoadError::internal)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_project_path_sanitizes_and_validates() {
        let state = AppState::new().unwrap();
        let temp_dir = TempDir::new().unwrap();

        let resolved = state
            .resolve_project_path("New/Project?Name", Some(&temp_dir.path().to_path_buf()))
            .expect("resolve path");
        assert_eq!(resolved.parent(), Some(temp_dir.path()));
        assert_eq!(
            resolved.extension().and_then(|ext| ext.to_str()),
            Some("stalx")
        );
        let filename = resolved.file_name().unwrap_or_default().to_string_lossy();
        assert!(
            filename.contains("New_Project_Name"),
            "expected sanitized filename, got {filename}"
        );

        let err = state
            .resolve_project_path("", Some(&temp_dir.path().to_path_buf()))
            .expect_err("empty name should fail");
        assert!(err.contains("Project name cannot be empty"));

        let file = tempfile::NamedTempFile::new().unwrap();
        let err = state
            .resolve_project_path("Valid", Some(&file.path().to_path_buf()))
            .expect_err("file path should be rejected");
        assert!(err.contains("Path is not a directory"));
    }

    #[test]
    fn resolve_project_path_whitespace_only_name() {
        let state = AppState::new().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let err = state
            .resolve_project_path("   ", Some(&temp_dir.path().to_path_buf()))
            .expect_err("whitespace-only name should fail");
        assert!(err.contains("cannot be empty"), "got: {err}");
    }

    #[test]
    fn resolve_project_path_non_alphanumeric_only() {
        let state = AppState::new().unwrap();
        let temp_dir = TempDir::new().unwrap();
        // Non-alphanumeric chars get replaced with '_', but '_' passes sanitization.
        // The sanitizer only rejects empty-after-trim. Characters like '@' → '_' survive.
        let resolved = state
            .resolve_project_path("@#$%^&", Some(&temp_dir.path().to_path_buf()))
            .unwrap();
        let filename = resolved.file_name().unwrap().to_str().unwrap();
        assert_eq!(
            filename, "______.stalx",
            "special chars should be replaced with underscores"
        );
    }

    #[test]
    fn resolve_project_path_default_directory() {
        let state = AppState::new().unwrap();
        let result = state.resolve_project_path("TestProject", None);
        assert!(
            result.is_ok(),
            "should resolve with default dir: {:?}",
            result.err()
        );
        let path = result.unwrap();
        // Must be absolute: a relative default (e.g. "./stalagmight/...") breaks
        // saving. Regression guard for app_dirs::document_dir() returning "."
        // instead of None and short-circuiting the $HOME/Documents fallback.
        assert!(
            path.is_absolute(),
            "default project path must be absolute: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "TestProject.stalx"
        );
        let parent = path.parent().expect("default path should have parent");
        assert_eq!(parent.file_name().unwrap().to_str().unwrap(), "stalagmight");
    }

    #[test]
    fn resolve_project_path_replaces_spaces_and_symbols_with_underscores() {
        let state = AppState::new().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let resolved = state
            .resolve_project_path("My Cave-2024", Some(&temp_dir.path().to_path_buf()))
            .unwrap();
        let filename = resolved.file_name().unwrap().to_str().unwrap();
        assert_eq!(filename, "My_Cave_2024.stalx");
    }

    #[test]
    fn resolve_project_path_creates_nonexistent_default_dir() {
        let state = AppState::new().unwrap();
        // Use the default directory path (None) - should succeed even if
        // the app documents subdirectory needs to be created.
        let result = state.resolve_project_path("CreateDirTest", None).unwrap();
        let parent = result.parent().expect("default path should have parent");
        assert!(parent.exists(), "default app directory should exist");
        assert_eq!(parent.file_name().unwrap().to_str().unwrap(), "stalagmight");
    }

    #[test]
    fn set_surface_elevation_sets_clears_and_rejects_non_finite() {
        let state = AppState::new().unwrap();
        assert!(
            state
                .create_project("Elevation Test".to_string(), None)
                .success
        );

        // Not recorded by default.
        assert_eq!(
            state.project.lock().as_ref().unwrap().surface_elevation,
            None
        );

        // Set a value.
        let set = state.set_surface_elevation(Some(42.5));
        assert!(set.success, "{:?}", set.error);
        assert_eq!(
            state.project.lock().as_ref().unwrap().surface_elevation,
            Some(42.5)
        );

        // Clear back to None.
        let clear = state.set_surface_elevation(None);
        assert!(clear.success, "{:?}", clear.error);
        assert_eq!(
            state.project.lock().as_ref().unwrap().surface_elevation,
            None
        );

        // Reject non-finite input at the boundary.
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let result = state.set_surface_elevation(Some(bad));
            assert!(!result.success, "expected {bad} to be rejected");
            assert_eq!(
                state.project.lock().as_ref().unwrap().surface_elevation,
                None,
                "a rejected value must not be stored"
            );
        }
    }

    #[test]
    fn set_surface_elevation_undo_restores_prior_value() {
        let state = AppState::new().unwrap();
        assert!(
            state
                .create_project("Elevation Undo Test".to_string(), None)
                .success
        );

        assert!(state.set_surface_elevation(Some(10.0)).success);
        assert!(state.set_surface_elevation(Some(20.0)).success);
        assert_eq!(
            state.project.lock().as_ref().unwrap().surface_elevation,
            Some(20.0)
        );

        let undo = state.undo();
        assert!(undo.success, "{:?}", undo.error);
        assert_eq!(
            state.project.lock().as_ref().unwrap().surface_elevation,
            Some(10.0),
            "undo must restore the prior elevation"
        );

        let redo = state.redo();
        assert!(redo.success, "{:?}", redo.error);
        assert_eq!(
            state.project.lock().as_ref().unwrap().surface_elevation,
            Some(20.0),
            "redo must reapply the edit"
        );
    }

    #[test]
    fn next_available_project_path_adds_incrementing_suffix() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().join("Collision.stalx");
        let first_suffix = temp_dir.path().join("Collision_1.stalx");

        std::fs::write(&base_path, b"base").unwrap();
        std::fs::write(&first_suffix, b"first").unwrap();

        let available = AppState::next_available_project_path(&base_path);
        assert_eq!(available, temp_dir.path().join("Collision_2.stalx"));
    }

    /// [#576 RV13-1] A link at a name is a name taken - even one that leads
    /// nowhere yet, or to an empty file. `exists()` and `metadata()` follow the
    /// link: a dangling one was "free" and a link to an empty file a "placeholder",
    /// so the create opened the name and SQLite made or wrote the link's target,
    /// wherever it points.
    #[cfg(unix)]
    #[test]
    fn next_available_project_path_takes_a_link_as_a_name_taken() {
        let temp_dir = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let dir = temp_dir.path();
        let link = |name: &str, to: &str| {
            std::os::unix::fs::symlink(elsewhere.path().join(to), dir.join(name)).unwrap();
        };
        link("Cave.stalx", "planted.stalx");
        link("Cave_1.stalx", "planted-1.stalx");
        std::fs::write(elsewhere.path().join("empty.stalx"), b"").unwrap();
        link("Empty.stalx", "empty.stalx");
        // A real empty file is still a placeholder (Android's SAF makes one).
        std::fs::write(dir.join("Placeholder.stalx"), b"").unwrap();

        for (name, expected) in [
            ("Cave.stalx", "Cave_2.stalx"),
            ("Empty.stalx", "Empty_1.stalx"),
            ("Placeholder.stalx", "Placeholder.stalx"),
        ] {
            let available = AppState::next_available_project_path(&dir.join(name));
            assert_eq!(available, dir.join(expected), "{name}");
        }
    }

    /// [#576 RV14-1] A create the write-path policy refuses leaves everything else
    /// as it was. The check ran on the final path only after the create had
    /// cancelled whatever open or load was in progress and emitted its "creating
    /// database" phase, so a refused create stopped a load the user was waiting on
    /// and showed a progress step before its error.
    #[test]
    fn a_refused_create_cancels_no_load_and_emits_no_progress() {
        struct Recording(std::sync::Mutex<Vec<String>>);
        impl crate::core::events::EventSink for Recording {
            fn emit(&self, event: &str, _payload: serde_json::Value) {
                self.0.lock().unwrap().push(event.to_owned());
            }

            fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
                false
            }
        }

        let dir = TempDir::new().unwrap();
        let state = AppState::new().unwrap();
        let events = Arc::new(Recording(std::sync::Mutex::new(Vec::new())));
        state.set_event_sink(Arc::clone(&events) as Arc<dyn crate::core::events::EventSink>);
        // An open or a load the user is waiting on.
        let in_flight = state.start_new_load();

        let checked = std::cell::RefCell::new(Vec::new());
        let refuse = |path: &std::path::Path| {
            checked.borrow_mut().push(path.to_path_buf());
            Err("Refused to write here".to_string())
        };
        let refused = state.do_create_project_full_checked(
            &refuse,
            "Cave".into(),
            None,
            Some(dir.path().to_path_buf()),
            None,
            false,
        );

        let cancelled = in_flight.load(std::sync::atomic::Ordering::SeqCst);
        let emitted = events.0.lock().unwrap().clone();
        assert_eq!(
            refused.map(|_| ()).map_err(|e| e.message),
            Err("Refused to write here".to_string())
        );
        assert_eq!(
            *checked.borrow(),
            [dir.path().join("Cave.stalx")],
            "the check runs on the file the create opens"
        );
        assert!(
            !cancelled && emitted.is_empty(),
            "a refused create: the load in progress cancelled = {cancelled}; \
             progress events emitted = {emitted:?} (want false and none)"
        );
        assert!(
            std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "a refused create makes nothing"
        );
    }

    /// [#576 RV15-2] The pre-check the webview runs before it touches its loading
    /// state judges the file the create would open - the name resolved and any
    /// `_N` rename made - and answers what the create would, touching nothing.
    #[test]
    fn check_create_target_judges_the_file_the_create_would_open() {
        let dir = TempDir::new().unwrap();
        // The folder's `Cave.stalx` is taken, so a create there opens `Cave_1`.
        std::fs::write(dir.path().join("Cave.stalx"), b"taken").unwrap();
        struct Silent;
        impl crate::core::events::EventSink for Silent {
            fn emit(&self, _event: &str, _payload: serde_json::Value) {}

            fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
                false
            }
        }
        let state = AppState::new().unwrap();
        // The create needs a sink to reach its check; the pre-check needs none.
        state.set_event_sink(Arc::new(Silent));
        let in_flight = state.start_new_load();
        let checked = std::cell::RefCell::new(Vec::new());
        let refuse = |path: &std::path::Path| {
            checked.borrow_mut().push(path.to_path_buf());
            Err("Refused to write here".to_string())
        };

        let named_folder = state
            .check_create_target(&refuse, "Cave", Some(&dir.path().to_path_buf()), None)
            .map_err(|e| (e.code, e.message));
        let named_file = state
            .check_create_target(&refuse, "Ignored", None, Some(dir.path().join("Cave")))
            .map_err(|e| e.message);
        let refused_create = state
            .do_create_project_full_checked(
                &refuse,
                "Cave".into(),
                None,
                Some(dir.path().to_path_buf()),
                None,
                false,
            )
            .map(|_| ())
            .map_err(|e| (e.code, e.message));

        assert_eq!(named_folder, refused_create, "the create's own refusal");
        assert_eq!(named_file, Err("Refused to write here".to_string()));
        assert_eq!(
            *checked.borrow(),
            [
                dir.path().join("Cave_1.stalx"),
                dir.path().join("Cave_1.stalx"),
                dir.path().join("Cave_1.stalx"),
            ],
            "the pre-checks and the create judge the same file"
        );
        assert!(
            !in_flight.load(std::sync::atomic::Ordering::SeqCst),
            "a pre-check cancels no load"
        );
        let accept = |_: &std::path::Path| Ok(());
        assert!(state
            .check_create_target(&accept, "Cave", Some(&dir.path().join("new")), None)
            .is_ok());
        assert!(
            !dir.path().join("new").exists() && !dir.path().join("Cave_1.stalx").exists(),
            "a pre-check makes nothing"
        );
        // A name that does not resolve is refused as the create refuses it.
        assert_eq!(
            state
                .check_create_target(&accept, "  ", Some(&dir.path().to_path_buf()), None)
                .map_err(|e| e.code),
            Err(crate::models::CommandErrorCode::Io)
        );
    }

    /// [#576 RV16-3] With neither a folder nor a file named there is nothing to
    /// check: the default location is not resolved (resolving it creates the
    /// default folder) and the check is never asked.
    #[test]
    fn check_create_target_without_a_named_target_checks_and_makes_nothing() {
        let state = AppState::new().unwrap();
        let asked = std::cell::RefCell::new(Vec::new());
        let record = |path: &std::path::Path| {
            asked.borrow_mut().push(path.to_path_buf());
            Ok(())
        };

        let answer = state
            .check_create_target(&record, "Cave", None, None)
            .map_err(|e| e.message);

        assert_eq!(
            (answer, asked.borrow().clone()),
            (Ok(()), Vec::new()),
            "the answer, and the paths the check was asked about"
        );
    }
}

/// [#576 RV5-F4-2] Each merge group's anchoring datum, as `(a section of the
/// group, where the datum is now)`: the first datum of the group that resolves,
/// which is the one [`Project::realign_after_shot_edit`] holds. One entry per
/// group; a group with no resolvable datum has nothing absolute to hold.
fn datum_holds(project: &Project) -> Vec<(crate::models::SectionId, (f64, f64))> {
    let mut covered = std::collections::HashSet::new();
    let mut holds = Vec::new();
    for datum in &project.zero_datums {
        let section_id = datum.station_ref.section_id;
        if covered.contains(&section_id) {
            continue;
        }
        covered.extend(project.get_merged_section_ids(section_id));
        covered.insert(section_id);
        if let Some(at) = project.datum_anchor_position(section_id) {
            holds.push((section_id, at));
        }
    }
    holds
}
