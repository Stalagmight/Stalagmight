//! Loading progress module for project open/create operations.
//!
//! This module provides types for tracking loading progress, error handling,
//! and display settings during project load operations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use ts_rs::TS;

use crate::core::events::{self, EventSink, IpcEvent, LoadingProgressEvent};
use crate::models::CommandErrorCode;
use crate::persistence::RefusedSchemaObject;

// ============================================================================
// LoadError
// ============================================================================

/// A file the load refused because it carries a `speleo_link` row and is not the
/// canonical cache copy of that project (see `core::state::speleodb_foreign`).
/// Everything the frontend's managed-file prompt needs to offer its two ways
/// forward: open the managed copy from SpeleoDB, or strip this file's link.
/// `Deserialize` as well as `Serialize`: this goes OUT with the refusal and
/// comes back IN with the strip command, which re-checks it against the file
/// before deleting a link row (the prompt can outlive the file it is about).
#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SpeleoManagedFile {
    /// The refused file, as the load was asked to open it.
    pub path: String,
    pub project_id: String,
    pub project_name: String,
    pub endpoint: String,
    /// False when `project_id` failed UUID validation, so the app must not build
    /// a cache path from it and cannot look the project up on the server: only
    /// the remove-link action is offered.
    pub can_open_remote: bool,
}

/// Error type for load/create operations.
///
/// [#576 C4] No longer an IPC type: the load commands convert it into a
/// [`crate::core::CommandError`] (`From<LoadError>`), which carries the same code
/// and payloads. Its `Display` is the plain `message`: the JSON rendering it used
/// to have existed for a string-typed IPC error the frontend parsed, and inside
/// another error's message (a SpeleoDB reload failure) it put raw JSON in front of
/// the user.
#[derive(Debug, Clone)]
pub struct LoadError {
    pub code: CommandErrorCode,
    pub message: String,
    pub details: Option<String>,
    /// Set only by [`Self::speleodb_managed`].
    pub speleodb_managed: Option<SpeleoManagedFile>,
}

impl LoadError {
    /// Create a new LoadError with the given code and message.
    pub fn new(code: CommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
            speleodb_managed: None,
        }
    }

    /// Create a new LoadError with details.
    pub fn with_details(
        code: CommandErrorCode,
        message: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            details: Some(details.into()),
            speleodb_managed: None,
        }
    }

    /// Error when a project is already open.
    pub fn already_open() -> Self {
        Self::new(CommandErrorCode::AlreadyOpen, "Project already open")
    }

    /// Error when the operation was cancelled.
    pub fn cancelled() -> Self {
        Self::new(CommandErrorCode::Cancelled, "Operation cancelled")
    }

    /// [#576 FZ-FZ-INT-R2] A save renamed a new file over the path while the open
    /// was reading it. Retryable: the same open again reads the file now there.
    pub fn file_replaced_during_load() -> Self {
        Self::new(
            CommandErrorCode::FileReplacedDuringLoad,
            "The project file was saved while it was being opened; open it again.",
        )
    }

    /// Error when a file is not found.
    pub fn not_found(path: &str) -> Self {
        Self::new(
            CommandErrorCode::NotFound,
            format!("File not found: {path}"),
        )
    }

    /// Internal error with a message.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(CommandErrorCode::InternalError, message)
    }

    /// Error when the database file is corrupted.
    /// The `path` is stored in `details` so the frontend can offer recovery options.
    pub fn corrupt(message: impl Into<String>, path: &str) -> Self {
        Self::with_details(CommandErrorCode::Corrupt, message, path)
    }

    /// The open was refused because the file's database schema holds something
    /// Stalagmight never creates (a view, trigger or virtual table, or an object
    /// named like a SQLite pragma), so it was changed by another program and
    /// could run its own commands if opened (#608). `path` rides in `details`.
    pub fn unsafe_file(refused: &RefusedSchemaObject, path: &str) -> Self {
        Self::with_details(
            CommandErrorCode::UnsafeFile,
            format!(
                "This file was not opened: it contains {} named \"{}\". Stalagmight \
                 never creates these, so the file was changed by another program, and \
                 opening it could let it run its own database commands. Your other \
                 projects are not affected.",
                refused.what,
                refused.display_name()
            ),
            path,
        )
    }

    /// Error when the OS denied filesystem access to the file - e.g. macOS
    /// forgot a previously-granted folder permission after an OS/app update.
    /// The `path` rides in `details` so the frontend can name the file.
    pub fn permission(message: impl Into<String>, path: &str) -> Self {
        Self::with_details(CommandErrorCode::FsPermissionDenied, message, path)
    }

    /// The open was refused: this file is a non-canonical copy of a SpeleoDB
    /// project. Carries the link so the frontend can raise the managed-file
    /// prompt instead of a dead-end error.
    pub fn speleodb_managed(managed: SpeleoManagedFile) -> Self {
        let mut err = Self::new(
            CommandErrorCode::SpeleodbManaged,
            format!(
                "\"{}\" is managed by SpeleoDB (project \"{}\")",
                managed.path, managed.project_name
            ),
        );
        err.speleodb_managed = Some(managed);
        err
    }
}

/// How much of an underlying error may be quoted into a user-facing message.
///
/// SQLite error text embeds the offending SQL and identifiers, which on a file
/// of unknown provenance is attacker-chosen and unbounded - it must not be
/// pasted whole into a dialog, a log line or a telemetry field.
pub const MAX_QUOTED_ERROR_CHARS: usize = 300;

/// [`MAX_QUOTED_ERROR_CHARS`] worth of `detail`, on a char boundary, with an
/// ellipsis when something was cut.
pub fn clamp_quoted_error(detail: &str) -> String {
    let mut clamped: String = detail.chars().take(MAX_QUOTED_ERROR_CHARS).collect();
    if detail.chars().nth(MAX_QUOTED_ERROR_CHARS).is_some() {
        clamped.push_str("...");
    }
    clamped
}

/// Windows' `MAX_PATH`: the character budget a path gets INCLUDING its
/// terminating NUL, so 259 usable characters.
const LEGACY_PATH_LIMIT: usize = 260;

