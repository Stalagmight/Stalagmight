//! Read a downloaded project into memory and check it before the short,
//! session-owned cache commit.
//!
//! A download is never installed as it came. [`AppState::prepare_download`]
//! sniffs it (plan 6.1, 10.1): a version 2 file is decoded with the decoder's
//! size and count limits and its invariants, a legacy (SQLite) revision is
//! migrated in memory with the frozen 0.6.0 reader (plan 7). Either way the
//! result is a [`PreparedDownload`]: the survey, the link the cache will carry
//! (identity, BASE, a clean dirty bit, the project's undo policy), and the
//! history rule applied ([`InheritedUndo`]). Only then is anything locked or
//! written, and the cache file is always version 2: a legacy revision is
//! upgraded on install (its original stays on the server).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::open_file::FileState;
use super::project_load::Opened;
use super::speleodb_cache::{InheritedUndo, LocalEditsPolicy};
use super::AppState;
use crate::core::HistoryDropped;
use crate::models::survey::Project;
use crate::models::SpeleoLink;
use crate::persistence::migrate::{LegacyError, LegacyParity, MigrateError};
use crate::persistence::stalx_v2::{
    decode, encode, rewrite_local, sniff, DecodeError, DecodeNote, DecodeOptions, Extras,
    Fingerprint, Format, History, LocalState, Mode,
};
use crate::speleodb::SpeleoDbError;

/// The name a downloaded project goes by in a refusal the user reads.
const DOWNLOADED_FILE: &str = "project.stalx";

/// The refusal of a version 2 download the decoder would not take.
fn decode_refusal(error: &DecodeError) -> SpeleoDbError {
    match error {
        DecodeError::NewerVersion { found, supported } => {
            SpeleoDbError::NewerSchema(crate::models::NewerSchema {
                file: DOWNLOADED_FILE.into(),
                found: i32::try_from(*found).unwrap_or(i32::MAX),
                supported: i32::try_from(*supported).unwrap_or(i32::MAX),
            })
        }
        other => SpeleoDbError::Parse(format!("the downloaded project file was refused: {other}")),
    }
}

/// The refusal of a legacy download the migration would not take.
fn migrate_refusal(error: &MigrateError) -> SpeleoDbError {
    match error {
        MigrateError::Legacy(LegacyError::NewerSchema { found, supported }) => {
            SpeleoDbError::NewerSchema(crate::models::NewerSchema {
                file: DOWNLOADED_FILE.into(),
                found: *found,
                supported: *supported,
            })
        }
        MigrateError::Legacy(LegacyError::Io(e)) => {
            SpeleoDbError::Io(format!("could not read the downloaded project: {e}"))
        }
        other => {
            tracing::warn!(
                class = other.class(),
                purpose = "install a downloaded legacy revision",
                "speleodb: a downloaded legacy project could not be migrated"
            );
            SpeleoDbError::Parse(format!("the downloaded project file was refused: {other}"))
        }
    }
}

pub(super) fn sync_cache_directory(dest: &Path) {
    #[cfg(unix)]
    if let Some(parent) = dest.parent() {
        if let Err(error) = std::fs::File::open(parent).and_then(|dir| dir.sync_all()) {
            tracing::warn!(%error, "could not synchronize the installed cache directory");
        }
    }
    #[cfg(not(unix))]
    let _ = dest;
}

/// A downloaded project, read and checked in memory, ready to install.
#[derive(Debug)]
pub(super) struct PreparedDownload {
    /// The survey the download holds (load never mutates).
    pub(super) project: Project,
    /// What the cache file will hold besides the survey: the link stamped in,
    /// no per-machine state from the machine that pushed it, the history the
    /// [`InheritedUndo`] rule leaves.
    pub(super) extras: Extras,
    /// The bytes of a version 2 download: the installed copy takes their
    /// survey members verbatim (`rewrite_local`), which also works for a file
    /// that opens read-only (unknown content from a newer build). `None` for a
    /// migrated legacy revision, which is encoded at install.
    original: Option<Vec<u8>>,
    /// The link the cache is stamped with.
    pub(super) link: SpeleoLink,
    /// Why the downloaded history was dropped when it was meant to be kept.
    pub(super) history_dropped: Option<HistoryDropped>,
    /// A migrated legacy revision's record parity (dive-plan pins, plan 10.3).
    legacy_parity: Option<LegacyParity>,
}

