//! The side tables a save carries from the open file into the new one (#576 A3).
//!
//! `Database::save_project` writes the survey: exactly the tables in
//! [`SAVE_PROJECT_TABLES`](crate::persistence::SAVE_PROJECT_TABLES), rebuilt from the
//! `Project` model. Everything else the schema holds lives outside that model, and
//! the save builds a FRESH `.stalx.tmp` and renames it over the destination - so
//! each of those tables is carried by hand: snapshotted from the open file before
//! the temp exists, written into the temp before it is renamed. A table nobody
//! carries is emptied by every save, which has shipped twice (#191, the SpeleoDB
//! link; #303, the Unwind chain pointer).
//!
//! [`Sidecar`] is the one list of those tables, each with what happens when it
//! cannot be carried (maintainer decision A3-D2, "stop unless cosmetic"):
//!
//! | Side table | Unreadable / unwritable | Why |
//! |---|---|---|
//! | ignored warnings | the save stops | the user's own decisions (#517) |
//! | SpeleoDB link | the save stops | the project's identity, not decoration |
//! | undo-chain markers | the save stops | they decide which undo rows are the user's (#303) |
//! | display settings | saved without them, with a warning | per-machine view state |
//! | undo history | saved without it, with a warning | never written partially |
//!
//! One more refusal shares the policy without being an entry: per-project settings
//! (`project_settings`, which the core save writes from the model) that the OPEN
//! could not read are held as defaults, so every save stops while they are unread
//! ([`unread_settings_refusal`], #576 B15).
//!
//! A stopped save writes nothing: no temp file is left and the destination is
//! untouched. A warning rides the successful save's `CommandResult.warnings`, which
//! the frontend shows (`surfaceSaveWarnings`). The coverage test in
//! `save_sidecar_tests` pins that every table `init_schema` creates is either
//! written by the core save or listed here, so a new table cannot be forgotten
//! silently again.

use std::collections::HashMap;
use std::path::Path;

use super::AppState;
use crate::core::loading::clamp_quoted_error;
use crate::core::{
    CommandError, CommandErrorCode, CommandResult, ValidationWarning, WarningSeverity,
};
use crate::models::{Project, SectionId, SpeleoLink};
use crate::persistence::{Database, ResolvedWarning};

/// The warning on a save whose undo history could not be written and was removed
/// from the saved file.
pub(super) const UNDO_HISTORY_DROPPED_CODE: &str = "UNDO_HISTORY_DROPPED";

/// The warning on a save that could not carry the project's display settings.
pub(super) const DISPLAY_SETTINGS_NOT_SAVED_CODE: &str = "DISPLAY_SETTINGS_NOT_SAVED";

/// [#576 RV2-R-7] The warning on a Save As away from an open file that could not
/// be reopened: the side tables only that file holds are not in the new one.
pub(super) const SIDE_TABLES_NOT_CARRIED_CODE: &str = "SIDE_TABLES_NOT_CARRIED";

/// The warning on a save that wrote the file but could not reopen it afterwards
/// (`project_io.rs`): DB-backed actions stay off until the project is reopened.
pub(super) const DATABASE_NOT_REOPENED_CODE: &str = "DATABASE_NOT_REOPENED";

/// How a save relates to the file the session has open. Resolved once, by
/// [`SaveRequest::mode`] before anything is written, so no step has to infer it
/// again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SaveMode {
    /// The first write of a deferred new project: there is no file yet, so the only
    /// side table with anything in it is the in-memory display settings. An
    /// implicit save picks a free name rather than overwrite an existing file.
    FirstSave,
    /// Rewrites the open file: Ctrl+S, autosave, the pre-push flush, or an explicit
    /// path naming the open file.
    InPlace,
    /// Writes another file and moves the session onto it.
    SaveAs,
    /// [#576, W2-SAVE question 1] A Save As to ANOTHER file away from an open file
    /// whose `speleo_link` row could not be read - the way out the save refusal
    /// leaves (`read_speleo_link`). Moves the session like [`Self::SaveAs`], but
    /// writes an unlinked project, so like [`Self::DetachedCopy`] it sheds the
    /// undo-chain markers that describe a server relationship it no longer has.
    UnlinkedSaveAs,
    /// An explicit destination for a SpeleoDB-managed project: writes an UNLINKED
    /// copy and leaves the session on the managed file (docs/saving.md, "Save a
    /// Copy"). An explicit path naming the managed file itself resolves here too,
    /// and the save refuses it.
    DetachedCopy,
}

