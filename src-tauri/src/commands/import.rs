use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Runtime};

use crate::commands::event_sink::tauri_sink;
use crate::commands::import_runner::{
    guarded, run_file_import, ApplyPath, ConvertedImport, ImportKind,
};
use crate::commands::pool;
use crate::core::state::{AppState, VirtualShotMode};
use crate::core::{CommandError, CommandErrorCode, CommandResult, ImportOutcome};
use crate::models::placement_suggest::{
    suggest_all_placements, suggest_placements, CrossSectionSuggestion, ExistingSection,
    ImportSectionForSuggestion, OrphanPlacementSuggestion, PlacementSuggestion,
};
use crate::models::{AzimuthReference, ShotData};
use crate::parsers::compass::{CompassFile, CompassImportOptions};
use crate::parsers::gpx::{GpxFile, GpxSectionPlacement};
use crate::parsers::tml::{TmlFile, TmlImportOptions, TmlRootPlacement};
use crate::parsers::walls::{WallsFile, WallsImportOptions};
use crate::parsers::DmpFile;

/// Import a DMP file and return parsed data
#[tauri::command]
pub async fn import_dmp(path: PathBuf) -> Result<DmpFile, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(
            path = ?path,
            purpose = "prepare import preview",
            "Parsing DMP file for import preview"
        );
        let result = guarded(|| crate::parsers::parse_dmp(&path));
        if let Err(ref e) = result {
            tracing::error!(
                path = ?path,
                purpose = "prepare import preview",
                reason = %e,
                "Failed to parse DMP file for import preview"
            );
        }
        result.map_err(|e| CommandError::failed(e.to_string()))
    })
    .await
}

/// Calculate SHA256 hash of a file.
///
/// Streamed through the importers' capped, regular-files-only reader (#576
/// RV3-R-11): a whole-file `fs::read` here blocked on a FIFO swapped in after
/// the parse, and read a file grown past its family's ceiling in full.
#[tauri::command]
pub async fn calculate_file_hash(path: PathBuf) -> Result<String, CommandError> {
    pool::run_stateless(move || {
        tracing::debug!(
            path = ?path,
            purpose = "detect duplicate imports",
            "Calculating file hash for import deduplication"
        );
        let mut hasher = Sha256::new();
        crate::parsers::stream_import_file(&path, |chunk| hasher.update(chunk)).map_err(|e| {
            tracing::error!(
                path = ?path,
                purpose = "detect duplicate imports",
                reason = %e,
                "Failed to read file while calculating import hash"
            );
            format!("Failed to read file: {e:#}")
        })?;
        let result = hasher.finalize();

        let hash = format!("{result:x}");
        tracing::debug!(
            path = ?path,
            purpose = "detect duplicate imports",
            hash = %hash,
            "Calculated file hash for import deduplication"
        );
        Ok(hash)
    })
    .await
}

/// Check if a file hash has already been imported to the project
#[tauri::command]
pub async fn check_duplicate_import<R: Runtime>(
    app: AppHandle<R>,
    hash: String,
) -> Result<bool, CommandError> {
    pool::run(app, move |state| {
        tracing::debug!(
            hash = %hash,
            purpose = "detect duplicate imports",
            "Checking whether file hash was previously imported"
        );
        state
            .settled_read(|| state.check_duplicate_import(&hash))
            .map_err(|e| CommandError::failed(e.to_string()))
    })
    .await
}

/// Batch-check which section content hashes have already been imported.
/// Returns a parallel Vec<bool> (true = already imported).
#[tauri::command]
pub async fn check_section_hashes<R: Runtime>(
    app: AppHandle<R>,
    hashes: Vec<String>,
) -> Result<Vec<bool>, CommandError> {
    pool::run(app, move |state| {
        tracing::debug!(
            count = hashes.len(),
            purpose = "detect duplicate section imports",
            "Checking section content hashes"
        );
        state
            .settled_read(|| state.check_section_hashes(&hashes))
            .map_err(|e| CommandError::failed(e.to_string()))
    })
    .await
}

pub(crate) use crate::core::command::decorate_receipt;
pub(crate) use crate::parsers::tml::tml_unit_conversion_note;

/// The import receipt's unit-conversion note for a Compass file, derived from
/// `CompassFile::length_unit` (`build_preview`'s "feet"/"meters"/"mixed" label -
/// see its doc comment for how a multi-survey file collapses to one label).
fn compass_unit_conversion_note(length_unit: Option<&str>) -> Option<String> {
    match length_unit {
        Some("feet") => Some("Lengths converted from feet to meters".to_string()),
        Some("mixed") => Some("Some surveys measured in feet; converted to meters".to_string()),
        _ => None,
    }
}

/// The import receipt's unit-conversion note for a Walls project. Walls has no
/// single project-wide unit label the way Compass does (`#Units` can flip
/// per-survey or mid-file), so this is derived from `WallsFile::any_feet_units`
/// - true when ANY shot anywhere in the project was measured in feet.
fn walls_unit_conversion_note(any_feet_units: bool) -> Option<String> {
    any_feet_units.then(|| "Some surveys measured in feet; converted to meters".to_string())
}

/// #92: durably persist a newly-imported project. A project whose initial save is
/// still deferred (created with no save path, never written) has no `.stalx` and no
/// Recent Projects entry, so importing into it would otherwise be invisible and
/// lost on close - regardless of which import type or UI is used. Every import
/// command funnels its result through here, so resolving the deferred save once an
/// import lands guarantees the project becomes real on disk + in recents on its
/// first import. Already-persisted projects are left untouched, so their unrelated
/// pending edits are never force-committed.
///
/// A failed deferred save is surfaced as a partial success - the import keeps its
/// `success` (so the frontend still refreshes and marks the project dirty, keeping
/// the in-memory import under the close-guard) but carries a warning about the
/// failed save. Returning a hard failure here would make the UI treat the import as
/// failed, never mark dirty, and silently lose the imported data on exit.
pub(crate) fn finalize_deferred_import(
    state: &AppState,
    import_result: CommandResult,
) -> CommandResult {
    if !import_result.success || !state.initial_save_deferred() {
        return import_result;
    }
    let saved = crate::commands::project::save_project_impl(state, None);
    merge_deferred_save_outcome(import_result, saved)
}

