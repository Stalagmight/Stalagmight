//! What the open project's file holds besides the survey and its history
//! (stream AS: no live database handle).
//!
//! Before the station-graph switch every per-file fact lived in the open
//! SQLite database and was written in place. Now it is in memory in
//! [`FileState`] and reaches the disk two ways (plan 6.5):
//! - a normal save writes everything (survey, link, local state, ignores,
//!   history);
//! - a local-only durable save ([`AppState::persist_local_state`]) rewrites
//!   `speleodb`, `local` and `history` and copies every other member of the
//!   last saved file verbatim. SpeleoDB state changes and display toggles use
//!   it, so they are durable at once, as the in-place writes were. It works
//!   on read-only projects and unknown `requires`. It is never used on a
//!   legacy (SQLite) file not yet upgraded, nor on a project never saved:
//!   there the change waits in memory for the first real save
//!   (`local_pending`).
//!
//! Lock order: `project` -> `file` -> `undo_stack` (`file` takes the place
//! `database` had). Nothing is taken while `file` is held except
//! `undo_stack`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::models::SpeleoLink;
use crate::persistence::migrate::{LegacyParity, MigratedExtras, Verification};
use crate::persistence::stalx_v2::{
    write_atomic, Extras, Fingerprint, Guard, History, IgnoredWarning, LocalSync, SpeleoDbLink,
    WriteError,
};

use super::AppState;

/// Coded-failure prefix of a save refused because the file on disk changed
/// since it was last read or written (plan 6.5's external-modification
/// guard). The frontend matches `error.starts_with(EXTERNALLY_MODIFIED_CODE)`
/// and asks: reload, overwrite (`save_project_overwriting`), or save as.
pub const EXTERNALLY_MODIFIED_CODE: &str = "EXTERNALLY_MODIFIED";

/// Coded-failure prefix of a save refused because the project is open
/// read-only (an unknown `requires` feature or unknown structural content:
/// "made with a newer Stalagmight", plan 6.4).
pub const READ_ONLY_FILE_CODE: &str = "READ_ONLY_FILE";

/// Where the open project came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FileOrigin {
    /// Nothing open, or created in this session and never written.
    #[default]
    New,
    /// Read from, or written as, a `.stalx` version 2 file.
    V2,
    /// Migrated in memory from a legacy SQLite `.stalx`; the first save
    /// writes version 2 over it after moving the original into the backups
    /// folder (plan 7.2.10).
    Legacy,
}

/// The one-line notice an upgrade shows when the migration moved stations
/// (plan 7.2.10, G4: whenever `verification.moved().slots > 0` or anything is
/// unexplained).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConversionNotice {
    pub(crate) slots_moved: usize,
    pub(crate) max_displacement_m: f64,
    pub(crate) unexplained: usize,
}

impl ConversionNotice {
    pub(crate) fn of(verification: &Verification) -> Option<Self> {
        let moved = verification.moved();
        let unexplained = verification.mismatches.len();
        (moved.slots > 0 || unexplained > 0).then_some(Self {
            slots_moved: moved.slots,
            max_displacement_m: moved.max_displacement_m,
            unexplained,
        })
    }

    pub(crate) fn message(&self) -> String {
        format!(
            "Converted from the old file format; {} stations moved by up to {:.2} m; \
             the original is kept in Recovery.",
            self.slots_moved, self.max_displacement_m
        )
    }
}

/// The open project's per-file state (see the module docs).
#[derive(Debug, Default)]
pub(crate) struct FileState {
    /// Every member of the file but the survey and the history: SpeleoDB
    /// link identity, per-machine state (display settings, sync state, unwind
    /// chain, watermark), ignored warnings, preserved unknown members,
    /// `requires`, the read-only reason. `extras.history` is unused: the undo
    /// stack holds the history.
    pub(crate) extras: Extras,
    /// The `history` member as last read or written, which a local-only save
    /// writes while the survey has unsaved changes (the stacks then describe
    /// a body the file does not hold).
    pub(crate) saved_history: History,
    /// The bytes last read or written at the project's path (the guard).
    /// `None` until something of ours is on disk. For a legacy origin it is
    /// the SQLite file's, so the upgrading save is guarded too.
    pub(crate) fingerprint: Option<Fingerprint>,
    pub(crate) origin: FileOrigin,
    /// The survey (or its ignores) changed since the last save.
    pub(crate) dirty: bool,
    /// Durable state changed in memory that only the next real save can
    /// write (legacy origin or never saved).
    pub(crate) local_pending: bool,
    /// Shown once after a legacy open that moved stations.
    pub(crate) conversion: Option<ConversionNotice>,
    /// Record parity the legacy model kept for `.plnx` pins (dive-plan
    /// resolver, plan 10.3); `None` for version 2 files.
    pub(crate) legacy_parity: Option<LegacyParity>,
}

