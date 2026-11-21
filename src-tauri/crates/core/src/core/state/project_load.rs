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
//! Load is pure: the project in memory is exactly the file's (no heals), and
//! nothing derived is written back. An open writes at most two things, both
//! state the file already implies (docs/saving.md, "What an open writes"):
//! the link probe of `load_project_full` rolls a legacy file's hot journal
//! back into it in place (SQLite's own recovery, as 0.6.0's open did), and a
//! linked file whose undo mode was never set gets this machine's, by a
//! local-only save that copies the history byte for byte
//! ([`AppState::persist_pending_local_state`]).
//!
//! The read runs without any lock (a legacy migration takes seconds), so the
//! file can change between the read and the commit: one of our own writes
//! (a save, a display toggle's local-only save, Save As or Save a Copy onto
//! that path) or another program's. The commit re-reads the fingerprint
//! under the `project` guard, which every write of ours to the open file
//! holds, and refuses a read the file no longer holds; the open then reads it
//! again, up to [`OPEN_ATTEMPTS`] reads. Committed, the guard's fingerprint
//! was stale: the next save was refused as "changed by another program" for
//! our own write. Those writes are serialized by that guard, so a second read
//! converges; a file another program keeps rewriting is opened as last read,
//! and the guard then says so at the next save, which is true.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::open_file::FileState;
use super::speleodb_cache::OpenProjectToken;
use super::speleodb_cache_install::PreparedDownload;
use super::AppState;
use crate::core::loading::{
    DisplaySettings, LoadError, LoadProjectFullResult, LoadingPhase, ProgressEmitter,
};
use crate::core::{CommandResult, HistoryDropped};
use crate::models::survey::changeset::EntryLimits;
use crate::models::survey::Project;
use crate::persistence::migrate::{LegacyError, MigrateError};
use crate::persistence::stalx_v2::{
    decode, sniff, DecodeError, DecodeOptions, Fingerprint, Format, History, MAX_PROJECT_FILE_BYTES,
};
use crate::speleodb::SpeleoDbError;

/// A project file read into memory, not yet committed into the slot.
pub(crate) struct Opened {
    pub(crate) project: Project,
    pub(crate) file: FileState,
    /// The history the file holds (version 2), loaded into the stacks at
    /// commit; empty for a migrated file.
    pub(crate) history: History,
    /// Why the history was dropped before the commit (a legacy file's).
    pub(crate) history_dropped: Option<HistoryDropped>,
    /// Shots a legacy file stored with a kind 0.6.0 could not read (#27),
    /// for the load's notice; empty for a version 2 file.
    pub(crate) unrecognised_shot_kinds: Vec<crate::models::UnrecognisedShotKind>,
}

impl Opened {
    /// Whether the file at the project's path still holds the bytes this was
    /// read from. Exact only under the `project` guard (every write of ours
    /// to the open file holds it); a file that grew, shrank, changed or went
    /// away does not.
    fn still_on_disk(&self) -> bool {
        let (Some(path), Some(expected)) = (
            self.project.file_path.as_deref(),
            self.file.fingerprint.as_ref(),
        ) else {
            return true;
        };
        Fingerprint::read(path, expected.len).is_ok_and(|(_, found)| found.same_content(expected))
    }
}

/// How many times an open reads a file that changed between its read and its
/// commit; the last read is committed whatever the file holds by then.
const OPEN_ATTEMPTS: usize = 3;

/// What [`AppState::commit_opened`] does with a read the file no longer
/// holds (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IfChanged {
    /// Refuse it: the caller reads the file again.
    ReadAgain,
    /// Commit it as read (the last attempt): the guard reports the change at
    /// the next save.
    OpenAsRead,
}

impl IfChanged {
    fn of_attempt(attempt: usize) -> Self {
        if attempt < OPEN_ATTEMPTS {
            Self::ReadAgain
        } else {
            Self::OpenAsRead
        }
    }
}

/// The file changed between the read and the commit (see the module docs).
#[derive(Debug)]
pub(crate) struct ChangedWhileOpening;

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
        // Not damaged: a feature this build does not know may relax the rule
        // it breaks here. The newer-version prompt, never the repair offer
        // (review finding C-file-format-2).
        DecodeError::NewerFeatures { features, .. } => {
            LoadError::newer_features(&file_name(path), features, &path_text(path))
        }
        other => LoadError::corrupt(other.to_string(), &path_text(path)),
    }
}