impl PreparedDownload {
    /// The bytes the cache file gets, with `display` (the outgoing copy's view
    /// toggles) as its per-machine view.
    pub(super) fn cache_bytes(
        &mut self,
        display: BTreeMap<String, String>,
    ) -> Result<Vec<u8>, SpeleoDbError> {
        self.extras.local.display = display;
        let bytes = match &self.original {
            Some(original) => rewrite_local(original, &self.extras),
            None => encode(&self.project, &self.extras, Mode::Normal),
        };
        bytes.map_err(|e| SpeleoDbError::Io(format!("could not write the downloaded project: {e}")))
    }

    /// The opened project the prepared reload commits, now that its bytes are
    /// at `dest` with `fingerprint`.
    pub(super) fn into_opened(self, dest: PathBuf, fingerprint: Fingerprint) -> Opened {
        let Self {
            mut project,
            extras,
            history_dropped,
            legacy_parity,
            ..
        } = self;
        project.file_path = Some(dest);
        let mut file = FileState::from_v2(extras, fingerprint);
        file.legacy_parity = legacy_parity;
        let history = file.saved_history.clone();
        Opened {
            project,
            file,
            history,
            history_dropped,
            unrecognised_shot_kinds: Vec::new(),
        }
    }
}

impl AppState {
    /// Read the download at `source` and check it (see the module docs), then
    /// stamp `link` into it: the project's own undo policy from the download
    /// (materialized with this machine's default when the download has none,
    /// #303), a clean dirty bit, and none of the pushing machine's per-machine
    /// state. `undo` decides the downloaded history: [`InheritedUndo::Drop`]
    /// empties it, the Unwind's waiver keeps the undo entries (a push never
    /// carries redo). Nothing on disk changes.
    pub(super) fn prepare_download(
        &self,
        source: &Path,
        mut link: SpeleoLink,
        undo: InheritedUndo,
        options: &DecodeOptions,
    ) -> Result<PreparedDownload, SpeleoDbError> {
        let (bytes, _) =
            Fingerprint::read(source, options.max_file_bytes).map_err(|e| match e.kind() {
                std::io::ErrorKind::InvalidData => {
                    SpeleoDbError::Parse(format!("the downloaded project file was refused: {e}"))
                }
                _ => SpeleoDbError::Io(format!("could not read the downloaded project: {e}")),
            })?;
        let (project, mut extras, original, unreadable, legacy_parity) = match sniff(&bytes) {
            Format::V2 => {
                let decoded = decode(&bytes, options).map_err(|e| decode_refusal(&e))?;
                let unreadable = decoded
                    .notes
                    .iter()
                    .any(|n| matches!(n, DecodeNote::HistoryDropped { .. }));
                if let Some(reason) = decoded.read_only() {
                    tracing::warn!(
                        features = ?reason.unknown_features,
                        unknown_content = reason.unknown_content_total,
                        purpose = "install a downloaded project",
                        "speleodb: the download was made with a newer Stalagmight; it opens read-only"
                    );
                }
                (
                    decoded.project,
                    decoded.extras,
                    Some(bytes),
                    unreadable.then_some(HistoryDropped::Unreadable),
                    None,
                )
            }
            Format::Legacy => {
                drop(bytes);
                let config = self.get_config();
                let migrated = crate::persistence::migrate::migrate_path_for_app(
                    source,
                    &config.closure_adjustment,
                )
                .map_err(|e| migrate_refusal(&e))?;
                tracing::info!(
                    stations = migrated.report.stations,
                    clean = migrated.report.verification.is_clean(),
                    moved = migrated.report.verification.moved().slots,
                    purpose = "install a downloaded legacy revision",
                    "speleodb: migrated a legacy revision in memory; the cache gets version 2"
                );
                let mut project = migrated.project;
                if !migrated.extras.validation_stored {
                    project.settings.validation = config.validation;
                }
                let mut file = FileState::default();
                file.set_speleo_link(migrated.extras.speleo_link.as_ref());
                let mut extras = file.extras;
                extras.ignored_warnings = migrated.extras.ignored_warnings;
                extras.generator = super::project_io::generator();
                (
                    project,
                    extras,
                    None,
                    migrated
                        .had_history
                        .then_some(HistoryDropped::EarlierVersion),
                    Some(migrated.extras.legacy_parity),
                )
            }
            Format::Unknown => {
                return Err(SpeleoDbError::Parse(
                    "the downloaded project file was refused: it is not a Stalagmight project"
                        .to_owned(),
                ))
            }
        };
        // [#303] The project's undo policy rides the server copy (it is project
        // state, not machine state), so the downloaded value is the authority; a
        // copy that has none gets this machine's default, made explicit.
        let downloaded_mode = extras.speleodb.as_ref().and_then(|l| l.undo_mode);
        link.undo_mode = Some(self.effective_undo_mode(downloaded_mode));
        // Nothing per-machine from the machine that pushed it: its view, its sync
        // state, its Unwind position and watermark (a push never carries `local`,
        // so this only matters for a file that reached the server another way).
        extras.local = LocalState::default();
        let mut file = FileState {
            extras,
            ..FileState::default()
        };
        file.set_speleo_link(Some(&link));
        let mut extras = file.extras;
        // The Unwind's waiver is the only caller that keeps the undo entries
        // (`undo_history_keep_is_unwind_only` counts the sites that ask for it).
        let history_dropped = if undo == InheritedUndo::Drop {
            extras.history = History::default();
            None
        } else {
            extras.history.redo.clear();
            unreadable
        };
        Ok(PreparedDownload {
            project,
            extras,
            original,
            link,
            history_dropped,
            legacy_parity,
        })
    }