/// What a save does when a side table cannot be read from the open file or
/// written into the new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SidecarPolicy {
    /// The save stops before anything is renamed into place: nothing is written,
    /// the destination is untouched. An unreadable table is refused with
    /// `unreadable_code`, the code the frontend routes on.
    Refuse { unreadable_code: CommandErrorCode },
    /// The save goes ahead without the table, and its result carries
    /// `warning_code`.
    WarnAndContinue { warning_code: &'static str },
    /// The undo history: a write that fails is replaced by an EMPTY history (a
    /// partial one is never renamed into place) and the result carries
    /// `warning_code`. Only a failure to empty it stops the save.
    DropHistory { warning_code: &'static str },
}

/// One side table (or a pair that is only ever carried together).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sidecar {
    DisplaySettings,
    ResolvedWarnings,
    SpeleoLink,
    UndoMarkers,
    History,
}

impl Sidecar {
    /// Every side table, in the order a save writes them.
    pub(super) const ALL: [Self; 5] = [
        Self::DisplaySettings,
        Self::ResolvedWarnings,
        Self::SpeleoLink,
        Self::UndoMarkers,
        Self::History,
    ];

    /// The SQLite tables this entry carries.
    pub(super) const fn tables(self) -> &'static [&'static str] {
        match self {
            Self::DisplaySettings => &["display_settings"],
            Self::ResolvedWarnings => &["resolved_warnings"],
            Self::SpeleoLink => &["speleo_link"],
            Self::UndoMarkers => &["speleo_undo_chain", "speleo_undo_watermark"],
            Self::History => &["undo_stack", "redo_stack"],
        }
    }

    pub(super) const fn policy(self) -> SidecarPolicy {
        match self {
            Self::DisplaySettings => SidecarPolicy::WarnAndContinue {
                warning_code: DISPLAY_SETTINGS_NOT_SAVED_CODE,
            },
            Self::ResolvedWarnings => SidecarPolicy::Refuse {
                unreadable_code: CommandErrorCode::IgnoresUnreadable,
            },
            Self::SpeleoLink => SidecarPolicy::Refuse {
                unreadable_code: CommandErrorCode::SpeleodbLinkUnreadable,
            },
            Self::UndoMarkers => SidecarPolicy::Refuse {
                unreadable_code: CommandErrorCode::UndoMarkersUnreadable,
            },
            Self::History => SidecarPolicy::DropHistory {
                warning_code: UNDO_HISTORY_DROPPED_CODE,
            },
        }
    }

    /// Whether a save in `mode` carries this table into the file it writes.
    ///
    /// A detached copy is a plain unmanaged project, so it sheds the link AND the
    /// two markers that describe this machine's relationship to the server (#303):
    /// a copy that kept the watermark and was later linked would have its first
    /// push cut the user's history as "inherited". An unlinked Save As writes the
    /// same kind of file (its link could not be read), so it sheds them too.
    pub(super) const fn carried_in(self, mode: SaveMode) -> bool {
        match self {
            Self::SpeleoLink | Self::UndoMarkers => {
                !matches!(mode, SaveMode::DetachedCopy | SaveMode::UnlinkedSaveAs)
            }
            Self::DisplaySettings | Self::ResolvedWarnings | Self::History => true,
        }
    }

    /// What the user calls it.
    const fn noun(self) -> &'static str {
        match self {
            Self::DisplaySettings => "display settings",
            Self::ResolvedWarnings => "list of ignored warnings",
            Self::SpeleoLink => "SpeleoDB link",
            Self::UndoMarkers => "SpeleoDB undo-history markers",
            Self::History => "Undo and Redo history",
        }
    }

    const fn pronoun(self) -> &'static str {
        match self {
            Self::SpeleoLink => "it",
            Self::DisplaySettings | Self::ResolvedWarnings | Self::UndoMarkers | Self::History => {
                "them"
            }
        }
    }

    /// The refusal for a [`SidecarPolicy::Refuse`] table the open file could not
    /// give up. Nothing exists on disk yet, so nothing was written; the quoted
    /// cause is clamped, because SQLite error text embeds the file's own
    /// identifiers.
    fn unreadable_refusal(self, code: CommandErrorCode, cause: &anyhow::Error) -> CommandResult {
        // The link alone has a way out: a Save As to ANOTHER file leaves the managed
        // file untouched, so it goes ahead unlinked (#576 B35(b)).
        let way_out = if matches!(self, Self::SpeleoLink) {
            " Use Save As to save your changes to another file."
        } else {
            ""
        };
        CommandResult::refused(CommandError::new(
            code,
            format!(
                "Can't save: this project's {} could not be read ({}). Saving would \
                 rewrite the file without {}, so nothing was written - the file on disk is \
                 unchanged.{way_out}",
                self.noun(),
                clamp_quoted_error(&cause.to_string()),
                self.pronoun(),
            ),
        ))
    }

    /// The warning for a table a save went ahead without.
    pub(super) fn lost_warning(self, code: &str) -> ValidationWarning {
        let message = match self {
            Self::History => "The project was saved, but its Undo and Redo history could not be \
                              preserved and was removed."
                .to_owned(),
            Self::DisplaySettings => "The project was saved, but its display settings could not \
                                      be kept. They will be back to their defaults when the \
                                      project is next opened."
                .to_owned(),
            // Refused rather than warned about under today's policy; worded so that a
            // policy change cannot put a false sentence on screen.
            Self::ResolvedWarnings | Self::SpeleoLink | Self::UndoMarkers => format!(
                "The project was saved, but its {} could not be kept.",
                self.noun()
            ),
        };
        ValidationWarning {
            code: code.to_owned(),
            message,
            severity: WarningSeverity::Warning,
        }
    }
}

