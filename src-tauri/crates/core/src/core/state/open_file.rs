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
//!   (`local_pending`), and on a legacy file it counts as unsaved
//!   ([`AppState::has_unsaved_changes`]); only a SpeleoDB unlink upgrades a
//!   legacy file at once ([`AppState::persist_local_state_upgrading`]).
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
    LegacyParity, MigratedExtras, MigrationReport, NameChange, Verification,
};
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

/// The one-line notice an upgrade shows when the migration changed what the
/// user sees (plan 7.2.10, G4, C4): stations moved in plan (a torn legacy merge
/// collapsed, or an unexplained raw difference), stations whose depth changed
/// (a snap's derived alignment changed a depth tie, or an unexplained one),
/// stations the conversion could not place, or a name a line showed (review
/// findings D-migration-upgrade-8, UX-3 and UX-1; since I-3 every line keeps
/// the names 0.6.0 showed on it, a merged station's included, so only a name
/// a line showed twice is lettered). Computed View differences never raise it
/// (C4).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConversionNotice {
    pub(crate) slots_moved: usize,
    pub(crate) max_displacement_m: f64,
    /// Slots whose depth changed beyond the 1e-9 m tolerance, and by how much
    /// at most (review finding D-migration-upgrade-6).
    pub(crate) depth_changed: usize,
    pub(crate) max_depth_change_m: f64,
    /// Slots with no finite migrated position.
    pub(crate) unplaced: usize,
    pub(crate) unexplained: usize,
    /// The slots whose name changed, in slot order (survey content: shown,
    /// never logged).
    pub(crate) names_changed: Vec<NameChange>,
    /// Merged stations whose members' different comments were joined into
    /// one (each line now shows both).
    pub(crate) comments_joined: usize,
    /// Where the unconverted original is, which the notice tells the user.
    pub(crate) original: ConversionOriginal,
}

/// Where the original of a converted legacy file is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ConversionOriginal {
    /// A local file: converted in memory, written as version 2 by its first
    /// save, which moves the original into the backups folder (Recent
    /// Projects: Restore an earlier copy).
    #[default]
    LocalFile,
    /// A SpeleoDB revision downloaded into the cache: the cache copy is
    /// already version 2 when it opens, and no local backup is made; the
    /// original is the revision itself, unchanged on the server (Revision
    /// history).
    SpeleoDbRevision,
}

/// How many name changes the notice spells out before "and N more".
const NAMES_LISTED: usize = 5;

impl ConversionNotice {
    /// [`Self::of`] plus what else a line shows differently: the names the
    /// conversion changed and the comments it joined. A notice also when
    /// nothing moved but something visible changed (review finding UX-1).
    pub(crate) fn of_migration(
        report: &MigrationReport,
        names_changed: &[NameChange],
    ) -> Option<Self> {
        let names_changed = names_changed.to_vec();
        let comments_joined = report.comments_joined;
        match Self::of(&report.verification) {
            Some(notice) => Some(Self {
                names_changed,
                comments_joined,
                ..notice
            }),
            None => (!names_changed.is_empty() || comments_joined > 0).then_some(Self {
                slots_moved: 0,
                max_displacement_m: 0.0,
                depth_changed: 0,
                max_depth_change_m: 0.0,
                unplaced: 0,
                unexplained: 0,
                names_changed,
                comments_joined,
                original: ConversionOriginal::LocalFile,
            }),
        }
    }

    /// The same notice for a SpeleoDB revision the cache got converted.
    pub(crate) fn downloaded(self) -> Self {
        Self {
            original: ConversionOriginal::SpeleoDbRevision,
            ..self
        }
    }

    pub(crate) fn of(verification: &Verification) -> Option<Self> {
        let moved = verification.moved();
        let depth = verification.depth_changed();
        let notice = Self {
            slots_moved: moved.slots,
            max_displacement_m: moved.max_displacement_m,
            depth_changed: depth.slots,
            max_depth_change_m: depth.max_displacement_m,
            unplaced: verification.unplaced(),
            unexplained: verification.mismatches.len(),
            names_changed: Vec::new(),
            comments_joined: 0,
            original: ConversionOriginal::LocalFile,
        };
        (notice.slots_moved > 0
            || notice.depth_changed > 0
            || notice.unplaced > 0
            || notice.unexplained > 0)
            .then_some(notice)
    }

    pub(crate) fn message(&self) -> String {
        let mut what = Vec::new();
        if self.slots_moved > 0 {
            what.push(format!(
                "{} stations moved by up to {:.2} m",
                self.slots_moved, self.max_displacement_m
            ));
        }
        if self.depth_changed > 0 {
            what.push(format!(
                "{} stations changed depth by up to {:.2} m",
                self.depth_changed, self.max_depth_change_m
            ));
        }
        if self.unplaced > 0 {
            what.push(format!("{} stations could not be placed", self.unplaced));
        }
        if !self.names_changed.is_empty() {
            what.push(self.names_part());
        }
        match self.comments_joined {
            0 => {}
            1 => what.push("a merged station's two comments were joined".to_string()),
            n => what.push(format!("the comments of {n} merged stations were joined")),
        }
        if what.is_empty() {
            // `of` and `of_migration` raise no notice without one of the
            // counts above; never "0 stations moved" all the same.
            what.push("some positions differ from the old version's".to_owned());
        }
        // The second sentence says where the original is: a local file's
        // upgrade keeps it in the backups folder (written by the first save,
        // or already by a Recents rename or a link strip of the closed file);
        // a downloaded revision's cache copy keeps none, the server has it.
        let original = match self.original {
            ConversionOriginal::LocalFile => {
                "The new format is written the first time the project is saved, or already \
                 was if you renamed it in Recent Projects or removed its SpeleoDB link; the \
                 original file is kept: to go back to it, right-click the project in Recent \
                 Projects and choose Restore an earlier copy."
            }
            ConversionOriginal::SpeleoDbRevision => {
                "This computer's copy is already converted, and the original revision is \
                 unchanged on SpeleoDB: it stays in the project's Revision history."
            }
        };
        format!(
            "Converted from the old file format; {}. {original}",
            what.join("; ")
        )
    }

