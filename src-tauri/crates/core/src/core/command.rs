use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use ts_rs::TS;

use crate::models::{
    shot_display_position, ClosureStrategy, Coordinates, GpsCoordinates, SectionId, SectionUpdates,
    ShotData, ShotId, ShotKind, ShotUpdates, StationRef, SurveyDirection,
};

/// Identifies either a station or a shot as a comment holder
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommentHolder {
    Station {
        station_ref: StationRef,
    },
    Shot {
        section_id: SectionId,
        shot_id: ShotId,
    },
}

impl std::fmt::Display for CommentHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentHolder::Station { station_ref } => write!(f, "station {station_ref}"),
            CommentHolder::Shot {
                section_id,
                shot_id,
            } => {
                let pos = shot_display_position(*shot_id, *section_id);
                write!(f, "shot {section_id}:{pos}")
            }
        }
    }
}

/// All persistent commands that modify project state
///
/// Generated as the TS `Command` (#576 C3), which `execute_batch` takes and the
/// frontend's `BatchCommand` narrows. The frontend only ever SENDS one, so every
/// `Option` field is declared `?:` - serde reads a missing key as `None`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    // Project commands
    CreateProject {
        name: String,
        #[ts(optional)]
        location: Option<String>,
    },
    OpenProject {
        path: PathBuf,
    },
    SaveProject {
        #[ts(optional)]
        path: Option<PathBuf>,
    },
    CloseProject,

    // Section commands
    CreateSection {
        #[ts(optional)]
        name: Option<String>,
        #[ts(optional)]
        surveyor: Option<String>,
        #[ts(type = "string")]
        #[ts(optional)]
        date: Option<NaiveDate>,
        color: String,
        #[serde(default)]
        #[ts(optional)]
        direction: Option<SurveyDirection>,
        shots: Vec<ShotData>,
    },
    DeleteSection {
        section_id: SectionId,
    },
    UpdateSectionMetadata {
        section_id: SectionId,
        updates: SectionUpdates,
    },
    /// Reverse a section end-to-end so the far end becomes station 0, keeping
    /// every station's world coordinate (see [`crate::models::Project::reverse_section`]).
    /// `swap_left_right` also mirrors the `left`/`right` passage dimensions; it
    /// is opt-in because whether LRUD was recorded facing along travel is a
    /// survey convention we cannot infer.
    ReverseSection {
        section_id: SectionId,
        #[serde(default)]
        swap_left_right: bool,
    },

    // Shot commands
    CreateShot {
        section_id: SectionId,
        shot: ShotData,
        #[ts(optional)]
        after_shot_id: Option<ShotId>,
        #[serde(default)]
        #[ts(optional)]
        before_shot_id: Option<ShotId>,
    },
    UpdateShot {
        section_id: SectionId,
        shot_id: ShotId,
        updates: ShotUpdates,
    },
    DeleteShot {
        section_id: SectionId,
        shot_id: ShotId,
    },
    /// Set a shot's [`ShotKind`] (#27). Replaces the old boolean
    /// `SetShotReference`.
    ///
    /// Reads pre-#27 persisted undo history **losslessly** rather than forcing
    /// it to be discarded: the variant aliases the old `set_shot_reference` /
    /// `set_shot_virtual` tags, and the `kind` field aliases the old
    /// `is_reference` bool through `models::shot::de_shot_kind`, which
    /// maps `true` to [`ShotKind::Jump`] per q2. An old entry's inverse
    /// (`SetShotReference { is_reference: !x }`) therefore still round-trips, so
    /// upgrading does not cost the user their undo stack.
    #[serde(alias = "set_shot_virtual", alias = "set_shot_reference")]
    SetShotKind {
        section_id: SectionId,
        shot_id: ShotId,
        #[serde(
            default,
            alias = "is_reference",
            alias = "is_virtual",
            deserialize_with = "crate::models::shot::de_shot_kind"
        )]
        kind: ShotKind,
    },

    // Station commands
    MergeStations {
        station_a: StationRef,
        station_b: StationRef,
    },
    UnmergeStations {
        station_ref: StationRef,
    },
    UnmergeStationPair {
        station_a: StationRef,
        station_b: StationRef,
    },
    CreateLoopClosure {
        station_a: StationRef,
        station_b: StationRef,
        closure_strategy: ClosureStrategy,
    },
    RemoveLoopClosure {
        #[ts(type = "number")]
        closure_id: u64,
    },
    SetZeroDatum {
        station_ref: StationRef,
        coordinates: GpsCoordinates,
    },
    RemoveZeroDatum {
        station_ref: StationRef,
    },

    // Station comment commands
    SetStationComment {
        station_ref: StationRef,
        #[ts(optional)]
        comment: Option<String>,
    },

    // Move commands
    MoveSection {
        section_id: SectionId,
        offset: Coordinates,
    },
    MoveSegment {
        section_id: SectionId,
        offset: Coordinates,
    },
    MoveStation {
        station_ref: StationRef,
        offset: Coordinates,
    },

    // Comment move commands
    MoveComment {
        source: CommentHolder,
        target: CommentHolder,
        swap: bool,
    },

    /// Forget import-dedup bookkeeping for an import that was WITHDRAWN (#394).
    ///
    /// `execute_import_sections` records the file hash and each section's content
    /// hash so a later re-import can be recognised as a duplicate, and deleting
    /// the sections does not touch those lists (`cleanup_section_references`
    /// walks merges, closures, offsets, comments and datums - nothing else). So
    /// the DMP wizard's "Cancel, remove what was imported" left the project
    /// claiming the file was already imported while none of it remained, and the
    /// selection dialog silently DESELECTED every section on the next attempt.
    ///
    /// Batched with the deletes it belongs to, so one undo restores the sections
    /// and their tracking together - the snapshot pair covers both, since both
    /// live on `Project`.
    UntrackImport {
        /// File-level hashes to forget (`Project::imported_file_hashes`).
        file_hashes: Vec<String>,
        /// Section content hashes to forget (`Project::imported_section_hashes`).
        section_hashes: Vec<String>,
    },

    // Batch commands
    Batch {
        commands: Vec<Command>,
    },

    // Project merge command
    MergeProjects {
        /// Paths to the source projects to merge
        source_paths: Vec<PathBuf>,
        /// Path for the merged output project
        output_path: PathBuf,
        /// Name for the merged project
        output_name: String,
        /// Whether to archive (rename) the original files
        archive_originals: bool,
    },
    /// Repair the open project's structural integrity (drop dangling refs, re-mint
    /// duplicate UUIDs, re-cascade offsets). Applied directly via the
    /// `repair_project_integrity` IPC command, which records its own snapshot-pair
    /// undo entry; this variant exists only to LABEL that entry in the undo
    /// history and is never run through `execute()`.
    RepairIntegrity,
    /// Per-project settings change (name + validation thresholds, #57). Like
    /// RepairIntegrity, applied via `update_project_settings` which records its
    /// own snapshot-pair undo entry; this variant only LABELS that entry and is
    /// never run through `execute()`.
    UpdateProjectSettings,
    /// Entrance elevation edit (#465). Like UpdateProjectSettings, applied via
    /// `set_surface_elevation` which records its own snapshot-pair undo entry;
    /// this variant only LABELS that entry and is never run through `execute()`.
    SetSurfaceElevation,
}