/// True when `path` is at or over the legacy Windows `MAX_PATH` budget.
///
/// A plain character count with no platform gate of its own - callers gate it on
/// `cfg!(windows)`, which keeps the arithmetic testable on every platform
/// instead of only on the one where it matters.
pub(crate) fn over_legacy_path_limit(path: &Path) -> bool {
    path.to_string_lossy().encode_utf16().count() >= LEGACY_PATH_LIMIT
}

/// True if `err` (or any error in its chain) is a CORROBORATED filesystem
/// permission denial: a typed `PermissionDenied` io error, or a message that
/// says so in as many words. macOS forgets previously-granted folder access
/// after an OS or app update, which surfaces as EPERM (os error 1) / EACCES
/// (os error 13) or a `PermissionDenied` io error.
///
/// SQLite's "unable to open database file" is deliberately NOT on that list.
/// It is `SQLITE_CANTOPEN` with no errno behind it, and Windows raises it just
/// as readily for a missing folder, a disconnected drive or an over-`MAX_PATH`
/// path - which is how a 335-character save destination came to be answered
/// with macOS folder-permission instructions (#429). A caller that sees it must
/// corroborate it against the filesystem itself: [`read_access_denied`] on the
/// read side, `classify_save_failure` (`core::state::project_io`) on the write
/// side.
///
/// Deliberately conservative on the string side: it must NOT classify a
/// genuine corruption ("integrity check failed") as a permission problem,
/// since the whole point is to stop showing the wrong remediation.
pub(crate) fn is_permission_denied(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        // A typed io error decodes the platform errno for us: EACCES/EPERM on
        // unix and ERROR_ACCESS_DENIED on Windows all map to PermissionDenied,
        // so we never hardcode platform-specific error numbers.
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::PermissionDenied {
                return true;
            }
        }
        // String fallback for causes that don't reach us as a typed io error.
        let msg = cause.to_string().to_ascii_lowercase();
        if msg.contains("permission denied") || msg.contains("operation not permitted") {
            return true;
        }
        // Unix errno text that can show up in a stringified (non-typed) cause.
        // Gated to unix: "(os error 1)"/"(os error 13)" are EPERM/EACCES there
        // but unrelated codes on Windows.
        #[cfg(unix)]
        if msg.contains("(os error 1)") || msg.contains("(os error 13)") {
            return true;
        }
    }
    false
}

/// True if `err` (or any error in its chain) says the path itself is not there:
/// a typed `NotFound` io error, or a platform message naming a missing
/// file/path or an unreachable device (a removed stick, a dropped share).
pub(crate) fn is_path_not_found(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::NotFound {
                return true;
            }
        }
        let msg = cause.to_string().to_ascii_lowercase();
        if msg.contains("no such file or directory")
            || msg.contains("cannot find the file")
            || msg.contains("cannot find the path")
            || msg.contains("the network path was not found")
            || msg.contains("the device is not ready")
        {
            return true;
        }
    }
    false
}

/// The schema refusal inside an open error, if that is why the open failed.
pub(crate) fn refused_schema(err: &anyhow::Error) -> Option<&RefusedSchemaObject> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<RefusedSchemaObject>())
}

/// True if any cause carries SQLite's errno-free `SQLITE_CANTOPEN` message -
/// the ambiguous one that means "the OS would not give me this file" and
/// nothing more. See [`is_permission_denied`] for why it needs corroborating.
pub(crate) fn mentions_unopenable_database(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .to_string()
            .to_ascii_lowercase()
            .contains("unable to open database file")
    })
}

/// Corroborate the ambiguous message on a READ: the file is there (the caller
/// has already established that) but the OS refuses to hand it over. That is
/// the macOS lost-folder-access case, where SQLite reports `SQLITE_CANTOPEN`
/// with nothing else to go on.
pub(crate) fn read_access_denied(path: &Path) -> bool {
    matches!(
        std::fs::File::open(path),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied
    )
}

/// The whole `anyhow` chain on one line, outermost cause first. What a failing
/// save writes to the log, so a bug report can be read without a repro (#429).
pub(crate) fn error_chain(err: &anyhow::Error) -> String {
    err.chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" <- ")
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LoadError {}

// ============================================================================
// DisplaySettings
// ============================================================================

/// Display settings for API payloads (used in load_project_full response).
///
/// This struct contains the 13 display settings that are persisted per-project.
/// It is used ONLY for the consolidated load API response, not for storage.
///
/// NOTE: show_shots and show_shot_comments were historically excluded here on
/// the theory that showShots was "session-based" - but the frontend has always
/// persisted BOTH via save_display_setting, so excluding them meant the saved
/// values were written and never read back (#275 B2). They are first-class now.
#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DisplaySettings {
    pub show_stations: bool,
    pub show_shots: bool,
    pub show_section_names: bool,
    pub show_station_labels: bool,
    pub show_shot_data: bool,
    pub show_reference_shots: bool,
    /// #27: jump shots hide independently of reference shots. Defaults to the
    /// stored `showReferenceShots` value so an existing project keeps showing
    /// exactly what it showed before the split.
    pub show_jumps: bool,
    pub show_zero_datum: bool,
    pub show_grid: bool,
    pub show_satellite: bool,
    pub show_depth_gradient: bool,
    pub show_station_comments: bool,
    pub show_shot_comments: bool,
    pub grid_spacing: f64,
    /// Geometry display mode ("measured" | "closureAdjusted"). Carried here so a
    /// project reopened through the typed load path restores the saved mode
    /// ("remember last used") instead of always defaulting to measured.
    pub geometry_mode: String,
}

impl Default for DisplaySettings {
    /// Default values matching uiStore/DISPLAY_DEFAULTS exactly.
    fn default() -> Self {
        Self {
            show_stations: true,
            show_shots: true,
            show_section_names: true,
            show_station_labels: false,
            show_shot_data: false,
            show_reference_shots: true,
            show_jumps: true,
            show_zero_datum: true,
            show_grid: false,
            show_satellite: true,
            show_depth_gradient: false,
            show_station_comments: false,
            show_shot_comments: false,
            grid_spacing: 10.0,
            geometry_mode: "measured".to_string(),
        }
    }
}