/// The load error of a migration refusal.
pub(super) fn migrate_error(path: &Path, error: &MigrateError) -> LoadError {
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
        MigrateError::Legacy(LegacyError::RefusedSchema(refused)) => {
            LoadError::unsafe_file(refused, &path_text(path))
        }
        MigrateError::Legacy(LegacyError::Io(e)) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                LoadError::permission(e.to_string(), &path_text(path))
            } else {
                LoadError::new("IO", e.to_string())
            }
        }
        refusal if refusal.is_conversion_refusal() => {
            tracing::warn!(
                class = refusal.class(),
                purpose = "never call a healthy file corrupt",
                "this build cannot convert a legacy project"
            );
            LoadError::cannot_convert(&conversion_refusal_reason(refusal), &path_text(path))
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

/// What a file the conversion refuses needs, in words
/// ([`MigrateError::is_conversion_refusal`]).
pub(super) fn conversion_refusal_reason(error: &MigrateError) -> String {
    match error {
        MigrateError::Legacy(LegacyError::TooLarge { max }) => format!(
            "it is larger than the {} MiB this version reads",
            max / (1024 * 1024)
        ),
        other => other.to_string(),
    }
}

impl AppState {
    /// Read the project file at `path` into memory (no state changes).
    pub(crate) fn read_project_file(&self, path: &Path) -> Result<Opened, LoadError> {
        let options = DecodeOptions::default();
        let (bytes, fingerprint) =
            Fingerprint::read(path, MAX_PROJECT_FILE_BYTES).map_err(|e| match e.kind() {
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
                    match note {
                        // Informative by the format's rule: a newer writer
                        // whose formulas change what users see also requires
                        // a feature, which opens the file read-only.
                        crate::persistence::stalx_v2::DecodeNote::OtherSolver { found }
                            if *found > crate::persistence::stalx_v2::SOLVER =>
                        {
                            tracing::warn!(
                                found,
                                ours = crate::persistence::stalx_v2::SOLVER,
                                "Saved by a newer solver; positions are derived with this \
                                 version's formulas"
                            );
                        }
                        _ => tracing::info!(note = ?note, "decode note"),
                    }
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
                let mut file =
                    FileState::from_v2(decoded.extras, fingerprint, Some(Arc::from(bytes)));
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
                    unrecognised_shot_kinds: Vec::new(),
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
                let mut extras = migrated.extras;
                let unrecognised_shot_kinds = std::mem::take(&mut extras.unrecognised_shot_kinds);
                let file = FileState::from_migrated(extras, Some(fingerprint), &migrated.report);
                Ok(Opened {
                    project,
                    file,
                    history: History::default(),
                    history_dropped: migrated
                        .had_history
                        .then_some(HistoryDropped::EarlierVersion),
                    unrecognised_shot_kinds,
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
    /// holds no lock. A lock held for the project that leaves is released
    /// first (detached). Returns the revision the commit made (read under its
    /// guard), the key of anything derived from the opened survey.
    ///
    /// When the file no longer holds the bytes `opened` was read from
    /// (checked under the guard; module docs), [`IfChanged::ReadAgain`]
    /// refuses with nothing committed: the caller reads it again.
    pub(crate) fn commit_opened(
        &self,
        opened: Opened,
        if_changed: IfChanged,
    ) -> Result<u64, ChangedWhileOpening> {
        self.speleodb_release_held_lock_detached();
        let mut proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !opened.still_on_disk() {
            if if_changed == IfChanged::ReadAgain {
                tracing::info!(
                    purpose = "keep the save guard on the bytes on disk",
                    "The project file changed while it was opened; reading it again"
                );
                return Err(ChangedWhileOpening);
            }
            tracing::warn!(
                attempts = OPEN_ATTEMPTS,
                purpose = "keep the save guard on the bytes on disk",
                "The project file kept changing while it was opened; opened as last read"
            );
        }
        self.commit_opened_locked(&mut proj_guard, opened, None);
        let revision = self.current_revision();
        drop(proj_guard);
        self.persist_pending_local_state();
        Ok(revision)
    }

    /// [`Self::commit_opened`] under the caller's `project` guard, in ONE
    /// acquisition with whatever the caller did before (the prepared reload
    /// installs the file first). `carry_from` is the session generation whose
    /// edit claim the reopened project keeps (a fast-forward that keeps its
    /// edit session); `None` re-derives the gate from the link alone. The
    /// caller runs [`Self::persist_pending_local_state`] once the guard is
    /// dropped.
    pub(super) fn commit_opened_locked(
        &self,
        proj_guard: &mut Option<Project>,
        opened: Opened,
        carry_from: Option<u64>,
    ) {
        let Opened {
            project,
            mut file,
            history,
            history_dropped,
            unrecognised_shot_kinds: _,
        } = opened;
        // A pull that migrated a legacy revision into this cache path left its
        // conversion notice for this open (`replace_cache_for_session`). Any
        // other open drops a pending notice: it described another install.
        let pending = self
            .pending_download_notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some((path, notice)) = pending {
            if file.conversion.is_none()
                && project
                    .file_path
                    .as_deref()
                    .is_some_and(|p| super::project_io::same_file(p, &path))
            {
                file.conversion = Some(notice);
            }
        }
        // A legacy file this session upgraded while it was closed (a Recents
        // rename, a SpeleoDB link strip) left its notice for this open. An open
        // that reads a legacy file there again (a restored original) has its own.
        if let Some(notice) = project
            .file_path
            .as_deref()
            .and_then(|p| self.take_pending_upgrade_notice(p))
        {
            if file.origin == super::open_file::FileOrigin::V2 && file.conversion.is_none() {
                file.conversion = Some(notice);
            }
        }
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
        let read_only = file.extras.read_only.is_some();
        let history_unreadable = history_dropped.is_some();
        *proj_guard = Some(project);
        {
            let mut session = self
                .speleodb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.bump_speleodb_session();
            self.speleodb_commit_open_gate(&mut session, link.as_ref(), carry_from);
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
        let history_unread = read_only || history_unreadable || dropped.is_some();
        self.with_file(|file| file.history_unread = history_unread);
        self.note_history_dropped(dropped);
        self.initial_save_deferred
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Write durable state the commit changed in memory only (a materialized
    /// undo mode), now that no guard is held. The one write an open of a
    /// version 2 file makes; it copies the file's history byte for byte
    /// (review finding J-docs-tests-truth-3). A failure leaves it in memory
    /// for the next save.
    pub(super) fn persist_pending_local_state(&self) {
        if self.with_file(|f| f.local_pending) {
            if let Err(e) = self.persist_local_state_keeping_history() {
                tracing::warn!(error = %e, "Could not write the materialized undo mode; it stays in memory");
            }
        }
    }

    /// Open a project (the executor's `Command::OpenProject` and tests): no
    /// progress events, no canonical-storage guard.
    pub(super) fn open_project(&self, path: PathBuf) -> CommandResult {
        let mut attempt = 1;
        loop {
            let opened = match self.read_project_file(&path) {
                Ok(opened) => opened,
                Err(error) => return CommandResult::failure(error.message),
            };
            if self
                .commit_opened(opened, IfChanged::of_attempt(attempt))
                .is_ok()
            {
                self.note_recent_project(&path);
                return CommandResult::success();
            }
            attempt += 1;
        }
    }

    /// Install a prepared download at `dest` and open it in place, in ONE
    /// acquisition of the `project` guard (the fast-forward and the revision
    /// swap): there is no window in which the old project is closed and the
    /// new one not yet open, so nothing can slip in between.
    ///
    /// `consent` is the open project the transition was decided over
    /// (generation and revision, [`OpenProjectToken`]), re-checked before and
    /// again under the guard together with the open link's identity: an edit
    /// made while the download ran, or a different project opened meanwhile,
    /// refuses with [`SpeleoDbError::SessionMoved`] before a byte is written.
    /// `editing` carries the edit claim bound to the consent's generation into
    /// the reopened project (a fast-forward inside an edit session); a reload
    /// never starts a session, so a claim released before it leaves the
    /// project read-only.
    ///
    /// Deliberately without the canonical-storage guard of
    /// `do_load_project_full` (`foreign_link`): `dest` is always the managed
    /// cache path of the project being reloaded.
    pub(super) fn open_project_download(
        &self,
        mut prepared: PreparedDownload,
        dest: PathBuf,
        consent: OpenProjectToken,
        editing: bool,
    ) -> Result<(), SpeleoDbError> {
        tracing::info!(
            path = ?dest,
            purpose = "reload a SpeleoDB project in place",
            "Installing and opening a downloaded project"
        );
        let link_matches = || {
            self.speleodb_open_link().is_ok_and(|live| {
                prepared.link.project_id == live.project_id
                    && crate::speleodb::auth::normalize_endpoint(&prepared.link.endpoint)
                        == crate::speleodb::auth::normalize_endpoint(&live.endpoint)
            })
        };
        {
            let _guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !self.open_project_token_holds_locked(consent) || !link_matches() {
                return Err(SpeleoDbError::SessionMoved);
            }
        }
        let mut proj_guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.open_project_token_holds_locked(consent) || !link_matches() {
            return Err(SpeleoDbError::SessionMoved);
        }
        if self
            .active_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
        {
            return Err(SpeleoDbError::SessionMoved);
        }
        let display = self.outgoing_display(proj_guard.as_ref(), &dest);
        let fingerprint = self.install_prepared(&mut prepared, &dest, display)?;
        let opened = prepared.into_opened(dest.clone(), fingerprint);
        let carry_from = editing.then_some(consent.generation);
        self.commit_opened_locked(&mut proj_guard, opened, carry_from);
        drop(proj_guard);
        self.persist_pending_local_state();
        self.note_recent_project(&dest);
        tracing::info!(
            path = ?dest,
            purpose = "reload a SpeleoDB project in place",
            "Project reloaded"
        );
        Ok(())
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
        progress.start_phase(LoadingPhase::OpeningFile);
        use super::speleodb_foreign::LinkedSpelling;
        let path = match self.foreign_link(path) {
            LinkedSpelling::AsSpelled => path.to_path_buf(),
            LinkedSpelling::Canonical(canonical) => canonical,
            LinkedSpelling::Foreign(managed) => return Err(LoadError::speleodb_managed(managed)),
        };
        if progress.is_cancelled() {
            return Err(LoadError::cancelled());
        }
        progress.complete_phase(LoadingPhase::OpeningFile);
        let mut attempt = 1;
        loop {
            let if_changed = IfChanged::of_attempt(attempt);
            if let Some(result) = self.load_full_once(&path, progress, if_changed)? {
                return Ok(result);
            }
            attempt += 1;
        }
    }

    /// One read, the views computed from it, and the commit; `None` when the
    /// commit refused the read because the file changed meanwhile.
    fn load_full_once(
        &self,
        path: &Path,
        progress: &mut ProgressEmitter,
        if_changed: IfChanged,
    ) -> Result<Option<LoadProjectFullResult>, LoadError> {
        let cancelled = |progress: &ProgressEmitter| {
            if progress.is_cancelled() {
                Err(LoadError::cancelled())
            } else {
                Ok(())
            }
        };
        progress.start_phase(LoadingPhase::LoadingProjectData);
        let opened = self.read_project_file(path)?;
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
        let warnings =
            super::warnings::opened_project_warnings(&opened.project, &raw, &opened.file);
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
        // The raw layout of the committed revision, keyed by the revision the
        // commit made: the load does not take `command_lock`, so an edit may
        // land before the seed, and keyed by the revision read now the opened
        // survey's layout would be cached as the edited one's. Keyed by the
        // commit's, it is merely stale then, and never read.
        let mut opened = opened;
        let unrecognised = std::mem::take(&mut opened.unrecognised_shot_kinds);
        if !unrecognised.is_empty() {
            tracing::warn!(
                count = unrecognised.len(),
                "Shots with an unreadable kind were loaded as Reference"
            );
        }
        self.set_unrecognised_shot_kinds(progress.loading_id(), unrecognised);
        let Ok(revision) = self.commit_opened(opened, if_changed) else {
            return Ok(None);
        };
        *self
            .derived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(super::app_state::DerivedCache::new(revision, raw));
        self.note_recent_project(path);
        progress.complete_phase(LoadingPhase::Finalizing);
        tracing::info!(path = ?path, purpose = "load project for workspace", "Project loaded via load_project_full");
        Ok(Some(LoadProjectFullResult::from_envelope(
            progress.loading_id(),
            envelope,
            warnings,
            display_settings,
            is_remote,
        )))
    }
}