/// Everything a save carries besides the survey, snapshotted from the open file.
pub(super) struct CarriedSidecars {
    mode: SaveMode,
    display_settings: HashMap<String, String>,
    resolved_warnings: Vec<ResolvedWarning>,
    speleo_link: Option<SpeleoLink>,
    undo_chain: Option<String>,
    undo_watermark: Option<String>,
    /// The warnings for tables the save is going ahead without.
    warnings: Vec<ValidationWarning>,
}

impl CarriedSidecars {
    pub(super) fn mode(&self) -> SaveMode {
        self.mode
    }

    pub(super) fn speleo_link(&self) -> Option<&SpeleoLink> {
        self.speleo_link.as_ref()
    }

    fn warn_lost(&mut self, sidecar: Sidecar, code: &str) {
        if !self.warnings.iter().any(|w| w.code == code) {
            self.warnings.push(sidecar.lost_warning(code));
        }
    }

    /// Read `sidecar` from the open file.
    fn snapshot(
        &mut self,
        sidecar: Sidecar,
        db: &Database,
        project: &Project,
    ) -> anyhow::Result<()> {
        match sidecar {
            Sidecar::DisplaySettings => {
                // The file's own, unless it has none: then the in-memory ones, which
                // is where a setting changed while the handle was missing went (see
                // `restore_missing_database`).
                let stored = db.load_display_settings()?;
                if !stored.is_empty() {
                    self.display_settings = stored;
                }
            }
            Sidecar::ResolvedWarnings => {
                // [#517] The verbatim read takes the table in whatever shape the file
                // has (a pre-v8 file with no `shot_uuid`, one whose migration chain a
                // forged `PRAGMA user_version` skipped) and coerces every hostile
                // VALUE, so what fails here is STRUCTURAL: a missing key column, an
                // unreadable `sqlite_master`, a corrupt page. One nonsense row must
                // never land here - that would make one poisoned cell into a project
                // that can never be saved again.
                self.resolved_warnings =
                    carry_resolved_warnings(db.load_resolved_warnings_verbatim()?, project);
            }
            Sidecar::UndoMarkers => {
                self.undo_chain = db.load_speleo_undo_chain()?;
                self.undo_watermark = db.load_speleo_undo_watermark()?;
            }
            // The link is read in its own turn of the pass (`read_speleo_link`),
            // because it decides the mode; the history's source is the live undo
            // stack, not the file.
            Sidecar::SpeleoLink | Sidecar::History => {}
        }
        Ok(())
    }
}

