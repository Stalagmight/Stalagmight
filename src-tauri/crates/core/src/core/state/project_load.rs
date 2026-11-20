//! Opening a project file (plan 6.1, 6.6, 7; C0).
//!
//! The bytes are read once and fingerprinted (the save guard), then sniffed:
//! - version 2 JSON: `stalx_v2::decode` (count limits, `format` / `version` /
//!   `conventions` / `requires`, `validate`). An unknown `requires` feature
//!   or unknown structural content opens read-only (`Extras.read_only`); an
//!   invalid survey is refused with the salvage route's message; the history
//!   is decoded entry by entry and dropped whole with the banner on any
//!   failure;
//! - legacy SQLite: `migrate::migrate_path_for_app` with the user's
//!   least-squares config (the frozen 0.6.0 open, then the conversion and its
//!   verification). The project remembers its legacy origin: the first save
//!   upgrades the file after backing the original up. The history is
//!   dropped with the existing banner; the conversion notice is kept when
//!   the verification moved slots or left anything unexplained.
//!
//! Load is pure: the project in memory is exactly the file's (no heals).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::open_file::FileState;
use super::AppState;
use crate::core::loading::{
    DisplaySettings, LoadError, LoadProjectFullResult, LoadingPhase, ProgressEmitter,
};
use crate::core::{CommandResult, HistoryDropped};
use crate::models::survey::changeset::EntryLimits;
use crate::models::survey::Project;
use crate::persistence::migrate::{LegacyError, MigrateError};
use crate::persistence::stalx_v2::{
    decode, sniff, DecodeError, DecodeOptions, Fingerprint, Format, History,
};

/// A project file read into memory, not yet committed into the slot.
pub(crate) struct Opened {
    pub(crate) project: Project,
    pub(crate) file: FileState,
    /// The history the file holds (version 2), loaded into the stacks at
    /// commit; empty for a migrated file.
    pub(crate) history: History,
    /// Why the history was dropped before the commit (a legacy file's).
    pub(crate) history_dropped: Option<HistoryDropped>,
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path_text(path), |n| n.to_string_lossy().into_owned())
}

/// The load error of a version 2 decode refusal.
fn decode_error(path: &Path, error: &DecodeError) -> LoadError {
    match error {
        DecodeError::NewerVersion { found, supported } => LoadError::newer_schema(
            &crate::models::NewerSchema {
                file: file_name(path),
                found: i32::try_from(*found).unwrap_or(i32::MAX),
                supported: i32::try_from(*supported).unwrap_or(i32::MAX),
            },
            &path_text(path),
        ),
        DecodeError::Invalid { .. } => LoadError::corrupt(
            format!("The project breaks an invariant and cannot be opened normally: {error}"),
            &path_text(path),
        ),
        other => LoadError::corrupt(other.to_string(), &path_text(path)),
    }
}

/// The load error of a migration refusal.
fn migrate_error(path: &Path, error: &MigrateError) -> LoadError {
    match error {
        MigrateError::Legacy(LegacyError::NewerSchema { found, supported }) => {
            LoadError::newer_schema(
                &crate::models::NewerSchema {
                    file: file_name(path),
                    found: *found,
                    supported: *supported,
                },
                &path_text(path),
            )
        }
        MigrateError::Legacy(LegacyError::Io(e)) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                LoadError::permission(e.to_string(), &path_text(path))
            } else {
                LoadError::new("IO", e.to_string())
            }
        }
        other => {
            tracing::warn!(
                class = other.class(),
                "a legacy project could not be migrated"
            );
            LoadError::corrupt(other.to_string(), &path_text(path))
        }
    }
}

