//! The one pipeline every file importer runs (#576 C10).
//!
//! Ariane, Compass, Walls and GPX each used to carry their own copy of parse
//! -> convert -> apply -> receipt -> deferred save, and the copies drifted:
//! only some reported progress, only some logged the converter's warnings,
//! GPX passed none of them to its receipt. [`run_file_import`] is that
//! sequence once; an importer supplies how to parse its file and how to turn
//! the result into import configs ([`ConvertedImport`]).
//!
//! It also guards the two steps that read untrusted file content (#576 B8):
//! a panic in a parser or converter becomes a failed import with a message,
//! instead of unwinding out of the command - which leaves the frontend's
//! promise unsettled and the dialog waiting forever.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::commands::import::{decorate_receipt, finalize_deferred_import};
use crate::core::events::EventSink;
use crate::core::loading::{LoadingPhase, ProgressEmitter};
use crate::core::state::{AppState, VirtualShotMode};
use crate::core::{CommandError, CommandErrorCode, CommandResult};
use crate::models::{ImportClosure, ImportSectionConfig};

/// How an importer's messages name it.
pub(crate) struct ImportKind {
    /// The receipt's `source_format` ("Ariane TML", "Compass", ...).
    pub(crate) receipt_label: &'static str,
    /// How a failure names the file: "Failed to parse {label} file".
    pub(crate) error_label: &'static str,
    /// The `purpose` every log line of the import carries.
    pub(crate) purpose: &'static str,
}

/// What an importer's converter hands the runner to apply.
pub(crate) struct ConvertedImport {
    pub(crate) sections: Vec<ImportSectionConfig>,
    pub(crate) closures: Vec<ImportClosure>,
    /// The converter's warnings; they lead the receipt.
    pub(crate) warnings: Vec<String>,
    /// The receipt's "sections found" count.
    pub(crate) sections_found: u32,
    pub(crate) unit_conversion: Option<String>,
    /// Which import command applies the sections.
    pub(crate) apply: ApplyPath,
}

/// Which import command applies an importer's sections.
pub(crate) enum ApplyPath {
    /// `execute_import_sections`.
    Sections,
    /// `execute_tml_import_sections`, which merges Ariane's entrance
    /// elevation (#431; `None` when the file states none) with the import.
    Ariane { surface_elevation: Option<f64> },
}

/// Parse the file at `path`, convert it, apply it as one undoable import,
/// decorate the receipt and resolve a deferred first save - reporting each
/// phase to `sink` (the loading overlay).
///
/// `mode` is the virtual-shot classification: Ariane passes its dialog's
/// choice, the others `Auto` (decision C10-D1: no dialog of theirs offers an
/// override). `load_token` is the name the dialog gave this import [#576
/// W44-NF3]: its Cancel (`cancel_loading`) names it, and cancels nothing else.
#[expect(
    clippy::too_many_arguments,
    reason = "the importer's identity, its file and its two steps; a struct \
              would only rename them"
)]
pub(crate) fn run_file_import<P>(
    state: &AppState,
    sink: Arc<dyn EventSink>,
    load_token: Option<&str>,
    kind: &ImportKind,
    path: &Path,
    file_hash: Option<String>,
    mode: VirtualShotMode,
    parse: impl FnOnce(&Path) -> Result<P>,
    convert: impl FnOnce(&P) -> Result<ConvertedImport>,
) -> CommandResult {
    let purpose = kind.purpose;
    let label = kind.error_label;
    tracing::info!(path = ?path, purpose, "Executing {label} import");

    // [#576 NF-FIX3A-2] This import is the active load, so the loading overlay's
    // Cancel (`cancel_loading`) reaches it; checked between the phases, up to the
    // apply. Released on every way out.
    let active = ActiveImport::register(state, load_token);
    let mut emitter = ProgressEmitter::new_for_import(sink, Arc::clone(&active.token));

    emitter.start_phase(LoadingPhase::ImportParsing);
    #[cfg(test)]
    parse_gate::hold(path);
    let parsed = match guarded(|| parse(path)) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose,
                reason = %e,
                "Failed to parse {label} file for import"
            );
            return CommandResult::failure(format!("Failed to parse {label} file: {e}"));
        }
    };
    emitter.complete_phase(LoadingPhase::ImportParsing);
    if emitter.is_cancelled() {
        return cancelled(label);
    }

    emitter.start_phase(LoadingPhase::ImportConverting);
    let converted = match guarded(|| convert(&parsed)) {
        Ok(converted) => converted,
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose,
                reason = %e,
                "Failed to convert {label} data to import configs"
            );
            return CommandResult::failure(format!("Failed to convert {label} data: {e}"));
        }
    };
    emitter.complete_phase(LoadingPhase::ImportConverting);
    // The last point where nothing has been committed: once the apply starts the
    // import lands whole (one undoable step), and Cancel no longer stops it.
    if emitter.is_cancelled() {
        return cancelled(label);
    }

    let ConvertedImport {
        sections,
        closures,
        warnings,
        sections_found,
        unit_conversion,
        apply,
    } = converted;
    for warning in &warnings {
        tracing::warn!(purpose, "{warning}");
    }
    tracing::info!(
        path = ?path,
        section_count = sections.len(),
        closure_count = closures.len(),
        warning_count = warnings.len(),
        purpose,
        "Converted {label} to import configs, executing import"
    );

    emitter.start_phase(LoadingPhase::ImportApplyingChanges);
    let mut result = match apply {
        ApplyPath::Ariane { surface_elevation } => state.execute_tml_import_sections(
            sections,
            closures,
            file_hash,
            Vec::new(),
            Vec::new(),
            mode,
            surface_elevation,
        ),
        ApplyPath::Sections => state.execute_import_sections(
            sections,
            closures,
            file_hash,
            Vec::new(),
            Vec::new(),
            mode,
        ),
    };
    emitter.complete_phase(LoadingPhase::ImportApplyingChanges);

    decorate_receipt(
        &mut result,
        kind.receipt_label,
        path,
        sections_found,
        unit_conversion,
        warnings,
    );

    emitter.start_phase(LoadingPhase::ImportFinalizing);
    emitter.complete_phase(LoadingPhase::ImportFinalizing);

    let result = finalize_deferred_import(state, result);
    drop(active);
    result
}

