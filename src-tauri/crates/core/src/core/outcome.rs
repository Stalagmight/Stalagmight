//! [#576 A12] The success values of the commands that used to answer with a
//! `CommandResult`, one type per kind of answer.
//!
//! `CommandResult` was the union of every command's output: eleven fields, eight
//! of them filled by one or two commands, and `reversed_sections` meaning two
//! different things depending on who filled it. It survives inside the core as
//! the accumulator the `AppState` methods build their answer in, but it no longer
//! crosses the IPC boundary: each command converts it with
//! [`CommandResult::into_outcome`] into the type below that says exactly what that
//! command reports, and a failure into a [`CommandError`].
//!
//! [#576 D22] Every survey mutation's answer also carries [`RemoteLinkBits`], the
//! open project's SpeleoDB "not yet pushed" state as the backend holds it after
//! the mutation. One helper fills it (the app crate's `pool::run_mutation`, via
//! [`MutationOutcome`]), and the frontend applies it in one place, so no editing
//! path keeps a hand-written copy of the backend's dirty bookkeeping.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::command::{CommandResult, ImportReceipt, ReversedSection, ValidationWarning};
use crate::core::command_error::{CommandError, CommandErrorCode, HistoryStepFailure};

/// [#576 D22] The open project's SpeleoDB link bits after a mutation: the same
/// two `speleodb_link_status` reports, read from the backend's own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RemoteLinkBits {
    /// The copy holds edits the server has not seen (`speleo_link.has_unpushed_edits`).
    pub has_unpushed_edits: bool,
    /// A chain step has moved the copy off the server tip (the Unwind pointer is set).
    pub unwound: bool,
}

/// A success value that carries the [`RemoteLinkBits`] (#576 D22). Implemented by
/// every survey mutation's answer; `pool::run_mutation` is the one writer.
pub trait MutationOutcome {
    fn set_remote_link(&mut self, link: Option<RemoteLinkBits>);
}

/// Build a command's success value from the internal accumulator. Only called on
/// a successful `CommandResult` (see [`CommandResult::into_outcome`]).
pub trait FromCommandResult: Sized {
    /// # Errors
    /// `INTERNAL_ERROR` when the successful result lacks a field this answer
    /// requires - a backend bug, never a user error.
    fn from_success(result: CommandResult) -> Result<Self, CommandError>;
}

/// An edit with nothing to report beyond its validation warnings.
#[derive(Debug, Clone, Serialize, TS)]
pub struct EditOutcome {
    pub warnings: Vec<ValidationWarning>,
    /// See [`RemoteLinkBits`]; `null` for a local project.
    pub remote_link: Option<RemoteLinkBits>,
}

/// An edit that created (or names) one section.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SectionCreated {
    pub section_id: u32,
    pub warnings: Vec<ValidationWarning>,
    pub remote_link: Option<RemoteLinkBits>,
}

/// An edit that created one shot.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ShotCreated {
    pub shot_id: u32,
    pub warnings: Vec<ValidationWarning>,
    pub remote_link: Option<RemoteLinkBits>,
}

/// `reverse_section`: the reverse it applied.
///
/// No `section_id`: a reverse of a section with no shots is a provable no-op that
/// `AppState::execute` answers with a bare success (#24), so the producer does not
/// always set one, and the frontend never read it.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ReverseOutcome {
    /// The PARITY-REDUCED set of sections whose numbering this command reversed
    /// (see [`Command::reversed_section_ids`](crate::core::Command::reversed_section_ids)),
    /// each with the shot count in force at the reverse.
    pub reversed_sections: Vec<ReversedSection>,
    pub warnings: Vec<ValidationWarning>,
    pub remote_link: Option<RemoteLinkBits>,
}