/// A side-table write that stops the save. The caller discards the temp file and
/// reports it through `save_failure`.
pub(super) struct SidecarWriteFailure {
    pub(super) context: String,
    pub(super) error: anyhow::Error,
}

/// What `save_project` knows about a save before it reads anything.
#[derive(Clone, Copy)]
pub(super) struct SaveRequest<'a> {
    /// Where the file will be written.
    pub(super) destination: &'a Path,
    /// The project has never been written (`initial_save_deferred`).
    pub(super) deferred: bool,
    /// No path was given.
    pub(super) implicit: bool,
    /// The destination is the open file.
    pub(super) rewrites_open_file: bool,
}

impl SaveRequest<'_> {
    /// The mode this save runs in, once the open file's link is known: `linked`
    /// is whether it carries a `speleo_link` row that could be read.
    pub(super) fn mode(self, linked: bool) -> SaveMode {
        if self.deferred {
            SaveMode::FirstSave
        } else if linked && !self.implicit {
            SaveMode::DetachedCopy
        } else if self.implicit || self.rewrites_open_file {
            SaveMode::InPlace
        } else {
            SaveMode::SaveAs
        }
    }
}

impl AppState {
    /// Snapshot every side table the save carries, before any temp file exists,
    /// and resolve the [`SaveMode`] on the way.
    ///
    /// One pass in [`Sidecar::ALL`] order. The link's turn decides the mode, so the
    /// tables before it are ones every mode carries (pinned by the registry test),
    /// and which refusal a broken file gets does not depend on the mode: a database
    /// no read can reach is refused for its ignore list first, as it was (#517) -
    /// not for a SpeleoDB link a local project never had.
    ///
    /// A [`SidecarPolicy::Refuse`] table that cannot be read is the refusal
    /// returned; a cosmetic one is recorded as a warning and the save carries what
    /// the session holds in memory instead.
    pub(super) fn snapshot_sidecars(
        &self,
        project: &Project,
        request: SaveRequest<'_>,
    ) -> Result<CarriedSidecars, Box<CommandResult>> {
        // The in-memory display settings: the only copy a project without a file
        // has. Taken before the database lock, never under it.
        let cached = self.display_settings_cache.lock().clone();
        let mut carried = CarriedSidecars {
            mode: request.mode(false),
            display_settings: cached,
            resolved_warnings: Vec::new(),
            speleo_link: None,
            undo_chain: None,
            undo_watermark: None,
            warnings: Vec::new(),
        };
        // Recover a poisoned lock rather than silently carrying nothing.
        let db_guard = self.database.lock();
        // No handle: a first save (nothing on disk to carry), or a Save As away from
        // a file that could not be reopened (`restore_missing_database`) - the user's
        // way out, which goes ahead. The mode above stands (no link could be read).
        // In the second case the tables only that file holds are UNREADABLE, not
        // absent, and the new file goes without them: say so (#576 RV2-R-7).
        let Some(db) = db_guard.as_ref() else {
            // [#576 RV6a-15] Only when there IS an original file: a project never
            // saved (no `file_path`) has nothing that could have failed to read.
            if !request.deferred && project.file_path.is_some() {
                carried.warnings.push(ValidationWarning {
                    code: SIDE_TABLES_NOT_CARRIED_CODE.to_owned(),
                    message: "The project was saved, but the original file could not be read, \
                              so the new file is without its list of ignored warnings, its \
                              SpeleoDB link and its undo-history markers."
                        .to_owned(),
                    severity: WarningSeverity::Warning,
                });
            }
            return Ok(carried);
        };
        let mut mode_known = false;
        for sidecar in Sidecar::ALL {
            if sidecar == Sidecar::SpeleoLink {
                carried.speleo_link = match read_speleo_link(db, request)? {
                    LinkRead::Read(link) => {
                        carried.mode = request.mode(link.is_some());
                        link
                    }
                    LinkRead::UnreadableForAnotherFile => {
                        carried.mode = SaveMode::UnlinkedSaveAs;
                        None
                    }
                };
                mode_known = true;
                continue;
            }
            if mode_known && !sidecar.carried_in(carried.mode) {
                continue;
            }
            let Err(e) = carried.snapshot(sidecar, db, project) else {
                continue;
            };
            match sidecar.policy() {
                SidecarPolicy::Refuse { unreadable_code } => {
                    tracing::error!(
                        error = %e,
                        tables = ?sidecar.tables(),
                        purpose = "carry the project's side tables across save",
                        "Refused to save: a side table could not be read, and the save would \
                         rebuild the file without it"
                    );
                    return Err(Box::new(sidecar.unreadable_refusal(unreadable_code, &e)));
                }
                SidecarPolicy::WarnAndContinue { warning_code }
                | SidecarPolicy::DropHistory { warning_code } => {
                    tracing::warn!(
                        error = %e,
                        tables = ?sidecar.tables(),
                        purpose = "carry the project's side tables across save",
                        "A side table could not be read; saving without it"
                    );
                    carried.warn_lost(sidecar, warning_code);
                }
            }
        }
        Ok(carried)
    }

