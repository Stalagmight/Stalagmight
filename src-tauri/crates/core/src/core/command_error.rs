//! [#576 C4] The one error every fallible IPC command rejects with.
//!
//! Commands used to fail in four shapes - a `CommandResult` with
//! `success: false`, a `Result<_, String>`, a `Result<_, SpeleoErrorDto>` and a
//! `Result<_, LoadError>` - and the frontend told the failures apart by reading
//! their English text. Every command now returns `Result<T, CommandError>`: the
//! [`CommandErrorCode`] is what the frontend routes on, `message` is display-only
//! (nothing parses it), and the optional payloads below say the rest. Each payload
//! is set only for the codes named on it, so its presence is a sound
//! discriminant, and is omitted from the wire otherwise.
//!
//! The subsystems keep their own error types where they need them
//! ([`crate::core::LoadError`], [`crate::speleodb::SpeleoErrorDto`]) and carry the
//! same code, so the conversion at the command boundary is lossless.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use crate::models::CommandErrorCode;

use crate::core::command::ReversedSection;
use crate::core::loading::SpeleoManagedFile;
use crate::core::outcome::RemoteLinkBits;
use crate::speleodb::{DumpCandidateKind, SpeleoDumpCandidate, TransportCause, UnpushedLocalEdits};

/// Where a failed multi-step undo/redo stopped (`HISTORY_STEP_FAILED`): step
/// `failed_step` of `total_steps` (1-based). A single-step undo/redo that ran and
/// failed reports step 1 of 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct HistoryStepFailure {
    pub failed_step: u32,
    pub total_steps: u32,
}

/// The error of every fallible IPC command (#576 C4). See the module docs.
///
/// On the wire it is ONE flat object - `code`, `message` and whichever payload
/// fields are set - although the payloads live behind one `Box` in Rust: that keeps
/// a `Result<_, CommandError>` small (the payload is one pointer, allocated only on
/// the failure path), so clippy's `result_large_err` needs no per-function excuse.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CommandError {
    /// What kind of failure this is - the ONLY thing the frontend routes on.
    pub code: CommandErrorCode,
    /// Human-readable, display-only. Never carries a code marker, and nothing
    /// parses it.
    pub message: String,
    /// The optional payloads, flattened into the same wire object.
    #[serde(flatten)]
    pub payload: Box<CommandErrorPayload>,
}

/// [`CommandError`]'s optional payloads. Each is set only for the codes named on
/// it, so its presence is a sound discriminant, and is omitted from the wire
/// otherwise (and declared `?:`, the pair rule in docs/ipc-typegen.md).
#[derive(Debug, Clone, Default, Serialize, TS)]
pub struct CommandErrorPayload {
    /// `CORRUPT`, `FS_PERMISSION_DENIED` on an open, `PRE_RELEASE_FILE`: the path
    /// of the file the open failed on (the former `LoadError::details`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// `SPELEODB_MANAGED`: the managed file the open refused, for the prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speleodb_managed: Option<SpeleoManagedFile>,
    /// SpeleoDB: the HTTP status of a server answer (none for transport / local).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// SpeleoDB (`SPELEODB_RETRY`): why the server was never reached. Set ONLY
    /// for a transport failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportCause>,
    /// `SPELEODB_UNPUSHED_LOCAL_EDITS`: the local work a refused re-pull would
    /// have destroyed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unpushed_local: Option<UnpushedLocalEdits>,
    /// `SPELEODB_AMBIGUOUS`: the openable files the dump offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dump_candidates: Option<Vec<SpeleoDumpCandidate>>,
    /// `SPELEODB_AMBIGUOUS`: which ambiguity `dump_candidates` describes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dump_candidate_kind: Option<DumpCandidateKind>,
    /// `HISTORY_STEP_FAILED`: which step of the jump failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_step: Option<HistoryStepFailure>,
    /// `HISTORY_STEP_FAILED`: the ordered per-step log of the sections the steps
    /// that LANDED renumbered (see `HistoryOutcome::reversed_steps`); absent when
    /// none did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversed_sections: Option<Vec<ReversedSection>>,
    /// `delete_project`: the delete tore the open project's session down before
    /// it failed (see `DeleteProjectOutcome::session_closed`). Only ever `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_closed: Option<bool>,
    /// [#576 D22] A survey mutation's failure: the open project's SpeleoDB link
    /// bits as the backend holds them after the attempt (a failed undo can still
    /// have dirtied the copy). Absent for a local project and for every
    /// non-mutating command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_link: Option<RemoteLinkBits>,
}