/// `execute_batch`: the sections it created, and the reverses it applied.
#[derive(Debug, Clone, Serialize, TS)]
pub struct BatchOutcome {
    /// Every section the batch created; empty when it created none.
    pub section_ids: Vec<u32>,
    /// PARITY-REDUCED, as on [`ReverseOutcome::reversed_sections`]: a section the
    /// batch reversed an even number of times is not listed.
    pub reversed_sections: Vec<ReversedSection>,
    pub warnings: Vec<ValidationWarning>,
    pub remote_link: Option<RemoteLinkBits>,
}

/// `undo` / `redo` / `undo_to` / `redo_to`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct HistoryOutcome {
    /// An ORDERED PER-STEP LOG, one entry per reversing step in the order the
    /// steps ran, repeats intact - NOT parity-reduced, because the shot count can
    /// differ between steps. The consumer composes the entries into one transform.
    /// Named apart from the single-command `reversed_sections` precisely because
    /// the two must not be read alike (#576 A12).
    pub reversed_steps: Vec<ReversedSection>,
    pub warnings: Vec<ValidationWarning>,
    pub remote_link: Option<RemoteLinkBits>,
}

/// A file import (`import_sections`, `execute_*_import`).
#[derive(Debug, Clone, Serialize, TS)]
pub struct ImportOutcome {
    /// The sections the import created; empty when none landed.
    pub section_ids: Vec<u32>,
    /// The backend-authoritative receipt (see [`ImportReceipt`]).
    pub import_receipt: Option<ImportReceipt>,
    /// Execute-phase warnings, plus the deferred first save's `SYSTEM` warning
    /// when the import landed but could not be written (#92/#104).
    pub warnings: Vec<ValidationWarning>,
    pub remote_link: Option<RemoteLinkBits>,
}

/// `save_project`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SaveOutcome {
    /// Where the open project was saved; `null` when the save wrote a detached
    /// copy instead (see `unlinked_copy_path`).
    pub saved_path: Option<String>,
    /// Set when an explicit destination on a SpeleoDB-managed project wrote a
    /// DETACHED COPY: the open project itself was not saved.
    pub unlinked_copy_path: Option<String>,
    /// What the save could not keep (display settings, undo history, ...).
    pub warnings: Vec<ValidationWarning>,
}

/// `delete_project`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct DeleteProjectOutcome {
    /// The deleted file backed the open project, whose session is now closed
    /// (matched with `same_file`, so the frontend cannot predict it).
    pub session_closed: bool,
    pub warnings: Vec<ValidationWarning>,
}

/// A project-file command with only warnings to report (`close_project`,
/// `merge_projects`, `remove_from_recent_projects`).
#[derive(Debug, Clone, Serialize, TS)]
pub struct ProjectFileOutcome {
    pub warnings: Vec<ValidationWarning>,
}

macro_rules! carries_remote_link {
    ($($ty:ty),* $(,)?) => {$(
        impl MutationOutcome for $ty {
            fn set_remote_link(&mut self, link: Option<RemoteLinkBits>) {
                self.remote_link = link;
            }
        }
    )*};
}

carries_remote_link!(
    EditOutcome,
    SectionCreated,
    ShotCreated,
    ReverseOutcome,
    BatchOutcome,
    HistoryOutcome,
    ImportOutcome,
    crate::core::command::IntegrityRepairResult,
);

fn missing(field: &str) -> CommandError {
    CommandError::internal(format!(
        "The command reported success without its {field}; nothing to show for it."
    ))
}

impl FromCommandResult for EditOutcome {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            warnings: result.warnings,
            remote_link: None,
        })
    }
}

impl FromCommandResult for SectionCreated {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            section_id: result.section_id.ok_or_else(|| missing("section id"))?,
            warnings: result.warnings,
            remote_link: None,
        })
    }
}

impl FromCommandResult for ShotCreated {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            shot_id: result.shot_id.ok_or_else(|| missing("shot id"))?,
            warnings: result.warnings,
            remote_link: None,
        })
    }
}