    /// Whether a download reserved under `generation` can no longer install:
    /// a newer open or login moved the session, or a native open is loading.
    fn download_is_stale(&self, generation: u64) -> bool {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.speleodb_session_generation() != generation
            || self
                .active_loading
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
    }

    pub(super) fn reserve_download_generation(&self) -> u64 {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.bump_speleodb_session();
        self.speleodb_session_generation()
    }

    /// Install a prepared download at `dest` without opening it (the pull: the
    /// frontend opens the cache path afterwards). Network and validation happen
    /// before this commit. A newer open or login invalidates the download
    /// (`generation`); a pending native reader excludes replacement too. When
    /// `dest` is the open project it is closed, under the same `project`
    /// acquisition as the write, so nothing can land between the two.
    pub(super) fn replace_cache_for_session(
        &self,
        mut prepared: PreparedDownload,
        dest: &Path,
        project_id: &str,
        policy: LocalEditsPolicy,
        generation: u64,
    ) -> Result<(), SpeleoDbError> {
        // A download the session has already moved past is refused as such, before
        // the guard reads the copy: there is nothing to ask the user about.
        if self.download_is_stale(generation) {
            return Err(SpeleoDbError::SessionMoved);
        }
        if policy == LocalEditsPolicy::Protect {
            self.guard_local_edits(project_id, dest)?;
        }
        // The view toggles of the copy on disk, read before any lock is taken: a
        // legacy copy is read through the migration, which must not run under the
        // `project` guard. Another install cannot land in between: it would move
        // the session generation, which the commit below checks.
        let on_disk_display = super::speleodb_cache::file_display(dest);
        let _lock_ops = self.speleodb_lock_ops_guard();
        let mut project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let loading = self
            .active_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.speleodb_session_generation() != generation || loading.is_some() {
            return Err(SpeleoDbError::SessionMoved);
        }
        drop(loading);
        let replaces_open = project
            .as_ref()
            .and_then(|p| p.file_path.as_deref())
            .is_some_and(|path| super::project_io::same_file(path, dest));
        if replaces_open && policy == LocalEditsPolicy::Protect {
            // The open project's link in memory is the authority for its own file:
            // an edit since the guard above read the disk counts.
            let link = self.speleo_link();
            if link.as_ref().is_none_or(|link| link.has_unpushed_edits) {
                return Err(SpeleoDbError::WouldDiscardLocalEdits {
                    project_id: project_id.into(),
                    local_path: dest.to_string_lossy().into_owned(),
                    last_edit_at: link.and_then(|link| link.last_edit_at),
                });
            }
        }
        // The open copy's toggles in memory are the authority for its own file.
        let display = if replaces_open {
            self.with_file(|file| file.display_settings().clone())
        } else {
            on_disk_display
        };
        let bytes = prepared.cache_bytes(display)?;
        super::speleodb_reload::install_cache_bytes(dest, &bytes)?;
        if replaces_open {
            self.close_open_project_locked(&mut project);
        } else {
            // Installing a cache is itself a completed mutation, even from home.
            self.bump_speleodb_session();
        }
        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "synthetic cache durability tests")]