/// One section whose station numbering an operation reversed, together with the
/// shot count `N` that was IN FORCE at that reverse - the divisor the mirror
/// `i -> N - i` needs.
///
/// The count travels with the entry rather than being read off the refreshed
/// project because a multi-step `undo_to` can walk past a `CreateShot`: the
/// post-range count is then not the count the earlier steps mirrored against, and
/// a consumer that divided by it would land one station out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ReversedSection {
    pub section_id: u32,
    pub shot_count: u32,
}

pub use crate::models::{sanitize_display_text, ImportReceipt};

/// The most of a file-controlled name worth keeping in the receipt. A dialog title
/// line, not a path: longer than any real survey filename, shorter than anything that
/// can push the rest of the title off the screen.
///
/// `pub(crate)` so `command_import.rs` can sanitize `zero_datum_created` at the
/// point it is born (see there): the DMP wizard's `import_sections` IPC command
/// never calls `decorate_receipt` (it only overwrites `source_format`/
/// `file_name`/`sections_found` directly), so sanitizing solely in
/// `decorate_receipt` left that route's `zero_datum_created` - a raw
/// `ImportSectionConfig::name`, i.e. file-controlled DMP section-label text -
/// unsanitized end to end.
pub(crate) const MAX_FILE_NAME_LEN: usize = 120;

/// Fill in the importer-specific fields of the base `ImportReceipt` that
/// `execute_import_sections` already attached (source/section counts, unit
/// conversion), and extend its warnings with the converter-phase warnings
/// collected while turning the parsed file into `ImportSectionConfig`s.
///
/// No-op when `result` carries no receipt (a FAILED import - the empty-import
/// path deliberately carries one, see `ImportReceipt::empty`), so a call site
/// never has to special-case that path.
///
/// Shared with the SpeleoDB Ariane adoption (#411), which is an Ariane import with
/// no wizard in front of it and must produce the same receipt as one that has.
pub fn decorate_receipt(
    result: &mut CommandResult,
    source_format: &str,
    path: &std::path::Path,
    sections_found: u32,
    unit_conversion: Option<String>,
    converter_warnings: Vec<String>,
) {
    let Some(receipt) = result.import_receipt.as_mut() else {
        return;
    };
    receipt.source_format = source_format.to_string();
    // Sanitized and clamped, because this lands in the receipt dialog's TITLE and is
    // not always a name off this machine's disk: the SpeleoDB adoption passes the
    // entry name from inside a dump downloaded from a server, where the whole 64 KB
    // a zip name may hold - bidi overrides included - is the archive author's to
    // choose. A local path is guarded by the same line at no cost.
    receipt.file_name = path
        .file_name()
        .map(|n| sanitize_display_text(&n.to_string_lossy(), MAX_FILE_NAME_LEN))
        .unwrap_or_default();
    receipt.sections_found = sections_found;
    receipt.unit_conversion = unit_conversion;
    // Same treatment as `file_name` above, and for the same reason: this is the
    // IMPORTED SECTION'S name (`ImportSectionConfig::name`), which for every
    // importer traces straight back to file content - a `.dat`/`.srv`/`.dmp`/
    // `.tml` section or survey label - and it renders verbatim in the receipt
    // dialog (F12c). Unsanitized, a hostile file could smuggle a bidi override
    // or an unbounded string into that dialog the same way an unsanitized
    // `file_name` could.
    receipt.zero_datum_created = receipt
        .zero_datum_created
        .take()
        .map(|name| sanitize_display_text(&name, MAX_FILE_NAME_LEN));
    // Converter warnings go FIRST, ahead of the execute-phase ones. They are
    // whole-file statements - an unrecognized unit, a section dropped entirely -
    // and there is a handful of them at most, while the execute phase can emit
    // one per shot. Appending them meant `cap_warnings`' 50-entry limit dropped
    // the highest-signal warning first on exactly the noisy file that needs it.
    // Every importer routes through here, so the ordering is the same for
    // Ariane, Compass, Walls, GPX and DMP.
    if !converter_warnings.is_empty() {
        let execute_warnings = std::mem::replace(&mut receipt.warnings, converter_warnings);
        receipt.warnings.extend(execute_warnings);
    }
    receipt.cap_warnings();
}

