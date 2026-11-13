use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, State};

use crate::commands::event_sink::tauri_sink;
use crate::core::loading::{LoadingPhase, ProgressEmitter};
use crate::core::state::{AppState, VirtualShotMode};
use crate::core::{Command, CommandResult};
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
pub fn import_dmp(path: PathBuf) -> Result<DmpFile, String> {
    tracing::info!(
        path = ?path,
        purpose = "prepare import preview",
        "Parsing DMP file for import preview"
    );
    let result = crate::parsers::parse_dmp(&path);
    if let Err(ref e) = result {
        tracing::error!(
            path = ?path,
            purpose = "prepare import preview",
            reason = %e,
            "Failed to parse DMP file for import preview"
        );
    }
    result.map_err(|e| e.to_string())
}

/// Calculate SHA256 hash of a file
#[tauri::command]
pub fn calculate_file_hash(path: PathBuf) -> Result<String, String> {
    tracing::debug!(
        path = ?path,
        purpose = "detect duplicate imports",
        "Calculating file hash for import deduplication"
    );
    let contents = fs::read(&path).map_err(|e| {
        tracing::error!(
            path = ?path,
            purpose = "detect duplicate imports",
            reason = %e,
            "Failed to read file while calculating import hash"
        );
        format!("Failed to read file: {e}")
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&contents);
    let result = hasher.finalize();

    let hash = format!("{result:x}");
    tracing::debug!(
        path = ?path,
        purpose = "detect duplicate imports",
        hash = %hash,
        "Calculated file hash for import deduplication"
    );
    Ok(hash)
}

/// Check if a file hash has already been imported to the project
#[tauri::command]
pub fn check_duplicate_import(state: State<'_, AppState>, hash: String) -> Result<bool, String> {
    check_duplicate_import_impl(state.inner(), hash)
}

pub(crate) fn check_duplicate_import_impl(state: &AppState, hash: String) -> Result<bool, String> {
    tracing::debug!(
        hash = %hash,
        purpose = "detect duplicate imports",
        "Checking whether file hash was previously imported"
    );
    state
        .check_duplicate_import(&hash)
        .map_err(|e| e.to_string())
}

/// Track an imported file hash in the project
#[tauri::command]
pub fn track_imported_file(state: State<'_, AppState>, hash: String) -> CommandResult {
    track_imported_file_impl(state.inner(), hash)
}

pub(crate) fn track_imported_file_impl(state: &AppState, hash: String) -> CommandResult {
    tracing::debug!(
        hash = %hash,
        purpose = "prevent duplicate imports",
        "Tracking imported file hash"
    );
    state.track_imported_file(hash)
}

/// Batch-check which section content hashes have already been imported.
/// Returns a parallel Vec<bool> (true = already imported).
#[tauri::command]
pub fn check_section_hashes(
    state: State<'_, AppState>,
    hashes: Vec<String>,
) -> Result<Vec<bool>, String> {
    tracing::debug!(
        count = hashes.len(),
        purpose = "detect duplicate section imports",
        "Checking section content hashes"
    );
    state
        .check_section_hashes(&hashes)
        .map_err(|e| e.to_string())
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
fn finalize_deferred_import(state: &AppState, import_result: CommandResult) -> CommandResult {
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
    // Don't leak the internal classification markers into the user-facing warning.
    let detail = if raw.starts_with(crate::core::loading::PERMISSION_DENIED_CODE) {
        "permission denied (check folder access in Privacy & Security)".to_string()
    } else if raw.starts_with(crate::core::loading::PATH_NOT_FOUND_CODE) {
        "the folder no longer exists or the drive is not connected".to_string()
    } else if raw.starts_with(crate::core::loading::PATH_TOO_LONG_CODE) {
        "the path is too long for the system to open".to_string()
    } else {
        raw
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
/// provenance for the import receipt - today only the MNemo DMP wizard passes
/// them (a later shard). All three are optional and `serde`-defaulted, so
/// existing callers (and the Playwright IPC mock) are unaffected, and the
/// base receipt `execute_import_sections` already attached is left at its
/// defaults when none are provided.
#[tauri::command]
pub fn import_sections(
    state: State<'_, AppState>,
    sections: Vec<crate::models::ImportSectionConfig>,
    file_hash: Option<String>,
    section_hashes: Option<Vec<String>>,
    existing_shot_updates: Option<Vec<crate::models::ExistingShotUpdate>>,
    virtual_shot_mode: Option<VirtualShotMode>,
    source_label: Option<String>,
    source_file_name: Option<String>,
    sections_found: Option<u32>,
) -> CommandResult {
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
            receipt.source_format = source_label.unwrap_or_else(|| "MNemo DMP".to_string());
            receipt.file_name = source_file_name.unwrap_or_default();
            receipt.sections_found = sections_found.unwrap_or(0);
        }
    }

    finalize_deferred_import(state.inner(), result)
}

/// Append another `.stalx`'s survey into the OPEN project (#674) as one undo
/// step, refusing (nothing changed) with no project open, the open project's
/// own file, or an unreadable file. The result carries `section_ids` - exactly
/// the added sections, the orphan-placement dialog's input - and an
/// `import_receipt`. Funnels through `finalize_deferred_import` like every
/// import, so an add into a never-saved project makes it real on disk.
///
/// Async, with the work on a blocking thread exactly as `load_project_full`
/// does: a synchronous command runs on the main thread, and the copy (up to
/// 512 MiB), the SQLite open and migration, the load and the geometry re-solve
/// would freeze the window for their whole length.
#[tauri::command]
pub async fn add_project_file<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    path: String,
) -> CommandResult {
    tracing::info!(
        purpose = "add a .stalx into the open project",
        "add_project_file called"
    );
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let result = state.execute(Command::add_project_file(PathBuf::from(path)));
        finalize_deferred_import(state.inner(), result)
    })
    .await
    .unwrap_or_else(|join_error| {
        CommandResult::failure(format!("The add did not finish: {join_error}"))
    })
}