/// Fold the deferred save's outcome into the import's result.
///
/// Both outcomes have something to carry, which is the whole point:
///
/// * FAILED - the partial-success warning described above.
/// * SUCCEEDED WITH WARNINGS - a save can write the file and still report a
///   degraded state (`DATABASE_NOT_REOPENED`, a failed collaboration sibling
///   write, dropped undo history). Those warnings are the only account of it
///   anywhere, and attaching them only on failure (#154) meant an import that
///   left the session half-broken reported as perfectly clean.
///
/// The import's own warnings are EXTENDED, never replaced: per-shot validation
/// warnings from `execute_import_sections` are already in there.
///
/// A successful save's warnings additionally go onto the RECEIPT, leading. The
/// `CommandResult.warnings` copy alone reaches nobody: the frontend keeps only
/// `code == "SYSTEM"` from that list (`saveFeedback.ts`), by design, so a save
/// that reports `DATABASE_NOT_REOPENED` or `UNDO_HISTORY_DROPPED` under its own
/// code would be surfaced nowhere at all. Leading, because the same precedent
/// governs the deferred-save FAILURE message the frontend puts at the top of
/// the receipt (#110/#420): "the save went wrong" outranks any format caveat.
fn merge_deferred_save_outcome(
    mut import_result: CommandResult,
    saved: CommandResult,
) -> CommandResult {
    if saved.success {
        if !saved.warnings.is_empty() {
            if let Some(receipt) = import_result.import_receipt.as_mut() {
                let messages = saved.warnings.iter().map(|w| w.message.clone());
                let existing = std::mem::take(&mut receipt.warnings);
                receipt.warnings.extend(messages);
                receipt.warnings.extend(existing);
                receipt.cap_warnings();
            }
            import_result.warnings.extend(saved.warnings);
        }
        return import_result;
    }
    tracing::warn!(
        error = ?saved.error,
        purpose = "persist new project after first import",
        "Deferred-save resolution failed after import"
    );
    let raw = saved.error.unwrap_or_else(|| "unknown error".to_string());
    // [#576 C4] Routed on the save's code: a classified filesystem failure gets
    // the plain-words remediation, anything else its own message.
    let detail = match saved.code {
        Some(CommandErrorCode::FsPermissionDenied) => {
            "permission denied (check folder access in Privacy & Security)".to_string()
        }
        Some(CommandErrorCode::FsPathNotFound) => {
            "the folder no longer exists or the drive is not connected".to_string()
        }
        Some(CommandErrorCode::FsPathTooLong) => {
            "the path is too long for the system to open".to_string()
        }
        _ => raw,
    };
    import_result.with_warning(&format!(
        "Imported, but could not save the new project to disk: {detail}. \
         Your changes are unsaved - please save manually."
    ))
}

/// Import multiple sections as a single atomic undo/redo unit.
/// Includes file hash tracking so undo also reverts the duplicate-detection state.
///
/// `source_label` / `source_file_name` / `sections_found` are additive
/// provenance for the import receipt - today only the mNemo DMP wizard passes
/// them (a later shard). All three are optional and `serde`-defaulted, so
/// existing callers (and the Playwright IPC mock) are unaffected, and the
/// base receipt `execute_import_sections` already attached is left at its
/// defaults when none are provided.
///
/// `source_warnings` are the parse warnings the caller was handed with the
/// file (`DmpFile::warnings`, #576 B13): what of the file could not be read.
/// They LEAD the receipt, the slot `decorate_receipt` gives every other
/// importer's converter warnings, for the same reason (a whole-file statement
/// outranks the per-shot ones `cap_warnings` would otherwise keep first).
/// The wizard sends them with the first import of a run only, because a run's
/// receipts are concatenated (`aggregateImportReceipts`).
#[tauri::command]
pub async fn import_sections<R: Runtime>(
    app: AppHandle<R>,
    sections: Vec<crate::models::ImportSectionConfig>,
    file_hash: Option<String>,
    section_hashes: Option<Vec<String>>,
    existing_shot_updates: Option<Vec<crate::models::ExistingShotUpdate>>,
    virtual_shot_mode: Option<VirtualShotMode>,
    source_label: Option<String>,
    source_file_name: Option<String>,
    sections_found: Option<u32>,
    source_warnings: Option<Vec<String>>,
) -> Result<ImportOutcome, CommandError> {
    pool::run_mutation(app, move |state| {
        let updates = existing_shot_updates.unwrap_or_default();
        tracing::info!(
            section_count = sections.len(),
            update_count = updates.len(),
            purpose = "batch import with overlap updates",
            "import_sections called",
        );
        let mut result = state.execute_import_sections(
            sections,
            Vec::new(),
            file_hash,
            section_hashes.unwrap_or_default(),
            updates,
            virtual_shot_mode.unwrap_or_default(),
        );

        if source_label.is_some() || source_file_name.is_some() || sections_found.is_some() {
            if let Some(receipt) = result.import_receipt.as_mut() {
                receipt.source_format = source_label.unwrap_or_else(|| "mNemo DMP".to_string());
                receipt.file_name = source_file_name.unwrap_or_default();
                receipt.sections_found = sections_found.unwrap_or(0);
            }
        }
        lead_receipt_with_source_warnings(&mut result, source_warnings.unwrap_or_default());

        finalize_deferred_import(state, result).into_outcome::<ImportOutcome>()
    })
    .await
}

/// Put the file's own parse warnings first on the receipt (see
/// [`import_sections`]). They name sections, which are file content, so they
/// are sanitized like every other file-derived receipt string, then capped.
/// No receipt (a refused import) has nothing to lead.
fn lead_receipt_with_source_warnings(result: &mut CommandResult, source_warnings: Vec<String>) {
    let Some(receipt) = result.import_receipt.as_mut() else {
        return;
    };
    let leading: Vec<String> = source_warnings
        .iter()
        .map(|w| crate::models::sanitize_display_text(w, usize::MAX))
        .filter(|w| !w.is_empty())
        .collect();
    if leading.is_empty() {
        return;
    }
    let execute_warnings = std::mem::replace(&mut receipt.warnings, leading);
    receipt.warnings.extend(execute_warnings);
    receipt.cap_warnings();
}

/// Parse a TML/TMLU file and return preview data for the import dialog.
#[tauri::command]
pub async fn parse_tml(path: PathBuf) -> Result<TmlFile, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(
            path = ?path,
            purpose = "prepare TML import preview",
            "Parsing Ariane TML/TMLU file for import preview"
        );
        let result = guarded(|| crate::parsers::tml::parse_tml(&path));
        if let Err(ref e) = result {
            tracing::error!(
                path = ?path,
                purpose = "prepare TML import preview",
                reason = %e,
                "Failed to parse TML/TMLU file for import preview"
            );
        }
        result.map_err(|e| CommandError::failed(e.to_string()))
    })
    .await
}