impl DisplaySettings {
    /// Convert from stored HashMap<String, String> format.
    /// Keys MUST match what save_display_setting writes (camelCase keys).
    /// Default values match uiStore/DISPLAY_DEFAULTS.
    pub fn from_stored(map: &HashMap<String, String>) -> Self {
        Self {
            show_stations: map.get("showStations").is_none_or(|s| s == "true"),
            show_shots: map.get("showShots").is_none_or(|s| s == "true"),
            show_section_names: map.get("showSectionNames").is_none_or(|s| s == "true"),
            show_station_labels: map.get("showStationLabels").is_some_and(|s| s == "true"),
            show_shot_data: map.get("showShotData").is_some_and(|s| s == "true"),
            show_reference_shots: map
                .get("showReferenceShots")
                .or_else(|| map.get("showVirtualShots"))
                .is_none_or(|s| s == "true"),
            // Falls back through the pre-#27 keys so a project saved before the
            // split reopens with jumps in the same state its single toggle left
            // them, rather than silently flipping them on.
            show_jumps: map
                .get("showJumps")
                .or_else(|| map.get("showReferenceShots"))
                .or_else(|| map.get("showVirtualShots"))
                .is_none_or(|s| s == "true"),
            show_zero_datum: map.get("showZeroDatum").is_none_or(|s| s == "true"),
            show_grid: map.get("showGrid").is_some_and(|s| s == "true"),
            show_satellite: map.get("showSatellite").is_none_or(|s| s == "true"),
            show_depth_gradient: map.get("showDepthGradient").is_some_and(|s| s == "true"),
            show_station_comments: map.get("showStationComments").is_some_and(|s| s == "true"),
            show_shot_comments: map.get("showShotComments").is_some_and(|s| s == "true"),
            grid_spacing: map
                .get("gridSpacing")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10.0),
            geometry_mode: map
                .get("geometryMode")
                .filter(|s| s.as_str() == "measured" || s.as_str() == "closureAdjusted")
                .cloned()
                .unwrap_or_else(|| "measured".to_string()),
        }
    }
}

// ============================================================================
// LoadingPhase
// ============================================================================

/// Loading phases for project open/create operations.
///
/// The `weight()` method on this enum is provided for convenience/reference.
/// The authoritative weights used for progress calculation are in ProgressEmitter's
/// phase_weights vec, which allows different weight distributions per flow type.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum LoadingPhase {
    // Project opening phases (8 total)
    OpeningDatabase,
    LoadingProjectData,
    LoadingSections,
    ComputingStations,
    ComputingStatistics,
    ValidatingData,
    LoadingSettings,
    Finalizing,
    // Project creation phases (3 total)
    CreatingDatabase,
    InitializingProject,
    SavingSettings,
    // Export phases (4 total)
    ExportPreparing,
    ExportGenerating,
    ExportWriting,
    ExportComplete,
    // Import phases (4 total) - used by TML and Compass survey-file imports
    ImportParsing,
    ImportConverting,
    ImportApplyingChanges,
    ImportFinalizing,
}

impl LoadingPhase {
    /// Human-readable label for the phase.
    pub fn label(&self) -> &'static str {
        match self {
            Self::OpeningDatabase => "Opening database...",
            Self::LoadingProjectData => "Loading survey data...",
            Self::LoadingSections => "Loading sections...",
            Self::ComputingStations => "Calculating station positions...",
            Self::ComputingStatistics => "Computing statistics...",
            Self::ValidatingData => "Validating data...",
            Self::LoadingSettings => "Loading display settings...",
            Self::Finalizing => "Finalizing...",
            Self::CreatingDatabase => "Creating database...",
            Self::InitializingProject => "Initializing project...",
            Self::SavingSettings => "Saving settings...",
            Self::ExportPreparing => "Preparing export...",
            Self::ExportGenerating => "Generating...",
            Self::ExportWriting => "Writing file...",
            Self::ExportComplete => "Export complete",
            Self::ImportParsing => "Reading survey file...",
            Self::ImportConverting => "Converting to project model...",
            Self::ImportApplyingChanges => "Applying sections and merges...",
            Self::ImportFinalizing => "Finalizing import...",
        }
    }

    /// Reference weight for this phase (out of 100 within its flow).
    /// NOTE: ProgressEmitter uses its own phase_weights vec for actual calculations,
    /// which allows different weight distributions per flow type (open vs create vs export).
    pub fn weight(&self) -> f32 {
        match self {
            // Open phases sum to 100
            Self::OpeningDatabase => 5.0,
            Self::LoadingProjectData => 20.0,
            Self::LoadingSections => 15.0,
            Self::ComputingStations => 20.0,
            Self::ComputingStatistics => 15.0,
            Self::ValidatingData => 10.0,
            Self::LoadingSettings => 5.0,
            Self::Finalizing => 10.0,
            // Create phases sum to 100
            Self::CreatingDatabase => 40.0,
            Self::InitializingProject => 40.0,
            Self::SavingSettings => 20.0,
            // Export phases sum to 100
            Self::ExportPreparing => 10.0,
            Self::ExportGenerating => 60.0,
            Self::ExportWriting => 20.0,
            Self::ExportComplete => 10.0,
            // Import phases sum to 100. Applying dominates because section
            // creation + station merges scale with N.
            Self::ImportParsing => 15.0,
            Self::ImportConverting => 20.0,
            Self::ImportApplyingChanges => 55.0,
            Self::ImportFinalizing => 10.0,
        }
    }
}

// ============================================================================
// LoadingProgress
// ============================================================================

/// Progress event payload sent to frontend.
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LoadingProgress {
    #[ts(type = "number")]
    pub loading_id: u64,
    pub phase: LoadingPhase,
    pub phase_index: u8,
    pub phase_count: u8,
    pub current: Option<u32>,
    pub total: Option<u32>,
    pub overall_percent: f32,
    pub is_indeterminate: bool,
}

