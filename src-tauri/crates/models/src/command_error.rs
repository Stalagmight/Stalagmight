//! [#576 C4/A6] The one routing code every IPC failure carries.
//!
//! A failed command used to say WHAT went wrong only in its message: a
//! `CommandResult.error`, a `Result<_, String>` or a `LoadError.code` string,
//! with routing markers (`EDIT_DISABLED: ...`, `FS_PERMISSION_DENIED: ...`)
//! prefixed onto the text and matched with `startsWith` on the frontend.
//! Rewording a message silently changed which notice, dialog or banner the user
//! got. Every failure now carries a [`CommandErrorCode`] instead, and its
//! message is display-only: nothing parses it.
//!
//! Lives in the models crate, the one every other crate depends on, so the
//! SpeleoDB crate's errors can carry the same code as the core's.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What kind of failure an IPC command reports, for the frontend to route on.
///
/// Wire form is the SCREAMING_SNAKE_CASE name (`EDIT_DISABLED`), the spelling
/// the old string markers had, pinned by
/// `command_error::tests::every_code_serializes_as_its_wire_name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    export,
    export_to = "../../bindings/",
    rename_all = "SCREAMING_SNAKE_CASE"
)]
pub enum CommandErrorCode {
    // --- Unclassified ----------------------------------------------------------
    /// No more specific code: the message says what went wrong, as is.
    Failed,
    /// The command itself broke (its body panicked on the blocking pool, or an
    /// invariant failed). Not the user's doing; the message is for a bug report.
    InternalError,
    /// [#576 A11] Refused because an earlier internal error poisoned the state
    /// this command would replay or publish (`AppState::refuse_after_crash`).
    RefusedAfterInternalError,

    // --- The edit gate: an EDIT refused because editing is blocked --------------
    /// A remote SpeleoDB project open without the write lock.
    EditDisabled,
    /// [#576 C5] The workspace is Plan or Export, which never edit the survey.
    WorkspaceReadOnly,
    /// [#576 B6-D1] An edit of a project a NEWER Stalagmight saved. The edit-gate
    /// twin of [`Self::NewerFileReadOnly`], which a refused SAVE carries: the two
    /// are worded differently (an edit notice vs. a refused save).
    NewerFileEditDisabled,
    /// [#576 NF-W4C-3-EDIT, RV5-F4-5] An edit refused while an Ariane adoption
    /// hands the open project over to SpeleoDB: no edit until its push has
    /// landed. Its own code so the notice says "being handed over" rather than
    /// inviting a Start editing that would race the adoption's lock operations.
    AdoptionInProgress,
    /// [#202] The lazy write-lock re-verify found the lock is no longer ours: the
    /// edit was blocked and the frontend raises the divergence prompt.
    EditReverifyDiverged,

    // --- Undo / redo -----------------------------------------------------------
    /// Undo with an empty undo stack. Nothing ran.
    NothingToUndo,
    /// Redo with an empty redo stack. Nothing ran.
    NothingToRedo,
    /// An undo/redo step ran and failed - the project CHANGED (the steps before
    /// it landed, and the failing one can mutate before it reports). The error
    /// carries `history_step` and the `reversed_sections` that landed.
    HistoryStepFailed,

    // --- Save refusals and filesystem failures ---------------------------------
    /// The OS denied filesystem access (e.g. macOS forgot a folder grant), on a
    /// save or on an open.
    FsPermissionDenied,
    /// [#429] The save destination is not reachable: folder gone, drive missing.
    FsPathNotFound,
    /// [#429] The save destination path is longer than the OS will open.
    FsPathTooLong,
    /// [#517] The save was refused because the ignore list could not be read.
    IgnoresUnreadable,
    /// [#576 A3] The save was refused because the SpeleoDB link could not be read.
    SpeleodbLinkUnreadable,
    /// [#576 A3] The save was refused because the SpeleoDB undo markers could not
    /// be read.
    UndoMarkersUnreadable,
    /// [#576 B15] The save was refused because stored per-project settings could
    /// not be read when the project was opened.
    ProjectSettingsUnreadable,
    /// [#576 B6-D1] A save (or upload) refused: a NEWER Stalagmight saved this
    /// project. See [`Self::NewerFileEditDisabled`] for the edit refusal.
    NewerFileReadOnly,
    /// [#576 HFI-2-D2] The write-path policy refused the path (an app or system
    /// folder, a file type the app does not write) before anything was looked
    /// at or written. The existence probe a save or export runs first carries it,
    /// so the frontend goes straight to the write, which refuses with the reason,
    /// instead of first asking "could not check whether it exists".
    WritePathRefused,

    // --- Import ----------------------------------------------------------------
    /// [#576 NF-FIX3A-2] The user cancelled a file import before anything landed.
    ImportCancelled,