/// Parse a TML/TMLU file, convert to import configs, and execute the import.
///
/// [#576 NF-FIX3A-2 / C2] Runs on the blocking pool (`pool::run`), so the main
/// thread stays free while the file is read and converted: the progress events
/// reach the dialog as they are emitted, and the dialog's Cancel
/// (`cancel_loading`) reaches the import between its phases.
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per import-dialog control; grouping them into a \
              struct would put a second, hand-written shape on the IPC wire"
)]
pub async fn execute_tml_import<R: Runtime>(
    app: AppHandle<R>,
    path: PathBuf,
    file_hash: Option<String>,
    include_excluded: bool,
    comments_on_stations: bool,
    virtual_shot_mode: Option<VirtualShotMode>,
    selected_sections: Option<Vec<u32>>,
    root_placement: Option<TmlRootPlacement>,
) -> Result<ImportOutcome, CommandError> {
    let sink = tauri_sink(app.clone());
    pool::run_mutation(app, move |state| {
        let opts = TmlImportOptions {
            include_excluded,
            comments_on_stations,
            // `None` => import every section; `Some(list)` => only the chosen
            // section indices (into the preview's `TmlFile::sections`, which the
            // dialog lists 1:1). An empty list imports nothing and lands the empty
            // receipt, exactly like a file whose every shot was filtered out.
            selected_sections: selected_sections.map(|v| v.into_iter().collect()),
            // `None` => the pre-#425 silent default (the file's GPS, else origin).
            root_placement,
        };
        run_file_import(
            state,
            sink,
            &TML_IMPORT,
            &path,
            file_hash,
            virtual_shot_mode.unwrap_or_default(),
            crate::parsers::tml::parse_tml,
            |tml| convert_tml(tml, &opts),
        )
        .into_outcome::<ImportOutcome>()
    })
    .await
}

/// The Ariane importer as the runner names it.
const TML_IMPORT: ImportKind = ImportKind {
    receipt_label: "Ariane TML",
    error_label: "TML",
    purpose: "import Ariane TML/TMLU",
};

/// Ariane's convert step for [`run_file_import`].
fn convert_tml(tml: &TmlFile, opts: &TmlImportOptions) -> anyhow::Result<ConvertedImport> {
    let (sections, closures, warnings) = crate::parsers::tml::convert_tml_with_options(tml, opts)?;
    // Entrance elevation (#431): `firstStartAbsoluteElevation`, already
    // unit-converted. 0.0 is the file's no-data value (mirrors the export
    // side, which writes 0.0 when the project has none), so it does not
    // overwrite an unset project. Two consequences follow from treating 0.0
    // as no-data: a genuine sea-level entrance can never be recorded (it
    // only round-trips because export writes 0.0 for `None` too), and a file
    // stating 0.0 against a project already holding a non-zero value produces
    // no mismatch warning, because the value is dropped here before the
    // no-clobber comparison ever sees it.
    let surface_elevation =
        crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation);
    Ok(ConvertedImport {
        sections,
        closures,
        warnings,
        sections_found: tml.sections.len() as u32,
        unit_conversion: tml_unit_conversion_note(tml.unit),
        apply: ApplyPath::Ariane { surface_elevation },
    })
}

/// Parse a Compass `.DAT`/`.MAK` file and return preview data for the import dialog.
#[tauri::command]
pub async fn parse_compass(path: PathBuf) -> Result<CompassFile, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(
            path = ?path,
            purpose = "prepare Compass import preview",
            "Parsing Compass .DAT/.MAK file for import preview"
        );
        // The preview is built from the file's data too, so it runs under the
        // same guard as the parse (#576 RV3-R-12).
        guarded(|| {
            crate::parsers::compass::parse_compass(&path)
                .map(|parsed| crate::parsers::compass::build_preview(&parsed))
        })
        .map_err(|e| {
            tracing::error!(
                path = ?path,
                purpose = "prepare Compass import preview",
                reason = %e,
                "Failed to parse Compass file for import preview"
            );
            CommandError::failed(e.to_string())
        })
    })
    .await
}

/// Parse a Compass file, convert to import configs, and execute the import.
/// On the blocking pool, like [`execute_tml_import`].
#[tauri::command]
pub async fn execute_compass_import<R: Runtime>(
    app: AppHandle<R>,
    path: PathBuf,
    file_hash: Option<String>,
    include_excluded: bool,
    apply_corrections: bool,
    comments_on_stations: bool,
) -> Result<ImportOutcome, CommandError> {
    let sink = tauri_sink(app.clone());
    pool::run_mutation(app, move |state| {
        let opts = CompassImportOptions {
            include_excluded,
            apply_corrections,
            comments_on_stations,
        };
        execute_compass_import_impl(state, sink, &path, file_hash, &opts)
            .into_outcome::<ImportOutcome>()
    })
    .await
}

/// The body of [`execute_compass_import`], on the pool: parse, convert and apply
/// the file at `path` into the open project. Its own function so the destructive
/// path is tested on a real file without a Tauri app (#576 E10).
pub(crate) fn execute_compass_import_impl(
    state: &AppState,
    sink: std::sync::Arc<dyn crate::core::events::EventSink>,
    path: &std::path::Path,
    file_hash: Option<String>,
    opts: &CompassImportOptions,
) -> CommandResult {
    run_file_import(
        state,
        sink,
        &COMPASS_IMPORT,
        path,
        file_hash,
        // Auto: no dialog on this path carries a per-file override, so the
        // chain rule decides (decision C10-D1).
        VirtualShotMode::Auto,
        crate::parsers::compass::parse_compass,
        |parsed| convert_compass(parsed, opts),
    )
}

/// The Compass importer as the runner names it.
const COMPASS_IMPORT: ImportKind = ImportKind {
    receipt_label: "Compass",
    error_label: "Compass",
    purpose: "import Compass",
};

/// Compass's convert step for [`run_file_import`].
fn convert_compass(
    parsed: &crate::parsers::compass::ParsedCompass,
    opts: &CompassImportOptions,
) -> anyhow::Result<ConvertedImport> {
    let (sections, closures, warnings) =
        crate::parsers::compass::convert_compass_to_import_configs(parsed, opts)?;
    let (sections_found, length_unit) = crate::parsers::compass::preview_scalars(parsed);
    Ok(ConvertedImport {
        sections,
        closures,
        warnings,
        sections_found,
        unit_conversion: compass_unit_conversion_note(length_unit.as_deref()),
        apply: ApplyPath::Sections,
    })
}

/// Parse a GPX file and return preview data for the import dialog.
#[tauri::command]
pub async fn parse_gpx(path: PathBuf) -> Result<GpxFile, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(
            path = ?path,
            purpose = "prepare GPX import preview",
            "Parsing GPX file for import preview"
        );
        let result = guarded(|| crate::parsers::gpx::parse_gpx(&path));
        if let Err(ref e) = result {
            tracing::error!(
                path = ?path,
                purpose = "prepare GPX import preview",
                reason = %e,
                "Failed to parse GPX file for import preview"
            );
        }
        result.map_err(|e| CommandError::failed(e.to_string()))
    })
    .await
}