/// Parse a TML/TMLU file and return preview data for the import dialog.
#[tauri::command]
pub fn parse_tml(path: PathBuf) -> Result<TmlFile, String> {
    tracing::info!(
        path = ?path,
        purpose = "prepare TML import preview",
        "Parsing Ariane TML/TMLU file for import preview"
    );
    let result = crate::parsers::tml::parse_tml(&path);
    if let Err(ref e) = result {
        tracing::error!(
            path = ?path,
            purpose = "prepare TML import preview",
            reason = %e,
            "Failed to parse TML/TMLU file for import preview"
        );
    }
    result.map_err(|e| e.to_string())
}

/// Parse a TML/TMLU file, convert to import configs, and execute the import.
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per import-dialog control; grouping them into a \
              struct would put a second, hand-written shape on the IPC wire"
)]
pub fn execute_tml_import(
    app: AppHandle,
    state: State<'_, AppState>,
    path: PathBuf,
    file_hash: Option<String>,
    include_excluded: bool,
    comments_on_stations: bool,
    virtual_shot_mode: Option<VirtualShotMode>,
    selected_sections: Option<Vec<u32>>,
    root_placement: Option<TmlRootPlacement>,
) -> CommandResult {
    tracing::info!(
        path = ?path,
        purpose = "import Ariane TML/TMLU",
        "Executing TML import"
    );

    let mut emitter = ProgressEmitter::new_for_import(tauri_sink(app));

    emitter.start_phase(LoadingPhase::ImportParsing);
    let tml = match crate::parsers::tml::parse_tml(&path) {
        Ok(tml) => tml,
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose = "import Ariane TML/TMLU",
                reason = %e,
                "Failed to parse TML/TMLU file for import"
            );
            return CommandResult::failure(format!("Failed to parse TML file: {e}"));
        }
    };
    emitter.complete_phase(LoadingPhase::ImportParsing);

    emitter.start_phase(LoadingPhase::ImportConverting);
    let opts = TmlImportOptions {
        include_excluded,
        comments_on_stations,
        // `None` ⇒ import every section; `Some(list)` ⇒ only the chosen section
        // indices (into the preview's `TmlFile::sections`, which the dialog
        // lists 1:1). An empty list imports nothing and lands the empty
        // receipt, exactly like a file whose every shot was filtered out.
        selected_sections: selected_sections.map(|v| v.into_iter().collect()),
        // `None` ⇒ the pre-#425 silent default (the file's GPS, else origin).
        root_placement,
    };
    let (sections, closures, converter_warnings) =
        match crate::parsers::tml::convert_tml_with_options(&tml, &opts) {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(
                    path = ?path,
                    purpose = "import Ariane TML/TMLU",
                    reason = %e,
                    "Failed to convert TML data to import configs"
                );
                return CommandResult::failure(format!("Failed to convert TML data: {e}"));
            }
        };
    emitter.complete_phase(LoadingPhase::ImportConverting);

    tracing::info!(
        path = ?path,
        section_count = sections.len(),
        closure_count = closures.len(),
        purpose = "import Ariane TML/TMLU",
        "Converted TML to import configs, executing import"
    );

    let sections_found = tml.sections.len() as u32;
    let unit_conversion = tml_unit_conversion_note(tml.unit);

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

    emitter.start_phase(LoadingPhase::ImportApplyingChanges);
    let mut result = state.execute_tml_import_sections(
        sections,
        closures,
        file_hash,
        Vec::new(),
        Vec::new(),
        virtual_shot_mode.unwrap_or_default(),
        surface_elevation,
    );
    emitter.complete_phase(LoadingPhase::ImportApplyingChanges);

    decorate_receipt(
        &mut result,
        "Ariane TML",
        &path,
        sections_found,
        unit_conversion,
        converter_warnings,
    );

    emitter.start_phase(LoadingPhase::ImportFinalizing);
    emitter.complete_phase(LoadingPhase::ImportFinalizing);

    finalize_deferred_import(state.inner(), result)
}

