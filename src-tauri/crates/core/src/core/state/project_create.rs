use super::open_file::FileState;
use super::AppState;
use crate::core::CommandResult;
use crate::models::survey::Project;
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

    /// The display settings a new project starts with (written into the
    /// file's `local` member).
    fn default_display_settings() -> Vec<(&'static str, String)> {
        let d = crate::core::loading::DisplaySettings::default();
        vec![
            ("showStations", d.show_stations.to_string()),
            ("showSectionNames", d.show_line_names.to_string()),
            ("showStationLabels", d.show_station_labels.to_string()),
            ("showShotData", d.show_shot_data.to_string()),
            ("showReferenceShots", d.show_reference_shots.to_string()),
            ("showJumps", d.show_jumps.to_string()),
            ("showZeroDatum", d.show_zero_datum.to_string()),
            ("showGrid", d.show_grid.to_string()),
            ("showSatellite", d.show_satellite.to_string()),
            ("showDepthGradient", d.show_depth_gradient.to_string()),
            ("gridSpacing", d.grid_spacing.to_string()),
        ]
    }

    /// Commit a new, empty project into the slot (no file yet).
    fn commit_new_project(&self, project: Project, display: Vec<(&'static str, String)>) {
        let mut proj = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *proj = Some(project);
        self.bump_speleodb_session();
        self.bump_revision();
        {
            let mut file = self
                .file
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *file = FileState::default();
            for (key, value) in display {
                file.extras.local.display.insert(key.to_string(), value);
            }
        }
        {
            let mut undo_stack = self
                .undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            undo_stack.clear();
            undo_stack.set_inherited_through(None);
            undo_stack.use_count_limit();
        }
        *self
            .derived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.speleodb_set_open_gate(None);
        self.set_history_discarded(false);
        drop(proj);
    }

    /// `Command::CreateProject`: a new project in memory, no file.
    pub(super) fn create_project(&self, name: String, location: Option<String>) -> CommandResult {
        let mut project = Project::new(name, location);
        project.settings.validation = self.get_config().validation;
        tracing::info!(
            project_name = %project.name,
            purpose = "start new project",
            "Project created in memory"
        );
        self.commit_new_project(project, Self::default_display_settings());
        CommandResult::success()
    }

    /// Rename the project and set its validation thresholds (one undo step).
    pub fn update_project_settings(
        &self,
        name: String,
        validation: crate::config::ValidationConfig,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        if let Err(f) = self.guard_mutation("Update project settings") {
            return f;
        }
        let linked = self.speleo_link().is_some();
        let result = self.run_recorded(
            |p| super::commands::Record::of(p, &crate::core::Command::UpdateProjectSettings),
            |project, _| {
                if linked && project.name != name {
                    return Err("Linked project names are managed by SpeleoDB".to_string());
                }
                project.name = name;
                project.settings.validation = validation;
                Ok(())
            },
        );
        result.map_or_else(|f| f, |()| CommandResult::success())
    }

    /// Set the entrance elevation (one undo step).
    pub fn set_surface_elevation(&self, elevation: Option<f64>) -> CommandResult {
        if elevation.is_some_and(|v| !v.is_finite()) {
            return CommandResult::failure("Elevation must be a finite number".to_string());
        }
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        if let Err(f) = self.guard_mutation("Set entrance elevation") {
            return f;
        }
        let result = self.run_recorded(
            |p| super::commands::Record::of(p, &crate::core::Command::SetSurfaceElevation),
            |project, _| {
                project.settings.surface_elevation = elevation;
                Ok(())
            },
        );
        result.map_or_else(|f| f, |()| CommandResult::success())
    }

    /// Create a project with progress reporting (the `create_project_full`
    /// IPC): a new project at a free path, written as version 2 at once
    /// unless `defer_initial_save` (#92: the first import writes it).
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
        let cancel_token = self.start_new_load();
        let mut progress = ProgressEmitter::new_for_create(sink, Arc::clone(&cancel_token));
        let fail = |e: LoadError| {
            self.clear_active_load(&cancel_token);
            Err(e)
        };
        progress.start_phase(LoadingPhase::CreatingFile);
        let mut path = if let Some(path) = save_path {
            if path.extension().is_none_or(|ext| ext != "stalx") {
                path.with_extension("stalx")
            } else {
                path
            }
        } else {
            match self.resolve_project_path(&name, save_directory.as_ref()) {
                Ok(path) => path,
                Err(e) => return fail(LoadError::new("IO", e)),
            }
        };
        let available = Self::next_available_project_path(&path);
        if available != path {
            tracing::warn!(
                requested_path = ?path,
                selected_path = ?available,
                purpose = "avoid overwriting existing project on create",
                "Project path already exists; using auto-renamed path"
            );
            path = available;
        }
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return fail(LoadError::new(
                        "IO",
                        format!("Failed to create directory: {e}"),
                    ));
                }
            }
        }
        if progress.is_cancelled() {
            return fail(LoadError::cancelled());
        }
        progress.complete_phase(LoadingPhase::CreatingFile);
        progress.start_phase(LoadingPhase::InitializingProject);
        let mut project = Project::new(name.clone(), project_location);
        project.settings.validation = self.get_config().validation;
        project.file_path = Some(path.clone());
        let wire = project.project_wire();
        progress.complete_phase(LoadingPhase::InitializingProject);
        progress.start_phase(LoadingPhase::SavingSettings);
        if progress.is_cancelled() {
            return fail(LoadError::cancelled());
        }
        self.commit_new_project(project, Self::default_display_settings());
        self.initial_save_deferred
            .store(defer_initial_save, std::sync::atomic::Ordering::Relaxed);
        if !defer_initial_save {
            let saved = self.save_project(None);
            if !saved.success {
                return fail(LoadError::new(
                    "IO",
                    saved
                        .error
                        .unwrap_or_else(|| "Failed to write the new project".to_string()),
                ));
            }
        }
        progress.complete_phase(LoadingPhase::SavingSettings);
        let loading_id = progress.loading_id();
        self.clear_active_load(&cancel_token);
        tracing::info!(
            path = ?path,
            name = %name,
            deferred = defer_initial_save,
            "Project created via create_project_full"
        );
        Ok(CreateProjectFullResult {
            loading_id,
            project: wire,
            display_settings: DisplaySettings::default(),
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
                .settings
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
                .settings
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
                .settings
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
                    .settings
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
                .settings
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
                .settings
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
                .settings
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