/// Parse a GPX file, convert to import configs, and execute the import.
/// On the blocking pool, like [`execute_tml_import`].
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per import-dialog control; grouping them into a \
              struct would put a second, hand-written shape on the IPC wire"
)]
pub async fn execute_gpx_import<R: Runtime>(
    app: AppHandle<R>,
    path: PathBuf,
    file_hash: Option<String>,
    import_waypoints: bool,
    import_tracks: bool,
    import_routes: bool,
    section_placements: Vec<GpxSectionPlacement>,
    depth_inverted: bool,
) -> Result<ImportOutcome, CommandError> {
    let sink = tauri_sink(app.clone());
    pool::run_mutation(app, move |state| {
        let selection = GpxSelection {
            import_waypoints,
            import_tracks,
            import_routes,
            depth_inverted,
        };
        tracing::info!(
            path = ?path,
            purpose = "import GPX",
            import_waypoints = selection.import_waypoints,
            import_tracks = selection.import_tracks,
            import_routes = selection.import_routes,
            "GPX import options"
        );
        run_file_import(
            state,
            sink,
            &GPX_IMPORT,
            &path,
            file_hash,
            // Auto, not a per-file override: the GPX dialog has no mode control,
            // so there is nothing to pass through - but the chain rule still has
            // to run. `parsers::gpx` seeds every leg as Reference on the promise
            // that the classifier can promote it, and skipping the pass (this path
            // used to call `finalize_deferred_import` directly) left a GPS surface
            // track tying two sumps together permanently excluded from the survey
            // graph.
            VirtualShotMode::Auto,
            crate::parsers::gpx::parse_gpx,
            |gpx| convert_gpx(gpx, &selection, &section_placements),
        )
        .into_outcome::<ImportOutcome>()
    })
    .await
}

/// The GPX importer as the runner names it.
const GPX_IMPORT: ImportKind = ImportKind {
    receipt_label: "GPX",
    error_label: "GPX",
    purpose: "import GPX",
};

/// What of a GPX file the dialog chose to import, and how.
pub(crate) struct GpxSelection {
    pub(crate) import_waypoints: bool,
    pub(crate) import_tracks: bool,
    pub(crate) import_routes: bool,
    pub(crate) depth_inverted: bool,
}

/// GPX's convert step for [`run_file_import`]. "Sections found" is the
/// number of sections the selection converts to, as it always was for GPX.
fn convert_gpx(
    gpx: &GpxFile,
    selection: &GpxSelection,
    placements: &[GpxSectionPlacement],
) -> anyhow::Result<ConvertedImport> {
    let (sections, warnings) = crate::parsers::gpx::convert_gpx_to_import_configs(
        gpx,
        selection.import_waypoints,
        selection.import_tracks,
        selection.import_routes,
        placements,
        selection.depth_inverted,
    )?;
    Ok(ConvertedImport {
        sections_found: sections.len() as u32,
        sections,
        closures: Vec::new(),
        warnings,
        unit_conversion: None,
        apply: ApplyPath::Sections,
    })
}

/// Suggest placement positions for an import section by detecting reference shot overlap.
#[tauri::command]
pub async fn suggest_import_placement<R: Runtime>(
    app: AppHandle<R>,
    shots: Vec<ShotData>,
    // `azimuth_reference` is what `shots` are measured against (#438). The
    // caller has it on the `ImportSectionConfig` it is about to send; `None`
    // falls back to the project default, which is only right when the importer
    // declared nothing.
    azimuth_reference: Option<AzimuthReference>,
) -> Result<Vec<PlacementSuggestion>, CommandError> {
    pool::run(app, move |state| {
        let project = state.settled_snapshot().ok_or("No project open")?;

        let existing_sections: Vec<ExistingSection> = project
            .sections
            .iter()
            .map(|s| ExistingSection {
                id: s.id,
                name: s
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Section {}", s.id)),
                shots: s.shots.clone(),
                azimuth_reference: project.azimuth_reference_for(s),
            })
            .collect();

        let config = state.get_config();
        Ok(suggest_placements(
            &shots,
            // The section does not exist yet, so its reference is whatever the
            // importer declared for it; the project default is the fallback (#438).
            azimuth_reference
                .map(AzimuthReference::sanitised)
                .or(project.default_azimuth_reference)
                .unwrap_or_default(),
            &existing_sections,
            &config.placement_suggestion,
        ))
    })
    .await
}

/// Suggest placements for ALL import sections against BOTH existing project
/// sections AND each other (import-to-import). Returns a flat list sorted by score.
#[tauri::command]
pub async fn suggest_all_import_placements<R: Runtime>(
    app: AppHandle<R>,
    import_sections: Vec<ImportSectionForSuggestion>,
) -> Result<Vec<CrossSectionSuggestion>, CommandError> {
    pool::run(app, move |state| {
        // One snapshot for both reads (#576 NF-AN6-4: the fallback used to take a
        // second whole-project copy of its own).
        let project = state.settled_snapshot();
        let existing_sections: Vec<ExistingSection> = match project.as_deref() {
            Some(project) => project
                .sections
                .iter()
                .map(|s| ExistingSection {
                    id: s.id,
                    name: s
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Section {}", s.id)),
                    shots: s.shots.clone(),
                    azimuth_reference: project.azimuth_reference_for(s),
                })
                .collect(),
            None => Vec::new(),
        };
        // Only the FALLBACK: each section carries its own reference and is scored
        // in it (#438).
        let fallback_reference = project
            .and_then(|p| p.default_azimuth_reference)
            .unwrap_or_default();
        let import_sections: Vec<ImportSectionForSuggestion> = import_sections
            .into_iter()
            .map(|mut section| {
                // Deserialized straight off the wire: re-impose the type's
                // invariants before anything computes with it.
                section.azimuth_reference =
                    section.azimuth_reference.map(AzimuthReference::sanitised);
                section
            })
            .collect();

        let config = state.get_config();
        Ok(suggest_all_placements(
            &import_sections,
            fallback_reference,
            &existing_sections,
            &config.placement_suggestion,
        ))
    })
    .await
}

/// Suggest placements for orphaned sections' leaf ends against newly imported sections.
/// Orphaned sections are those in connected components (via merges) with no zero datum.
#[tauri::command]
pub async fn suggest_orphan_placements<R: Runtime>(
    app: AppHandle<R>,
    newly_imported_section_ids: Vec<u32>,
) -> Result<Vec<OrphanPlacementSuggestion>, CommandError> {
    pool::run(app, move |state| {
        let project = state.settled_snapshot().ok_or("No project open")?;
        let config = state.get_config();
        Ok(project
            .suggest_orphan_placements(&newly_imported_section_ids, &config.placement_suggestion))
    })
    .await
}

/// Count sections not connected to a zero datum, restricted to components
/// touched by `newly_imported_section_ids`. Used by the import dialogs to add
/// an info line to the import receipt when the orphan-placement pass produced
/// no suggestions (#446).
#[tauri::command]
pub async fn count_unanchored_sections<R: Runtime>(
    app: AppHandle<R>,
    newly_imported_section_ids: Vec<u32>,
) -> Result<u32, CommandError> {
    pool::run(app, move |state| {
        let project = state.settled_snapshot().ok_or("No project open")?;
        Ok(project.count_unanchored_sections(&newly_imported_section_ids))
    })
    .await
}