/// Parse a Compass `.DAT`/`.MAK` file and return preview data for the import dialog.
#[tauri::command]
pub fn parse_compass(path: PathBuf) -> Result<CompassFile, String> {
    tracing::info!(
        path = ?path,
        purpose = "prepare Compass import preview",
        "Parsing Compass .DAT/.MAK file for import preview"
    );
    let parsed = crate::parsers::compass::parse_compass(&path).map_err(|e| {
        tracing::error!(
            path = ?path,
            purpose = "prepare Compass import preview",
            reason = %e,
            "Failed to parse Compass file for import preview"
        );
        e.to_string()
    })?;
    Ok(crate::parsers::compass::build_preview(&parsed))
}

/// Parse a Compass file, convert to import configs, and execute the import.
#[tauri::command]
pub fn execute_compass_import(
    app: AppHandle,
    state: State<'_, AppState>,
    path: PathBuf,
    file_hash: Option<String>,
    include_excluded: bool,
    apply_corrections: bool,
    comments_on_stations: bool,
) -> CommandResult {
    tracing::info!(
        path = ?path,
        purpose = "import Compass",
        "Executing Compass import"
    );

    let mut emitter = ProgressEmitter::new_for_import(tauri_sink(app));

    emitter.start_phase(LoadingPhase::ImportParsing);
    let parsed = match crate::parsers::compass::parse_compass(&path) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose = "import Compass",
                reason = %e,
                "Failed to parse Compass file for import"
            );
            return CommandResult::failure(format!("Failed to parse Compass file: {e}"));
        }
    };
    emitter.complete_phase(LoadingPhase::ImportParsing);

    emitter.start_phase(LoadingPhase::ImportConverting);
    let opts = CompassImportOptions {
        include_excluded,
        apply_corrections,
        comments_on_stations,
    };
    let (sections, closures, warnings) =
        match crate::parsers::compass::convert_compass_to_import_configs(&parsed, &opts) {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(
                    path = ?path,
                    purpose = "import Compass",
                    reason = %e,
                    "Failed to convert Compass data to import configs"
                );
                return CommandResult::failure(format!("Failed to convert Compass data: {e}"));
            }
        };
    emitter.complete_phase(LoadingPhase::ImportConverting);

    for warning in &warnings {
        tracing::warn!(purpose = "import Compass", "{warning}");
    }

    tracing::info!(
        path = ?path,
        section_count = sections.len(),
        closure_count = closures.len(),
        warning_count = warnings.len(),
        purpose = "import Compass",
        "Converted Compass to import configs, executing import"
    );

    let (sections_found, length_unit) = crate::parsers::compass::preview_scalars(&parsed);
    let unit_conversion = compass_unit_conversion_note(length_unit.as_deref());

    emitter.start_phase(LoadingPhase::ImportApplyingChanges);
    let mut result = state.execute_import_sections(
        sections,
        closures,
        file_hash,
        Vec::new(),
        Vec::new(),
        // Auto: no dialog on this path carries a per-file override, so the chain
        // rule decides. An Option parameter here was unreachable - no frontend
        // caller ever populated it - which read as "the override works" while
        // silently always meaning Auto.
        VirtualShotMode::Auto,
    );
    emitter.complete_phase(LoadingPhase::ImportApplyingChanges);

    decorate_receipt(
        &mut result,
        "Compass",
        &path,
        sections_found,
        unit_conversion,
        warnings,
    );

    emitter.start_phase(LoadingPhase::ImportFinalizing);
    emitter.complete_phase(LoadingPhase::ImportFinalizing);

    finalize_deferred_import(state.inner(), result)
}