/// Result of command execution - the accumulator the `AppState` methods build
/// their answer in.
///
/// [#576 A12/C4] No longer an IPC type: every command converts it at the
/// boundary with [`CommandResult::into_outcome`] into its own success value
/// (`EditOutcome`, `SaveOutcome`, ... in [`crate::core::outcome`]) or a
/// [`crate::core::CommandError`]. The field docs below that speak of "the wire"
/// describe what those outcome types carry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandResult {
    pub success: bool,
    pub warnings: Vec<ValidationWarning>,
    /// The failure's display message (`None` on success). Never carries a code
    /// marker: the classification is [`Self::code`].
    pub error: Option<String>,
    /// [#576 C4] What kind of failure this is, for the frontend to route on;
    /// `None` on success, and on a failure nothing classified (`FAILED`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<crate::models::CommandErrorCode>,
    /// [#576 C4] `HISTORY_STEP_FAILED`: which step of an undo/redo jump failed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub history_step: Option<crate::core::command_error::HistoryStepFailure>,
    /// ID of the created section (for CreateSection command)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section_id: Option<u32>,
    /// IDs of created sections (for Batch commands containing CreateSection);
    /// EMPTY, never absent, for every command that created none.
    ///
    /// Deliberately NOT `skip_serializing_if` (#441). ts-rs 7 can emit `field?:`
    /// only for an `Option<T>` - `optional` is rejected on any other type - so a
    /// skipped `Vec` generates as a REQUIRED `Array<number>` while the wire omits
    /// the key, and `tsc` is then structurally blind to every bare
    /// `result.section_ids.length`. That is what crashed the zero-section import
    /// receipt in #411.
    ///
    /// Absence and empty state the same fact here, so the honest fix is to always
    /// send the array rather than teach ~17 read sites a distinction that carries
    /// no information. The cost is one `"section_ids":[]` per command result -
    /// once per user action. Contrast [`crate::models::Station::closures_with`],
    /// where the identical skip is paid PER STATION on every `get_project_state`
    /// refresh and therefore stays, with `Option<Vec<_>>` carrying the optionality
    /// into the generated type instead.
    ///
    /// `default` is kept so a result serialized before this change still reads.
    #[serde(default)]
    pub section_ids: Vec<u32>,
    /// ID of the created shot (for CreateShot command)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shot_id: Option<u32>,
    /// Path the project was saved to (for SaveProject command)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_path: Option<String>,
    /// Set when this command TORE DOWN the open project's session - today only
    /// `delete_project`, and only when the `.stalx` it deleted was the file backing
    /// the open project (matched with `same_file`).
    ///
    /// The frontend has to be TOLD, because neither half of the obvious prediction
    /// holds:
    ///
    /// * `success` is not a precondition. The backend closes BEFORE handing the
    ///   file to the OS trash, and it must: SQLite holds the file open without
    ///   `FILE_SHARE_DELETE`, so on Windows the trash is denied while that handle
    ///   lives and a close-afterwards would never be reached. A trash that then
    ///   fails for an unrelated reason (no write access to the parent directory)
    ///   therefore returns FAILURE with the session already gone.
    /// * the deleted path is not what the backend matched. `same_file`
    ///   canonicalizes, so a symlink, a `..` segment or a Windows case/short-name
    ///   variant of the open project's path tears the session down while a `===` on
    ///   the raw strings says it did not.
    ///
    /// Reporting it here also replaces what the frontend used to do instead - call
    /// `get_project_state` afterwards and test it for null. That answers the same
    /// yes/no question by cloning the whole project and recomputing station
    /// coordinates, stats, penetration and frozen shots, which on the 75k-shot
    /// projects this app is built for made every delete-while-open needlessly slow.
    ///
    /// `Option<bool>`, only ever `Some(true)`, for the same wire contract as
    /// `section_id` / `shot_id` / `saved_path` above: `skip_serializing_if` keeps
    /// the key absent for every command that closed nothing, and the `Option` is
    /// what makes ts-rs emit `session_closed?: boolean` rather than a required
    /// field that would break every existing `CommandResult` literal on the
    /// frontend (see `reversed_sections` for the same reasoning).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_closed: Option<bool>,
    /// Set when this save wrote a DETACHED COPY instead of saving the project:
    /// an explicit Save-As destination on a SpeleoDB-managed project writes the
    /// `.stalx` without its `speleo_link` row and leaves the session on the
    /// managed file (see `save_project`). The frontend must be TOLD, because
    /// the two things it normally does on a successful save are both wrong
    /// here: `saved_path` would re-point `project.file_path` at a file this
    /// session does not have open, and clearing `isDirty` would claim the open
    /// project had been persisted when it was not. `saved_path` is deliberately
    /// left `None` for exactly that reason. Same wire contract as the optional
    /// fields above (`skip_serializing_if`, so ts-rs emits `unlinked_copy_path?:`
    /// and the key stays absent for every ordinary save).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlinked_copy_path: Option<String>,
    /// The sections whose station NUMBERING this operation reversed (station `i`
    /// became station `N - i`), each carrying the shot count `N` in force at that
    /// reverse - for `ReverseSection`, for a `Batch` of them, and for an undo/redo
    /// of either (undoing a reverse re-reverses the numbering, so it reports the
    /// same sections).
    ///
    /// Every station keeps its world position, so this is not a geometry change
    /// the frontend can see by diffing coordinates: it exists so a positional
    /// station reference held on the frontend (a selection, a pinned plan point)
    /// can be mirrored EXACTLY rather than inferred by comparing station
    /// positions across the operation - an inference that misses a section which
    /// was reversed and moved in the same action.
    ///
    /// TWO SHAPES, and a consumer must not treat them alike:
    ///
    /// * From ONE command (`ReverseSection` or a `Batch`), this is a
    ///   PARITY-REDUCED set: [`Command::reversed_section_ids`] drops a section the
    ///   batch reversed an even number of times, because such a batch leaves the
    ///   numbering exactly as it found it and there is nothing to mirror. At most
    ///   one entry per section, and `shot_count` is unambiguous because a batch
    ///   cannot change a SURVIVING section's shot count (`is_batch_allowed` admits
    ///   no shot create/delete; the `DeleteSection` it does admit removes the whole
    ///   section, which is then dropped from this report entirely).
    /// * From `undo_to` / `redo_to`, this is an ORDERED PER-STEP LOG: one entry
    ///   per reversing step, in the order the steps ran, repeats intact. It is
    ///   deliberately NOT parity-reduced, because `shot_count` can differ between
    ///   steps (a `CreateShot` undone between two reverses of one section). The
    ///   consumer must COMPOSE the entries into a single transform and apply that
    ///   once; replaying the mirror per entry is wrong.
    ///
    /// Composition is required because the FRONTEND's mirror is not an involution: it is
    /// `i -> N - i` CLAMPED into range (`utils/reverseAftermath`), and a clamp is lossy, so
    /// replaying it per entry does not undo itself. The counts can also differ between entries
    /// - a `CreateShot` undone between two reverses of one section changes `N` - which is
    /// exactly the case neither "cancel the repeat" nor "replay the mirror twice" gets right.
    ///
    /// The sibling `.plnx` needs none of this: its plans name a station by the shots it sits
    /// BETWEEN, so a reverse does not move them at all (see `state::dive_plan`).
    ///
    /// `skip_serializing_if` + `default`, so the key is simply absent for every
    /// command that reversed nothing and an older frontend is unaffected - and typed
    /// `Option` rather than `Vec` precisely so ts-rs emits it as `reversed_sections?:`
    /// instead of a required array. That absence IS the wire truth, and a required
    /// field would break every existing `CommandResult` literal on the frontend.
    /// `section_ids` above wears the same serde attributes minus the skip, because
    /// there the array is cheap enough to always send; the pairing rule is in #441.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reversed_sections: Option<Vec<ReversedSection>>,
    /// Backend-authoritative summary of an import command's outcome (see
    /// [`ImportReceipt`]'s doc comment for why its warnings must stay separate
    /// from the field above). Same wire contract as `reversed_sections`:
    /// `skip_serializing_if` + `default` keeps the key absent for every
    /// non-import command.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub import_receipt: Option<ImportReceipt>,
}

/// Result of an explicit "repair project integrity" action. `fixed` and
/// `remaining` are the drop in the FULL violation set (structural + merge/
/// closure alignment + orphan closures) measured before vs after the repair, so
/// they stay consistent with the integrity banner's count - unlike summing
/// per-operation counters that mix different units. `remaining` is whatever the
/// repair could not auto-fix (detect-only: a drifted aligned loop closure to
/// recompute/remove, non-finite shot geometry to correct by hand, or a GPS tie
/// point with impossible values - `invalid_zero_datum`, #576 B35(g) - which only
/// the user can correct). The partial-result toast names all three (#576
/// NF-W3-4).
///
/// `changed` is true when the repair actually mutated the project (an undo entry
/// was recorded). It is a STRICTLY broader signal than `fixed > 0`: the repair
/// always re-solves offsets, which heals sub-reporting-tolerance drift that never
/// counted as a violation - so `fixed` can be 0 while the project genuinely
/// changed. The frontend dirties the project on `changed` (not `fixed`) so that
/// silent sub-tolerance fixes are still auto-saved / prompted on close.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct IntegrityRepairResult {
    pub fixed: u32,
    pub remaining: u32,
    pub changed: bool,
    /// [#576 D22] The open project's SpeleoDB link bits after the repair; `null`
    /// for a local project. Filled by the IPC helper, `None` from the core.
    pub remote_link: Option<crate::core::outcome::RemoteLinkBits>,
}

/// Result of a structural-integrity check. `messages` is the human-readable
/// violation list (critical first; empty = sound). `save_blocked` is true when
/// the project currently has unresolvable cross-references - the exact condition
/// that refuses a save (manual AND auto-save). The frontend uses it to keep the
/// integrity banner non-dismissable while saving is blocked (so the Repair entry
/// point can't be hidden on a project that can't be saved), while still allowing
/// dismissal of detect-only issues that don't block saving.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct IntegrityCheckResult {
    pub messages: Vec<String>,
    pub save_blocked: bool,
}