/// Parse a Walls `.WPJ`/`.SRV` file and return preview data for the import dialog.
#[tauri::command]
pub async fn parse_walls(path: PathBuf) -> Result<WallsFile, CommandError> {
    pool::run_stateless(move || {
        tracing::info!(
            path = ?path,
            purpose = "prepare Walls import preview",
            "Parsing Walls .WPJ/.SRV file for import preview"
        );
        // The preview is built from the file's data too, so it runs under the
        // same guard as the parse (#576 RV3-R-12).
        guarded(|| {
            crate::parsers::walls::parse_walls(&path)
                .map(|parsed| crate::parsers::walls::build_preview(&parsed))
        })
        .map_err(|e| {
            tracing::error!(
                path = ?path,
                purpose = "prepare Walls import preview",
                reason = %e,
                "Failed to parse Walls file for import preview"
            );
            CommandError::failed(e.to_string())
        })
    })
    .await
}

/// Parse a Walls file, convert to import configs, and execute the import.
/// On the blocking pool, like [`execute_tml_import`].
#[tauri::command]
pub async fn execute_walls_import<R: Runtime>(
    app: AppHandle<R>,
    path: PathBuf,
    file_hash: Option<String>,
    comments_on_stations: bool,
    include_splays: bool,
    selected_surveys: Option<Vec<u32>>,
) -> Result<ImportOutcome, CommandError> {
    let sink = tauri_sink(app.clone());
    pool::run_mutation(app, move |state| {
        let opts = WallsImportOptions {
            comments_on_stations,
            include_splays,
            // `None` => import every survey; `Some(list)` => only the chosen survey
            // indices (into the preview's `WallsFile::surveys`, which is 1:1 with
            // the parser's survey order).
            selected_surveys: selected_surveys.map(|v| v.into_iter().collect()),
        };
        run_file_import(
            state,
            sink,
            &WALLS_IMPORT,
            &path,
            file_hash,
            // Auto: no dialog on this path carries a per-file override, so the
            // chain rule decides (decision C10-D1).
            VirtualShotMode::Auto,
            crate::parsers::walls::parse_walls,
            |parsed| convert_walls(parsed, &opts),
        )
        .into_outcome::<ImportOutcome>()
    })
    .await
}

/// The Walls importer as the runner names it.
const WALLS_IMPORT: ImportKind = ImportKind {
    receipt_label: "Walls",
    error_label: "Walls",
    purpose: "import Walls",
};