    /// Write every carried side table into the staged temp database `db`, while it
    /// can still be discarded. Returns the save's warnings, or the failure that
    /// stops it.
    pub(super) fn write_sidecars(
        &self,
        db: &Database,
        mut carried: CarriedSidecars,
    ) -> Result<Vec<ValidationWarning>, SidecarWriteFailure> {
        for sidecar in Sidecar::ALL {
            if !sidecar.carried_in(carried.mode) {
                continue;
            }
            let failure = match self.write_sidecar(sidecar, db, &carried) {
                Ok(SidecarWrite::Whole) => continue,
                Ok(SidecarWrite::Emptied) => None,
                Err(e) => Some(e),
            };
            match (sidecar.policy(), failure) {
                (SidecarPolicy::Refuse { .. }, Some(error)) => {
                    tracing::error!(
                        error = %error,
                        tables = ?sidecar.tables(),
                        purpose = "carry the project's side tables across save",
                        "Failed to write a side table into the fresh save file; save abandoned"
                    );
                    return Err(SidecarWriteFailure {
                        context: format!(
                            "Failed to carry the {} into the saved file, so nothing was \
                             replaced - the file on disk is unchanged",
                            sidecar.noun()
                        ),
                        error,
                    });
                }
                // Emptying the history failed too: a partial one would be renamed
                // into place, which is never allowed.
                (SidecarPolicy::DropHistory { .. }, Some(error)) => {
                    return Err(SidecarWriteFailure {
                        context: "Failed to finalize undo history".to_owned(),
                        error,
                    });
                }
                (SidecarPolicy::WarnAndContinue { warning_code }, Some(error)) => {
                    tracing::warn!(
                        error = %error,
                        tables = ?sidecar.tables(),
                        purpose = "carry the project's side tables across save",
                        "Failed to write a side table into the fresh save file; saving without it"
                    );
                    carried.warn_lost(sidecar, warning_code);
                }
                (
                    SidecarPolicy::WarnAndContinue { warning_code }
                    | SidecarPolicy::DropHistory { warning_code },
                    None,
                ) => carried.warn_lost(sidecar, warning_code),
                (SidecarPolicy::Refuse { .. }, None) => {}
            }
        }
        Ok(carried.warnings)
    }