fn parse_time(text: Option<&String>) -> Option<DateTime<Utc>> {
    let text = text?;
    match DateTime::parse_from_rfc3339(text) {
        Ok(t) => Some(t.with_timezone(&Utc)),
        Err(e) => {
            tracing::warn!(error = %e, "an unreadable SpeleoDB time was dropped");
            None
        }
    }
}

impl FileState {
    /// A version 2 file just decoded.
    pub(crate) fn from_v2(extras: Extras, fingerprint: Fingerprint) -> Self {
        let mut extras = extras;
        let saved_history = std::mem::take(&mut extras.history);
        Self {
            extras,
            saved_history,
            fingerprint: Some(fingerprint),
            origin: FileOrigin::V2,
            ..Self::default()
        }
    }

    /// A legacy file just migrated (its history is dropped, plan 7.2.8).
    pub(crate) fn from_migrated(
        migrated: MigratedExtras,
        fingerprint: Option<Fingerprint>,
        verification: &Verification,
    ) -> Self {
        let mut state = Self {
            fingerprint,
            origin: FileOrigin::Legacy,
            conversion: ConversionNotice::of(verification),
            legacy_parity: Some(migrated.legacy_parity),
            ..Self::default()
        };
        state.extras.local.display = migrated.display_settings;
        state.extras.ignored_warnings = migrated.ignored_warnings;
        state.set_speleo_link(migrated.speleo_link.as_ref());
        state.extras.local.speleodb.unwound_from = migrated.speleo_undo_chain;
        state.extras.local.speleodb.inherited_through =
            parse_time(migrated.speleo_undo_watermark.as_ref());
        state
    }

    /// The SpeleoDB link in the shape the SpeleoDB code reads (identity from
    /// `speleodb`, sync state from `local.speleodb`).
    pub(crate) fn speleo_link(&self) -> Option<SpeleoLink> {
        let link = self.extras.speleodb.as_ref()?;
        let sync = &self.extras.local.speleodb;
        Some(SpeleoLink {
            endpoint: link.endpoint.clone(),
            project_id: link.project_id.clone(),
            project_name: link.project_name.clone(),
            base_commit_sha: sync.base_commit.clone(),
            has_unpushed_edits: sync.unpushed_edits,
            last_edit_at: sync.last_edit_at.map(|t| t.to_rfc3339()),
            last_verify_at: sync.last_verify_at.map(|t| t.to_rfc3339()),
            undo_mode: link.undo_mode,
        })
    }

    /// Replace the link (identity and sync state); `None` unlinks and clears
    /// the SpeleoDB sync state, unwind chain and watermark included.
    pub(crate) fn set_speleo_link(&mut self, link: Option<&SpeleoLink>) {
        let Some(link) = link else {
            self.extras.speleodb = None;
            self.extras.local.speleodb = LocalSync::default();
            return;
        };
        self.extras.speleodb = Some(SpeleoDbLink {
            endpoint: link.endpoint.clone(),
            project_id: link.project_id.clone(),
            project_name: link.project_name.clone(),
            undo_mode: link.undo_mode,
        });
        let sync = &mut self.extras.local.speleodb;
        sync.base_commit.clone_from(&link.base_commit_sha);
        sync.unpushed_edits = link.has_unpushed_edits;
        sync.last_edit_at = parse_time(link.last_edit_at.as_ref());
        sync.last_verify_at = parse_time(link.last_verify_at.as_ref());
    }

    pub(crate) fn display_settings(&self) -> &BTreeMap<String, String> {
        &self.extras.local.display
    }

    pub(crate) fn ignored_warnings(&self) -> &[IgnoredWarning] {
        &self.extras.ignored_warnings
    }

    /// Whether the file can take a local-only durable save now.
    fn can_save_locally(&self) -> bool {
        self.origin == FileOrigin::V2 && self.fingerprint.is_some()
    }
}

/// Why a local-only save did not happen.
#[derive(Debug)]
pub(crate) enum LocalSaveError {
    /// The file on disk is not the one last read or written.
    ExternallyModified(String),
    Io(String),
    Encode(String),
}

impl std::fmt::Display for LocalSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExternallyModified(m) => write!(f, "{EXTERNALLY_MODIFIED_CODE}: {m}"),
            Self::Io(m) | Self::Encode(m) => f.write_str(m),
        }
    }
}

impl From<LocalSaveError> for String {
    fn from(value: LocalSaveError) -> Self {
        value.to_string()
    }
}