/// The failure a cancelled import answers: nothing was imported. Its code
/// (`IMPORT_CANCELLED`, #576 C4) lets the import dialogs say so quietly rather
/// than as an import error.
fn cancelled(label: &str) -> CommandResult {
    tracing::info!(
        purpose = "stop an import the user cancelled (#576 NF-FIX3A-2)",
        "{label} import cancelled before anything was applied"
    );
    CommandResult::refused(CommandError::new(
        CommandErrorCode::ImportCancelled,
        format!("the {label} import was cancelled; nothing was imported"),
    ))
}

/// An import registered as the app's active load (`AppState::start_named_load`), so
/// `cancel_loading` naming it flips its token; cleared when it ends, however it ends
/// (`clear_active_load` only clears its own token, never a newer load's).
struct ActiveImport<'a> {
    state: &'a AppState,
    token: Arc<AtomicBool>,
}

impl<'a> ActiveImport<'a> {
    fn register(state: &'a AppState, load_token: Option<&str>) -> Self {
        Self {
            state,
            token: state.start_named_load(load_token),
        }
    }
}

impl Drop for ActiveImport<'_> {
    fn drop(&mut self) {
        self.state.clear_active_load(&self.token);
    }
}

/// [#576 NF-FIX3A-2] Test seam: hold an import in its read phase - after it has
/// reported that phase, before it parses - until the test releases it, so a test
/// can dispatch `cancel_loading` while the import command is still running.
/// Keyed by the file's path, so tests running at once never hold each other's
/// imports; an import of a path nobody parked runs straight through.
#[cfg(test)]
pub(crate) mod parse_gate {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Condvar, Mutex, PoisonError};
    use std::time::Duration;

    /// How long a held import waits for its release before it reads its file
    /// regardless. A command run ON the dispatching thread (the defect this seam
    /// exposes) holds that thread for this long and then finishes, instead of
    /// waiting forever for a release its own thread would have to send.
    pub(crate) const HOLD_LIMIT: Duration = Duration::from_secs(3);

    /// `(entered, released)`.
    #[derive(Default)]
    struct Gate {
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }

    static GATES: Mutex<Vec<(PathBuf, Arc<Gate>)>> = Mutex::new(Vec::new());

    /// An import of `path` held in its read phase; released on drop.
    pub(crate) struct Parked {
        path: PathBuf,
        gate: Arc<Gate>,
    }

    /// Hold the next import of `path` in its read phase.
    pub(crate) fn park(path: &Path) -> Parked {
        let gate = Arc::new(Gate::default());
        GATES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((path.to_path_buf(), Arc::clone(&gate)));
        Parked {
            path: path.to_path_buf(),
            gate,
        }
    }

    impl Parked {
        /// Wait up to `limit` for the import to reach its read phase; whether it did.
        pub(crate) fn wait_entered(&self, limit: Duration) -> bool {
            let state = self
                .gate
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let (state, _) = self
                .gate
                .changed
                .wait_timeout_while(state, limit, |(entered, _)| !*entered)
                .unwrap_or_else(PoisonError::into_inner);
            state.0
        }

        /// Let the held import read its file.
        pub(crate) fn release(&self) {
            let mut state = self
                .gate
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.1 = true;
            self.gate.changed.notify_all();
        }
    }

    impl Drop for Parked {
        fn drop(&mut self) {
            self.release();
            GATES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|(path, gate)| !(path == &self.path && Arc::ptr_eq(gate, &self.gate)));
        }
    }

    /// The runner's side: report the read phase entered, then wait for the release
    /// (at most [`HOLD_LIMIT`]) if a test parked this path.
    pub(super) fn hold(path: &Path) {
        let gate = GATES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(parked, _)| parked == path)
            .map(|(_, gate)| Arc::clone(gate));
        let Some(gate) = gate else {
            return;
        };
        let mut state = gate.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 = true;
        gate.changed.notify_all();
        let _released = gate
            .changed
            .wait_timeout_while(state, HOLD_LIMIT, |(_, released)| !*released)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// A sink nobody listens to, for tests that run an import without an app.
#[cfg(test)]
pub(crate) struct SilentSink;

#[cfg(test)]
impl EventSink for SilentSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

/// `f`'s result, or an error naming the panic it raised (#576 B8). The
/// parsers and converters read untrusted files, so a panic in one must fail
/// that import rather than unwind out of the command. Only steps that touch
/// nothing but the file's own data run here; the apply step holds the
/// project's locks and is left to its own guards.
///
/// The panic is caught, so the panic hook records it as a caught panic, not a
/// crash: no crash marker (#576 RV3-R-12, `panic_hook::catching`).
pub(crate) fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    crate::logging::panic_hook::catching(|| catch_unwind(AssertUnwindSafe(f))).unwrap_or_else(
        |payload| {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no detail".to_string());
            Err(anyhow!(
                "the importer stopped on an internal error ({detail}); nothing was imported"
            ))
        },
    )
}