    fn write_sidecar(
        &self,
        sidecar: Sidecar,
        db: &Database,
        carried: &CarriedSidecars,
    ) -> anyhow::Result<SidecarWrite> {
        // The history's injection point is inside its own writer, so the
        // empty-history fallback is what the test exercises.
        #[cfg(test)]
        if sidecar != Sidecar::History {
            for &table in sidecar.tables() {
                fault::check_write(table)?;
            }
        }
        match sidecar {
            Sidecar::DisplaySettings => {
                // Every key is attempted; the first failure is the one reported.
                let mut first_failure = None;
                for (key, value) in &carried.display_settings {
                    if let Err(e) = db.save_display_setting(key, value) {
                        first_failure.get_or_insert(e);
                    }
                }
                if let Some(e) = first_failure {
                    return Err(e);
                }
            }
            Sidecar::ResolvedWarnings => {
                if !carried.resolved_warnings.is_empty() {
                    db.save_resolved_warnings_bulk(&carried.resolved_warnings)?;
                }
            }
            Sidecar::SpeleoLink => {
                // [Track B] (#191, D-B13c) base_commit_sha and the dirty bit are
                // carried verbatim, so a local save never un-remotes the project or
                // clears the dirty bit - only a server push clears it.
                if let Some(link) = &carried.speleo_link {
                    db.save_speleo_link(Some(link))?;
                }
            }
            Sidecar::UndoMarkers => {
                // Only written when one exists: `None` is already the fresh file's
                // state.
                if let Some(sha) = &carried.undo_chain {
                    db.save_speleo_undo_chain(Some(sha))?;
                }
                if let Some(ts) = &carried.undo_watermark {
                    db.save_speleo_undo_watermark(Some(ts))?;
                }
            }
            Sidecar::History => {
                return Ok(if self.persist_or_clear_undo_stacks_into(db)? {
                    SidecarWrite::Emptied
                } else {
                    SidecarWrite::Whole
                });
            }
        }
        Ok(SidecarWrite::Whole)
    }
}

/// What [`read_speleo_link`] found.
enum LinkRead {
    /// The row, or that there is none.
    Read(Option<SpeleoLink>),
    /// The row could not be read, and the save writes another file: it goes
    /// ahead as [`SaveMode::UnlinkedSaveAs`].
    UnreadableForAnotherFile,
}

/// The open file's `speleo_link` row.
///
/// [#576 B35(b)] A link that cannot be READ is not "no link": taken as one, the
/// rebuilt file has no row and the managed project silently becomes a plain local
/// file, its base revision and dirty bit gone. A save that would rewrite the open
/// file is refused (no temp exists yet; the file keeps its row). A Save As to
/// ANOTHER file leaves the open file untouched and is the user's way out, so it
/// goes ahead, unlinked - and without the undo-chain markers (`UnlinkedSaveAs`).
fn read_speleo_link(
    db: &Database,
    request: SaveRequest<'_>,
) -> Result<LinkRead, Box<CommandResult>> {
    match db.load_speleo_link() {
        Ok(link) => Ok(LinkRead::Read(link)),
        Err(e) if request.rewrites_open_file => {
            tracing::error!(
                error = %e,
                purpose = "preserve SpeleoDB hand-off link on save",
                "Refused to save: the speleo_link table could not be read, and the save would \
                 rebuild the file without it"
            );
            Err(Box::new(Sidecar::SpeleoLink.unreadable_refusal(
                CommandErrorCode::SpeleodbLinkUnreadable,
                &e,
            )))
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                destination = ?request.destination,
                purpose = "save to another file without the unreadable link",
                "The speleo_link table could not be read; saving to the other file as an \
                 unlinked project, without the undo-chain markers"
            );
            Ok(LinkRead::UnreadableForAnotherFile)
        }
    }
}

