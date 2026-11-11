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
    pub(super) fn next_available_project_path(path: &PathBuf) -> PathBuf {
        if !path.exists() {
            return path.clone();
        }
        // Treat 0-byte files as non-existent: Android's SAF (ACTION_CREATE_DOCUMENT)
        // creates an empty placeholder at the chosen path before returning.
        if path.metadata().is_ok_and(|m| m.len() == 0) {
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

            if !candidate_path.exists() {
                return candidate_path;
            }

            index += 1;
        }
    }

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

        if let Ok(mut proj) = self.project.lock() {
            *proj = Some(project);
            // A session boundary (a project committed into the slot), bumped under
            // the guard per the `speleodb_session` rule.
            self.bump_speleodb_session();
            // An add's pending ignores (#674) belong to the project that just left.
            self.clear_pending_ignores();
            // So does the record of renamed shot ids.
            self.load_renamed_shots
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }

        if let Ok(mut undo_stack) = self.undo_stack.lock() {
            undo_stack.clear();
            // [#303] A brand-new project is local: the configured entry window.
            undo_stack.use_count_limit();
        }

        // Clear display settings cache for new project
        self.clear_display_settings_cache();
        self.invalidate_adjusted_cache();
        // [Track B] (#191) A freshly-created local project is always editable;
        // the gate goes back to unmanaged (see `speleodb_edit_gate`).
        self.speleodb_set_open_gate(None);
        self.set_history_discarded(false);

        CommandResult::success()
    }

    /// Update per-project settings (#57): the project name and validation
    /// thresholds. Persisted into the .stalx on the next save; warnings recompute
    /// from the new thresholds on the next get_project_warnings.
    ///
    /// Undoable via a snapshot pair (mirrors repair_open_project_integrity) so an
    /// unrelated undo can't silently revert a settings change: without its own
    /// undo entry, undoing an edit made BEFORE the settings change would restore a
    /// snapshot carrying the old name/validation.
    pub fn update_project_settings(
        &self,
        name: String,
        validation: crate::config::ValidationConfig,
    ) -> CommandResult {
        // Serialize against execute()/undo()/redo() so the before/after pair is
        // consistent (same rationale as repair_open_project_integrity).
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B5): a settings change mutates the project,
        // so it is gated on a read-only remote project (returns the EDIT_DISABLED
        // coded failure).
        if let Err(f) = self.guard_mutation("Update project settings") {
            return f;
        }
        if self.speleodb_open_link().is_ok()
            && self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|project| project.name != name)
        {
            return CommandResult::failure("Linked project names are managed by SpeleoDB".into());
        }
        let before_snapshot = self.capture_project_snapshot().ok();
        {
            let mut guard = match self.project.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let Some(project) = guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            // No-op when nothing actually changed: touch() would still bump
            // modified_at, marking the project dirty and pushing a misleading
            // "Update project settings" undo entry for a save that changed nothing
            // (Codex). Compare before mutating.
            if project.name == name && project.validation == validation {
                return CommandResult::success();
            }
            project.name = name;
            project.validation = validation;
            project.touch();
        }
        // Record an undo entry (before -> after) so the change is on the stack.
        if let Some(before_json) = before_snapshot {
            if let Ok(after_json) = self.capture_project_snapshot() {
                if before_json != after_json {
                    self.push_snapshot_undo(
                        crate::core::Command::UpdateProjectSettings,
                        before_json,
                        after_json,
                    );
                    // Dirty-bit flip (#191, D-B16): settings mutated a remote
                    // project. Set-only; no-op for local-only.
                    self.mark_remote_forward_edit();
                }
            }
        }
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
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        if let Err(f) = self.guard_mutation("Set entrance elevation") {
            return f;
        }
        let before_snapshot = self.capture_project_snapshot().ok();
        {
            let mut guard = match self.project.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
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
                return CommandResult::success();
            }
            project.surface_elevation = elevation;
            project.touch();
        }
        if let Some(before_json) = before_snapshot {
            if let Ok(after_json) = self.capture_project_snapshot() {
                if before_json != after_json {
                    self.push_snapshot_undo(
                        crate::core::Command::SetSurfaceElevation,
                        before_json,
                        after_json,
                    );
                    self.mark_remote_forward_edit();
                }
            }
        }
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
    pub fn do_create_project_full(
        &self,
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

        // Resolve database path from explicit save path or from project name + optional save directory.
        let mut db_path = if let Some(path) = save_path {
            if path.extension().is_none_or(|ext| ext != "stalx") {
                path.with_extension("stalx")
            } else {
                path
            }
        } else {
            match self.resolve_project_path(&name, save_directory.as_ref()) {
                Ok(path) => path,
                Err(e) => {
                    early_return_err!(LoadError::new("IO", e));
                }
            }
        };

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

        // Create parent directories if they don't exist
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    early_return_err!(LoadError::new(
                        "IO",
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
                        "DATABASE",
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
                    "DATABASE",
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
                        "DATABASE",
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
        // All finalization commits use unwrap_or_else(PoisonError::into_inner) to ensure
        // data is written even if a previous panic poisoned a mutex.

        // ALL commits under ONE `project` acquisition, exactly as
        // `do_load_project_full`'s finalization and for the same reason (round
        // 4): this project carries a `file_path`, so `delete_project`'s
        // check-and-teardown can match it - and a delete landing between the
        // slot commit and the handle commits tore down a half-built session
        // that this function then finished building into an empty world.
        // (`create_project`, the in-memory sibling, commits a project with no
        // `file_path`, which the delete's identity check can never match.)
        let mut proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Commit 1: Project state mutation
        *proj_guard = Some(project.clone());
        // A session boundary (a project committed into the slot), bumped under
        // the guard per the `speleodb_session` rule.
        self.bump_speleodb_session();
        // An add's pending ignores (#674) belong to the project that just left.
        self.clear_pending_ignores();

        // Commit 2: Database handle mutation
        *self
            .database
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = db;

        // Commit 3: Clear undo stack (new project)
        self.undo_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        // [Track B] (#191) A freshly-created local project is always editable;
        // the gate goes back to unmanaged (see `speleodb_edit_gate`).
        self.speleodb_set_open_gate(None);
        self.set_history_discarded(false);

        // Track whether the initial save is still pending: a deferred new project
        // has no file on disk yet, so its first import must durably persist it.
        self.initial_save_deferred
            .store(defer_initial_save, std::sync::atomic::Ordering::Relaxed);
        // A new project has renamed nothing.
        self.load_renamed_shots
            .store(false, std::sync::atomic::Ordering::Relaxed);

        // Commit 4: Display settings cache
        if defer_initial_save {
            let mut cache = self
                .display_settings_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.clear();
            for (key, value) in &settings_map {
                cache.insert((*key).to_string(), value.clone());
            }
        } else {
            self.clear_display_settings_cache();
            self.invalidate_adjusted_cache();
        }

        // Commit 5: Add to recent projects only after a real file exists.
        if !defer_initial_save {
            let mut recent = self
                .recent_projects
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            recent.retain(|p| p != &db_path);
            recent.insert(0, db_path.clone());
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
            state
                .project
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .surface_elevation,
            None
        );

        // Set a value.
        let set = state.set_surface_elevation(Some(42.5));
        assert!(set.success, "{:?}", set.error);
        assert_eq!(
            state
                .project
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .surface_elevation,
            Some(42.5)
        );

        // Clear back to None.
        let clear = state.set_surface_elevation(None);
        assert!(clear.success, "{:?}", clear.error);
        assert_eq!(
            state
                .project
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .surface_elevation,
            None
        );

        // Reject non-finite input at the boundary.
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let result = state.set_surface_elevation(Some(bad));
            assert!(!result.success, "expected {bad} to be rejected");
            assert_eq!(
                state
                    .project
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .surface_elevation,
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
            state
                .project
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .surface_elevation,
            Some(20.0)
        );

        let undo = state.undo();
        assert!(undo.success, "{:?}", undo.error);
        assert_eq!(
            state
                .project
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .surface_elevation,
            Some(10.0),
            "undo must restore the prior elevation"
        );

        let redo = state.redo();
        assert!(redo.success, "{:?}", redo.error);
        assert_eq!(
            state
                .project
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .surface_elevation,
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
}