/// Parse a GPX file and return preview data for the import dialog.
#[tauri::command]
pub fn parse_gpx(path: PathBuf) -> Result<GpxFile, String> {
    tracing::info!(
        path = ?path,
        purpose = "prepare GPX import preview",
        "Parsing GPX file for import preview"
    );
    let result = crate::parsers::gpx::parse_gpx(&path);
    if let Err(ref e) = result {
        tracing::error!(
            path = ?path,
            purpose = "prepare GPX import preview",
            reason = %e,
            "Failed to parse GPX file for import preview"
        );
    }
    result.map_err(|e| e.to_string())
}

/// Parse a GPX file, convert to import configs, and execute the import.
#[tauri::command]
pub fn execute_gpx_import(
    state: State<'_, AppState>,
    path: PathBuf,
    file_hash: Option<String>,
    import_waypoints: bool,
    import_tracks: bool,
    import_routes: bool,
    section_placements: Vec<GpxSectionPlacement>,
    depth_inverted: bool,
) -> CommandResult {
    tracing::info!(
        path = ?path,
        purpose = "import GPX",
        import_waypoints,
        import_tracks,
        import_routes,
        "Executing GPX import"
    );

    let gpx = match crate::parsers::gpx::parse_gpx(&path) {
        Ok(gpx) => gpx,
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose = "import GPX",
                reason = %e,
                "Failed to parse GPX file for import"
            );
            return CommandResult::failure(format!("Failed to parse GPX file: {e}"));
        }
    };

    let sections = match crate::parsers::gpx::convert_gpx_to_import_configs(
        &gpx,
        import_waypoints,
        import_tracks,
        import_routes,
        &section_placements,
        depth_inverted,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose = "import GPX",
                reason = %e,
                "Failed to convert GPX data to import configs"
            );
            return CommandResult::failure(format!("Failed to convert GPX data: {e}"));
        }
    };

    tracing::info!(
        path = ?path,
        section_count = sections.len(),
        purpose = "import GPX",
        "Converted GPX to import configs, executing import"
    );

    let sections_found = sections.len() as u32;

    // Auto, not a per-file override: the GPX dialog has no mode control, so
    // there is nothing to pass through - but the chain rule still has to run.
    // `parsers::gpx` seeds every leg as Reference on the promise that the
    // classifier can promote it, and skipping the pass here (this path used
    // to call `finalize_deferred_import` directly) left a GPS surface track
    // tying two sumps together permanently excluded from the survey graph.
    let mut result = state.execute_import_sections(
        sections,
        Vec::new(),
        file_hash,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );

    decorate_receipt(&mut result, "GPX", &path, sections_found, None, Vec::new());

    finalize_deferred_import(state.inner(), result)
}