/// Walls's convert step for [`run_file_import`].
fn convert_walls(
    parsed: &crate::parsers::walls::ParsedWalls,
    opts: &WallsImportOptions,
) -> anyhow::Result<ConvertedImport> {
    let (sections, closures, warnings) =
        crate::parsers::walls::convert_walls_to_import_configs(parsed, opts)?;
    let (sections_found, any_feet_units) = crate::parsers::walls::preview_scalars(parsed);
    Ok(ConvertedImport {
        sections,
        closures,
        warnings,
        sections_found,
        unit_conversion: walls_unit_conversion_note(any_feet_units),
        apply: ApplyPath::Sections,
    })
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod receipt_tests {
    use super::*;
    use crate::core::command::ImportReceipt;
    use crate::core::{ValidationWarning, WarningSeverity};
    use stalagmight_core::test_fixtures::state_with_project;
    use std::fs;
    use std::path::Path;

    fn base_result() -> CommandResult {
        CommandResult::success().with_import_receipt(ImportReceipt {
            source_format: String::new(),
            file_name: String::new(),
            sections_found: 0,
            sections_imported: 3,
            shot_count: 12,
            closures_requested: 1,
            closures_created: 1,
            unit_conversion: None,
            warnings: Vec::new(),
            info: Vec::new(),
            zero_datum_created: None,
            hidden_warnings: 0,
        })
    }

    #[test]
    fn decorate_receipt_fills_placeholder_fields_and_extends_warnings() {
        let mut result = base_result();
        decorate_receipt(
            &mut result,
            "Ariane TML",
            Path::new("/tmp/cave.tml"),
            5,
            Some("Lengths converted from feet to meters".to_string()),
            vec!["1 section had no shots after filtering and was dropped".to_string()],
        );
        let receipt = result
            .import_receipt
            .expect("receipt must survive decoration");
        assert_eq!(receipt.source_format, "Ariane TML");
        assert_eq!(receipt.file_name, "cave.tml");
        assert_eq!(receipt.sections_found, 5);
        assert_eq!(
            receipt.unit_conversion.as_deref(),
            Some("Lengths converted from feet to meters")
        );
        // Base counts from execute_import_sections are untouched.
        assert_eq!(receipt.sections_imported, 3);
        assert_eq!(receipt.shot_count, 12);
        assert_eq!(receipt.warnings.len(), 1);
    }

    /// The cap keeps the FIRST 50 warnings, and a per-shot execute phase can
    /// blow past 50 on its own. A converter warning appended behind them - the
    /// unrecognized unit, which says the whole survey may be mis-scaled - was
    /// then the first thing dropped, on exactly the file that needs it most.
    #[test]
    fn a_converter_warning_survives_a_flood_of_execute_warnings() {
        let mut result = base_result();
        if let Some(receipt) = result.import_receipt.as_mut() {
            receipt.warnings = (0..60).map(|i| format!("shot {i} looks odd")).collect();
        }

        decorate_receipt(
            &mut result,
            "Ariane TML",
            Path::new("/tmp/cave.tml"),
            5,
            None,
            vec![
                "The file declares an unrecognized unit \"furlongs\"; all measurements were read as meters"
                    .to_string(),
            ],
        );

        let receipt = result
            .import_receipt
            .expect("receipt must survive decoration");
        assert!(
            receipt.warnings.iter().any(|w| w.contains("furlongs")),
            "the unit warning must outrank per-shot noise, got {:?}",
            receipt.warnings.first()
        );
        // And the flood is still capped, summary entry included.
        assert_eq!(receipt.warnings.len(), 51);
    }

    /// The whole point of `ImportReceipt::empty`: an import that landed nothing
    /// still has to carry the converter's explanation of why. Before it existed
    /// the empty path returned a bare `CommandResult::success()` and this
    /// decoration was a no-op, so the file's warnings never reached the user.
    #[test]
    fn an_empty_receipt_still_collects_the_converter_warnings() {
        let mut result = CommandResult::success().with_import_receipt(ImportReceipt::empty());
        decorate_receipt(
            &mut result,
            "Ariane TML",
            Path::new("/tmp/cave.tml"),
            2,
            None,
            vec!["2 sections had no shots after filtering and were dropped".to_string()],
        );
        let receipt = result
            .import_receipt
            .expect("an empty import must still report a receipt");
        assert_eq!(receipt.sections_imported, 0);
        assert_eq!(receipt.shot_count, 0);
        assert_eq!(receipt.sections_found, 2);
        assert_eq!(
            receipt.warnings,
            vec!["2 sections had no shots after filtering and were dropped".to_string()]
        );
    }

    fn degraded_save() -> CommandResult {
        CommandResult::success().with_warnings(vec![ValidationWarning {
            code: "DATABASE_NOT_REOPENED".to_string(),
            message: "File saved, but database could not be reopened.".to_string(),
            severity: WarningSeverity::Warning,
        }])
    }

    /// #154: a deferred save that SUCCEEDS can still report a degraded state.
    /// Dropping those warnings made an import that left the database closed
    /// look perfectly clean.
    #[test]
    fn a_successful_deferred_save_still_carries_its_warnings() {
        let import_result = CommandResult::success().with_warnings(vec![ValidationWarning {
            code: "LONG_SHOT".to_string(),
            message: "shot 3 is unusually long".to_string(),
            severity: WarningSeverity::Warning,
        }]);

        let merged = merge_deferred_save_outcome(import_result, degraded_save());
        assert!(merged.success);
        let codes: Vec<&str> = merged.warnings.iter().map(|w| w.code.as_str()).collect();
        assert_eq!(
            codes,
            vec!["LONG_SHOT", "DATABASE_NOT_REOPENED"],
            "the save's warnings must be appended to the import's own, not replace them"
        );
    }

    /// And they must reach the RECEIPT, which is the only surface that shows
    /// them: the frontend keeps just `code == "SYSTEM"` out of
    /// `CommandResult.warnings`, so a `DATABASE_NOT_REOPENED` there is read by
    /// nobody. Leading, ahead of the importer's own caveats.
    #[test]
    fn a_degraded_deferred_save_leads_the_receipt() {
        let import_result = CommandResult::success().with_import_receipt(ImportReceipt {
            warnings: vec!["1 section had no shots after filtering".to_string()],
            ..ImportReceipt::empty()
        });

        let merged = merge_deferred_save_outcome(import_result, degraded_save());
        let receipt = merged
            .import_receipt
            .expect("the receipt must survive the merge");
        assert_eq!(
            receipt.warnings,
            vec![
                "File saved, but database could not be reopened.".to_string(),
                "1 section had no shots after filtering".to_string(),
            ]
        );
    }

    /// The receipt cap still holds afterwards - the save's warnings are added
    /// to a list a hostile file can already have filled.
    #[test]
    fn a_degraded_deferred_save_respects_the_receipt_cap() {
        let import_result = CommandResult::success().with_import_receipt(ImportReceipt {
            warnings: (0..60).map(|i| format!("shot {i} looks odd")).collect(),
            ..ImportReceipt::empty()
        });

        let merged = merge_deferred_save_outcome(import_result, degraded_save());
        let receipt = merged
            .import_receipt
            .expect("the receipt must survive the merge");
        assert_eq!(receipt.warnings.len(), 51);
        assert!(receipt.warnings[0].contains("database could not be reopened"));
    }

    /// #576 C17: the realistic order - the importer's receipt is already
    /// capped (`decorate_receipt`) when the first import into a new project
    /// resolves its deferred save. The second cap used to take the first
    /// one's "... and 250 more" line for an ordinary warning, drop it with
    /// the 50th real warning, and report "2 more" when 251 were hidden.
    #[test]
    fn a_degraded_deferred_save_after_a_capped_receipt_keeps_the_hidden_count() {
        let mut import_result = base_result();
        decorate_receipt(
            &mut import_result,
            "Compass",
            Path::new("/tmp/cave.dat"),
            1,
            None,
            (0..300).map(|i| format!("converter note {i}")).collect(),
        );

        let merged = merge_deferred_save_outcome(import_result, degraded_save());
        let receipt = merged
            .import_receipt
            .expect("the receipt must survive the merge");
        assert_eq!(receipt.warnings.len(), 51);
        assert!(receipt.warnings[0].contains("database could not be reopened"));
        assert_eq!(receipt.warnings[49], "converter note 48");
        assert_eq!(
            receipt.warnings.last().map(String::as_str),
            Some("... and 251 more warnings"),
            "301 warnings, 50 shown"
        );
    }

    /// The clean case must stay clean - no phantom warning from a save that had
    /// nothing to say.
    #[test]
    fn a_clean_deferred_save_adds_nothing() {
        let merged =
            merge_deferred_save_outcome(CommandResult::success(), CommandResult::success());
        assert!(merged.warnings.is_empty());
    }

    /// The failure branch is unchanged: one "save manually" warning, stamped
    /// SYSTEM, and the import still reports success.
    #[test]
    fn a_failed_deferred_save_warns_and_keeps_the_import_successful() {
        let merged = merge_deferred_save_outcome(
            CommandResult::success(),
            CommandResult::failure("disk on fire".to_string()),
        );
        assert!(merged.success);
        assert_eq!(merged.warnings.len(), 1);
        assert_eq!(merged.warnings[0].code, "SYSTEM");
        assert!(merged.warnings[0].message.contains("disk on fire"));
    }

    /// The no-receipt case now means one thing only: the import FAILED. It used
    /// to also cover the empty-import path, which is exactly how a total loss
    /// came back silent (#411); that path carries `ImportReceipt::empty()` now,
    /// covered by the test above. A failed import still gets nothing - its
    /// error is what the frontend shows.
    #[test]
    fn decorate_receipt_is_a_no_op_on_a_failed_import() {
        let mut result = CommandResult::failure("Failed to parse Compass file".to_string());
        decorate_receipt(
            &mut result,
            "Compass",
            Path::new("/tmp/cave.dat"),
            1,
            None,
            vec!["should be dropped".to_string()],
        );
        assert!(result.import_receipt.is_none());
    }

    #[test]
    fn compass_unit_conversion_note_covers_feet_mixed_and_metric() {
        assert_eq!(
            compass_unit_conversion_note(Some("feet")).as_deref(),
            Some("Lengths converted from feet to meters")
        );
        assert_eq!(
            compass_unit_conversion_note(Some("mixed")).as_deref(),
            Some("Some surveys measured in feet; converted to meters")
        );
        assert_eq!(compass_unit_conversion_note(Some("meters")), None);
        assert_eq!(compass_unit_conversion_note(None), None);
    }

    #[test]
    fn walls_unit_conversion_note_covers_feet_and_metric() {
        assert_eq!(
            walls_unit_conversion_note(true).as_deref(),
            Some("Some surveys measured in feet; converted to meters")
        );
        assert_eq!(walls_unit_conversion_note(false), None);
    }

    /// #576 B13: the DMP file's parse warnings (a section cut short, a damaged
    /// record) lead the receipt ahead of the per-shot execute warnings, survive
    /// a flood of them, and are sanitized - they quote section names, which are
    /// file content.
    #[test]
    fn dmp_source_warnings_lead_the_receipt() {
        let mut result = base_result();
        if let Some(receipt) = result.import_receipt.as_mut() {
            receipt.warnings = (0..60).map(|i| format!("shot {i} looks odd")).collect();
        }
        lead_receipt_with_source_warnings(
            &mut result,
            vec![
                "Section \"A\u{202e}BC\" (#2 in the file) has no end-of-section marker".to_string(),
                String::new(),
            ],
        );
        let receipt = result.import_receipt.expect("receipt");
        assert_eq!(
            receipt.warnings.first().map(String::as_str),
            Some("Section \"ABC\" (#2 in the file) has no end-of-section marker"),
            "leads, sanitized; an empty entry is dropped"
        );
        assert_eq!(
            receipt.warnings.get(1).map(String::as_str),
            Some("shot 0 looks odd")
        );
        assert!(receipt.warnings.len() <= 51, "capped like any receipt");

        let mut refused = CommandResult::failure("refused".to_string());
        lead_receipt_with_source_warnings(&mut refused, vec!["dropped".to_string()]);
        assert!(refused.import_receipt.is_none());
    }

    // ----- the shared runner (#576 C10) ------------------------------------

    /// Records the phase of every progress event.
    #[derive(Default)]
    struct RecordingSink(std::sync::Mutex<Vec<String>>);

    impl crate::core::events::EventSink for RecordingSink {
        fn emit(&self, event: &str, payload: serde_json::Value) {
            if let Ok(mut events) = self.0.lock() {
                events.push(format!(
                    "{event}:{}",
                    payload["phase"].as_str().unwrap_or("?")
                ));
            }
        }

        fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
            false
        }
    }

    /// The phases `sink` saw, in order.
    fn phases(sink: &RecordingSink) -> Vec<String> {
        sink.0.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// The parts of a result the receipt-equality checks compare.
    fn comparable(result: &CommandResult) -> serde_json::Value {
        serde_json::to_value(result).expect("a command result serializes")
    }

    const TEST_IMPORT: ImportKind = ImportKind {
        receipt_label: "Synthetic",
        error_label: "Synthetic",
        purpose: "import Synthetic",
    };

    /// #576 B8 remainder: a panic in a parser used to unwind out of the
    /// command, so the frontend's promise never settled and the dialog
    /// waited forever. The runner turns it into a failed import that says
    /// what happened, and nothing is imported.
    #[test]
    fn a_panicking_parse_or_convert_fails_the_import_instead_of_unwinding() {
        let state = state_with_project("Synthetic Cave");
        let sink = std::sync::Arc::new(RecordingSink::default());
        let parse_panics = run_file_import(
            &state,
            std::sync::Arc::<RecordingSink>::clone(&sink),
            &TEST_IMPORT,
            Path::new("/tmp/synthetic.dat"),
            None,
            VirtualShotMode::Auto,
            |_| -> anyhow::Result<()> { panic!("index out of bounds: synthetic") },
            |()| -> anyhow::Result<ConvertedImport> { panic!("not reached") },
        );
        assert!(!parse_panics.success);
        assert_eq!(
            parse_panics.error.as_deref(),
            Some(
                "Failed to parse Synthetic file: the importer stopped on an internal error \
                 (index out of bounds: synthetic); nothing was imported"
            )
        );

        let detail = String::from("formatted detail");
        let convert_panics = run_file_import(
            &state,
            sink,
            &TEST_IMPORT,
            Path::new("/tmp/synthetic.dat"),
            None,
            VirtualShotMode::Auto,
            |_| Ok(()),
            move |()| -> anyhow::Result<ConvertedImport> { panic!("{detail}") },
        );
        assert_eq!(
            convert_panics.error.as_deref(),
            Some(
                "Failed to convert Synthetic data: the importer stopped on an internal error \
                 (formatted detail); nothing was imported"
            )
        );
        let project = state.get_project_state().expect("project");
        assert!(project.sections.is_empty(), "nothing was imported");
        assert!(!state.can_undo(), "and nothing was recorded");
    }

    /// Every runner import reports the four import phases, in order.
    #[test]
    fn the_runner_reports_every_import_phase() {
        let state = state_with_project("Synthetic Cave");
        let sink = std::sync::Arc::new(RecordingSink::default());
        let result = run_file_import(
            &state,
            std::sync::Arc::<RecordingSink>::clone(&sink),
            &TEST_IMPORT,
            Path::new("/tmp/synthetic.dat"),
            None,
            VirtualShotMode::Auto,
            |_| Ok(()),
            |()| {
                Ok(ConvertedImport {
                    sections: Vec::new(),
                    closures: Vec::new(),
                    warnings: vec!["a converter note".to_string()],
                    sections_found: 0,
                    unit_conversion: None,
                    apply: ApplyPath::Sections,
                })
            },
        );
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            result.import_receipt.expect("receipt").warnings,
            vec!["a converter note".to_string()]
        );
        let starts: Vec<String> = phases(&sink).into_iter().step_by(2).collect();
        assert_eq!(
            starts,
            [
                "loading-progress:import_parsing",
                "loading-progress:import_converting",
                "loading-progress:import_applying_changes",
                "loading-progress:import_finalizing",
            ]
        );
    }

    /// A synthetic one-survey Compass `.DAT` (feet, two legs).
    fn synthetic_compass_dat(dir: &Path) -> PathBuf {
        let path = dir.join("synthetic.dat");
        fs::write(
            &path,
            "Synthetic Cave\r\n\
             SURVEY NAME: A\r\n\
             SURVEY DATE: 7 10 1979\r\n\
             SURVEY TEAM:\r\n\
             Nobody\r\n\
             DECLINATION: 0.00  FORMAT: DMMDLRUDLADN  CORRECTIONS: 0.00 0.00 0.00\r\n\
             \r\n\
             FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\
             \r\n\
             A1 A2 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n\
             A2 A3 12.50 180.00 -10.00 1.00 2.00 3.00 4.00\r\n\
             \x0c\r\n",
        )
        .expect("write the synthetic .DAT");
        path
    }

    /// Compass on the runner produces exactly the result the hand-written
    /// command did: parse, convert, apply, decorate, finalize.
    #[test]
    fn a_compass_import_on_the_runner_keeps_its_receipt() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = synthetic_compass_dat(dir.path());
        let opts = CompassImportOptions {
            include_excluded: false,
            apply_corrections: true,
            comments_on_stations: false,
        };

        let legacy_state = state_with_project("Synthetic Cave");
        let parsed = crate::parsers::compass::parse_compass(&path).expect("parse");
        let (sections, closures, warnings) =
            crate::parsers::compass::convert_compass_to_import_configs(&parsed, &opts)
                .expect("convert");
        let (sections_found, length_unit) = crate::parsers::compass::preview_scalars(&parsed);
        let mut legacy = legacy_state.execute_import_sections(
            sections,
            closures,
            Some("hash".to_string()),
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
        );
        decorate_receipt(
            &mut legacy,
            "Compass",
            &path,
            sections_found,
            compass_unit_conversion_note(length_unit.as_deref()),
            warnings,
        );
        let legacy = finalize_deferred_import(&legacy_state, legacy);

        let state = state_with_project("Synthetic Cave");
        let runner = run_file_import(
            &state,
            std::sync::Arc::new(RecordingSink::default()),
            &COMPASS_IMPORT,
            &path,
            Some("hash".to_string()),
            VirtualShotMode::Auto,
            crate::parsers::compass::parse_compass,
            |parsed| convert_compass(parsed, &opts),
        );

        assert!(runner.success, "{:?}", runner.error);
        assert!(
            runner
                .import_receipt
                .as_ref()
                .is_some_and(|r| r.shot_count > 0 && r.unit_conversion.is_some()),
            "premise: a receipt with shots and a unit note, got {:?}",
            runner.import_receipt
        );
        assert_eq!(comparable(&runner), comparable(&legacy));
    }

    /// Walls on the runner produces exactly the result the hand-written
    /// command did.
    #[test]
    fn a_walls_import_on_the_runner_keeps_its_receipt() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("synthetic.srv");
        fs::write(
            &path,
            "#UNITS feet order=DAV\nA1 A2 10 90 0\nA2 A3 12.5 180 -10\n",
        )
        .expect("write the synthetic .SRV");
        let opts = WallsImportOptions {
            comments_on_stations: false,
            include_splays: false,
            selected_surveys: None,
        };

        let legacy_state = state_with_project("Synthetic Cave");
        let parsed = crate::parsers::walls::parse_walls(&path).expect("parse");
        let (sections, closures, warnings) =
            crate::parsers::walls::convert_walls_to_import_configs(&parsed, &opts)
                .expect("convert");
        let (sections_found, any_feet_units) = crate::parsers::walls::preview_scalars(&parsed);
        let mut legacy = legacy_state.execute_import_sections(
            sections,
            closures,
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
        );
        decorate_receipt(
            &mut legacy,
            "Walls",
            &path,
            sections_found,
            walls_unit_conversion_note(any_feet_units),
            warnings,
        );
        let legacy = finalize_deferred_import(&legacy_state, legacy);

        let state = state_with_project("Synthetic Cave");
        let runner = run_file_import(
            &state,
            std::sync::Arc::new(RecordingSink::default()),
            &WALLS_IMPORT,
            &path,
            None,
            VirtualShotMode::Auto,
            crate::parsers::walls::parse_walls,
            |parsed| convert_walls(parsed, &opts),
        );

        assert!(runner.success, "{:?}", runner.error);
        assert!(
            runner
                .import_receipt
                .as_ref()
                .is_some_and(|r| r.shot_count > 0 && r.unit_conversion.is_some()),
            "premise: a receipt with shots and a unit note, got {:?}",
            runner.import_receipt
        );
        assert_eq!(comparable(&runner), comparable(&legacy));
    }

    /// Ariane on the runner produces exactly the result the hand-written
    /// command did, entrance elevation included.
    #[test]
    fn a_tml_import_on_the_runner_keeps_its_receipt() {
        use crate::parsers::tml::test_helpers::{
            tml_shot_xml_depth, tml_shot_xml_gps, wrap_xml_with,
        };
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("synthetic.tmlu");
        let shots = [
            tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
            tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
            tml_shot_xml_depth(2, 1, "REAL", "Main", 12.0, -2.0),
        ]
        .join("\n");
        fs::write(&path, wrap_xml_with("ft", 328.084, &shots)).expect("write the .tmlu");
        let opts = || TmlImportOptions {
            include_excluded: false,
            comments_on_stations: false,
            selected_sections: None,
            root_placement: None,
        };

        let legacy_state = state_with_project("Synthetic Cave");
        let tml = crate::parsers::tml::parse_tml(&path).expect("parse");
        let (sections, closures, warnings) =
            crate::parsers::tml::convert_tml_with_options(&tml, &opts()).expect("convert");
        let mut legacy = legacy_state.execute_tml_import_sections(
            sections,
            closures,
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
            crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation),
        );
        decorate_receipt(
            &mut legacy,
            "Ariane TML",
            &path,
            tml.sections.len() as u32,
            tml_unit_conversion_note(tml.unit),
            warnings,
        );
        let legacy = finalize_deferred_import(&legacy_state, legacy);

        let state = state_with_project("Synthetic Cave");
        let opts = opts();
        let runner = run_file_import(
            &state,
            std::sync::Arc::new(RecordingSink::default()),
            &TML_IMPORT,
            &path,
            None,
            VirtualShotMode::Auto,
            crate::parsers::tml::parse_tml,
            |tml| convert_tml(tml, &opts),
        );

        assert!(runner.success, "{:?}", runner.error);
        assert!(
            runner
                .import_receipt
                .as_ref()
                .is_some_and(|r| r.shot_count > 0 && r.unit_conversion.is_some()),
            "premise: a receipt with shots and a unit note, got {:?}",
            runner.import_receipt
        );
        assert_eq!(comparable(&runner), comparable(&legacy));
        let elevation = |state: &AppState| {
            state
                .get_project_state()
                .and_then(|p| p.surface_elevation)
                .map(|e| (e * 1000.0).round())
        };
        assert_eq!(elevation(&state), Some(100_000.0), "328.084 ft is 100 m");
        assert_eq!(elevation(&state), elevation(&legacy_state));
    }

    /// GPX on the runner produces the result the hand-written command did -
    /// which already carried the converter's warnings (#576 B34) - and now
    /// reports its progress like every other importer.
    #[test]
    fn a_gpx_import_on_the_runner_keeps_its_receipt_and_reports_progress() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("synthetic.gpx");
        fs::write(
            &path,
            r#"<gpx version="1.1"><trk><name>Surface</name><trkseg>
                <trkpt lat="0.0" lon="0.0"/>
                <trkpt lat="0.0" lon="0.0005"><ele>350</ele></trkpt>
                <trkpt lat="0.0" lon="0.0010"><ele>349</ele></trkpt>
            </trkseg></trk></gpx>"#,
        )
        .expect("write the .gpx");
        let selection = GpxSelection {
            import_waypoints: true,
            import_tracks: true,
            import_routes: true,
            depth_inverted: false,
        };

        let legacy_state = state_with_project("Synthetic Cave");
        let gpx = crate::parsers::gpx::parse_gpx(&path).expect("parse");
        let (sections, warnings) =
            crate::parsers::gpx::convert_gpx_to_import_configs(&gpx, true, true, true, &[], false)
                .expect("convert");
        let sections_found = sections.len() as u32;
        let mut legacy = legacy_state.execute_import_sections(
            sections,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
        );
        decorate_receipt(&mut legacy, "GPX", &path, sections_found, None, warnings);
        let legacy = finalize_deferred_import(&legacy_state, legacy);

        let state = state_with_project("Synthetic Cave");
        let sink = std::sync::Arc::new(RecordingSink::default());
        let runner = run_file_import(
            &state,
            std::sync::Arc::<RecordingSink>::clone(&sink),
            &GPX_IMPORT,
            &path,
            None,
            VirtualShotMode::Auto,
            crate::parsers::gpx::parse_gpx,
            |gpx| convert_gpx(gpx, &selection, &[]),
        );

        assert!(runner.success, "{:?}", runner.error);
        assert_eq!(
            runner.import_receipt.as_ref().map(|r| r.warnings.clone()),
            Some(vec![
                "1 GPS point had no elevation; it was filled in from the nearest points along \
                 the track that have one."
                    .to_string()
            ])
        );
        assert_eq!(comparable(&runner), comparable(&legacy));
        assert_eq!(phases(&sink).len(), 8, "four phases, started and completed");
    }
}
