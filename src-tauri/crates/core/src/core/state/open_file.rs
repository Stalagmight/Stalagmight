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
//!   (`local_pending`). On a legacy file it counts as unsaved
//!   ([`AppState::has_unsaved_changes`]) only when a close that drops it
//!   changes what the user or the server sees (the rule on
//!   `FileState::local_pending_asks`): not a display toggle
//!   ([`AppState::persist_display_state`]: it never arms autosave, which
//!   would convert the file), a materialized undo mode or a lock
//!   re-verification time; only a SpeleoDB unlink upgrades a legacy file at
//!   once ([`AppState::persist_local_state_upgrading`]).
//!
//! Lock order: `project` -> `file` -> `undo_stack` (`file` takes the place
//! `database` had). Nothing is taken while `file` is held except
//! `undo_stack`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::models::SpeleoLink;
use crate::persistence::migrate::{
    LegacyParity, MigrateError, Migrated, MigratedExtras, MigrationReport,
};
use crate::persistence::stalx_v2::{
    write_atomic, Extras, Fingerprint, Guard, History, IgnoredWarning, LocalSync, SpeleoDbLink,
    WriteError,
};

use super::conversion_notice::{ConversionNotice, ConversionNoticeView};
use super::AppState;

/// Coded-failure prefix of a save refused because the file on disk changed
/// since it was last read or written (plan 6.5's external-modification
/// guard). The frontend matches `error.starts_with(EXTERNALLY_MODIFIED_CODE)`
/// and asks: reload, overwrite (`save_project_overwriting`), or save as.
pub const EXTERNALLY_MODIFIED_CODE: &str = "EXTERNALLY_MODIFIED";

/// Marker right after [`EXTERNALLY_MODIFIED_CODE`] when the guard refused
/// because the file is GONE (moved or deleted by another program):
/// `EXTERNALLY_MODIFIED: FILE_MISSING: <path> was moved or deleted ...`. The
/// prefix still routes it to the save prompt; the marker tells the prompt
/// that there is no file's version to reopen (the reopen would fail after the
/// close dropped the unsaved changes).
pub const FILE_MISSING_MARKER: &str = "FILE_MISSING";

/// Marker right after [`EXTERNALLY_MODIFIED_CODE`] when a FIRST save (the
/// project's path was chosen free) found a file at that path
/// (`WriteError::Exists`, from the guard's check or from the hard-link
/// placement): `EXTERNALLY_MODIFIED: FILE_EXISTS: <path> already exists`.
/// The file there is not this project's at all, so the prompt offers no
/// "Use the file's version" (it would open a stranger's file in place of the
/// project) and says that a file appeared at this location (pre-PR delta
/// review finding 5).
pub const FILE_EXISTS_MARKER: &str = "FILE_EXISTS";

/// The coded failure of a write the guard refused (`ExternallyModified`,
/// `Exists`), as every save path sends it to the frontend:
/// [`EXTERNALLY_MODIFIED_CODE`], then [`FILE_MISSING_MARKER`] when the file
/// is gone or [`FILE_EXISTS_MARKER`] when a first save found one, then the
/// sentence.
pub(crate) fn guard_refusal(e: &WriteError) -> String {
    write_error(e).to_string()
}

/// Coded-failure prefix of a save refused because the project is open
/// read-only (an unknown `requires` feature or unknown structural content:
/// "made with a newer Stalagmight", plan 6.4).
pub const READ_ONLY_FILE_CODE: &str = "READ_ONLY_FILE";