impl CommandError {
    /// A failure with `code` and display `message` and no payload.
    pub fn new(code: CommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            payload: Box::default(),
        }
    }

    /// An unclassified failure (`FAILED`): the message says what went wrong.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(CommandErrorCode::Failed, message)
    }

    /// The command itself broke (`INTERNAL_ERROR`).
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(CommandErrorCode::InternalError, message)
    }

    /// Attach the load payload `details` (a path).
    #[must_use]
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.payload.details = Some(details.into());
        self
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

/// A plain `String` error carries no classification: `FAILED`.
impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::failed(message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self::failed(message)
    }
}

/// A refused open / create: its code, message and load payloads, as they were.
impl From<crate::core::loading::LoadError> for CommandError {
    fn from(err: crate::core::loading::LoadError) -> Self {
        let mut out = Self::new(err.code, err.message);
        out.payload.details = err.details;
        out.payload.speleodb_managed = err.speleodb_managed;
        out
    }
}

/// A SpeleoDB failure: its code (one per `SpeleoDbAction`, which is not carried)
/// and its message and payloads, as they were.
impl From<crate::speleodb::SpeleoErrorDto> for CommandError {
    fn from(err: crate::speleodb::SpeleoErrorDto) -> Self {
        let mut out = Self::new(err.code, err.message);
        out.payload.status = err.status;
        out.payload.transport = err.transport;
        out.payload.unpushed_local = err.unpushed_local;
        out.payload.dump_candidates = err.dump_candidates;
        out.payload.dump_candidate_kind = err.dump_candidate_kind;
        out
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{CommandError, CommandErrorCode, HistoryStepFailure};

    #[test]
    fn the_error_stays_small() {
        // `Result<_, CommandError>` is the return type of every fallible command and
        // guard; clippy's `result_large_err` draws the line at 128 bytes.
        assert!(size_of::<CommandError>() <= 64);
    }

    #[test]
    fn an_error_without_payload_is_code_and_message_only() {
        let wire = serde_json::to_value(CommandError::new(
            CommandErrorCode::EditDisabled,
            "Undo is disabled: this SpeleoDB project is open read-only.",
        ))
        .expect("serialize");
        assert_eq!(
            wire,
            serde_json::json!({
                "code": "EDIT_DISABLED",
                "message": "Undo is disabled: this SpeleoDB project is open read-only.",
            })
        );
    }

    #[test]
    fn a_payload_is_on_the_wire_only_when_set() {
        let mut err = CommandError::new(CommandErrorCode::HistoryStepFailed, "Undo failed");
        err.payload.history_step = Some(HistoryStepFailure {
            failed_step: 2,
            total_steps: 3,
        });
        let wire = serde_json::to_value(&err).expect("serialize");
        let mut keys: Vec<&str> = wire
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["code", "history_step", "message"]);
        assert_eq!(wire["history_step"]["failed_step"], 2);
    }

    /// The ipc-typegen pair rule (docs/ipc-typegen.md) for the error envelope:
    /// every payload is omitted from the wire when unset, so every payload must be
    /// DECLARED optional, and the two required fields must not be.
    #[test]
    fn omitted_fields_are_exactly_the_ones_declared_optional() {
        use ts_rs::TS;

        let wire = serde_json::to_value(CommandError::failed("x")).expect("serialize");
        let mut present: Vec<&str> = wire
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        present.sort_unstable();
        assert_eq!(present, vec!["code", "message"]);

        let decl = CommandError::decl();
        for required in ["code: CommandErrorCode", "message: string"] {
            assert!(
                decl.contains(required),
                "`{required}` must stay required: {decl}"
            );
        }
        for omitted in [
            "details",
            "speleodb_managed",
            "status",
            "transport",
            "unpushed_local",
            "dump_candidates",
            "dump_candidate_kind",
            "history_step",
            "reversed_sections",
            "session_closed",
            "remote_link",
        ] {
            assert!(
                decl.contains(&format!("{omitted}?:")),
                "`{omitted}` is omitted from the wire when unset, so its declaration \
                 must carry the `?`: {decl}"
            );
        }
    }

    #[test]
    fn a_string_error_is_unclassified() {
        let err: CommandError = "disk full".to_owned().into();
        assert_eq!(err.code, CommandErrorCode::Failed);
        assert_eq!(err.message, "disk full");
    }
}