    // --- Project open / create (the former `LoadError` codes) ------------------
    /// A project is already open.
    AlreadyOpen,
    /// The open / create was cancelled.
    Cancelled,
    /// The file to open does not exist.
    NotFound,
    /// The project file is corrupt; `details` carries its path.
    Corrupt,
    /// [#608] The open was refused because the file's database schema holds
    /// something the app never creates, so opening it could run SQL the file
    /// chose: anything the persistence schema guard's `check_schema` refuses -
    /// a view, trigger or virtual table, an object named like a SQLite pragma,
    /// a CHECK the app never writes, a generated column, an expression or
    /// partial index, or a `DEFAULT (expr)` (#576 RV18-6). `details` carries
    /// its path.
    UnsafeFile,
    /// [#576 FZ-FZ-INT-R2] The open was refused because a save renamed a new file
    /// over the path while the open was reading it: what it read is a file that is
    /// no longer there. Nothing was committed. Retryable: the same open again reads
    /// the file now at the path (the frontend repeats it once, silently).
    FileReplacedDuringLoad,
    /// The file is a non-canonical copy of a SpeleoDB project; the error carries
    /// `speleodb_managed`.
    SpeleodbManaged,
    /// A database operation failed while opening or creating a project.
    Database,
    /// A filesystem operation failed while creating a project.
    Io,
    /// The file was saved by a pre-release build this one cannot open.
    PreReleaseFile,
    /// [#576 RV4-R-M3-OPEN] An older-format file where nothing may be written
    /// (the app's own folders, a system folder): the open would upgrade it in
    /// place, so it is refused and left as it was; the message says to copy the
    /// project somewhere writable.
    UpgradeRefused,
    /// [#576 RV6b-2] A per-project write (a display setting) refused because the
    /// project it was made for is no longer the open one: the user switched
    /// projects while the write was queued or in flight. Nothing was written.
    /// [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] Likewise a save or a survey edit whose
    /// expected project is not the open one.
    ProjectChanged,

    // --- SpeleoDB (the former `SpeleoDbAction`, one code per action) ----------
    /// The session token is dead: log in again.
    SpeleodbRelogin,
    /// 403 on a read endpoint: the account has read-only access.
    SpeleodbReadOnly,
    /// 403 on a write endpoint: no write permission, or the lock fence.
    SpeleodbFence,
    /// A run-once write without a confirmed server commit: stay dirty.
    SpeleodbStayDirty,
    /// Upload-304: the remote already has this state (benign).
    SpeleodbUpToDate,
    /// A retry-safe call exhausted its retries, or the server was not reached
    /// (`transport` says why).
    SpeleodbRetry,
    /// An Ariane-only project: offer the adoption flow.
    SpeleodbAdopt,
    /// A re-pull refused over local edits never pushed; the error carries
    /// `unpushed_local`.
    SpeleodbUnpushedLocalEdits,
    /// The dump held more than one openable file; the error carries
    /// `dump_candidates` and `dump_candidate_kind`.
    SpeleodbAmbiguous,
    /// Any other SpeleoDB failure: surface it, fail closed.
    SpeleodbFatal,
}

impl CommandErrorCode {
    /// Every code, in declaration order, for the wire-name tests. A new variant
    /// gets a wire name from the exhaustive [`Self::as_str`]; list it here too.
    pub const ALL: [Self; 43] = [
        Self::Failed,
        Self::InternalError,
        Self::RefusedAfterInternalError,
        Self::EditDisabled,
        Self::WorkspaceReadOnly,
        Self::NewerFileEditDisabled,
        Self::AdoptionInProgress,
        Self::EditReverifyDiverged,
        Self::NothingToUndo,
        Self::NothingToRedo,
        Self::HistoryStepFailed,
        Self::FsPermissionDenied,
        Self::FsPathNotFound,
        Self::FsPathTooLong,
        Self::IgnoresUnreadable,
        Self::SpeleodbLinkUnreadable,
        Self::UndoMarkersUnreadable,
        Self::ProjectSettingsUnreadable,
        Self::NewerFileReadOnly,
        Self::WritePathRefused,
        Self::ImportCancelled,
        Self::AlreadyOpen,
        Self::Cancelled,
        Self::NotFound,
        Self::Corrupt,
        Self::UnsafeFile,
        Self::FileReplacedDuringLoad,
        Self::SpeleodbManaged,
        Self::Database,
        Self::Io,
        Self::PreReleaseFile,
        Self::UpgradeRefused,
        Self::ProjectChanged,
        Self::SpeleodbRelogin,
        Self::SpeleodbReadOnly,
        Self::SpeleodbFence,
        Self::SpeleodbStayDirty,
        Self::SpeleodbUpToDate,
        Self::SpeleodbRetry,
        Self::SpeleodbAdopt,
        Self::SpeleodbUnpushedLocalEdits,
        Self::SpeleodbAmbiguous,
        Self::SpeleodbFatal,
    ];