mod tests {
    use super::*;
    use crate::core::state::stalx_v2_fixtures::{
        extras_for, fixture_link, history_counts, line_survey, link_at, name_at, seed_linked,
        seed_with_history, write_file,
    };
    use crate::persistence::stalx_v2::temp_path;

    #[test]
    fn a_failed_install_preserves_the_cache_and_cleans_the_stage() {
        let dir = tempfile::tempdir().expect("temp");
        let source = dir.path().join("source.stalx");
        let dest = dir.path().join("working.stalx");
        seed_linked(&source, "Synthetic", &fixture_link("https://x", false));
        seed_linked(&dest, "Working", &fixture_link("https://x", false));
        let before = std::fs::read(&dest).expect("old bytes");
        // A directory where the install writes its temp: the write fails before the
        // rename, as a full disk or a permission denial would.
        std::fs::create_dir(temp_path(&dest)).expect("block the temp");
        std::fs::write(temp_path(&dest).join("keep"), b"x").expect("non-empty");
        let state = AppState::new().expect("state");
        let prepared = state
            .prepare_download(
                &source,
                fixture_link("https://x", false),
                InheritedUndo::Keep,
                &DecodeOptions::default(),
            )
            .expect("prepare");
        let result = state.replace_cache_for_session(
            prepared,
            &dest,
            crate::core::state::stalx_v2_fixtures::FIXTURE_ID,
            LocalEditsPolicy::Discard,
            state.speleodb_session_generation(),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(&dest).expect("unchanged"), before);
    }

    #[test]
    fn a_version_2_download_keeps_its_survey_members_byte_for_byte() {
        let dir = tempfile::tempdir().expect("temp");
        let source = dir.path().join("source.stalx");
        let mut server = fixture_link("https://server.example", false);
        server.base_commit_sha = None;
        write_file(
            &source,
            &line_survey("Surveyed"),
            &extras_for(Some(&server)),
        );
        let state = AppState::new().expect("state");
        let mut prepared = state
            .prepare_download(
                &source,
                fixture_link("https://x", false),
                InheritedUndo::Drop,
                &DecodeOptions::default(),
            )
            .expect("prepare");
        let bytes = prepared.cache_bytes(BTreeMap::new()).expect("bytes");
        let original = std::fs::read(&source).expect("source");
        let body = |text: &[u8]| {
            let text = String::from_utf8_lossy(text).into_owned();
            // Top-level members start a physical line (plan 6.2); the summary also
            // has a `stations` key, so the newline is what finds the member.
            let start = text
                .find("\n\"sections\"")
                .or_else(|| text.find("\n\"stations\""));
            start.map(|at| text[at..].to_owned()).unwrap_or_default()
        };
        assert!(!body(&original).is_empty());
        assert_eq!(body(&bytes), body(&original), "the body is copied verbatim");
    }

    #[test]
    fn the_prepared_link_takes_the_downloads_undo_policy_and_drops_machine_state() {
        let dir = tempfile::tempdir().expect("temp");
        let source = dir.path().join("source.stalx");
        let mut server = fixture_link("https://old.example", true);
        server.undo_mode = Some(crate::models::UndoMode::Disabled);
        seed_with_history(&source, "Pushed", &server, 3, 1);
        let state = AppState::new().expect("state");
        let mut prepared = state
            .prepare_download(
                &source,
                fixture_link("https://example.com", false),
                InheritedUndo::Drop,
                &DecodeOptions::default(),
            )
            .expect("prepare");
        assert_eq!(
            prepared.link.undo_mode,
            Some(crate::models::UndoMode::Disabled),
            "the project's own policy, not this machine's default"
        );
        let dest = dir.path().join("cache").join("project.stalx");
        let bytes = prepared.cache_bytes(BTreeMap::new()).expect("bytes");
        super::super::speleodb_reload::install_cache_bytes(&dest, &bytes).expect("install");
        let link = link_at(&dest).expect("linked");
        assert_eq!(link.endpoint, "https://example.com");
        assert_eq!(link.base_commit_sha.as_deref(), Some("abc123"));
        assert!(!link.has_unpushed_edits);
        assert_eq!(link.undo_mode, Some(crate::models::UndoMode::Disabled));
        assert_eq!(history_counts(&dest), (0, 0));
        assert_eq!(name_at(&dest), "Pushed");
    }
}