// ============================================================================
// CancellationToken and CancelledError
// ============================================================================

/// Cancellation token using atomic bool.
#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Create a new cancellation token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancel the operation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check if the operation was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Reset the cancellation state.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }
}

/// Error type for cancelled operations.
#[derive(Debug)]
pub struct CancelledError;

impl std::fmt::Display for CancelledError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Operation cancelled")
    }
}

impl std::error::Error for CancelledError {}

// ============================================================================
// ProgressEmitter
// ============================================================================

/// Global loading ID counter.
/// Starts at 1 because 0 is reserved as "unknown/awaiting handshake" on the frontend.
static LOADING_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Throttling constants.
const THROTTLE_ITEMS: u32 = 10;
const THROTTLE_MS: u64 = 50;
const HEARTBEAT_MS: u64 = 2000;

/// Emits loading progress events to the frontend.
pub struct ProgressEmitter {
    sink: Arc<dyn EventSink>,
    loading_id: u64,
    cancelled: Arc<AtomicBool>,
    phase_weights: Vec<(LoadingPhase, f32)>,
    current_phase_idx: usize,
    last_emit: Instant,
    last_item: u32,
}

impl ProgressEmitter {
    /// Create a new emitter for project open operations (8 phases summing to 100%).
    pub fn new_for_open(sink: Arc<dyn EventSink>, cancelled: Arc<AtomicBool>) -> Self {
        let weights = vec![
            (LoadingPhase::OpeningDatabase, 5.0),
            (LoadingPhase::LoadingProjectData, 20.0),
            (LoadingPhase::LoadingSections, 15.0),
            (LoadingPhase::ComputingStations, 20.0),
            (LoadingPhase::ComputingStatistics, 15.0),
            (LoadingPhase::ValidatingData, 10.0),
            (LoadingPhase::LoadingSettings, 5.0),
            (LoadingPhase::Finalizing, 10.0),
        ];
        debug_assert!(
            (weights.iter().map(|(_, w)| w).sum::<f32>() - 100.0).abs() < 0.01,
            "Open phase weights must sum to 100"
        );
        Self::new(sink, weights, cancelled)
    }

    /// Create a new emitter for project create operations (3 phases summing to 100%).
    pub fn new_for_create(sink: Arc<dyn EventSink>, cancelled: Arc<AtomicBool>) -> Self {
        let weights = vec![
            (LoadingPhase::CreatingDatabase, 40.0),
            (LoadingPhase::InitializingProject, 40.0),
            (LoadingPhase::SavingSettings, 20.0),
        ];
        debug_assert!(
            (weights.iter().map(|(_, w)| w).sum::<f32>() - 100.0).abs() < 0.01,
            "Create phase weights must sum to 100"
        );
        Self::new(sink, weights, cancelled)
    }

    /// Create a new emitter for survey-file import operations (Ariane, Compass,
    /// Walls, GPX) with 4 phases summing to 100%. `cancelled` is the import's
    /// active-load token (#576 NF-FIX3A-2): the runner checks it between phases.
    pub fn new_for_import(sink: Arc<dyn EventSink>, cancelled: Arc<AtomicBool>) -> Self {
        let weights = vec![
            (LoadingPhase::ImportParsing, 15.0),
            (LoadingPhase::ImportConverting, 20.0),
            (LoadingPhase::ImportApplyingChanges, 55.0),
            (LoadingPhase::ImportFinalizing, 10.0),
        ];
        debug_assert!(
            (weights.iter().map(|(_, w)| w).sum::<f32>() - 100.0).abs() < 0.01,
            "Import phase weights must sum to 100"
        );
        Self::new(sink, weights, cancelled)
    }

    /// Create a new emitter for export operations (4 phases summing to 100%).
    ///
    /// Export operations don't support cancellation, so this uses a dummy token.
    pub fn new_for_export(sink: Arc<dyn EventSink>) -> Self {
        let weights = vec![
            (LoadingPhase::ExportPreparing, 10.0),
            (LoadingPhase::ExportGenerating, 60.0),
            (LoadingPhase::ExportWriting, 20.0),
            (LoadingPhase::ExportComplete, 10.0),
        ];
        debug_assert!(
            (weights.iter().map(|(_, w)| w).sum::<f32>() - 100.0).abs() < 0.01,
            "Export phase weights must sum to 100"
        );
        // Export doesn't support cancellation - use a dummy token that's never cancelled
        let dummy_cancelled = Arc::new(AtomicBool::new(false));
        Self::new(sink, weights, dummy_cancelled)
    }