    /// The wire name (`"EDIT_DISABLED"`), for logs and tracing fields.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "FAILED",
            Self::InternalError => "INTERNAL_ERROR",
            Self::RefusedAfterInternalError => "REFUSED_AFTER_INTERNAL_ERROR",
            Self::EditDisabled => "EDIT_DISABLED",
            Self::WorkspaceReadOnly => "WORKSPACE_READ_ONLY",
            Self::NewerFileEditDisabled => "NEWER_FILE_EDIT_DISABLED",
            Self::AdoptionInProgress => "ADOPTION_IN_PROGRESS",
            Self::EditReverifyDiverged => "EDIT_REVERIFY_DIVERGED",
            Self::NothingToUndo => "NOTHING_TO_UNDO",
            Self::NothingToRedo => "NOTHING_TO_REDO",
            Self::HistoryStepFailed => "HISTORY_STEP_FAILED",
            Self::FsPermissionDenied => "FS_PERMISSION_DENIED",
            Self::FsPathNotFound => "FS_PATH_NOT_FOUND",
            Self::FsPathTooLong => "FS_PATH_TOO_LONG",
            Self::IgnoresUnreadable => "IGNORES_UNREADABLE",
            Self::SpeleodbLinkUnreadable => "SPELEODB_LINK_UNREADABLE",
            Self::UndoMarkersUnreadable => "UNDO_MARKERS_UNREADABLE",
            Self::ProjectSettingsUnreadable => "PROJECT_SETTINGS_UNREADABLE",
            Self::NewerFileReadOnly => "NEWER_FILE_READ_ONLY",
            Self::WritePathRefused => "WRITE_PATH_REFUSED",
            Self::ImportCancelled => "IMPORT_CANCELLED",
            Self::AlreadyOpen => "ALREADY_OPEN",
            Self::Cancelled => "CANCELLED",
            Self::NotFound => "NOT_FOUND",
            Self::Corrupt => "CORRUPT",
            Self::UnsafeFile => "UNSAFE_FILE",
            Self::FileReplacedDuringLoad => "FILE_REPLACED_DURING_LOAD",
            Self::SpeleodbManaged => "SPELEODB_MANAGED",
            Self::Database => "DATABASE",
            Self::Io => "IO",
            Self::PreReleaseFile => "PRE_RELEASE_FILE",
            Self::UpgradeRefused => "UPGRADE_REFUSED",
            Self::ProjectChanged => "PROJECT_CHANGED",
            Self::SpeleodbRelogin => "SPELEODB_RELOGIN",
            Self::SpeleodbReadOnly => "SPELEODB_READ_ONLY",
            Self::SpeleodbFence => "SPELEODB_FENCE",
            Self::SpeleodbStayDirty => "SPELEODB_STAY_DIRTY",
            Self::SpeleodbUpToDate => "SPELEODB_UP_TO_DATE",
            Self::SpeleodbRetry => "SPELEODB_RETRY",
            Self::SpeleodbAdopt => "SPELEODB_ADOPT",
            Self::SpeleodbUnpushedLocalEdits => "SPELEODB_UNPUSHED_LOCAL_EDITS",
            Self::SpeleodbAmbiguous => "SPELEODB_AMBIGUOUS",
            Self::SpeleodbFatal => "SPELEODB_FATAL",
        }
    }

    /// An EDIT refused because editing is blocked (the read-only notice family):
    /// the SpeleoDB lock, the Plan/Export workspace, a newer file, or an adoption
    /// hand-over. Not the re-verify divergence, which has its own prompt.
    pub const fn is_edit_gate(self) -> bool {
        matches!(
            self,
            Self::EditDisabled
                | Self::WorkspaceReadOnly
                | Self::NewerFileEditDisabled
                | Self::AdoptionInProgress
        )
    }
}

impl std::fmt::Display for CommandErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::CommandErrorCode;

    #[test]
    fn every_code_serializes_as_its_wire_name() {
        for code in CommandErrorCode::ALL {
            let wire = serde_json::to_value(code).expect("serialize code");
            assert_eq!(wire, serde_json::Value::String(code.as_str().to_owned()));
            let back: CommandErrorCode = serde_json::from_value(wire).expect("deserialize code");
            assert_eq!(back, code);
        }
    }

    #[test]
    fn wire_names_are_unique() {
        let mut names: Vec<&str> = CommandErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            CommandErrorCode::ALL.len(),
            "duplicate wire name"
        );
    }

    #[test]
    fn the_edit_gate_family_is_exactly_the_read_only_notices() {
        let gate: Vec<CommandErrorCode> = CommandErrorCode::ALL
            .into_iter()
            .filter(|c| c.is_edit_gate())
            .collect();
        assert_eq!(
            gate,
            vec![
                CommandErrorCode::EditDisabled,
                CommandErrorCode::WorkspaceReadOnly,
                CommandErrorCode::NewerFileEditDisabled,
                CommandErrorCode::AdoptionInProgress,
            ]
        );
    }
}