impl FromCommandResult for ReverseOutcome {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            reversed_sections: result.reversed_sections.unwrap_or_default(),
            warnings: result.warnings,
            remote_link: None,
        })
    }
}

impl FromCommandResult for BatchOutcome {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            section_ids: result.section_ids,
            reversed_sections: result.reversed_sections.unwrap_or_default(),
            warnings: result.warnings,
            remote_link: None,
        })
    }
}

impl FromCommandResult for HistoryOutcome {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            reversed_steps: result.reversed_sections.unwrap_or_default(),
            warnings: result.warnings,
            remote_link: None,
        })
    }
}

impl FromCommandResult for ImportOutcome {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            section_ids: result.section_ids,
            import_receipt: result.import_receipt,
            warnings: result.warnings,
            remote_link: None,
        })
    }
}

impl FromCommandResult for SaveOutcome {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            saved_path: result.saved_path,
            unlinked_copy_path: result.unlinked_copy_path,
            warnings: result.warnings,
        })
    }
}

impl FromCommandResult for DeleteProjectOutcome {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            session_closed: result.session_closed.unwrap_or(false),
            warnings: result.warnings,
        })
    }
}

impl FromCommandResult for ProjectFileOutcome {
    fn from_success(result: CommandResult) -> Result<Self, CommandError> {
        Ok(Self {
            warnings: result.warnings,
        })
    }
}

impl CommandResult {
    /// The IPC answer of a command whose `AppState` method built a
    /// `CommandResult`: its success value `T`, or its failure as a
    /// [`CommandError`].
    ///
    /// # Errors
    /// The failure the result carries ([`Self::into_error`]), or `INTERNAL_ERROR`
    /// when a success lacks a field `T` requires.
    pub fn into_outcome<T: FromCommandResult>(self) -> Result<T, CommandError> {
        if self.success {
            T::from_success(self)
        } else {
            Err(self.into_error())
        }
    }

    /// This (failed) result as a [`CommandError`]: its code (`FAILED` when the
    /// producer set none), its message, and the failure payloads it carries.
    pub fn into_error(self) -> CommandError {
        let mut err = CommandError::new(
            self.code.unwrap_or(CommandErrorCode::Failed),
            self.error.unwrap_or_default(),
        );
        err.payload.history_step = self.history_step;
        // A failed jump's per-step log of what landed (`undo_to` / `redo_to`).
        err.payload.reversed_sections = self.reversed_sections.filter(|r| !r.is_empty());
        err.payload.session_closed = self.session_closed.filter(|closed| *closed);
        err
    }

    /// A failure carrying `err`'s code, message and history payload - for a
    /// producer (a guard, the save) that classified its refusal.
    pub fn refused(err: CommandError) -> Self {
        Self {
            success: false,
            error: Some(err.message),
            code: Some(err.code),
            history_step: err.payload.history_step,
            reversed_sections: err.payload.reversed_sections,
            session_closed: err.payload.session_closed,
            ..Self::default()
        }
    }
}