    fn new(
        sink: Arc<dyn EventSink>,
        phase_weights: Vec<(LoadingPhase, f32)>,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sink,
            loading_id: LOADING_ID_COUNTER.fetch_add(1, Ordering::SeqCst),
            cancelled,
            phase_weights,
            current_phase_idx: 0,
            last_emit: Instant::now()
                .checked_sub(Duration::from_secs(10))
                .unwrap_or(Instant::now()), // Ensure first emit goes through
            last_item: 0,
        }
    }

    /// Get the loading ID for this emitter.
    pub fn loading_id(&self) -> u64 {
        self.loading_id
    }

    /// Check if the operation was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Start a new phase - emits progress immediately.
    ///
    /// If the phase is not found in the emitter's phase list, logs a warning
    /// and keeps the current phase index to avoid misreporting progress.
    pub fn start_phase(&mut self, phase: LoadingPhase) {
        if let Some(idx) = self.phase_weights.iter().position(|(p, _)| *p == phase) {
            self.current_phase_idx = idx;
        } else {
            // Phase not in this emitter's flow - log warning but don't change index
            tracing::warn!(
                phase = ?phase,
                purpose = "report loading progress to UI",
                "start_phase called with phase not in emitter's phase_weights"
            );
        }
        self.last_item = 0;
        // Emit with indeterminate=true derived from having no current/total
        self.emit_progress(phase, None, None);
    }

    /// Update progress within a phase with throttling.
    /// Use 0-indexed current (0..total-1) or 1-indexed (1..total) - both work.
    ///
    /// Throttle: always emit at the start (current=0/1) and end (current=total)
    /// of a phase, otherwise emit only when BOTH (a) at least `THROTTLE_ITEMS`
    /// items have advanced since the last emit AND (b) at least `THROTTLE_MS`
    /// have elapsed. The previous implementation OR'd the conditions, which
    /// for fast inner loops (e.g. computing 5k+ stations in ~1 s) emitted
    /// ~500 events because the items rule kept tripping every couple of ms.
    /// Each event triggers a full Zustand + React re-render on the JS side,
    /// so a fast Rust loop translated into a ~5 s freeze processing the
    /// event backlog. AND-gating caps emission at ~20/s in practice.
    pub fn update_phase(&mut self, phase: LoadingPhase, current: u32, total: u32) {
        let at_boundary = current == 0 || current == 1 || current == total;
        let enough_items = current >= self.last_item + THROTTLE_ITEMS;
        let enough_time = self.last_emit.elapsed() >= Duration::from_millis(THROTTLE_MS);
        let should_emit = at_boundary || (enough_items && enough_time);

        if should_emit {
            self.last_item = current;
            self.emit_progress(phase, Some(current), Some(total));
        }
    }

    /// Complete a phase - emits progress at 100% for this phase.
    pub fn complete_phase(&mut self, phase: LoadingPhase) {
        // Emit 100% completion for this phase (current=1, total=1 means 100%)
        self.emit_progress(phase, Some(1), Some(1));
    }

    /// Emit heartbeat for long-running indeterminate phases.
    pub fn heartbeat(&mut self, phase: LoadingPhase) {
        if self.last_emit.elapsed() >= Duration::from_millis(HEARTBEAT_MS) {
            self.emit_progress(phase, None, None);
        }
    }

    /// Emit a progress event. Derives is_indeterminate from current/total.
    fn emit_progress(&mut self, phase: LoadingPhase, current: Option<u32>, total: Option<u32>) {
        self.last_emit = Instant::now();

        let overall_percent = self.calculate_overall_percent(current, total);
        // Indeterminate when we don't have valid progress values
        let is_indeterminate = current.is_none() || total.is_none() || total == Some(0);

        let progress = LoadingProgress {
            loading_id: self.loading_id,
            phase,
            phase_index: self.current_phase_idx as u8,
            phase_count: self.phase_weights.len() as u8,
            current,
            total,
            overall_percent,
            is_indeterminate,
        };

        match events::payload::<LoadingProgressEvent>(&progress) {
            Ok(payload) => self.sink.emit(LoadingProgressEvent::NAME, payload),
            Err(e) => tracing::warn!(
                error = %e,
                purpose = "report loading progress to UI",
                "Failed to serialize loading progress event"
            ),
        }
    }

    fn calculate_overall_percent(&self, current: Option<u32>, total: Option<u32>) -> f32 {
        let completed: f32 = self
            .phase_weights
            .iter()
            .take(self.current_phase_idx)
            .map(|(_, w)| w)
            .sum();

        let current_weight = self
            .phase_weights
            .get(self.current_phase_idx)
            .map_or(0.0, |(_, w)| *w);

        let phase_progress = match (current, total) {
            (Some(c), Some(t)) if t > 0 => (c as f32 / t as f32) * current_weight,
            _ => 0.0,
        };

        (completed + phase_progress).min(100.0)
    }
}

// ============================================================================
// LoadProjectFullResult
// ============================================================================

/// Result of a successful load_project_full operation.
/// Contains all data needed to initialize the frontend state.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LoadProjectFullResult {
    /// Unique ID for this load operation (for filtering stale events)
    #[ts(type = "number")]
    pub loading_id: u64,
    /// The loaded project. Serialized in positional wire form (like
    /// `ProjectState`) so cross-references reach the frontend as
    /// `StationRef { section_id, station_index }`, not the internal `StationKey`
    /// shape derived serde would emit.
    #[serde(serialize_with = "crate::models::project::serialize_project_ipc")]
    pub project: crate::models::Project,
    /// Computed station coordinates
    pub stations: Vec<crate::models::Station>,
    /// Project statistics
    pub stats: crate::models::ProjectStats,
    /// Project warnings (validation results)
    pub warnings: Vec<crate::models::ProjectWarning>,
    /// Display settings from the project database
    pub display_settings: DisplaySettings,
    /// Is this a SpeleoDB project (it carries a `speleo_link` row)? The SAME
    /// value this load used to set the backend edit gate (`set_edit_enabled
    /// (!is_remote)`), handed to the frontend so its read-only gate resolves
    /// synchronously with the load instead of waiting on a `speleodb_link_status`
    /// round trip that arrives after the project has already rendered (#270).
    pub is_remote: bool,
    /// [#576 B6-D1] The file's schema version when a NEWER Stalagmight saved it -
    /// the SAME value this load committed as `AppState::newer_file_schema`, which
    /// opens the project read-only - else `None`. Handed to the frontend so its
    /// read-only gate and banner settle with the load, exactly as `is_remote`.
    ///
    /// Carried on the binary envelope (`load_binary::encode`), which is what the
    /// frontend loads through, AND on the JSON `load_project_full` (#576 P-NEWER
    /// open question 1): that command is registered too, and without the field
    /// it handed a client such a project with nothing saying it is read-only.
    /// Either way the backend refuses every edit and save of such a project; the
    /// flag only decides what the UI presents.
    pub newer_schema_version: Option<i32>,
    /// [#576 B35(b) review] The file carries a `speleo_link` table whose row could
    /// not be read. The project opened as a local one (`is_remote` false), but
    /// every save that would rewrite this file is refused, because the rebuilt file
    /// would lose the row; the frontend says so as soon as the project opens rather
    /// than at the first failed save. Binary envelope only, so the generated
    /// binding does not carry it.
    #[serde(skip)]
    #[ts(skip)]
    pub speleo_link_unreadable: bool,
    /// [#576 B15] The stored per-project settings this load could not read
    /// (`validation`, `surface_elevation`, `default_azimuth_reference`) - the SAME
    /// list this load committed as `AppState::unreadable_project_settings`, which
    /// refuses every save. The project opened with defaults in their place; the
    /// frontend names them so the user knows why saving is off. Binary envelope
    /// only, so the generated binding does not carry it.
    #[serde(skip)]
    #[ts(skip)]
    pub unreadable_project_settings: Vec<String>,
    /// [#576 B35(h)] What this load reset or dropped because it could not use it
    /// (non-finite shot values, section and station offsets, GPS tie points), per
    /// kind. The next save makes the reset permanent, so the frontend shows one
    /// notice after the open. Binary envelope only.
    #[serde(skip)]
    #[ts(skip)]
    pub load_repairs: crate::persistence::LoadRepairs,
}