/// Validation warning from command execution
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ValidationWarning {
    pub code: String,
    pub message: String,
    pub severity: WarningSeverity,
}

pub use crate::models::warning::WarningSeverity;

/// [#576 NF-W2SAVE-1] The `speleodb_done` answer: the Done outcome, plus the
/// warnings of the project save Done runs before taking the upload snapshot (what
/// that save could not keep - display settings, undo history). They are one-shot -
/// the save after the push does not repeat them - so they ride here and the
/// frontend shows them like a normal save's warnings. Empty when nothing was
/// saved or nothing was lost.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoDoneReport {
    pub result: crate::speleodb::DoneResult,
    pub save_warnings: Vec<ValidationWarning>,
}

/// [#576 NF-W4M-2] The `speleodb_publish` answer: the publish outcome, plus the
/// warnings of the two saves publish runs - the one that relocates the project
/// into the SpeleoDB cache and the pre-push save of its initial push - for what
/// they could not keep. One-shot like Done's and Push's (see
/// [`SpeleoDoneReport`]); empty when nothing was lost.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoPublishReport {
    pub result: crate::speleodb::SpeleoPublishResult,
    pub save_warnings: Vec<ValidationWarning>,
}

/// [#576 NF-W2SAVE-1] The `speleodb_push` answer: the Push outcome plus the
/// pre-push save's warnings (see [`SpeleoDoneReport`]).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SpeleoPushReport {
    pub result: crate::speleodb::PushResult,
    pub save_warnings: Vec<ValidationWarning>,
}

impl CommandResult {
    pub fn success() -> Self {
        Self {
            success: true,
            ..Self::default()
        }
    }

    /// An unclassified failure (`FAILED` at the boundary). A producer that knows
    /// what kind of failure it is uses [`Self::refused`] instead.
    pub fn failure(error: String) -> Self {
        Self {
            success: false,
            error: Some(error),
            ..Self::default()
        }
    }

    /// Attach validation warnings to this result.
    pub fn with_warnings(mut self, warnings: Vec<ValidationWarning>) -> Self {
        self.warnings = warnings;
        self
    }

    /// Attach a created section ID to this result.
    pub fn with_section_id(mut self, section_id: u32) -> Self {
        self.section_id = Some(section_id);
        self
    }

    /// Attach multiple section IDs to this result (for batch operations).
    pub fn with_section_ids(mut self, section_ids: Vec<u32>) -> Self {
        self.section_ids = section_ids;
        self
    }

    /// Attach a created shot ID to this result.
    pub fn with_shot_id(mut self, shot_id: u32) -> Self {
        self.shot_id = Some(shot_id);
        self
    }

    /// Attach the saved file path to this result.
    pub fn with_path(mut self, path: String) -> Self {
        self.saved_path = Some(path);
        self
    }

    /// Record whether this command tore the open project's session down.
    ///
    /// Takes the verdict rather than asserting it so both of `delete_project`'s
    /// return paths - the success one AND the early failure after a denied trash,
    /// which is the one the frontend most needs the bit on - can carry it from the
    /// same variable and neither can forget it. A `false` deliberately leaves the
    /// field `None` so the key stays off the wire for every command that closed
    /// nothing (see the field's doc comment); `Some(false)` is never produced.
    pub fn with_session_closed(mut self, closed: bool) -> Self {
        if closed {
            self.session_closed = Some(true);
        }
        self
    }

    /// Record that this save wrote a detached (unlinked) copy at `path` rather
    /// than saving the open project (see the field's doc comment).
    pub fn with_unlinked_copy_path(mut self, path: String) -> Self {
        self.unlinked_copy_path = Some(path);
        self
    }

    /// Attach the sections whose station numbering this operation reversed.
    pub fn with_reversed_sections(mut self, sections: Vec<ReversedSection>) -> Self {
        self.reversed_sections = Some(sections);
        self
    }

    /// Attach an import receipt to this result.
    pub fn with_import_receipt(mut self, receipt: ImportReceipt) -> Self {
        self.import_receipt = Some(receipt);
        self
    }

    /// Attach a single warning message to this result.
    pub fn with_warning(mut self, message: &str) -> Self {
        self.warnings.push(ValidationWarning {
            code: "SYSTEM".to_string(),
            message: message.to_string(),
            severity: WarningSeverity::Warning,
        });
        self
    }
}

impl Command {
    /// Whether this command can change measured survey geometry.
    ///
    /// This classification belongs to the command rather than to any one executor:
    /// geometry reconciliation, cache invalidation, and other consumers must agree on
    /// the same exhaustive answer. `UpdateShot` is instance-aware because the variant
    /// carries both measurements and metadata, and a batch inherits the union of its
    /// members.
    #[must_use]
    pub fn affects_geometry(&self) -> bool {
        match self {
            Command::CreateProject { .. }
            | Command::OpenProject { .. }
            | Command::SaveProject { .. }
            | Command::CloseProject
            | Command::UpdateSectionMetadata { .. }
            | Command::SetShotKind { .. }
            | Command::SetStationComment { .. }
            | Command::MoveComment { .. }
            | Command::RepairIntegrity
            | Command::UpdateProjectSettings
            // Import-dedup bookkeeping is a set of hashes; it moves no station.
            | Command::UntrackImport { .. }
            | Command::SetSurfaceElevation => false,
            Command::UpdateShot { updates, .. } => updates.touches_geometry(),
            Command::Batch { commands } => commands.iter().any(Command::affects_geometry),
            Command::CreateSection { .. }
            | Command::DeleteSection { .. }
            | Command::ReverseSection { .. }
            | Command::CreateShot { .. }
            | Command::DeleteShot { .. }
            | Command::MergeStations { .. }
            | Command::UnmergeStations { .. }
            | Command::UnmergeStationPair { .. }
            | Command::CreateLoopClosure { .. }
            | Command::RemoveLoopClosure { .. }
            | Command::SetZeroDatum { .. }
            | Command::RemoveZeroDatum { .. }
            | Command::MoveSection { .. }
            | Command::MoveSegment { .. }
            | Command::MoveStation { .. }
            | Command::MergeProjects { .. } => true,
        }
    }

    /// Reduce a reverse log to its NET effect: keep an id only when it occurs an
    /// ODD number of times, in first-occurrence order.
    ///
    /// Parity is associative and commutative, so reducing sub-lists and then the
    /// concatenation gives the same answer as reducing once - nested batches are
    /// safe. Within a batch a SURVIVING section's shot count `N` cannot change
    /// (`is_batch_allowed` admits no `CreateShot`/`DeleteShot`; `merge_stations`
    /// only records relations and `move_segment` only offsets; the `DeleteSection`
    /// it admits takes the whole section, and `settle_reversed_sections` drops a
    /// section that is gone), so a section reversed
    /// twice is not renumbered AT ALL: it must be neither reported nor fixed up.
    /// Reporting it would be actively wrong, because the two fix-ups
    /// `settle_reversed_sections` runs are applied once per list entry against one
    /// final project state, and neither is an involution there - a repeated id
    /// walks every saved `.plnx` key two stations along a numbering that never
    /// moved.
    fn net_reversed(ids: Vec<SectionId>) -> Vec<SectionId> {
        let mut counts: HashMap<SectionId, usize> = HashMap::new();
        for id in &ids {
            *counts.entry(*id).or_insert(0) += 1;
        }
        let mut net: Vec<SectionId> = Vec::new();
        for id in ids {
            if counts.get(&id).is_some_and(|n| n % 2 == 1) && !net.contains(&id) {
                net.push(id);
            }
        }
        net
    }