/// Where a failed history step stopped, for [`CommandResult::refused`]'s callers.
pub fn history_step_failure(failed_step: usize, total_steps: usize) -> HistoryStepFailure {
    HistoryStepFailure {
        failed_step: u32::try_from(failed_step).unwrap_or(u32::MAX),
        total_steps: u32::try_from(total_steps).unwrap_or(u32::MAX),
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{
        BatchOutcome, EditOutcome, HistoryOutcome, MutationOutcome, RemoteLinkBits, SectionCreated,
    };
    use crate::core::command::{CommandResult, ReversedSection};
    use crate::core::command_error::{CommandError, CommandErrorCode};

    #[test]
    fn a_failed_result_becomes_its_coded_error() {
        let mut result = CommandResult::refused(CommandError::new(
            CommandErrorCode::NothingToUndo,
            "Nothing to undo",
        ));
        result.reversed_sections = Some(Vec::new());
        let err = result
            .into_outcome::<HistoryOutcome>()
            .expect_err("a failure stays a failure");
        assert_eq!(err.code, CommandErrorCode::NothingToUndo);
        assert_eq!(err.message, "Nothing to undo");
        assert!(
            err.payload.reversed_sections.is_none(),
            "an empty log is not sent"
        );
    }

    #[test]
    fn an_uncoded_failure_is_unclassified() {
        let err = CommandResult::failure("boom".to_owned())
            .into_outcome::<EditOutcome>()
            .expect_err("failure");
        assert_eq!(err.code, CommandErrorCode::Failed);
        assert_eq!(err.message, "boom");
    }

    #[test]
    fn a_success_missing_a_required_field_is_an_internal_error() {
        let err = CommandResult::success()
            .into_outcome::<SectionCreated>()
            .expect_err("no section id");
        assert_eq!(err.code, CommandErrorCode::InternalError);
    }

    #[test]
    fn a_batch_reports_its_sections_and_reverses() {
        let reversed = ReversedSection {
            section_id: 3,
            shot_count: 4,
        };
        let mut outcome = CommandResult::success()
            .with_section_ids(vec![7])
            .with_reversed_sections(vec![reversed])
            .into_outcome::<BatchOutcome>()
            .expect("success");
        assert_eq!(outcome.section_ids, vec![7]);
        assert_eq!(outcome.reversed_sections, vec![reversed]);
        assert!(
            outcome.remote_link.is_none(),
            "filled by the IPC helper, not here"
        );
        let bits = RemoteLinkBits {
            has_unpushed_edits: true,
            unwound: false,
        };
        outcome.set_remote_link(Some(bits));
        assert_eq!(outcome.remote_link, Some(bits));
    }

    /// The ipc-typegen pair rule (docs/ipc-typegen.md) for the success values:
    /// nothing on them is omitted from the wire - an empty `section_ids` is `[]`
    /// (#441) and an unset `remote_link` is `null` - so nothing is declared `?`.
    #[test]
    fn no_outcome_field_is_omitted_or_declared_optional() {
        use super::{
            DeleteProjectOutcome, ImportOutcome, ProjectFileOutcome, ReverseOutcome, SaveOutcome,
            ShotCreated,
        };
        use ts_rs::TS;

        let empty_batch = serde_json::to_value(
            CommandResult::success()
                .into_outcome::<BatchOutcome>()
                .expect("success"),
        )
        .expect("serialize");
        assert_eq!(empty_batch["section_ids"], serde_json::json!([]));
        assert_eq!(empty_batch["reversed_sections"], serde_json::json!([]));
        assert_eq!(empty_batch["remote_link"], serde_json::Value::Null);

        for decl in [
            EditOutcome::decl(),
            SectionCreated::decl(),
            ShotCreated::decl(),
            ReverseOutcome::decl(),
            BatchOutcome::decl(),
            HistoryOutcome::decl(),
            ImportOutcome::decl(),
            SaveOutcome::decl(),
            DeleteProjectOutcome::decl(),
            ProjectFileOutcome::decl(),
        ] {
            assert!(
                !decl.contains("?:"),
                "an outcome field is never omitted: {decl}"
            );
        }
        assert!(BatchOutcome::decl().contains("section_ids: Array<number>"));
        assert!(EditOutcome::decl().contains("remote_link: RemoteLinkBits | null"));
    }

    #[test]
    fn a_mutation_outcome_always_sends_its_remote_link_key() {
        // Once per payload, so `null` rather than omitted (docs/ipc-typegen.md):
        // the generated `remote_link: RemoteLinkBits | null` is then the truth.
        let wire = serde_json::to_value(EditOutcome {
            warnings: Vec::new(),
            remote_link: None,
        })
        .expect("serialize");
        assert_eq!(
            wire,
            serde_json::json!({ "warnings": [], "remote_link": null })
        );
    }
}