/// Suggest placement positions for an import section by detecting reference shot overlap.
#[tauri::command]
pub fn suggest_import_placement(
    state: State<'_, AppState>,
    shots: Vec<ShotData>,
    // `azimuth_reference` is what `shots` are measured against (#438). The
    // caller has it on the `ImportSectionConfig` it is about to send; `None`
    // falls back to the project default, which is only right when the importer
    // declared nothing.
    azimuth_reference: Option<AzimuthReference>,
) -> Result<Vec<PlacementSuggestion>, String> {
    let project = state.get_project_state().ok_or("No project open")?;

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
}

/// Suggest placements for ALL import sections against BOTH existing project
/// sections AND each other (import-to-import). Returns a flat list sorted by score.
#[tauri::command]
pub fn suggest_all_import_placements(
    state: State<'_, AppState>,
    import_sections: Vec<ImportSectionForSuggestion>,
) -> Result<Vec<CrossSectionSuggestion>, String> {
    let existing_sections: Vec<ExistingSection> = match state.get_project_state() {
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
    let fallback_reference = state
        .get_project_state()
        .and_then(|p| p.default_azimuth_reference)
        .unwrap_or_default();
    let import_sections: Vec<ImportSectionForSuggestion> = import_sections
        .into_iter()
        .map(|mut section| {
            // Deserialized straight off the wire: re-impose the type's
            // invariants before anything computes with it.
            section.azimuth_reference = section.azimuth_reference.map(AzimuthReference::sanitised);
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
}

/// Suggest placements for orphaned sections' leaf ends against datum-anchored
/// sections. Orphaned sections are those in connected components (via merges)
/// with no zero datum; an orphan the import touched may go onto any anchored
/// section, a pre-existing one only onto a newly imported anchored section
/// (see `Project::suggest_orphan_placements`).
///
/// Async, with the matching on a blocking thread as `add_project_file` does:
/// an orphan from the import is matched against every anchored section of the
/// open project, which on a large survey is too long to hold the main thread.
#[tauri::command]
pub async fn suggest_orphan_placements<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    newly_imported_section_ids: Vec<u32>,
) -> Result<Vec<OrphanPlacementSuggestion>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let project = state.get_project_state().ok_or("No project open")?;
        let config = state.get_config();
        Ok(project
            .suggest_orphan_placements(&newly_imported_section_ids, &config.placement_suggestion))
    })
    .await
    .unwrap_or_else(|join_error| {
        Err(format!(
            "The placement suggestions did not finish: {join_error}"
        ))
    })
}

/// Count sections not connected to a zero datum, restricted to components
/// touched by `newly_imported_section_ids`. Used by the import dialogs to add
/// an info line to the import receipt when the orphan-placement pass produced
/// no suggestions (#446).
#[tauri::command]
pub fn count_unanchored_sections(
    state: State<'_, AppState>,
    newly_imported_section_ids: Vec<u32>,
) -> Result<u32, String> {
    let project = state.get_project_state().ok_or("No project open")?;
    Ok(project.count_unanchored_sections(&newly_imported_section_ids))
}