    /// The sections whose station numbering this command reverses, in command
    /// order, `Batch` included (reversing a multi-selection is one batched user
    /// action) and PARITY-REDUCED there via [`Self::net_reversed`].
    ///
    /// Read declaratively off the command rather than inferred afterwards from
    /// moved stations: a reverse keeps every station's world position, so there
    /// is nothing in the geometry to diff, and a section that was reversed AND
    /// moved in the same action defeats a position-based heuristic outright.
    /// Undo/redo of a snapshot pair reuses this on the stored command - undoing a
    /// reverse re-reverses the numbering, so the same sections apply.
    ///
    /// The reduction belongs HERE, at the producer, and not in `undo_to`/`redo_to`:
    /// those aggregate ACROSS commands, where the shot count can differ between
    /// steps, so their per-step entries must stay intact.
    pub fn reversed_section_ids(&self) -> Vec<SectionId> {
        match self {
            Command::ReverseSection { section_id, .. } => vec![*section_id],
            Command::Batch { commands } => Self::net_reversed(
                commands
                    .iter()
                    .flat_map(Command::reversed_section_ids)
                    .collect(),
            ),
            _ => Vec::new(),
        }
    }

    /// Get a human-readable description of this command
    pub fn description(&self) -> String {
        match self {
            Command::CreateProject { name, .. } => format!("Create project '{name}'"),
            Command::OpenProject { path } => {
                format!("Open project '{}'", path.display())
            }
            Command::SaveProject { path } => match path {
                Some(p) => format!("Save project to '{}'", p.display()),
                None => "Save project".to_string(),
            },
            Command::CloseProject => "Close project".to_string(),
            Command::CreateSection { name, .. } => {
                format!("Create section '{}'", name.as_deref().unwrap_or("unnamed"))
            }
            Command::DeleteSection { section_id } => {
                format!("Delete section {section_id}")
            }
            Command::UpdateSectionMetadata { section_id, .. } => {
                format!("Update section {section_id} metadata")
            }
            Command::ReverseSection { section_id, .. } => {
                format!("Reverse section {section_id}")
            }
            Command::CreateShot { section_id, .. } => {
                format!("Create shot in section {section_id}")
            }
            Command::UpdateShot {
                section_id,
                shot_id,
                ..
            } => format!(
                "Update shot {}:{}",
                section_id,
                shot_display_position(*shot_id, *section_id)
            ),
            Command::DeleteShot {
                section_id,
                shot_id,
            } => format!(
                "Delete shot {}:{}",
                section_id,
                shot_display_position(*shot_id, *section_id)
            ),
            Command::SetShotKind {
                section_id,
                shot_id,
                kind,
            } => {
                let pos = shot_display_position(*shot_id, *section_id);
                match kind {
                    ShotKind::Normal => format!("Mark shot {section_id}:{pos} as permanent"),
                    ShotKind::Jump => format!("Mark shot {section_id}:{pos} as jump"),
                    ShotKind::Reference => format!("Mark shot {section_id}:{pos} as reference"),
                }
            }
            Command::MergeStations {
                station_a,
                station_b,
                ..
            } => format!("Merge stations {station_a} and {station_b}"),
            Command::UnmergeStations { station_ref } => {
                format!("Unmerge station {station_ref}")
            }
            Command::UnmergeStationPair {
                station_a,
                station_b,
            } => {
                format!("Unmerge station pair {station_a} and {station_b}")
            }
            Command::CreateLoopClosure {
                station_a,
                station_b,
                ..
            } => format!("Create loop closure between {station_a} and {station_b}"),
            Command::RemoveLoopClosure { closure_id } => {
                format!("Remove loop closure #{closure_id}")
            }
            Command::SetZeroDatum { station_ref, .. } => {
                format!("Set zero datum at {station_ref}")
            }
            Command::RemoveZeroDatum { station_ref } => {
                format!("Remove zero datum from {station_ref}")
            }
            Command::SetStationComment {
                station_ref,
                comment,
            } => {
                if comment.is_some() {
                    format!("Set comment on station {station_ref}")
                } else {
                    format!("Remove comment from station {station_ref}")
                }
            }
            Command::MoveSection { section_id, .. } => {
                format!("Move section {section_id}")
            }
            Command::MoveSegment { section_id, .. } => {
                format!("Move segment (section {section_id})")
            }
            Command::MoveStation { station_ref, .. } => {
                format!("Move station {station_ref}")
            }
            Command::MoveComment {
                source,
                target,
                swap,
            } => {
                if *swap {
                    format!("Swap comments between {source} and {target}")
                } else {
                    format!("Move comment from {source} to {target}")
                }
            }
            Command::Batch { commands } => {
                format!("Batch of {} commands", commands.len())
            }
            Command::MergeProjects {
                source_paths,
                output_name,
                ..
            } => {
                format!(
                    "Merge {} projects into '{}'",
                    source_paths.len(),
                    output_name
                )
            }
            Command::UntrackImport { section_hashes, .. } => {
                format!("Forget {} imported sections", section_hashes.len())
            }
            Command::RepairIntegrity => "Repair integrity".to_string(),
            Command::UpdateProjectSettings => "Update project settings".to_string(),
            Command::SetSurfaceElevation => "Set entrance elevation".to_string(),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::models::{
        ClosureStrategy, Coordinates, GpsCoordinates, SectionUpdates, ShotData, ShotUpdates,
        StationRef,
    };
    use chrono::NaiveDate;

    /// Pins the `Command` variant SET, so removing one cannot pass silently.
    ///
    /// `Command` is `#[serde(tag = "type")]` with no catch-all, so a persisted
    /// undo entry naming a deleted variant stops deserializing - and
    /// `UndoStack::load_from_persisted_with_timestamps` SKIPS a bad entry rather
    /// than truncating, keeping the entries on BOTH sides. The stack then loads
    /// with a HOLE, and undoing past it replays older entries against a tree they
    /// no longer describe. `ApplyCollabChanges` is the worked example: schema v15
    /// has to call `drop_legacy_undo_history` for exactly that reason, as v11->v12
    /// and v12->v13 did before it.
    ///
    /// Its sibling `legacy_set_shot_reference_undo_entries_still_deserialize`
    /// covers the other half of this hazard - a RENAMED tag, which can be kept
    /// loadable with `#[serde(alias = ...)]`. A removal cannot.
    ///
    /// If this fires because you removed a variant: either keep its tag
    /// deserializable, or bump `SCHEMA_VERSION` and drop the persisted history in
    /// the new migration step. If you added one, just update the count.
    #[test]
    fn command_variant_set_is_pinned_so_a_removal_cannot_pass_silently() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = format!("{manifest_dir}/src/core/command.rs");
        let src =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {path}: {e}"));

        let start = src
            .find("pub enum Command {")
            .expect("pub enum Command not found");
        let end = src[start..]
            .find("\n}\n")
            .expect("end of `enum Command` not found");

        // Variants sit at exactly four-space indent. Fields and attributes are
        // deeper, and doc comments start with `/`, so an uppercase first char at
        // that indent identifies a variant.
        let variants: Vec<&str> = src[start..start + end]
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("    ")?;
                if rest.starts_with(' ') || !rest.starts_with(char::is_uppercase) {
                    return None;
                }
                rest.split(|c: char| !c.is_alphanumeric()).next()
            })
            .filter(|v| !v.is_empty())
            .collect();