// ============================================================================
// OpenFileFlags
// ============================================================================

/// [#576 NF-W2F-1] The open-time flags that describe the open project's FILE, as
/// they stand now. The same values a load hands the frontend
/// ([`LoadProjectFullResult::newer_schema_version`] /
/// [`LoadProjectFullResult::speleo_link_unreadable`] /
/// [`LoadProjectFullResult::unreadable_project_settings`]), re-asked after a SpeleoDB
/// in-place tree swap (revision View/Restore, fast-forward, take remote): the
/// backend reloads the swapped file, but the frontend does not go through
/// `loadProject` there, so without this its read-only banner and gate kept
/// describing the previous bytes until the next open.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct OpenFileFlags {
    /// The file's schema version when a newer Stalagmight saved it, else `None`.
    pub newer_schema_version: Option<i32>,
    /// The file carries a `speleo_link` table whose row cannot be read.
    pub speleo_link_unreadable: bool,
    /// [#576 RV4-R-S5] The stored project settings the open could not read
    /// ([`LoadProjectFullResult::unreadable_project_settings`]): each refuses
    /// every save until set again. Empty when they were all read.
    pub unreadable_project_settings: Vec<String>,
}

// ============================================================================
// CreateProjectFullResult
// ============================================================================

/// Result of a successful create_project_full operation.
/// Contains all data needed to initialize the frontend state for a new project.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CreateProjectFullResult {
    /// Unique ID for this load operation (for filtering stale events)
    #[ts(type = "number")]
    pub loading_id: u64,
    /// The created project. Serialized in positional wire form (see
    /// `LoadProjectFullResult`); harmless for a fresh project (no cross-refs yet),
    /// but keeps every Project-bearing IPC payload consistent.
    #[serde(serialize_with = "crate::models::project::serialize_project_ipc")]
    pub project: crate::models::Project,
    /// Display settings (defaults for new project)
    pub display_settings: DisplaySettings,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;

    /// [#576 C4] `Display` is the message as written: quotes and control
    /// characters pass through, and neither the code nor a JSON wrapper is added
    /// (a SpeleoDB reload failure quotes it inside its own message).
    #[test]
    fn test_load_error_display_is_the_plain_message() {
        for message in [
            "Test message",
            r#"Message with "quotes""#,
            "Line1\nLine2\tTabbed",
        ] {
            let error = LoadError::new(CommandErrorCode::Database, message);
            assert_eq!(error.to_string(), message);
            assert!(!error.to_string().contains("DATABASE"));
        }
        let corrupt = LoadError::corrupt("broken", "/tmp/a.stalx");
        assert_eq!(corrupt.to_string(), "broken", "details are not in the text");
    }

    #[test]
    fn test_load_error_constructors() {
        assert_eq!(
            LoadError::already_open().code,
            CommandErrorCode::AlreadyOpen
        );
        assert_eq!(LoadError::cancelled().code, CommandErrorCode::Cancelled);
        assert_eq!(
            LoadError::not_found("/path/to/file").code,
            CommandErrorCode::NotFound
        );
        assert_eq!(
            LoadError::internal("error").code,
            CommandErrorCode::InternalError
        );
    }

    /// The managed-file prompt's payload reaches the IPC error the open rejects
    /// with (`From<LoadError> for CommandError`), and an error without it omits
    /// the key entirely, so the frontend's `speleodb_managed` check is reliable.
    #[test]
    fn speleodb_managed_carries_the_link_to_the_ipc_error() {
        let err = LoadError::speleodb_managed(SpeleoManagedFile {
            path: "/tmp/foreign.stalx".to_string(),
            project_id: "11111111-2222-3333-4444-555555555555".to_string(),
            project_name: "Cave".to_string(),
            endpoint: "https://sdb.example".to_string(),
            can_open_remote: true,
        });
        let parsed = serde_json::to_value(crate::core::CommandError::from(err))
            .expect("the IPC error serializes");
        assert_eq!(parsed["code"], "SPELEODB_MANAGED");
        let managed = &parsed["speleodb_managed"];
        assert_eq!(managed["path"], "/tmp/foreign.stalx");
        assert_eq!(managed["projectId"], "11111111-2222-3333-4444-555555555555");
        assert_eq!(managed["projectName"], "Cave");
        assert_eq!(managed["endpoint"], "https://sdb.example");
        assert_eq!(managed["canOpenRemote"], true);
        let plain = serde_json::to_value(crate::core::CommandError::from(LoadError::internal("x")))
            .expect("the IPC error serializes");
        assert!(plain.get("speleodb_managed").is_none());
        let corrupt = serde_json::to_value(crate::core::CommandError::from(LoadError::corrupt(
            "broken",
            "/tmp/a.stalx",
        )))
        .expect("the IPC error serializes");
        assert_eq!(corrupt["code"], "CORRUPT");
        assert_eq!(corrupt["details"], "/tmp/a.stalx");
    }

    #[test]
    fn test_display_settings_default_matches_ui_store() {
        let defaults = DisplaySettings::default();

        // Verify defaults match DISPLAY_DEFAULTS in uiStore
        assert!(defaults.show_stations); // true
        assert!(defaults.show_shots); // true
        assert!(!defaults.show_shot_comments); // false
        assert!(defaults.show_section_names); // true
        assert!(!defaults.show_station_labels); // false
        assert!(!defaults.show_shot_data); // false
        assert!(defaults.show_reference_shots); // true
        assert!(defaults.show_zero_datum); // true
        assert!(!defaults.show_grid); // false
        assert!(defaults.show_satellite); // true
        assert!(!defaults.show_depth_gradient); // false
        assert!((defaults.grid_spacing - 10.0).abs() < 0.001); // 10.0
    }

    #[test]
    fn test_display_settings_from_stored_defaults() {
        let empty: HashMap<String, String> = HashMap::new();
        let settings = DisplaySettings::from_stored(&empty);

        // Verify defaults match DISPLAY_DEFAULTS in uiStore
        assert!(settings.show_stations); // true
        assert!(settings.show_shots); // true
        assert!(!settings.show_shot_comments); // false
        assert!(settings.show_section_names); // true
        assert!(!settings.show_station_labels); // false
        assert!(!settings.show_shot_data); // false
        assert!(settings.show_reference_shots); // true
        assert!(settings.show_zero_datum); // true
        assert!(!settings.show_grid); // false
        assert!(settings.show_satellite); // true - absent key reads as on
        assert!(!settings.show_depth_gradient); // false
        assert!((settings.grid_spacing - 10.0).abs() < 0.001); // 10.0
    }

    #[test]
    fn test_display_settings_from_stored_with_values() {
        let mut map = HashMap::new();
        map.insert("showStations".into(), "false".into());
        map.insert("showGrid".into(), "true".into());
        map.insert("gridSpacing".into(), "5.0".into());

        let settings = DisplaySettings::from_stored(&map);

        assert!(!settings.show_stations);
        assert!(settings.show_grid);
        assert!((settings.grid_spacing - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_display_settings_from_stored_all_fields() {
        // Verify ALL fields are mapped correctly from stored HashMap
        let mut stored = HashMap::new();
        stored.insert("showStations".to_string(), "false".to_string());
        stored.insert("showShots".to_string(), "false".to_string());
        stored.insert("showShotComments".to_string(), "true".to_string());
        stored.insert("showSectionNames".to_string(), "false".to_string());
        stored.insert("showStationLabels".to_string(), "true".to_string());
        stored.insert("showShotData".to_string(), "true".to_string());
        stored.insert("showReferenceShots".to_string(), "false".to_string());
        stored.insert("showZeroDatum".to_string(), "false".to_string());
        stored.insert("showGrid".to_string(), "true".to_string());
        stored.insert("showSatellite".to_string(), "true".to_string());
        stored.insert("showDepthGradient".to_string(), "true".to_string());
        stored.insert("gridSpacing".to_string(), "25.5".to_string());

        let settings = DisplaySettings::from_stored(&stored);

        // Verify all fields - each should be the opposite of the default
        assert!(!settings.show_stations, "showStations should be false");
        assert!(!settings.show_shots, "showShots should be false");
        assert!(
            settings.show_shot_comments,
            "showShotComments should be true"
        );
        assert!(
            !settings.show_section_names,
            "showSectionNames should be false"
        );
        assert!(
            settings.show_station_labels,
            "showStationLabels should be true"
        );
        assert!(settings.show_shot_data, "showShotData should be true");
        assert!(
            !settings.show_reference_shots,
            "showReferenceShots should be false"
        );
        assert!(!settings.show_zero_datum, "showZeroDatum should be false");
        assert!(settings.show_grid, "showGrid should be true");
        assert!(settings.show_satellite, "showSatellite should be true");
        assert!(
            settings.show_depth_gradient,
            "showDepthGradient should be true"
        );
        assert!(
            (settings.grid_spacing - 25.5).abs() < 0.001,
            "gridSpacing should be 25.5"
        );
    }

    #[test]
    fn test_show_shots_and_shot_comments_round_trip_stored_false_true() {
        // The #275 B2 regression case: the frontend has always persisted
        // showShots/showShotComments via save_display_setting, but this struct
        // used to omit them, so a saved "false"/"true" was written and never
        // read back. Pin the exact stored values a real project would carry.
        let mut stored = HashMap::new();
        stored.insert("showShots".to_string(), "false".to_string());
        stored.insert("showShotComments".to_string(), "true".to_string());
        let settings = DisplaySettings::from_stored(&stored);
        assert!(!settings.show_shots, "saved showShots=false must load");
        assert!(
            settings.show_shot_comments,
            "saved showShotComments=true must load"
        );
        // And absence still yields the uiStore defaults (true / false).
        let empty = DisplaySettings::from_stored(&HashMap::new());
        assert!(empty.show_shots);
        assert!(!empty.show_shot_comments);
    }

    #[test]
    fn test_display_settings_geometry_mode() {
        // Default (absent) and invalid values fall back to "measured".
        assert_eq!(DisplaySettings::default().geometry_mode, "measured");
        assert_eq!(
            DisplaySettings::from_stored(&HashMap::new()).geometry_mode,
            "measured"
        );
        let mut bad = HashMap::new();
        bad.insert("geometryMode".to_string(), "bogus".to_string());
        assert_eq!(DisplaySettings::from_stored(&bad).geometry_mode, "measured");

        // A saved closureAdjusted preference round-trips (remember last used).
        let mut saved = HashMap::new();
        saved.insert("geometryMode".to_string(), "closureAdjusted".to_string());
        assert_eq!(
            DisplaySettings::from_stored(&saved).geometry_mode,
            "closureAdjusted"
        );
    }

    #[test]
    fn test_loading_phase_labels() {
        assert_eq!(LoadingPhase::OpeningDatabase.label(), "Opening database...");
        assert_eq!(
            LoadingPhase::ComputingStations.label(),
            "Calculating station positions..."
        );
        assert_eq!(
            LoadingPhase::CreatingDatabase.label(),
            "Creating database..."
        );
    }

    #[test]
    fn test_loading_phase_weights_sum_to_100_for_open() {
        let open_phases = [
            LoadingPhase::OpeningDatabase,
            LoadingPhase::LoadingProjectData,
            LoadingPhase::LoadingSections,
            LoadingPhase::ComputingStations,
            LoadingPhase::ComputingStatistics,
            LoadingPhase::ValidatingData,
            LoadingPhase::LoadingSettings,
            LoadingPhase::Finalizing,
        ];
        let sum: f32 = open_phases.iter().map(LoadingPhase::weight).sum();
        assert!(
            (sum - 100.0).abs() < 0.01,
            "Open phases should sum to 100, got {sum}"
        );
    }

    #[test]
    fn test_loading_phase_weights_sum_to_100_for_create() {
        let create_phases = [
            LoadingPhase::CreatingDatabase,
            LoadingPhase::InitializingProject,
            LoadingPhase::SavingSettings,
        ];
        let sum: f32 = create_phases.iter().map(LoadingPhase::weight).sum();
        assert!(
            (sum - 100.0).abs() < 0.01,
            "Create phases should sum to 100, got {sum}"
        );
    }

    #[test]
    fn test_cancellation_token() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());

        token.reset();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn test_cancellation_token_clone() {
        let token1 = CancellationToken::new();
        let token2 = token1.clone();

        token1.cancel();
        assert!(
            token2.is_cancelled(),
            "Cloned token should see cancellation"
        );
    }

    #[test]
    fn load_and_create_results_serialize_project_cross_refs_in_wire_form() {
        use crate::models::{
            Project, ProjectStats, SectionData, ShotData, StationKey, StationMerge,
        };

        // A project with one resolvable station merge, stored in the internal
        // StationKey form.
        let mut project = Project::new("Wire".to_string(), None);
        let a = project
            .add_section(SectionData {
                shots: vec![ShotData {
                    length: 1.0,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .id;
        let b = project
            .add_section(SectionData {
                shots: vec![ShotData {
                    length: 1.0,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .id;
        project.ensure_shot_uuids();
        project.record_station_merge(StationMerge {
            station_a: StationKey::positional(a, 0),
            station_b: StationKey::positional(b, 0),
            merged_at: chrono::Utc::now(),
        });

        let stats = ProjectStats {
            section_count: 2,
            shot_count: 2,
            total_length: 2.0,
            min_depth: 0.0,
            max_depth: 0.0,
            station_count: 4,
            loop_count: 0,
            penetration_distance: 0.0,
        };

        // The merge endpoint must serialize as positional wire form
        // ({section_id, station_index}), NOT the internal StationKey shape
        // ({section_id, anchor}). This is what makes the JSON load/create routes
        // match what the frontend reads (and the binary route emits).
        let assert_wire = |v: &serde_json::Value, label: &str| {
            let sa = &v["project"]["station_merges"][0]["station_a"];
            assert!(
                sa.get("station_index").is_some(),
                "{label}: cross-ref must be positional wire form, got {sa}"
            );
            assert!(
                sa.get("anchor").is_none(),
                "{label}: cross-ref must NOT be the internal StationKey shape, got {sa}"
            );
        };

        let load = LoadProjectFullResult {
            loading_id: 1,
            project: project.clone(),
            stations: Vec::new(),
            stats,
            warnings: Vec::new(),
            display_settings: DisplaySettings::default(),
            is_remote: false,
            newer_schema_version: None,
            speleo_link_unreadable: false,
            unreadable_project_settings: Vec::new(),
            load_repairs: crate::persistence::LoadRepairs::default(),
        };
        assert_wire(
            &serde_json::to_value(&load).expect("serialize load result"),
            "LoadProjectFullResult",
        );

        let create = CreateProjectFullResult {
            loading_id: 1,
            project,
            display_settings: DisplaySettings::default(),
        };
        assert_wire(
            &serde_json::to_value(&create).expect("serialize create result"),
            "CreateProjectFullResult",
        );
    }
}

#[cfg(test)]
mod unsafe_file_tests {
    #![expect(clippy::unwrap_used, reason = "test assertions")]

    use super::{refused_schema, LoadError};
    use crate::models::{CommandErrorCode, Project};
    use crate::persistence::Database;

    /// A real refused open of a file carrying a view whose name hides a
    /// right-to-left override: the load error is UNSAFE_FILE, names what was
    /// found, and shows the override as `?` rather than echoing it.
    #[test]
    fn a_hostile_schema_becomes_an_unsafe_file_error_with_a_safe_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("crafted.stalx");
        Database::open(&path)
            .unwrap()
            .save_project(&Project::new("Crafted".to_string(), None))
            .unwrap();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE VIEW \"evil\u{202E}txt\" AS SELECT 1;")
            .unwrap();

        let Err(err) = Database::open_existing(&path) else {
            unreachable!("a file with a view must be refused");
        };
        let refused = refused_schema(&err).unwrap();
        let load = LoadError::unsafe_file(refused, &path.display().to_string());

        assert_eq!(load.code, CommandErrorCode::UnsafeFile);
        assert!(
            load.message.contains("a view named \"evil?txt\""),
            "{}",
            load.message
        );
        assert!(!load.message.contains('\u{202E}'));
        assert_eq!(
            load.details.as_deref(),
            Some(path.display().to_string().as_str())
        );
    }

    /// The production load maps a schema refusal to UNSAFE_FILE before the
    /// permission and corruption classifiers can claim it. Guarded by order in
    /// the source: the load needs a Wry AppHandle no unit test has (the idiom of
    /// `the_production_load_refuses_a_managed_file_before_it_opens_the_database`).
    #[test]
    fn the_production_load_reports_a_refused_schema_as_unsafe_file() {
        let src = include_str!("state/project_load.rs");
        let start = src.find("fn do_load_project_full").unwrap();
        let body = &src[start..];
        // The open's error arm: the refusal check, then (in the same arm) the
        // permission and corruption classifiers it must run ahead of.
        let open = body.find("Database::open_existing(&path)").unwrap();
        let arm = &body[open..];
        let refused = arm.find("loading::refused_schema(&e)").unwrap();
        let unsafe_file = arm.find("LoadError::unsafe_file(").unwrap();
        let permission = arm.find("loading::is_permission_denied(&e)").unwrap();
        let corrupt = arm.find("LoadError::corrupt(").unwrap();
        assert!(refused < unsafe_file && unsafe_file < permission && permission < corrupt);
    }
}