impl AppState {
    /// Read the project file at `path` into memory (no state changes).
    pub(crate) fn read_project_file(&self, path: &Path) -> Result<Opened, LoadError> {
        let options = DecodeOptions::default();
        let (bytes, fingerprint) =
            Fingerprint::read(path, options.max_file_bytes).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => LoadError::not_found(&path_text(path)),
                std::io::ErrorKind::PermissionDenied => {
                    LoadError::permission(e.to_string(), &path_text(path))
                }
                _ => LoadError::new("IO", e.to_string()),
            })?;
        match sniff(&bytes) {
            Format::V2 => {
                let decoded = decode(&bytes, &options).map_err(|e| decode_error(path, &e))?;
                for note in &decoded.notes {
                    tracing::info!(note = ?note, "decode note");
                }
                if let Some(reason) = decoded.read_only() {
                    tracing::warn!(
                        features = ?reason.unknown_features,
                        unknown_content = reason.unknown_content_total,
                        "Opening read-only: made with a newer Stalagmight"
                    );
                }
                let mut project = decoded.project;
                project.file_path = Some(path.to_path_buf());
                let mut file = FileState::from_v2(decoded.extras, fingerprint);
                let history = file.saved_history.clone();
                // An unreadable history member was already dropped by the
                // decoder (tier 3); the banner says so.
                let history_dropped = decoded
                    .notes
                    .iter()
                    .any(|n| {
                        matches!(
                            n,
                            crate::persistence::stalx_v2::DecodeNote::HistoryDropped { .. }
                        )
                    })
                    .then_some(HistoryDropped::Unreadable);
                file.dirty = false;
                Ok(Opened {
                    project,
                    file,
                    history,
                    history_dropped,
                })
            }
            Format::Legacy => {
                drop(bytes);
                let config = self.get_config();
                let migrated = crate::persistence::migrate::migrate_path_for_app(
                    path,
                    &config.closure_adjustment,
                )
                .map_err(|e| migrate_error(path, &e))?;
                let report = &migrated.report;
                tracing::info!(
                    stations = report.stations,
                    clean = report.verification.is_clean(),
                    moved = report.verification.moved().slots,
                    purpose = "open a legacy project",
                    "Migrated a legacy project in memory; the first save upgrades it"
                );
                let mut project = migrated.project;
                if !migrated.extras.validation_stored {
                    project.settings.validation = config.validation;
                }
                project.file_path = Some(path.to_path_buf());
                let file = FileState::from_migrated(
                    migrated.extras,
                    Some(fingerprint),
                    &migrated.report.verification,
                );
                Ok(Opened {
                    project,
                    file,
                    history: History::default(),
                    history_dropped: migrated
                        .had_history
                        .then_some(HistoryDropped::EarlierVersion),
                })
            }
            Format::Unknown => Err(LoadError::corrupt(
                "This is not a Stalagmight project file.",
                &path_text(path),
            )),
        }
    }

    /// Commit a read file into the project slot: the project, its file
    /// state, the edit gate, the undo stacks, the banner flags. The caller
    /// holds no lock.
    pub(crate) fn commit_opened(&self, opened: Opened) {
        let Opened {
            project,
            mut file,
            history,
            history_dropped,
        } = opened;
        // A legacy NULL undo mode is materialized from this machine's default
        // on first open, and made durable (#303).
        let mut link = file.speleo_link();
        if let Some(l) = link.as_mut() {
            if self.materialize_undo_mode(l) {
                file.set_speleo_link(Some(l));
                file.local_pending = true;
            }
        }
        let watermark = file.extras.local.speleodb.inherited_through;
        self.speleodb_release_held_lock_detached();
        let mut proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *proj_guard = Some(project);
        {
            let mut session = self
                .speleodb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bump_speleodb_session();
            self.speleodb_commit_open_gate(&mut session, link.as_ref(), None);
        }
        self.bump_revision();
        *self
            .derived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = file;
        self.set_history_discarded(false);
        self.note_history_dropped(history_dropped);
        self.sync_undo_depth_limit_for(link.as_ref());
        let dropped = {
            let mut stack = self
                .undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stack.set_inherited_through(watermark);
            stack
                .load_persisted(&history, &EntryLimits::default())
                .err()
        };
        self.note_history_dropped(dropped);
        self.initial_save_deferred
            .store(false, std::sync::atomic::Ordering::Relaxed);
        drop(proj_guard);
        if self.with_file(|f| f.local_pending) {
            if let Err(e) = self.persist_local_state() {
                tracing::warn!(error = %e, "Could not write the materialized undo mode; it stays in memory");
            }
        }
    }

    /// Open a project (the executor's `Command::OpenProject` and tests): no
    /// progress events, no canonical-storage guard.
    pub(super) fn open_project(&self, path: PathBuf) -> CommandResult {
        match self.read_project_file(&path) {
            Ok(opened) => {
                self.commit_opened(opened);
                self.note_recent_project(&path);
                CommandResult::success()
            }
            Err(error) => CommandResult::failure(error.message),
        }
    }

    /// Open a downloaded SpeleoDB project (pull, install, revision swap):
    /// ported by the SpeleoDB work package.
    pub(super) fn open_project_prepared<F>(
        &self,
        _path: PathBuf,
        _expected: Option<(u64, Vec<u8>)>,
        _before_commit: F,
    ) -> Result<CommandResult, crate::speleodb::SpeleoErrorDto>
    where
        F: FnOnce() -> Result<(), crate::speleodb::SpeleoErrorDto>,
    {
        crate::core::stub::not_ported("project_load::open_project_prepared")
    }

    /// See [`Self::open_project_prepared`].
    pub(super) fn open_project_download(
        &self,
        _source: PathBuf,
        _dest: PathBuf,
        _expected: (u64, Vec<u8>),
        _editing: bool,
    ) -> Result<CommandResult, crate::speleodb::SpeleoErrorDto> {
        crate::core::stub::not_ported("project_load::open_project_download")
    }

    /// Load a project with full progress reporting (the `load_project_full`
    /// IPC). All computation happens before the commit; cancellation is
    /// honoured up to the commit, which is not cancellable.
    pub fn do_load_project_full(&self, path: PathBuf) -> Result<LoadProjectFullResult, LoadError> {
        let sink = self
            .get_event_sink()
            .ok_or_else(|| LoadError::internal("EventSink not initialized"))?;
        tracing::info!(path = ?path, purpose = "load project for workspace", "Starting full project load");
        if self.open_file_path().as_ref() == Some(&path) {
            return Err(LoadError::already_open());
        }
        let cancel_token = self.start_new_load();
        let mut progress = ProgressEmitter::new_for_open(sink, Arc::clone(&cancel_token));
        let result = self.load_full(&path, &mut progress);
        self.clear_active_load(&cancel_token);
        result
    }

    fn load_full(
        &self,
        path: &Path,
        progress: &mut ProgressEmitter,
    ) -> Result<LoadProjectFullResult, LoadError> {
        let cancelled = |progress: &ProgressEmitter| {
            if progress.is_cancelled() {
                Err(LoadError::cancelled())
            } else {
                Ok(())
            }
        };
        progress.start_phase(LoadingPhase::OpeningDatabase);
        use super::speleodb_foreign::LinkedSpelling;
        let path = match self.foreign_link(path) {
            LinkedSpelling::AsSpelled => path.to_path_buf(),
            LinkedSpelling::Canonical(canonical) => canonical,
            LinkedSpelling::Foreign(managed) => return Err(LoadError::speleodb_managed(managed)),
        };
        cancelled(progress)?;
        progress.complete_phase(LoadingPhase::OpeningDatabase);
        progress.start_phase(LoadingPhase::LoadingProjectData);
        let opened = self.read_project_file(&path)?;
        cancelled(progress)?;
        progress.complete_phase(LoadingPhase::LoadingProjectData);
        progress.start_phase(LoadingPhase::ComputingStations);
        let include_reference_legs = self.get_config().gas.include_reference_legs;
        let raw = Arc::new(opened.project.raw_positions());
        let envelope = opened
            .project
            .load_envelope_input_with(&raw, include_reference_legs);
        cancelled(progress)?;
        progress.complete_phase(LoadingPhase::ComputingStations);
        progress.start_phase(LoadingPhase::ValidatingData);
        let warnings = self.compute_project_warnings(&opened.project);
        cancelled(progress)?;
        progress.complete_phase(LoadingPhase::ValidatingData);
        progress.start_phase(LoadingPhase::LoadingSettings);
        let stored: HashMap<String, String> = opened
            .file
            .display_settings()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let display_settings = DisplaySettings::from_stored(&stored);
        let is_remote = opened.file.extras.speleodb.is_some();
        cancelled(progress)?;
        progress.complete_phase(LoadingPhase::LoadingSettings);
        progress.start_phase(LoadingPhase::Finalizing);
        cancelled(progress)?;
        self.commit_opened(opened);
        // The raw layout of the committed revision.
        {
            let revision = self.current_revision();
            *self
                .derived
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(super::app_state::DerivedCache {
                    revision,
                    raw,
                    adjusted: None,
                });
        }
        self.note_recent_project(&path);
        progress.complete_phase(LoadingPhase::Finalizing);
        tracing::info!(path = ?path, purpose = "load project for workspace", "Project loaded via load_project_full");
        Ok(LoadProjectFullResult::from_envelope(
            progress.loading_id(),
            envelope,
            warnings,
            display_settings,
            is_remote,
        ))
    }
}