/// Read the file at `path` and check it is still the one `expected`
/// describes; returns its bytes.
pub(super) fn read_unchanged(
    path: &Path,
    expected: &Fingerprint,
) -> Result<Vec<u8>, LocalSaveError> {
    match Fingerprint::read(path, expected.len) {
        Ok((bytes, found)) if found.same_content(expected) => Ok(bytes),
        Ok(_) => Err(LocalSaveError::ExternallyModified(format!(
            "{} was changed by another program since it was opened",
            path.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            Err(LocalSaveError::ExternallyModified(format!(
                "{} was changed by another program since it was opened",
                path.display()
            )))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(LocalSaveError::ExternallyModified(format!(
                "{} was moved or deleted by another program since it was opened",
                path.display()
            )))
        }
        Err(e) => Err(LocalSaveError::Io(format!(
            "could not read {}: {e}",
            path.display()
        ))),
    }
}

pub(super) fn write_error(e: &WriteError) -> LocalSaveError {
    match e {
        WriteError::ExternallyModified { .. } | WriteError::Exists { .. } => {
            LocalSaveError::ExternallyModified(e.to_string())
        }
        WriteError::Io { .. } => LocalSaveError::Io(e.to_string()),
    }
}

impl AppState {
    /// Run `f` on the open project's file state.
    pub(crate) fn with_file<R>(&self, f: impl FnOnce(&mut FileState) -> R) -> R {
        let mut guard = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }

    /// The open project's SpeleoDB link (in memory, the authority).
    pub(crate) fn speleo_link(&self) -> Option<SpeleoLink> {
        self.with_file(|file| file.speleo_link())
    }

    /// Change the link in memory and make it durable at once (local-only
    /// save). `f` returns whether it changed anything. Takes the `project`
    /// lock: a caller holding that guard uses
    /// [`Self::mutate_speleo_link_locked`].
    pub(crate) fn mutate_speleo_link(
        &self,
        f: impl FnOnce(&mut Option<SpeleoLink>) -> bool,
    ) -> Result<(), String> {
        let path = self.open_file_path();
        self.mutate_speleo_link_at(path.as_deref(), f)
    }

    /// [`Self::mutate_speleo_link`] for a caller that holds the `project`
    /// guard: `open` is the project in that guard (its file is the one the
    /// local-only save rewrites).
    pub(crate) fn mutate_speleo_link_locked(
        &self,
        open: Option<&crate::models::survey::Project>,
        f: impl FnOnce(&mut Option<SpeleoLink>) -> bool,
    ) -> Result<(), String> {
        self.mutate_speleo_link_at(open.and_then(|p| p.file_path.as_deref()), f)
    }

    fn mutate_speleo_link_at(
        &self,
        path: Option<&Path>,
        f: impl FnOnce(&mut Option<SpeleoLink>) -> bool,
    ) -> Result<(), String> {
        let changed = self.with_file(|file| {
            let mut link = file.speleo_link();
            let changed = f(&mut link);
            if changed {
                file.set_speleo_link(link.as_ref());
            }
            changed
        });
        if changed {
            self.persist_local_state_at(path)?;
        }
        Ok(())
    }

    /// The local-only durable save (plan 6.5 mode 4): rewrite `speleodb`,
    /// `local` and `history` of the file on disk, copying every other member
    /// verbatim. A legacy or never-saved project keeps the change in memory
    /// for its first real save (`local_pending`). Guarded: a file changed on
    /// disk since it was last read or written is not touched. Takes the
    /// `project` lock (for the path): a caller holding that guard uses
    /// [`Self::persist_local_state_locked`].
    pub(crate) fn persist_local_state(&self) -> Result<(), LocalSaveError> {
        let path = self.open_file_path();
        self.persist_local_state_at(path.as_deref())
    }

    /// [`Self::persist_local_state`] for a caller that holds the `project`
    /// guard: `open` is the project in that guard.
    pub(crate) fn persist_local_state_locked(
        &self,
        open: Option<&crate::models::survey::Project>,
    ) -> Result<(), LocalSaveError> {
        self.persist_local_state_at(open.and_then(|p| p.file_path.as_deref()))
    }

    fn persist_local_state_at(&self, path: Option<&Path>) -> Result<(), LocalSaveError> {
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (Some(path), true) = (path, file.can_save_locally()) else {
            file.local_pending = true;
            return Ok(());
        };
        let Some(expected) = file.fingerprint.clone() else {
            file.local_pending = true;
            return Ok(());
        };
        let original = read_unchanged(path, &expected)?;
        let mut extras = file.extras.clone();
        extras.history = if file.dirty {
            file.saved_history.clone()
        } else {
            self.undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .persisted()
        };
        let bytes = crate::persistence::stalx_v2::rewrite_local(&original, &extras)
            .map_err(|e| LocalSaveError::Encode(e.to_string()))?;
        let fingerprint =
            write_atomic(path, &bytes, Guard::Expect(&expected)).map_err(|e| write_error(&e))?;
        file.fingerprint = Some(fingerprint);
        file.saved_history = extras.history;
        file.local_pending = false;
        Ok(())
    }

    /// The conversion notice of a legacy open, taken once.
    pub fn take_conversion_notice(&self) -> Option<String> {
        self.with_file(|file| file.conversion.take().map(|n| n.message()))
    }

    /// Whether the open project has changes no save has written yet.
    pub fn has_unsaved_changes(&self) -> bool {
        self.with_file(|file| file.dirty)
    }

    /// The path the open project saves to, if any.
    pub(crate) fn open_file_path(&self) -> Option<PathBuf> {
        self.project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|p| p.file_path.clone())
    }
}