        assert_eq!(
            variants.len(),
            30,
            "Command's variant set changed. A removal leaves persisted undo \
             history unloadable at that entry and holed either side of it - keep \
             the tag deserializable, or bump SCHEMA_VERSION and call \
             drop_legacy_undo_history in the new migration step. Saw: {variants:?}"
        );
    }

    #[test]
    fn geometry_classification_enumerates_every_command_variant() {
        let station_a = StationRef::new(1, 0);
        let station_b = StationRef::new(1, 1);
        let geometry_update = ShotUpdates {
            length: Some(4.0),
            ..Default::default()
        };
        let cases = vec![
            (
                Command::CreateProject {
                    name: String::new(),
                    location: None,
                },
                false,
            ),
            (
                Command::OpenProject {
                    path: PathBuf::new(),
                },
                false,
            ),
            (Command::SaveProject { path: None }, false),
            (Command::CloseProject, false),
            (
                Command::CreateSection {
                    name: None,
                    surveyor: None,
                    date: None,
                    color: String::new(),
                    direction: None,
                    shots: Vec::new(),
                },
                true,
            ),
            (Command::DeleteSection { section_id: 1 }, true),
            (
                Command::UpdateSectionMetadata {
                    section_id: 1,
                    updates: SectionUpdates::default(),
                },
                false,
            ),
            (
                Command::ReverseSection {
                    section_id: 1,
                    swap_left_right: false,
                },
                true,
            ),
            (
                Command::CreateShot {
                    section_id: 1,
                    shot: ShotData::default(),
                    after_shot_id: None,
                    before_shot_id: None,
                },
                true,
            ),
            (
                Command::UpdateShot {
                    section_id: 1,
                    shot_id: 1,
                    updates: geometry_update,
                },
                true,
            ),
            (
                Command::DeleteShot {
                    section_id: 1,
                    shot_id: 1,
                },
                true,
            ),
            (
                Command::SetShotKind {
                    section_id: 1,
                    shot_id: 1,
                    kind: ShotKind::Jump,
                },
                false,
            ),
            (
                Command::MergeStations {
                    station_a,
                    station_b,
                },
                true,
            ),
            (
                Command::UnmergeStations {
                    station_ref: station_a,
                },
                true,
            ),
            (
                Command::UnmergeStationPair {
                    station_a,
                    station_b,
                },
                true,
            ),
            (
                Command::CreateLoopClosure {
                    station_a,
                    station_b,
                    closure_strategy: ClosureStrategy::ProportionalByLength,
                },
                true,
            ),
            (Command::RemoveLoopClosure { closure_id: 1 }, true),
            (
                Command::SetZeroDatum {
                    station_ref: station_a,
                    coordinates: GpsCoordinates::new(1.0, 2.0, 3.0).unwrap(),
                },
                true,
            ),
            (
                Command::RemoveZeroDatum {
                    station_ref: station_a,
                },
                true,
            ),
            (
                Command::SetStationComment {
                    station_ref: station_a,
                    comment: None,
                },
                false,
            ),
            (
                Command::MoveSection {
                    section_id: 1,
                    offset: Coordinates::default(),
                },
                true,
            ),
            (
                Command::MoveSegment {
                    section_id: 1,
                    offset: Coordinates::default(),
                },
                true,
            ),
            (
                Command::MoveStation {
                    station_ref: station_a,
                    offset: Coordinates::default(),
                },
                true,
            ),
            (
                Command::MoveComment {
                    source: CommentHolder::Station {
                        station_ref: station_a,
                    },
                    target: CommentHolder::Station {
                        station_ref: station_b,
                    },
                    swap: false,
                },
                false,
            ),
            (
                Command::Batch {
                    commands: vec![Command::CloseProject],
                },
                false,
            ),
            (
                Command::MergeProjects {
                    source_paths: Vec::new(),
                    output_path: PathBuf::new(),
                    output_name: String::new(),
                    archive_originals: false,
                },
                true,
            ),
            (Command::RepairIntegrity, false),
            (Command::UpdateProjectSettings, false),
            (Command::SetSurfaceElevation, false),
            (
                Command::UntrackImport {
                    file_hashes: Vec::new(),
                    section_hashes: Vec::new(),
                },
                false,
            ),
        ];

        assert_eq!(
            cases.len(),
            30,
            "add every new Command variant to this table"
        );
        for (command, expected) in cases {
            assert_eq!(command.affects_geometry(), expected, "{command:?}");
        }

        let metadata_update = Command::UpdateShot {
            section_id: 1,
            shot_id: 1,
            updates: ShotUpdates {
                comment: Some(Some("metadata".to_owned())),
                ..Default::default()
            },
        };
        assert!(!metadata_update.affects_geometry());
        assert!(Command::Batch {
            commands: vec![metadata_update, Command::DeleteSection { section_id: 1 }],
        }
        .affects_geometry());
    }

    #[test]
    fn legacy_set_shot_reference_undo_entries_still_deserialize() {
        // Pre-#27 persisted undo history is JSON-tagged `set_shot_reference`
        // with an `is_reference` bool. It must still load, otherwise upgrading
        // would silently throw away the user's undo stack.
        let legacy =
            r#"{"type":"set_shot_reference","section_id":1,"shot_id":2,"is_reference":true}"#;
        let parsed: Command = serde_json::from_str(legacy).unwrap();
        // q2: a legacy virtual shot reads as a Jump, and the ids survive.
        assert!(
            matches!(
                parsed,
                Command::SetShotKind {
                    section_id: 1,
                    shot_id: 2,
                    kind: ShotKind::Jump,
                }
            ),
            "legacy set_shot_reference must map onto SetShotKind: {parsed:?}"
        );

        // The false case, which is what an old inverse command carried.
        let legacy_false =
            r#"{"type":"set_shot_reference","section_id":1,"shot_id":2,"is_reference":false}"#;
        let parsed: Command = serde_json::from_str(legacy_false).unwrap();
        assert!(matches!(
            parsed,
            Command::SetShotKind {
                kind: ShotKind::Normal,
                ..
            }
        ));

        // And the even older `set_shot_virtual` tag.
        let ancient = r#"{"type":"set_shot_virtual","section_id":1,"shot_id":2,"is_virtual":true}"#;
        let parsed: Command = serde_json::from_str(ancient).unwrap();
        assert!(matches!(
            parsed,
            Command::SetShotKind {
                kind: ShotKind::Jump,
                ..
            }
        ));
    }
    use std::path::PathBuf;

    // Re-import CommentHolder so tests can use it without qualification
    use super::CommentHolder;

    #[test]
    fn command_result_builder_sets_fields() {
        let warning = ValidationWarning {
            code: "test".to_string(),
            message: "warn".to_string(),
            severity: WarningSeverity::Warning,
        };

        let success = CommandResult::success();
        assert!(success.success);
        assert!(success.warnings.is_empty());
        assert!(success.error.is_none());
        assert!(success.section_id.is_none());
        assert!(success.section_ids.is_empty());
        assert!(success.shot_id.is_none());
        assert!(success.saved_path.is_none());
        assert!(success.unlinked_copy_path.is_none());

        let with_path = CommandResult::success().with_path("/tmp/test.stalx".to_string());
        assert!(with_path.success);
        assert_eq!(with_path.saved_path.as_deref(), Some("/tmp/test.stalx"));

        let with_copy =
            CommandResult::success().with_unlinked_copy_path("/tmp/copy.stalx".to_string());
        assert!(with_copy.success);
        assert_eq!(
            with_copy.unlinked_copy_path.as_deref(),
            Some("/tmp/copy.stalx")
        );
        assert!(
            with_copy.saved_path.is_none(),
            "a copy save must never also claim saved_path (the frontend re-points on it)"
        );

        let with_warnings = CommandResult::success().with_warnings(vec![warning.clone()]);
        assert!(with_warnings.success);
        assert_eq!(with_warnings.warnings.len(), 1);
        assert!(with_warnings.error.is_none());
        assert!(with_warnings.section_id.is_none());
        assert!(with_warnings.section_ids.is_empty());
        assert!(with_warnings.shot_id.is_none());

        let with_section = CommandResult::success().with_section_id(7);
        assert!(with_section.success);
        assert_eq!(with_section.section_id, Some(7));
        assert!(with_section.section_ids.is_empty());
        assert!(with_section.shot_id.is_none());

        let with_section_and_warnings = CommandResult::success()
            .with_section_id(8)
            .with_warnings(vec![warning.clone()]);
        assert!(with_section_and_warnings.success);
        assert_eq!(with_section_and_warnings.section_id, Some(8));
        assert_eq!(with_section_and_warnings.warnings.len(), 1);
        assert!(with_section_and_warnings.shot_id.is_none());

        let with_sections = CommandResult::success().with_section_ids(vec![1, 2, 3]);
        assert!(with_sections.success);
        assert!(with_sections.section_id.is_none());
        assert_eq!(with_sections.section_ids, vec![1, 2, 3]);
        assert!(with_sections.shot_id.is_none());

        let with_sections_and_warnings = CommandResult::success()
            .with_section_ids(vec![4, 5])
            .with_warnings(vec![warning.clone()]);
        assert!(with_sections_and_warnings.success);
        assert_eq!(with_sections_and_warnings.section_ids, vec![4, 5]);
        assert_eq!(with_sections_and_warnings.warnings.len(), 1);
        assert!(with_sections_and_warnings.shot_id.is_none());

        let with_shot = CommandResult::success().with_shot_id(42);
        assert!(with_shot.success);
        assert_eq!(with_shot.shot_id, Some(42));
        assert!(with_shot.section_id.is_none());
        assert!(with_shot.section_ids.is_empty());

        let with_shot_and_warnings = CommandResult::success()
            .with_shot_id(99)
            .with_warnings(vec![warning]);
        assert!(with_shot_and_warnings.success);
        assert_eq!(with_shot_and_warnings.shot_id, Some(99));
        assert_eq!(with_shot_and_warnings.warnings.len(), 1);

        let failure = CommandResult::failure("boom".to_string());
        assert!(!failure.success);
        assert!(failure.warnings.is_empty());
        assert_eq!(failure.error.as_deref(), Some("boom"));
        assert!(failure.section_id.is_none());
        assert!(failure.section_ids.is_empty());
        assert!(failure.shot_id.is_none());
    }

    #[test]
    fn command_description_covers_all_variants() {
        let shot_data = ShotData {
            azimuth_in: 10.0,
            azimuth_out: 20.0,
            length: 3.0,
            depth_in: 1.0,
            depth_out: 2.0,
            ..Default::default()
        };
        let section_updates = SectionUpdates {
            name: Some(Some("Updated".to_string())),
            ..Default::default()
        };
        let shot_updates = ShotUpdates {
            length: Some(4.0),
            ..Default::default()
        };
        let station_a = StationRef::new(1, 0);
        let station_b = StationRef::new(1, 1);
        let gps = GpsCoordinates::new(10.0, 20.0, 5.0).unwrap();

        let cases = vec![
            (
                Command::CreateProject {
                    name: "Alpha".to_string(),
                    location: None,
                },
                "Create project 'Alpha'",
            ),
            (
                Command::OpenProject {
                    path: PathBuf::from("alpha.stalx"),
                },
                "Open project 'alpha.stalx'",
            ),
            (Command::SaveProject { path: None }, "Save project"),
            (
                Command::SaveProject {
                    path: Some(PathBuf::from("alpha.stalx")),
                },
                "Save project to 'alpha.stalx'",
            ),
            (Command::CloseProject, "Close project"),
            (
                Command::CreateSection {
                    name: Some("Main".to_string()),
                    surveyor: None,
                    date: Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()),
                    color: "#fff".to_string(),
                    direction: Some(SurveyDirection::In),
                    shots: vec![shot_data.clone()],
                },
                "Create section 'Main'",
            ),
            (Command::DeleteSection { section_id: 3 }, "Delete section 3"),
            (
                Command::UpdateSectionMetadata {
                    section_id: 3,
                    updates: section_updates,
                },
                "Update section 3 metadata",
            ),
            (
                Command::CreateShot {
                    section_id: 4,
                    shot: shot_data,
                    after_shot_id: None,
                    before_shot_id: None,
                },
                "Create shot in section 4",
            ),
            (
                Command::UpdateShot {
                    section_id: 4,
                    shot_id: 2,
                    updates: shot_updates,
                },
                "Update shot 4:3",
            ),
            (
                Command::DeleteShot {
                    section_id: 4,
                    shot_id: 2,
                },
                "Delete shot 4:3",
            ),
            (
                Command::SetShotKind {
                    section_id: 4,
                    shot_id: 2,
                    kind: ShotKind::Reference,
                },
                "Mark shot 4:3 as reference",
            ),
            (
                Command::SetShotKind {
                    section_id: 4,
                    shot_id: 2,
                    kind: ShotKind::Jump,
                },
                "Mark shot 4:3 as jump",
            ),
            (
                Command::SetShotKind {
                    section_id: 4,
                    shot_id: 2,
                    kind: ShotKind::Normal,
                },
                "Mark shot 4:3 as permanent",
            ),
            (
                Command::MergeStations {
                    station_a,
                    station_b,
                },
                "Merge stations 1:0 and 1:1",
            ),
            (
                Command::CreateLoopClosure {
                    station_a,
                    station_b,
                    closure_strategy: ClosureStrategy::ProportionalByLength,
                },
                "Create loop closure between 1:0 and 1:1",
            ),
            (
                Command::UnmergeStations {
                    station_ref: station_a,
                },
                "Unmerge station 1:0",
            ),
            (
                Command::UnmergeStationPair {
                    station_a,
                    station_b,
                },
                "Unmerge station pair 1:0 and 1:1",
            ),
            (
                Command::RemoveLoopClosure { closure_id: 42 },
                "Remove loop closure #42",
            ),
            (
                Command::SetZeroDatum {
                    station_ref: station_b,
                    coordinates: gps,
                },
                "Set zero datum at 1:1",
            ),
            (
                Command::RemoveZeroDatum {
                    station_ref: station_b,
                },
                "Remove zero datum from 1:1",
            ),
            (
                Command::MoveSection {
                    section_id: 2,
                    offset: Coordinates::new(1.0, 2.0, 3.0),
                },
                "Move section 2",
            ),
            (
                Command::MoveSegment {
                    section_id: 2,
                    offset: Coordinates::new(1.0, 2.0, 3.0),
                },
                "Move segment (section 2)",
            ),
            (
                Command::SetStationComment {
                    station_ref: station_a,
                    comment: Some("Test comment".to_string()),
                },
                "Set comment on station 1:0",
            ),
            (
                Command::SetStationComment {
                    station_ref: station_a,
                    comment: None,
                },
                "Remove comment from station 1:0",
            ),
            (
                Command::MoveStation {
                    station_ref: station_a,
                    offset: Coordinates::new(1.0, 0.0, 0.0),
                },
                "Move station 1:0",
            ),
            (
                Command::MoveComment {
                    source: CommentHolder::Station {
                        station_ref: station_a,
                    },
                    target: CommentHolder::Shot {
                        section_id: 1,
                        shot_id: 0,
                    },
                    swap: false,
                },
                "Move comment from station 1:0 to shot 1:1",
            ),
            (
                Command::MoveComment {
                    source: CommentHolder::Station {
                        station_ref: station_a,
                    },
                    target: CommentHolder::Station {
                        station_ref: station_b,
                    },
                    swap: true,
                },
                "Swap comments between station 1:0 and station 1:1",
            ),
            (
                Command::Batch {
                    commands: vec![Command::CloseProject],
                },
                "Batch of 1 commands",
            ),
            (
                Command::MergeProjects {
                    source_paths: vec![PathBuf::from("a.stalx"), PathBuf::from("b.stalx")],
                    output_path: PathBuf::from("out.stalx"),
                    output_name: "Merged".to_string(),
                    archive_originals: false,
                },
                "Merge 2 projects into 'Merged'",
            ),
        ];

        for (command, expected) in cases {
            assert_eq!(command.description(), expected);
        }
    }

    fn blank_receipt() -> ImportReceipt {
        ImportReceipt {
            source_format: String::new(),
            file_name: String::new(),
            sections_found: 0,
            sections_imported: 0,
            shot_count: 0,
            closures_requested: 0,
            closures_created: 0,
            unit_conversion: None,
            warnings: Vec::new(),
            info: Vec::new(),
            zero_datum_created: None,
            hidden_warnings: 0,
        }
    }

    /// `zero_datum_created` (F12c) traces to file content (an
    /// `ImportSectionConfig::name`) exactly like `file_name` does, and renders
    /// verbatim in the receipt dialog - `decorate_receipt` must run it through
    /// the same `sanitize_display_text` guard: strip the bidi override so it
    /// cannot visually reorder the dialog line around it, and clamp to
    /// `MAX_FILE_NAME_LEN` so a hostile file can't inflate the IPC payload.
    #[test]
    fn decorate_receipt_sanitizes_and_clamps_zero_datum_created() {
        let poisoned = format!("A\u{202e}B{}", "x".repeat(10_000));
        let mut result = CommandResult::success().with_import_receipt(ImportReceipt {
            zero_datum_created: Some(poisoned),
            ..blank_receipt()
        });

        decorate_receipt(
            &mut result,
            "mNemo DMP",
            std::path::Path::new("/tmp/cave.dmp"),
            1,
            None,
            Vec::new(),
        );

        let receipt = result
            .import_receipt
            .expect("receipt must survive decoration");
        let name = receipt
            .zero_datum_created
            .expect("field must still be Some after sanitizing");
        assert!(
            !name.contains('\u{202e}'),
            "bidi override must be stripped: {name:?}"
        );
        assert_eq!(
            name.chars().count(),
            MAX_FILE_NAME_LEN,
            "must clamp to MAX_FILE_NAME_LEN chars, got {}",
            name.chars().count()
        );
    }

    /// A hostile/malformed import file can produce an unbounded number of
    /// warnings; `cap_warnings` must fold anything past the cap into one
    /// summary entry rather than shipping all of it back over IPC.
    #[test]
    fn cap_warnings_caps_a_huge_warning_count() {
        let mut receipt = blank_receipt();
        receipt.warnings = (0..10_000).map(|i| format!("warning {i}")).collect();
        receipt.cap_warnings();

        assert_eq!(receipt.warnings.len(), 51);
        for warning in &receipt.warnings[..50] {
            assert!(warning.chars().count() <= 300);
        }
        assert_eq!(receipt.warnings[50], "... and 9950 more warnings");
    }

    /// Each individual warning is clamped to 300 chars, truncating on a char
    /// boundary (never a raw byte slice) since a warning can embed arbitrary
    /// user-controlled text, including multi-byte characters.
    #[test]
    fn cap_warnings_clamps_an_overlong_warning_on_a_char_boundary() {
        let mut receipt = blank_receipt();
        // Multi-byte char (3 bytes each in UTF-8) so a byte-slice truncation
        // at 300 would panic; a char-boundary truncation must not.
        receipt.warnings = vec!["\u{2603}".repeat(500)];
        receipt.cap_warnings();

        assert_eq!(receipt.warnings.len(), 1);
        assert_eq!(receipt.warnings[0].chars().count(), 300);
    }

    #[test]
    fn cap_warnings_is_a_no_op_under_the_cap() {
        let mut receipt = blank_receipt();
        receipt.warnings = vec!["short warning".to_string()];
        receipt.cap_warnings();

        assert_eq!(receipt.warnings, vec!["short warning".to_string()]);
    }

    /// #576 C17: `execute_import_sections` caps its receipt, and
    /// `decorate_receipt` caps again after putting the converter's warnings
    /// first. The second cap treated the first one's "... and N more" line as
    /// an ordinary warning - dropping it, and counting only what IT cut - so
    /// 60 execute warnings plus 3 converter ones said "4 more" when 13 were
    /// hidden.
    #[test]
    fn decorate_receipt_counts_what_an_earlier_cap_hid() {
        let mut receipt = blank_receipt();
        receipt.warnings = (0..60).map(|i| format!("shot {i} looks odd")).collect();
        receipt.cap_warnings();
        let mut result = CommandResult::success().with_import_receipt(receipt);

        decorate_receipt(
            &mut result,
            "Compass",
            std::path::Path::new("/tmp/cave.dat"),
            1,
            None,
            (0..3).map(|i| format!("converter note {i}")).collect(),
        );

        let warnings = result.import_receipt.expect("receipt").warnings;
        assert_eq!(warnings.len(), 51);
        assert_eq!(warnings[0], "converter note 0");
        assert_eq!(
            warnings.last().map(String::as_str),
            Some("... and 13 more warnings"),
            "63 warnings, 50 shown"
        );
        assert_eq!(
            warnings
                .iter()
                .filter(|w| w.starts_with("... and "))
                .count(),
            1,
            "one summary line"
        );
    }
}