    /// "3 station names changed (a merged station has one name on all its
    /// lines): Side 0 is now 3, Loop 0 is now 1, ..."
    fn names_part(&self) -> String {
        let n = self.names_changed.len();
        let listed: Vec<String> = self
            .names_changed
            .iter()
            .take(NAMES_LISTED)
            .map(|c| format!("{} {} is now {}", c.line, c.from, c.to))
            .collect();
        let more = n.saturating_sub(NAMES_LISTED);
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        let noun = if n == 1 { "name" } else { "names" };
        format!(
            "{n} station {noun} changed (a line showed one name twice): {}{tail}",
            listed.join(", ")
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
    /// save). `f` returns whether it changed anything.
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
        self.persist_local_state_guarded(false)
    }

    /// [`Self::persist_local_state`] for the write an OPEN makes (a
    /// materialized undo mode): it copies the file's `history` member byte
    /// for byte, so an open never changes the history on disk, whatever the
    /// persisted window (review finding J-docs-tests-truth-3).
    pub(super) fn persist_local_state_keeping_history(&self) -> Result<(), LocalSaveError> {
        self.persist_local_state_guarded(true)
    }

    fn persist_local_state_guarded(&self, keep_history: bool) -> Result<(), LocalSaveError> {
        // Under the `project` guard, for the whole save: every change of the
        // survey, of the stacks and of the dirty flag happens under it (an
        // undo pops its entry before it marks the project dirty), so only
        // under it do the flag and the stacks describe the body on disk.
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = guard.as_ref().and_then(|p| p.file_path.clone());
        let result = self.persist_local_state_with(path, keep_history);
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
        self.persist_local_state_with(path, false)
    }

    /// [`Self::persist_local_state_at`]; `keep_history` copies the file's
    /// `history` member verbatim (the open's write).
    fn persist_local_state_with(
        &self,
        path: Option<PathBuf>,
        keep_history: bool,
    ) -> Result<(), LocalSaveError> {
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

    /// The conversion notice of a legacy open, taken once.
    pub fn take_conversion_notice(&self) -> Option<String> {
        self.with_file(|file| file.conversion.take().map(|n| n.message()))
    }

    /// Whether the open project has changes no save has written yet: survey
    /// edits, ignores, and on a legacy (SQLite) file not yet upgraded the
    /// local state that waits for the upgrading save (display toggles,
    /// SpeleoDB bookkeeping, `local_pending`). Those were written in place at
    /// once by 0.6.0; here only a save writes them, so "Don't save" on close
    /// discards them and the close prompt must ask (review finding
    /// D-migration-upgrade-5).
    pub fn has_unsaved_changes(&self) -> bool {
        self.with_file(|file| file.dirty || file.origin == FileOrigin::Legacy && file.local_pending)
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
    pub(crate) fn persist_local_state_upgrading(&self) -> Result<(), LocalSaveError> {
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = guard.as_ref().and_then(|p| p.file_path.clone());
        let legacy = self.with_file(|f| f.origin == FileOrigin::Legacy && f.fingerprint.is_some());
        let result = match (path, legacy) {
            (Some(path), true) => self.upgrade_legacy_on_disk(&path),
            (path, _) => self.persist_local_state_with(path, false),
        };
        drop(guard);
        result
    }

    /// [`Self::persist_local_state_upgrading`]'s upgrade; the caller holds the
    /// `project` guard.
    fn upgrade_legacy_on_disk(&self, path: &Path) -> Result<(), LocalSaveError> {
        let config = self.get_config();
        let migrated =
            crate::persistence::migrate::migrate_path_for_app(path, &config.closure_adjustment)
                .map_err(|e| {
                    LocalSaveError::Io(format!("could not read {}: {e}", path.display()))
                })?;
        let mut project = migrated.project;
        if !migrated.extras.validation_stored {
            project.settings.validation = config.validation;
        }
        let parity = migrated.extras.legacy_parity.clone();
        let on_disk = FileState::from_migrated(migrated.extras, None, &migrated.report);
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(expected) = file.fingerprint.clone() else {
            return Ok(());
        };
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
        match super::legacy_upgrade::upgrade(path, &bytes, Guard::Expect(&expected)) {
            Ok(fingerprint) => {
                file.fingerprint = Some(fingerprint);
                file.origin = FileOrigin::V2;
                file.saved_history = History::default();
                file.history_unread = false;
                file.local_pending = false;
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

    /// The path the open project saves to, if any.
    pub(crate) fn open_file_path(&self) -> Option<PathBuf> {
        self.project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|p| p.file_path.clone())
    }
}