/// How many times [`AppState::persist_local_state_upgrading`] migrates a
/// legacy file our own write changed meanwhile; the last migration runs
/// under the `project` guard, so it is the one written.
const UPGRADE_ATTEMPTS: usize = 3;

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
    /// The file's history did not load into the stacks (an unreadable member
    /// or entry, a newer history format) or the file is read-only: until a
    /// real save writes the stacks, a local-only save copies the file's
    /// `history` member verbatim instead of writing the (empty or partial)
    /// stacks over it.
    pub(crate) history_unread: bool,
    /// The bytes last read or written at the project's path (the guard).
    /// `None` until something of ours is on disk. For a legacy origin it is
    /// the SQLite file's, so the upgrading save is guarded too.
    pub(crate) fingerprint: Option<Fingerprint>,
    /// Those bytes themselves, with the fingerprint computed from them, where
    /// the read or write that set the fingerprint kept them. A local-only
    /// save rewrites from them instead of reading the file first, so a
    /// display toggle reads the file once (the guard's check), not twice,
    /// while it holds the project lock (review finding C-file-format-10).
    /// Used only while its fingerprint is the guard's: any other write of
    /// the fingerprint makes it miss, and the save reads the file as before.
    pub(crate) saved_bytes: Option<(Fingerprint, Arc<[u8]>)>,
    pub(crate) origin: FileOrigin,
    /// The survey (or its ignores) changed since the last save.
    pub(crate) dirty: bool,
    /// Durable state changed in memory that only the next real save can
    /// write (legacy origin or never saved).
    pub(crate) local_pending: bool,
    /// Part of that state is something the close prompt must ask about. The
    /// rule (pre-PR delta review finding 3): a deferred change asks when a
    /// close that drops it changes what the user or the server sees
    /// afterwards, and only then:
    /// - asks: the SpeleoDB name fold (the next open shows the old name), the
    ///   link's sync state (the dirty bit, BASE, the Unwind chain pointer,
    ///   the inherited-history watermark), the history a confirmed push
    ///   pruned (the next push would send the pruned entries again);
    /// - does not ask: what the next open derives again identically (an undo
    ///   mode materialized from this machine's default, which every open of
    ///   the 0.6.0 file materializes the same way), what only paces a re-check
    ///   (the lock re-verification time: the next edit re-verifies sooner),
    ///   and view state (a display toggle, demo issue I-6).
    ///
    /// Asking about the second kind on a 0.6.0 file would ask "unsaved
    /// changes" on every exit from a file the user only looked at, and arm
    /// autosave, which would convert it. Either kind waits for the next save,
    /// which writes it with the conversion.
    pub(crate) local_pending_asks: bool,
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
    /// A version 2 file just decoded. `bytes`, when given, are the bytes
    /// `fingerprint` was computed from (kept for the local-only save).
    pub(crate) fn from_v2(
        extras: Extras,
        fingerprint: Fingerprint,
        bytes: Option<Arc<[u8]>>,
    ) -> Self {
        let mut extras = extras;
        let saved_history = std::mem::take(&mut extras.history);
        Self {
            extras,
            saved_history,
            saved_bytes: bytes.map(|b| (fingerprint.clone(), b)),
            fingerprint: Some(fingerprint),
            origin: FileOrigin::V2,
            ..Self::default()
        }
    }

    /// The bytes on disk if this state still holds the ones the guard
    /// expects (see [`Self::saved_bytes`]).
    fn bytes_for(&self, expected: &Fingerprint) -> Option<Arc<[u8]>> {
        self.saved_bytes
            .as_ref()
            .filter(|(fingerprint, bytes)| {
                fingerprint.same_content(expected) && bytes.len() as u64 == fingerprint.len
            })
            .map(|(_, bytes)| Arc::clone(bytes))
    }

    /// A legacy file just migrated (its history is dropped, plan 7.2.8).
    pub(crate) fn from_migrated(
        migrated: MigratedExtras,
        fingerprint: Option<Fingerprint>,
        report: &MigrationReport,
    ) -> Self {
        let mut state = Self {
            fingerprint,
            origin: FileOrigin::Legacy,
            conversion: ConversionNotice::of_migration(report, &migrated.names_changed),
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
        let same_project =
            self.extras.speleodb.as_ref().is_some_and(|old| {
                old.endpoint == link.endpoint && old.project_id == link.project_id
            });
        if !same_project {
            // A newer build's sync keys describe the link they were kept
            // with, never another project's.
            self.extras.local.speleodb.preserved.clear();
        }
        self.extras.speleodb = Some(SpeleoDbLink {
            endpoint: link.endpoint.clone(),
            project_id: link.project_id.clone(),
            project_name: link.project_name.clone(),
            undo_mode: link.undo_mode,
        });
        let sync = &mut self.extras.local.speleodb;
        if !same_project || sync.base_commit != link.base_commit_sha {
            // The survey digest describes the BASE it was recorded for (S2).
            sync.base_survey = None;
            sync.base_survey_edit = None;
        }
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
    /// The file is gone (moved or deleted): coded with
    /// [`FILE_MISSING_MARKER`] after the guard's code.
    Missing(String),
    /// A first save found a file at its path: coded with
    /// [`FILE_EXISTS_MARKER`] after the guard's code.
    Exists(String),
    Io(String),
    Encode(String),
}

impl std::fmt::Display for LocalSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExternallyModified(m) => write!(f, "{EXTERNALLY_MODIFIED_CODE}: {m}"),
            Self::Missing(m) => write!(f, "{EXTERNALLY_MODIFIED_CODE}: {FILE_MISSING_MARKER}: {m}"),
            Self::Exists(m) => write!(f, "{EXTERNALLY_MODIFIED_CODE}: {FILE_EXISTS_MARKER}: {m}"),
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
            Err(LocalSaveError::Missing(format!(
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
        WriteError::ExternallyModified {
            found_len: None, ..
        } => LocalSaveError::Missing(e.to_string()),
        WriteError::ExternallyModified { .. } => LocalSaveError::ExternallyModified(e.to_string()),
        WriteError::Exists { .. } => LocalSaveError::Exists(e.to_string()),
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
    /// save). `f` returns whether it changed anything. Tests only: the app
    /// changes the link through `speleodb_mutate_link_held` and the landing.
    #[cfg(test)]
    pub(crate) fn mutate_speleo_link(
        &self,
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
            self.persist_local_state()?;
        }
        Ok(())
    }

    /// The local-only durable save (plan 6.5 mode 4): rewrite `speleodb`,
    /// `local` and `history` of the file on disk, copying every other member
    /// verbatim. A legacy or never-saved project keeps the change in memory
    /// for its first real save (`local_pending`). Guarded: a file changed on
    /// disk since it was last read or written is not touched.
    pub(crate) fn persist_local_state(&self) -> Result<(), LocalSaveError> {
        self.persist_local_state_guarded(false, true)
    }

    /// [`Self::persist_local_state`] for a display toggle (per-machine view
    /// state): on a legacy or never-saved project the change waits for the
    /// next save as any local state does, but it alone does not make the
    /// project unsaved ([`Self::has_unsaved_changes`], demo issue I-6).
    pub(crate) fn persist_display_state(&self) -> Result<(), LocalSaveError> {
        self.persist_local_state_guarded(false, false)
    }

    /// [`Self::persist_local_state`] for the write an OPEN makes (a
    /// materialized undo mode): it copies the file's `history` member byte
    /// for byte, so an open never changes the history on disk, whatever the
    /// persisted window (review finding J-docs-tests-truth-3). Deferred on a
    /// 0.6.0 file, it does not make the project unsaved: every open derives
    /// the same mode again (the rule on [`FileState::local_pending_asks`]).
    pub(super) fn persist_local_state_keeping_history(&self) -> Result<(), LocalSaveError> {
        self.persist_local_state_guarded(true, false)
    }

    /// [`Self::persist_local_state_at`] for state whose loss at a close
    /// changes nothing the user or the server sees (the lock re-verification
    /// time): deferred on a 0.6.0 file, it does not make the project unsaved
    /// (the rule on [`FileState::local_pending_asks`]).
    pub(crate) fn persist_unasked_state_at(
        &self,
        path: Option<PathBuf>,
    ) -> Result<(), LocalSaveError> {
        self.persist_local_state_with(path, false, false)
    }

    /// `asks`: a change deferred on a legacy file makes the project unsaved
    /// (every local state but a display toggle).
    fn persist_local_state_guarded(
        &self,
        keep_history: bool,
        asks: bool,
    ) -> Result<(), LocalSaveError> {
        // Under the `project` guard, for the whole save: every change of the
        // survey, of the stacks and of the dirty flag happens under it (an
        // undo pops its entry before it marks the project dirty), so only
        // under it do the flag and the stacks describe the body on disk.
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = guard.as_ref().and_then(|p| p.file_path.clone());
        let result = self.persist_local_state_with(path, keep_history, asks);
        drop(guard);
        result
    }

    /// [`Self::persist_local_state`] for a caller that already holds the
    /// `project` guard (which is not reentrant): `path` is the open project's
    /// `file_path`, read through that guard. Takes `file`, then `undo_stack`
    /// and `config` (the persisted window).
    pub(crate) fn persist_local_state_at(
        &self,
        path: Option<PathBuf>,
    ) -> Result<(), LocalSaveError> {
        self.persist_local_state_with(path, false, true)
    }

    /// [`Self::persist_local_state_at`]; `keep_history` copies the file's
    /// `history` member verbatim (the open's write); `asks` as
    /// [`Self::persist_local_state_guarded`].
    fn persist_local_state_with(
        &self,
        path: Option<PathBuf>,
        keep_history: bool,
        asks: bool,
    ) -> Result<(), LocalSaveError> {
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (Some(path), true, Some(expected)) =
            (path, file.can_save_locally(), file.fingerprint.clone())
        else {
            file.local_pending = true;
            file.local_pending_asks |= asks;
            return Ok(());
        };
        // Read once: the bytes this state last read or wrote, when it kept
        // them, else the file (checked against the guard). Either way the
        // write below checks the file on disk against the guard before its
        // rename, so a file another program changed is never overwritten.
        let original = match file.bytes_for(&expected) {
            Some(bytes) => bytes,
            None => Arc::from(read_unchanged(&path, &expected)?),
        };
        let mut extras = file.extras.clone();
        let keep_file_history = keep_history || !file.dirty && file.history_unread;
        extras.history = if file.dirty || keep_file_history {
            file.saved_history.clone()
        } else {
            self.history_to_persist(&file)
        };
        let bytes = if keep_file_history {
            crate::persistence::stalx_v2::rewrite_local_keeping_history(&original, &extras)
        } else {
            crate::persistence::stalx_v2::rewrite_local(&original, &extras)
        }
        .map_err(|e| LocalSaveError::Encode(e.to_string()))?;
        let fingerprint =
            write_atomic(&path, &bytes, Guard::Expect(&expected)).map_err(|e| write_error(&e))?;
        file.saved_bytes = Some((fingerprint.clone(), Arc::from(bytes)));
        file.fingerprint = Some(fingerprint);
        file.saved_history = extras.history;
        file.local_pending = false;
        file.local_pending_asks = false;
        Ok(())
    }

    /// What a write that upgraded the CLOSED legacy file at `path` to version
    /// 2 (a Recents rename, a SpeleoDB link strip) still owes, as the
    /// upgrading save does it (review finding D-migration-upgrade-4): the
    /// `.plnx` beside it rewritten with the record parity of the migration
    /// that was written (`project`), and the conversion notice kept for the
    /// next open of `path` in this session. Never fails the write, which
    /// already happened.
    pub(super) fn finish_closed_upgrade(
        &self,
        path: &Path,
        project: &crate::models::survey::Project,
        parity: Option<&LegacyParity>,
        notice: Option<ConversionNotice>,
    ) {
        if let Some(parity) = parity {
            match super::dive_plan::upgrade_dive_plan_anchors_beside(path, project, parity) {
                Ok(0) => {}
                Ok(plans) => tracing::info!(plans, "Rewrote the dive plans' 0.6.0 anchors"),
                Err(e) => tracing::warn!(
                    error = %e,
                    purpose = "upgrade dive-plan anchors on a closed file's upgrade",
                    "Could not rewrite the dive plans' 0.6.0 anchors; they keep the 0.6.0 form"
                ),
            }
        }
        let Some(notice) = notice else {
            return;
        };
        tracing::info!(
            moved = notice.slots_moved,
            depth_changed = notice.depth_changed,
            unplaced = notice.unplaced,
            unexplained = notice.unexplained,
            purpose = "never move stations silently",
            "A closed legacy file's conversion changed positions; its next open says so"
        );
        let mut pending = self
            .pending_upgrade_notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|(p, _)| !super::project_io::same_file(p, path));
        pending.push((path.to_path_buf(), notice));
    }

    /// The notice [`Self::finish_closed_upgrade`] kept for `path`, taken.
    pub(super) fn take_pending_upgrade_notice(&self, path: &Path) -> Option<ConversionNotice> {
        let mut pending = self
            .pending_upgrade_notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let at = pending
            .iter()
            .position(|(p, _)| super::project_io::same_file(p, path))?;
        Some(pending.remove(at).1)
    }

    /// The conversion notice of a legacy open, taken once, as the banner
    /// shows it: a project opened from a version 2 file (a closed 0.6.0
    /// file a Recents rename or link strip upgraded) is already saved in the
    /// new format.
    pub fn take_conversion_notice(&self) -> Option<ConversionNoticeView> {
        self.with_file(|file| {
            let saved = file.origin == FileOrigin::V2;
            file.conversion.take().map(|n| n.view(saved))
        })
    }

    /// Whether the open project has changes no save has written yet: survey
    /// edits, ignores, and on a legacy (SQLite) file not yet upgraded the
    /// local state that waits for the upgrading save and that a close would
    /// lose for good (SpeleoDB bookkeeping, `local_pending_asks`). Those were
    /// written in place at once by 0.6.0; here only a save writes them, so
    /// "Don't save" on close discards them and the close prompt must ask
    /// (review finding D-migration-upgrade-5). State the next open derives
    /// again (a materialized undo mode), a re-verification time and a display
    /// toggle do not count (the rule on [`FileState::local_pending_asks`]):
    /// they wait for the next save too, but asking about them, or letting
    /// autosave write them, would convert a file the user only looked at.
    pub fn has_unsaved_changes(&self) -> bool {
        self.with_file(|file| {
            file.dirty || file.origin == FileOrigin::Legacy && file.local_pending_asks
        })
    }

    /// A local-only save that must be durable at once on a legacy (SQLite)
    /// file too: a SpeleoDB unlink, which a following open of another file
    /// would otherwise lose (the frontend's Save as unmanaged file closes the
    /// cache copy right after it; review finding D-migration-upgrade-5). On a
    /// legacy-origin project the file is upgraded first, as stripping a
    /// closed copy's link upgrades it (plan N11): the survey AS IT IS ON DISK
    /// (migrated again; the session's unsaved edits and ignores stay unsaved)
    /// with the session's local state, the original kept in Recovery, the
    /// `.plnx` pins rewritten. Anything else is
    /// [`Self::persist_local_state`].
    ///
    /// The migration reads the whole file (seconds on a large one), so it
    /// runs without the `project` guard, which every command waits on, as
    /// the open's does (review finding O2 of package U5). The commit takes
    /// the guard and writes only while the project is the one it read for:
    /// the same path and session, still legacy, the guard's fingerprint
    /// unchanged. A save, Save As or reload of ours meanwhile makes it start
    /// over ([`UPGRADE_ATTEMPTS`]); the last attempt migrates under the
    /// guard. Another program's change is the write's guard's to refuse.
    pub(crate) fn persist_local_state_upgrading(&self) -> Result<(), LocalSaveError> {
        let adjustment = self.get_config().closure_adjustment;
        self.persist_local_state_upgrading_with(|path| {
            crate::persistence::migrate::migrate_path_for_app(path, &adjustment)
        })
    }

    /// [`Self::persist_local_state_upgrading`] with the migration given (the
    /// app's, or a test's that looks at the window it runs in).
    pub(super) fn persist_local_state_upgrading_with(
        &self,
        mut migrate: impl FnMut(&Path) -> Result<Migrated, MigrateError>,
    ) -> Result<(), LocalSaveError> {
        let lock = || {
            self.project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        let mut attempt = 1;
        loop {
            let guard = lock();
            let path = guard.as_ref().and_then(|p| p.file_path.clone());
            let expected = self.with_file(|f| {
                if f.origin == FileOrigin::Legacy {
                    f.fingerprint.clone()
                } else {
                    None
                }
            });
            let (Some(path), Some(expected)) = (path.clone(), expected) else {
                let result = self.persist_local_state_with(path, false, true);
                drop(guard);
                return result;
            };
            let session = self.speleodb_session_generation();
            let (guard, migrated) = if attempt < UPGRADE_ATTEMPTS {
                drop(guard);
                let migrated = migrate(&path);
                (lock(), migrated)
            } else {
                let migrated = migrate(&path);
                (guard, migrated)
            };
            let unchanged = guard.as_ref().and_then(|p| p.file_path.as_deref())
                == Some(path.as_path())
                && self.speleodb_session_generation() == session
                && self.with_file(|f| {
                    f.origin == FileOrigin::Legacy && f.fingerprint.as_ref() == Some(&expected)
                });
            if !unchanged {
                drop(guard);
                tracing::info!(
                    attempt,
                    purpose = "make a SpeleoDB unlink durable on a legacy file",
                    "The project changed while its legacy file was migrated; starting over"
                );
                attempt += 1;
                continue;
            }
            let result = migrated
                .map_err(|e| LocalSaveError::Io(format!("could not read {}: {e}", path.display())))
                .and_then(|migrated| self.upgrade_legacy_on_disk(&path, &expected, migrated));
            drop(guard);
            return result;
        }
    }

    /// [`Self::persist_local_state_upgrading`]'s commit of `migrated` (the
    /// file at `path` as it was when its guard was `expected`); the caller
    /// holds the `project` guard.
    fn upgrade_legacy_on_disk(
        &self,
        path: &Path,
        expected: &Fingerprint,
        migrated: Migrated,
    ) -> Result<(), LocalSaveError> {
        let mut project = migrated.project;
        if !migrated.extras.validation_stored {
            project.settings.validation = self.get_config().validation;
        }
        let parity = migrated.extras.legacy_parity.clone();
        let on_disk = FileState::from_migrated(migrated.extras, None, &migrated.report);
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut extras = on_disk.extras;
        extras.speleodb.clone_from(&file.extras.speleodb);
        extras.local = file.extras.local.clone();
        extras.generator = super::project_io::generator();
        let bytes = crate::persistence::stalx_v2::encode(
            &project,
            &extras,
            crate::persistence::stalx_v2::Mode::Normal,
        )
        .map_err(|e| LocalSaveError::Encode(e.to_string()))?;
        match super::legacy_upgrade::upgrade(path, &bytes, Guard::Expect(expected)) {
            Ok(fingerprint) => {
                file.fingerprint = Some(fingerprint);
                file.origin = FileOrigin::V2;
                file.saved_history = History::default();
                file.history_unread = false;
                file.local_pending = false;
                file.local_pending_asks = false;
            }
            Err(failure) => {
                if let Some(settled) = failure.settled {
                    file.fingerprint = Some(settled);
                }
                return Err(match failure.error {
                    super::legacy_upgrade::UpgradeError::Write(e) => write_error(&e),
                    other => LocalSaveError::Io(other.to_string()),
                });
            }
        }
        drop(file);
        tracing::info!(
            path = ?path,
            purpose = "make a SpeleoDB unlink durable on a legacy file",
            "Upgraded a legacy project's file to write its local state"
        );
        self.finish_closed_upgrade(path, &project, Some(&parity), None);
        self.with_file(|file| file.legacy_parity = None);
        Ok(())
    }

    /// Why the open file is read-only ("made with a newer Stalagmight", plan
    /// 6.4), or `None` when the file allows edits.
    pub fn read_only_reason(&self) -> Option<String> {
        self.with_file(|file| file.extras.read_only.as_ref().map(ToString::to_string))
    }

    /// Whether the file on disk is still the 0.6.0 (SQLite) file the open
    /// project was migrated from: no save has upgraded it yet, so the status
    /// bar must not say "Saved" (demo issue N-6). Any save that writes the
    /// open project (to its own path or a Save As) makes it `false`.
    pub fn file_is_legacy(&self) -> bool {
        self.with_file(|file| file.origin == FileOrigin::Legacy)
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