/// Parse a Walls `.WPJ`/`.SRV` file and return preview data for the import dialog.
#[tauri::command]
pub fn parse_walls(path: PathBuf) -> Result<WallsFile, String> {
    tracing::info!(
        path = ?path,
        purpose = "prepare Walls import preview",
        "Parsing Walls .WPJ/.SRV file for import preview"
    );
    let parsed = crate::parsers::walls::parse_walls(&path).map_err(|e| {
        tracing::error!(
            path = ?path,
            purpose = "prepare Walls import preview",
            reason = %e,
            "Failed to parse Walls file for import preview"
        );
        e.to_string()
    })?;
    Ok(crate::parsers::walls::build_preview(&parsed))
}

/// Parse a Walls file, convert to import configs, and execute the import.
#[tauri::command]
pub fn execute_walls_import(
    app: AppHandle,
    state: State<'_, AppState>,
    path: PathBuf,
    file_hash: Option<String>,
    comments_on_stations: bool,
    include_splays: bool,
    selected_surveys: Option<Vec<u32>>,
) -> CommandResult {
    tracing::info!(
        path = ?path,
        purpose = "import Walls",
        "Executing Walls import"
    );

    let mut emitter = ProgressEmitter::new_for_import(tauri_sink(app));

    emitter.start_phase(LoadingPhase::ImportParsing);
    let parsed = match crate::parsers::walls::parse_walls(&path) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose = "import Walls",
                reason = %e,
                "Failed to parse Walls file for import"
            );
            return CommandResult::failure(format!("Failed to parse Walls file: {e}"));
        }
    };
    emitter.complete_phase(LoadingPhase::ImportParsing);

    emitter.start_phase(LoadingPhase::ImportConverting);
    let opts = WallsImportOptions {
        comments_on_stations,
        include_splays,
        // `None` ⇒ import every survey; `Some(list)` ⇒ only the chosen survey
        // indices (into the preview's `WallsFile::surveys`, which is 1:1 with
        // the parser's survey order).
        selected_surveys: selected_surveys.map(|v| v.into_iter().collect()),
    };
    let (sections, closures, warnings) =
        match crate::parsers::walls::convert_walls_to_import_configs(&parsed, &opts) {
            Ok(result) => result,
            Err(e) => {
                tracing::error!(
                    path = ?path,
                    purpose = "import Walls",
                    reason = %e,
                    "Failed to convert Walls data to import configs"
                );
                return CommandResult::failure(format!("Failed to convert Walls data: {e}"));
            }
        };
    emitter.complete_phase(LoadingPhase::ImportConverting);

    for warning in &warnings {
        tracing::warn!(purpose = "import Walls", "{warning}");
    }

    tracing::info!(
        path = ?path,
        section_count = sections.len(),
        closure_count = closures.len(),
        warning_count = warnings.len(),
        purpose = "import Walls",
        "Converted Walls to import configs, executing import"
    );

    let (sections_found, any_feet_units) = crate::parsers::walls::preview_scalars(&parsed);
    let unit_conversion = walls_unit_conversion_note(any_feet_units);

    emitter.start_phase(LoadingPhase::ImportApplyingChanges);
    let mut result = state.execute_import_sections(
        sections,
        closures,
        file_hash,
        Vec::new(),
        Vec::new(),
        // Auto: no dialog on this path carries a per-file override, so the chain
        // rule decides. An Option parameter here was unreachable - no frontend
        // caller ever populated it - which read as "the override works" while
        // silently always meaning Auto.
        VirtualShotMode::Auto,
    );
    emitter.complete_phase(LoadingPhase::ImportApplyingChanges);

    decorate_receipt(
        &mut result,
        "Walls",
        &path,
        sections_found,
        unit_conversion,
        warnings,
    );

    emitter.start_phase(LoadingPhase::ImportFinalizing);
    emitter.complete_phase(LoadingPhase::ImportFinalizing);

    finalize_deferred_import(state.inner(), result)
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod receipt_tests {
    use super::*;
    use crate::core::command::ImportReceipt;
    use crate::core::{ValidationWarning, WarningSeverity};
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
}