/// [#576 B15] The refusal for every save while stored per-project settings the
/// open could not read are held as defaults (`AppState::unreadable_project_settings`),
/// or `None` when there are none.
///
/// `project_settings` itself is core-written (its keys come from the model), so
/// it is not a registry entry; but an unreadable value is the same hazard as an
/// unreadable side table (the save would rebuild the file without what is
/// stored), and it gets the same policy: the save stops before anything is
/// written. Every save, Save As included (maintainer decision B15-D1); export
/// still works.
pub(super) fn unread_settings_refusal(keys: &[String]) -> Option<CommandResult> {
    let names: Vec<&str> = keys
        .iter()
        .map(|key| match key.as_str() {
            "validation" => "validation thresholds",
            "surface_elevation" => "entrance elevation",
            "default_azimuth_reference" => "default north reference",
            other => other,
        })
        .collect();
    let listed = match names.as_slice() {
        [] => return None,
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    let pronoun = if names.len() == 1 { "it" } else { "them" };
    // [#576 NF-W3-2] Every key the open can leave unread has a control in Project
    // Settings now, and setting it there lifts the refusal (B15-EDIT): say so,
    // rather than send the user away to close and reopen.
    Some(CommandResult::refused(CommandError::new(
        CommandErrorCode::ProjectSettingsUnreadable,
        format!(
            "Can't save: this project's stored {listed} could not be read when it was \
             opened. Saving would replace {pronoun} with defaults, so nothing was written - \
             the file on disk is unchanged. Export still works. To save again, set \
             {pronoun} in Project Settings and save that dialog."
        ),
    )))
}

/// Rewrite both history stacks of `db` in one transaction - the undo history's
/// write, and the point a test fails it at, so the empty-history fallback in
/// `persist_or_clear_undo_stacks_into` is what the test exercises. (Kept here so
/// `project_io.rs` has no test gate above its test modules: source tripwires read
/// each production file up to its first one.)
pub(super) fn replace_history_rows(
    db: &Database,
    undo_rows: Vec<crate::persistence::StoredHistoryRow>,
    redo_rows: Vec<crate::persistence::StoredHistoryRow>,
) -> anyhow::Result<()> {
    #[cfg(test)]
    fault::check_write("undo_stack")?;
    db.replace_history_stacks_stored(&undo_rows, &redo_rows)
}

/// How a side-table write that did not fail went.
enum SidecarWrite {
    Whole,
    /// The history could not be written and was emptied instead.
    Emptied,
}

/// The ignore rows the save carries, re-keyed to the project being written.
///
/// Resolved (ignored) warnings live in their own table, not in the `Project`
/// model, so the core save does not carry them (issue #57).
fn carry_resolved_warnings(rows: Vec<ResolvedWarning>, project: &Project) -> Vec<ResolvedWarning> {
    // Drop rows whose section no longer exists. resolved_warnings has a FOREIGN KEY
    // on section_id and the carry-over re-inserts in a single transaction - one
    // orphaned row (e.g. a section deleted after a warning on it was ignored;
    // delete_section is in-memory only and never prunes this table) would abort the
    // whole transaction and silently drop EVERY still-valid ignore.
    let section_ids: std::collections::HashSet<_> = project.sections.iter().map(|s| s.id).collect();
    // Re-derive each UUID-keyed row's stored (section_id, shot_id) from the live
    // shot's CURRENT position so two distinct live shots never share a (section_id,
    // shot_id) in the fresh save file (issue #57). save_project writes shot_id =
    // section_id*FEATURE_ID_STRIDE + sequence_order, so we derive the same value
    // from the in-memory project; load regenerates ids with this identical formula.
    // Without this, a renumber that lands a different shot on a stale UUID-keyed
    // row's old position would let INSERT OR REPLACE (against UNIQUE(section_id,
    // shot_id, warning_code)) silently delete the older shot's still-valid ignore.
    // Key by (section_id, uuid), not uuid alone: ensure_unique_shot_uuids only
    // de-dups within a section, so two sections can share a shot UUID and a
    // uuid-only key would collapse both live shots into one entry - remapping
    // section A's ignore to whichever section won the collect, losing A's ignore and
    // mis-suppressing B (issue #57 / Codex).
    let live_positions: HashMap<(SectionId, &str), (SectionId, u32)> = project
        .sections
        .iter()
        .flat_map(|s| {
            s.shots.iter().enumerate().filter_map(move |(seq, sh)| {
                sh.uuid.as_deref().map(|u| {
                    (
                        (s.id, u),
                        (s.id, s.id * crate::models::FEATURE_ID_STRIDE + seq as u32),
                    )
                })
            })
        })
        .collect();
    // Index live shots by their CURRENT (section_id, shot_id) -> (uuid, saved
    // position). A position-keyed (NULL-uuid) row written before its shot's UUID was
    // persisted (resolve_warning on a pre-v6 or not-yet-saved shot) is upgraded
    // here: THIS save persists every shot's UUID, so we re-key such a row to the
    // shot's saved position AND its now-persisted UUID. Without this, its stale
    // in-memory shot_id could point at a different shot (or none) after an
    // insert/delete on the next load (issue #57 / Codex).
    let live_by_id: HashMap<(SectionId, u32), (&str, (SectionId, u32))> = project
        .sections
        .iter()
        .flat_map(|s| {
            s.shots.iter().enumerate().filter_map(move |(seq, sh)| {
                sh.uuid.as_deref().map(|u| {
                    (
                        (s.id, sh.id),
                        (
                            u,
                            (s.id, s.id * crate::models::FEATURE_ID_STRIDE + seq as u32),
                        ),
                    )
                })
            })
        })
        .collect();
    // Remap each UUID-keyed row to its shot's live position; DROP a UUID-keyed row
    // whose shot was deleted (UUID no longer live) - it can never match a live shot
    // by UUID on reload, so it is dead. Keeping it with its stale id would let it
    // collide (under UNIQUE(section_id, shot_id, warning_code) + INSERT OR REPLACE)
    // with a live shot that a renumber moved onto that position, and REPLACE could
    // drop either row depending on insert order - i.e. silently lose the live shot's
    // still-valid ignore. Dropping dead rows here also stops them accumulating
    // across saves. NULL-uuid (legacy, position-keyed) rows are kept and matched by
    // position (the load-time backfill upgrades the ones that map to a shot).
    rows.into_iter()
        .filter_map(|mut r| {
            match r.shot_uuid.as_deref() {
                Some(uuid) => {
                    // Look up by (section_id, uuid): a UUID-keyed row's section_id is
                    // its shot's section, so this targets the right shot even when
                    // another section shares the UUID.
                    let &(sec, shot) = live_positions.get(&(r.section_id, uuid))?;
                    r.section_id = sec;
                    r.shot_id = shot;
                    Some(r)
                }
                None => {
                    // Upgrade a position-keyed row to its shot's saved position +
                    // now-persisted UUID when its (section_id, shot_id) still maps to
                    // a live shot. A row that maps to NO live shot is DEAD (its shot
                    // was deleted) - DROP it: carrying it forward would let the
                    // load-time position backfill re-attach it to whatever shot now
                    // occupies that saved position, suppressing an unrelated shot's
                    // warning (issue #57 / Codex).
                    let &(uuid, (sec, shot)) = live_by_id.get(&(r.section_id, r.shot_id))?;
                    r.shot_uuid = Some(uuid.to_string());
                    r.section_id = sec;
                    r.shot_id = shot;
                    Some(r)
                }
            }
        })
        // Then drop any (legacy) row whose section no longer exists so the FK on
        // section_id holds for every carried-over row.
        .filter(|r| section_ids.contains(&r.section_id))
        .collect()
}

/// Test-only fault injection for the side-table writes: they have no organic
/// trigger, because the temp file they write into is fresh.
#[cfg(test)]
pub(super) mod fault {
    use std::cell::RefCell;

    thread_local! {
        static FAILING_WRITES: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    /// Every write into `table` on this thread fails until the guard drops.
    pub(in crate::core::state) fn fail_writes_to(table: &'static str) -> FailingWrites {
        FAILING_WRITES.with(|failing| failing.borrow_mut().push(table));
        FailingWrites(table)
    }

    /// Clears its injection on drop, so a failed assertion cannot leak it into the
    /// next test on the same thread.
    pub(in crate::core::state) struct FailingWrites(&'static str);

    impl Drop for FailingWrites {
        fn drop(&mut self) {
            FAILING_WRITES.with(|failing| failing.borrow_mut().retain(|t| *t != self.0));
        }
    }

    /// The injected failure for a write into `table`, if one is armed.
    pub(in crate::core::state) fn check_write(table: &'static str) -> anyhow::Result<()> {
        if FAILING_WRITES.with(|failing| failing.borrow().contains(&table)) {
            anyhow::bail!("injected write failure into {table}");
        }
        Ok(())
    }
}
