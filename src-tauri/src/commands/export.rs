use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::commands::event_sink::tauri_sink;
use crate::config::GasConfig;
use crate::core::loading::{LoadingPhase, ProgressEmitter};
use crate::core::state::export_views::ExportViews;
use crate::core::AppState;
use crate::export::color_scale::{ExportValueKind, ValueHistogram};
use crate::export::{ExportImageOptions, ProfileFormat};
use crate::models::survey::Project;
use crate::models::{Cylinder, Diver, PlanSegment, StationId, StationRef};

/// True when `target` resolves to the same file on disk as the open project's
/// `.stalx` at `existing`. Every export replaces its target wholesale
/// (`write_atomic`), so exporting over the open project file would destroy
/// it. `existing` always exists (it is the open file); `target` may not exist
/// yet (a fresh export destination), so when it can't be canonicalized
/// directly, compare its canonicalized parent directory + file name against
/// the canonicalized project path. Errs toward catching the exact open-file
/// path rather than rejecting unrelated destinations.
fn is_open_project_file(target: &Path, existing: &Path) -> bool {
    let Ok(existing_canon) = std::fs::canonicalize(existing) else {
        return false;
    };
    // Fast path: `target` exists and canonicalizes to the same file.
    if std::fs::canonicalize(target).is_ok_and(|t| t == existing_canon) {
        return true;
    }
    // `target` may not exist yet: canonicalize its parent (which does) and
    // rejoin the file name.
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return false;
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    std::fs::canonicalize(parent).is_ok_and(|p| p.join(name) == existing_canon)
}

/// True when `target` is a file the OPEN PROJECT owns - either its `.stalx`, or the `.plnx`
/// dive-plan store that lives beside it.
///
/// The `.stalx` half has always been guarded. The `.plnx` was not, and it holds every saved dive
/// plan for that project: an export written over it destroys all of them, which for a dive-profile
/// export means destroying the very plans the feature exists to publish. It is not reachable by
/// picking `cave.plnx` in the save dialog (the frontend forces the extension), but the IPC boundary
/// takes an arbitrary path from the webview, and that is the boundary this guard is for.
///
/// The sibling is derived exactly as `AppState::plnx_context` derives it - same directory, same
/// stem, `.plnx` extension - which `with_extension` expresses in one call. A `.plnx` that does not
/// exist yet canonicalizes to nothing and is correctly not refused.
fn is_project_owned_file(target: &Path, project: &Project) -> bool {
    let Some(open_path) = project.file_path.as_deref() else {
        return false;
    };
    is_open_project_file(target, open_path)
        || is_open_project_file(target, &open_path.with_extension("plnx"))
}

/// The refusal every export answers for a target the open project owns.
const OWNED_FILE_REFUSAL: &str = "Cannot export over the open project file";

/// Generic helper that handles the shared export logic for all text-based formats.
fn export_format_impl(
    state: &AppState,
    path: PathBuf,
    format: &str,
    export_fn: fn(&Project, &Path) -> Result<(), anyhow::Error>,
) -> Result<(), String> {
    export_format_with_notes(state, path, format, move |project, path| {
        export_fn(project, path).map(|()| Vec::new())
    })
    .map(|_| ())
}

/// [`export_format_impl`] for an exporter that has something to TELL the user
/// about the document it just wrote (#438: the Ariane azimuth compromises).
///
/// Notes are not warnings and not errors: the export succeeded and the file is
/// good. They exist because the export is the only moment the user is looking
/// at this decision, and the alternative - a `tracing::warn` nobody reads - let
/// a file go out claiming something about its bearings that the project does
/// not say. The frontend raises one toast per note.
///
/// The exporter runs on a snapshot of the open survey (one clone, outside the
/// project lock, so a large export never stalls an edit).
fn export_format_with_notes(
    state: &AppState,
    path: PathBuf,
    format: &str,
    export_fn: impl FnOnce(&Project, &Path) -> Result<Vec<String>, anyhow::Error>,
) -> Result<Vec<String>, String> {
    let project = state.get_project_state().ok_or_else(|| {
        tracing::warn!(
            path = ?path,
            purpose = "export project for sharing",
            format = format,
            "Export requested with no project open"
        );
        "No project open".to_string()
    })?;

    // Never clobber a file the open project owns (same guard as the image export): a text export
    // named to the open project file would overwrite it with CSV/JSON/TML, and one named to the
    // `.plnx` sibling would take every saved dive plan with it.
    if is_project_owned_file(&path, &project) {
        tracing::warn!(
            path = ?path,
            open_project = ?project.file_path,
            format = format,
            "Refused to export over a file the open project owns"
        );
        return Err(OWNED_FILE_REFUSAL.to_string());
    }

    tracing::info!(
        path = ?path,
        purpose = "export project for sharing",
        format = format,
        "Exporting project to {}", format.to_uppercase()
    );
    match export_fn(&project, &path) {
        Ok(notes) => {
            tracing::info!(
                path = ?path,
                purpose = "export project for sharing",
                format = format,
                notes = notes.len(),
                "{} export completed", format.to_uppercase()
            );
            Ok(notes)
        }
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose = "export project for sharing",
                format = format,
                reason = %format_args!("{e:#}"),
                "{} export failed", format.to_uppercase()
            );
            Err(format!("{e:#}"))
        }
    }
}

/// Export project data to CSV
#[tauri::command]
pub fn export_csv(state: State<AppState>, path: PathBuf) -> Result<(), String> {
    export_csv_impl(state.inner(), path)
}

pub(crate) fn export_csv_impl(state: &AppState, path: PathBuf) -> Result<(), String> {
    export_format_impl(state, path, "csv", crate::export::survey::export_csv)
}

/// Export project data to JSON
#[tauri::command]
pub fn export_json(state: State<AppState>, path: PathBuf) -> Result<(), String> {
    export_json_impl(state.inner(), path)
}

pub(crate) fn export_json_impl(state: &AppState, path: PathBuf) -> Result<(), String> {
    export_format_impl(state, path, "json", crate::export::survey::export_json)
}

/// Export project data to Ariane TML/TMLU.
///
/// Returns the export NOTES (#438): what the single-flag `.tml` format could
/// not carry about this project's north references. Empty for an ordinary
/// project; the frontend toasts each one.
#[tauri::command]
pub fn export_tml(state: State<AppState>, path: PathBuf) -> Result<Vec<String>, String> {
    export_tml_impl(state.inner(), path)
}

pub(crate) fn export_tml_impl(state: &AppState, path: PathBuf) -> Result<Vec<String>, String> {
    export_format_with_notes(state, path, "tml", crate::export::survey::export_tml)
}

/// The open project and its views for an image (or its preview, or its
/// Levels histogram), with the reference-leg flag taken from the live config.
///
/// Server-populated (#27): the export must colour its distance ramp and label
/// its penetration with the same graph the app is showing, so the flag comes
/// from the config, never from whatever the frontend sent.
fn image_views(state: &AppState, options: &mut ExportImageOptions) -> Result<ExportViews, String> {
    options.include_reference_legs = state.get_config().gas.include_reference_legs;
    // The memoised Closures Computed View, so the image matches the on-screen
    // (adjusted) map; where nothing is adjusted it is the measured layout.
    state
        .export_views()
        .ok_or_else(|| "No project open".to_string())
}

/// Export map image with progress events via shared loading-progress infrastructure.
///
/// This command runs heavy export work (the Computed View solve on a cache miss, SVG
/// generation, PNG rasterization, PDF conversion) in a blocking thread pool to prevent freezing
/// the UI and ensure progress events propagate. Progress is emitted using the shared
/// ProgressEmitter (loading-progress events) for integration with the LoadingProgress component.
#[tauri::command]
pub async fn export_image(
    app_handle: AppHandle,
    path: PathBuf,
    options: ExportImageOptions,
) -> Result<(), String> {
    let format = options.format;
    let log_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let sink = tauri_sink(app_handle.clone());
        write_image_export(state.inner(), &path, options, || {
            ProgressEmitter::new_for_export(sink)
        })
    })
    .await
    .map_err(|e| {
        tracing::error!(
            path = ?log_path,
            purpose = "export map image for sharing",
            format = ?format,
            reason = %e,
            "Image export task panicked"
        );
        format!("Export task panicked: {e}")
    })?
}

/// [`export_image`] without progress events (the IPC-level tests).
#[cfg(test)]
pub(crate) fn export_image_impl(
    state: &AppState,
    path: PathBuf,
    options: ExportImageOptions,
) -> Result<(), String> {
    write_image_export(state, &path, options, || None::<ProgressEmitter>)
}

/// The image export: the views, the never-clobber guard, then the render and
/// the write, with the export's progress phases on the emitter `progress`
/// makes once the checks have passed (a refused export emits nothing).
fn write_image_export<E: Into<Option<ProgressEmitter>>>(
    state: &AppState,
    path: &Path,
    mut options: ExportImageOptions,
    progress: impl FnOnce() -> E,
) -> Result<(), String> {
    let views = image_views(state, &mut options).inspect_err(|_| {
        tracing::warn!(
            path = ?path,
            purpose = "export map image for sharing",
            format = ?options.format,
            "Export requested with no project open"
        );
    })?;

    // Never let an image export clobber a file the open project owns: the export renamed into
    // place would replace the project file, or the `.plnx` sibling holding every saved dive plan.
    if is_project_owned_file(path, &views.project) {
        tracing::warn!(
            path = ?path,
            open_project = ?views.project.file_path,
            purpose = "protect the open project file",
            "Refusing to export an image over a file the open project owns"
        );
        return Err(OWNED_FILE_REFUSAL.to_string());
    }

    tracing::info!(
        path = ?path,
        purpose = "export map image for sharing",
        format = ?options.format,
        "Starting image export"
    );
    let mut emitter: Option<ProgressEmitter> = progress().into();
    if let Some(emitter) = emitter.as_mut() {
        emitter.start_phase(LoadingPhase::ExportPreparing);
        emitter.start_phase(LoadingPhase::ExportGenerating);
    }
    let result = crate::export::survey::export_image_with_positions(
        &views.project,
        path,
        &options,
        views.adjusted.as_ref(),
        &views.raw,
    );
    match &result {
        Ok(()) => {
            if let Some(emitter) = emitter.as_mut() {
                emitter.start_phase(LoadingPhase::ExportWriting);
                emitter.complete_phase(LoadingPhase::ExportWriting);
                emitter.start_phase(LoadingPhase::ExportComplete);
                emitter.complete_phase(LoadingPhase::ExportComplete);
            }
            tracing::info!(
                path = ?path,
                purpose = "export map image for sharing",
                format = ?options.format,
                "Image export completed"
            );
        }
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose = "export map image for sharing",
                format = ?options.format,
                reason = %format_args!("{e:#}"),
                "Image export failed"
            );
        }
    }
    result.map_err(|e| format!("{e:#}"))
}

/// The plan inputs a dive-profile export needs - the same tuple `compute_gas_reach_field` takes,
/// and put through the same `validate_plan_inputs` before it reaches the engine.
///
/// The field is recomputed from these rather than accepted from the frontend. The exported file is
/// a document the diver may act on months later, so it has to be what the engine says *now*, not a
/// snapshot the UI happened to be holding: a client-supplied field could be stale (every plan edit
/// nulls it), or simply not match the plan on screen.
#[derive(serde::Deserialize, TS)]
pub struct DiveProfileRequest {
    pub origin: Option<StationRef>,
    pub endpoint: Option<StationRef>,
    pub waypoints: Vec<StationRef>,
    /// Per-span speed + propulsion (#317). Carried so the exported profile is priced at the same
    /// speeds the plan on screen is - a profile rendered at the config swim speed for a scootered
    /// dive would understate runtime and overstate gas, in a document the diver acts on later.
    pub segments: Vec<PlanSegment>,
    pub diver: Diver,
    pub cylinders: Vec<Cylinder>,
    pub gas_config: GasConfig,
    pub stage_drop_overrides: Vec<Option<StationRef>>,
}

/// Recompute the reach for `request` and render its profile in `format`.
///
/// Runs `validate_plan_inputs` and `validate_plan_segments` exactly as `compute_gas_reach_field`
/// does. Skipping them would make
/// this the one entry point where a NaN RMV or an uncapped way-point list reaches the engine - and
/// unlike the compute, which would just show nothing, this path renders a document the diver
/// carries underwater.
///
/// The slots resolve to stations exactly as the compute does (`gas_reach_field_wire`: a slot that
/// names no station is `None`, which the engine treats as the compute does), so the profile is the
/// route on screen. Takes the project by reference so ONE snapshot serves the whole command: the
/// name, the clobber guard and the route all come from the same project state.
fn render_profile_for(
    project: &Project,
    request: DiveProfileRequest,
    format: ProfileFormat,
) -> Result<String, String> {
    let (diver, cylinders, gas_config) = crate::commands::gas::validate_plan_inputs(
        &request.waypoints,
        request.diver,
        request.cylinders,
        request.gas_config,
    )?;
    crate::commands::gas::validate_plan_segments(&request.segments, request.waypoints.len())?;
    let station = |slot: &StationRef| project.station_at(*slot);
    let waypoints: Vec<Option<StationId>> = request.waypoints.iter().map(station).collect();
    let overrides: Vec<Option<StationId>> = request
        .stage_drop_overrides
        .iter()
        .map(|slot| slot.as_ref().and_then(station))
        .collect();
    let reach = project.gas_reach(
        request.origin.as_ref().and_then(station),
        request.endpoint.as_ref().and_then(station),
        &waypoints,
        &request.segments,
        &diver,
        &cylinders,
        &gas_config,
        &overrides,
    );
    crate::export::survey::render_dive_profile(
        project,
        &reach,
        &request.waypoints,
        format,
        &crate::export::ProfileContext {
            project_name: &project.name,
            units: gas_config.units,
            endpoint: request.endpoint,
        },
    )
    .map_err(|e| e.to_string())
}

/// Write the planned dive profile to `path` (#299).
///
/// `format` is `"subsurface-xml"` or `"text"`. Reuses the same no-project and never-clobber-the-open-
/// `.stalx` guards the other exports run under.
#[tauri::command]
pub async fn export_dive_profile(
    app_handle: AppHandle,
    path: PathBuf,
    format: String,
    request: DiveProfileRequest,
) -> Result<(), String> {
    // Parse BEFORE anything else logs it. `format` is arbitrary caller input, and the refusal
    // branch records it with Display - a newline-bearing value would forge log records.
    let format = ProfileFormat::from_wire(&format).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        export_dive_profile_impl(state.inner(), &path, format, request)
    })
    .await
    .map_err(|e| format!("Dive profile export task panicked: {e}"))?
}

fn export_dive_profile_impl(
    state: &AppState,
    path: &Path,
    format: ProfileFormat,
    request: DiveProfileRequest,
) -> Result<(), String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;
    let refuse = |path: &Path| {
        tracing::warn!(
            path = ?path,
            open_project = ?project.file_path,
            format = format.label(),
            "Refused to export a dive profile over a file the open project owns"
        );
        OWNED_FILE_REFUSAL.to_string()
    };
    if is_project_owned_file(path, &project) {
        return Err(refuse(path));
    }
    let rendered = render_profile_for(&project, request, format)?;
    tracing::info!(
        path = ?path,
        purpose = "export the planned dive profile",
        format = format.label(),
        "Exporting dive profile"
    );
    // Re-check immediately before the write. Rendering runs the whole multi-Dijkstra compute, so
    // unlike the sibling exports - which check and write adjacently - this path would otherwise
    // leave a wide window in which `path` could be swapped for a symlink to the open project.
    if is_project_owned_file(path, &project) {
        return Err(refuse(path));
    }
    crate::export::write_atomic(path, |w| {
        std::io::Write::write_all(w, rendered.as_bytes())?;
        Ok(())
    })
    .map_err(|e| {
        tracing::error!(path = ?path, reason = %e, "Dive profile export failed");
        format!("{e:#}")
    })?;
    tracing::info!(
        path = ?path,
        format = format.label(),
        "Dive profile export completed"
    );
    Ok(())
}

/// Render the planned dive profile as text WITHOUT writing it, for the clipboard (#299).
#[tauri::command]
pub async fn render_dive_profile_text(
    app_handle: AppHandle,
    request: DiveProfileRequest,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        render_dive_profile_text_impl(state.inner(), request)
    })
    .await
    .map_err(|e| format!("Dive profile render task panicked: {e}"))?
}

fn render_dive_profile_text_impl(
    state: &AppState,
    request: DiveProfileRequest,
) -> Result<String, String> {
    let project = state
        .get_project_state()
        .ok_or_else(|| "No project open".to_string())?;
    render_profile_for(&project, request, ProfileFormat::PlainText)
}

/// Generate the SVG markup that `export_image` would write, without
/// touching the filesystem. Used by the export dialog so the preview is
/// pixel-for-pixel identical to what export will produce.
///
/// Runs the (potentially heavy: the Computed View solve on a cache miss,
/// bounds + Dijkstra + polyline emission) work in `spawn_blocking` so the
/// slider can drag without stalling the Tauri main thread on large surveys -
/// mirroring `export_image`.
#[tauri::command]
pub async fn preview_svg_export(
    app_handle: AppHandle,
    options: ExportImageOptions,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        preview_svg_export_impl(state.inner(), options)
    })
    .await
    .map_err(|e| format!("Preview task panicked: {e}"))?
}

fn preview_svg_export_impl(
    state: &AppState,
    mut options: ExportImageOptions,
) -> Result<String, String> {
    // Same views and config-wins rule as the export itself, so the preview
    // cannot disagree with the file it is previewing (#27).
    let views = image_views(state, &mut options)?;
    crate::export::survey::generate_svg_string_with_positions(
        &views.project,
        &options,
        views.adjusted.as_ref(),
        &views.raw,
    )
    .map_err(|e| e.to_string())
}

/// Bin the values an image export would colour for `kind` (depth, or
/// penetration distance from the entrance) over the export's own auto range,
/// for the Export panel's Levels slider. Uses exactly the export's data: the
/// Computed View the image draws (depth), the measured network (distance).
#[tauri::command]
pub async fn export_value_histogram(
    app_handle: AppHandle,
    kind: ExportValueKind,
    bins: u32,
) -> Result<ValueHistogram, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        export_value_histogram_impl(state.inner(), kind, bins)
    })
    .await
    .map_err(|e| format!("Histogram task panicked: {e}"))?
}

pub(crate) fn export_value_histogram_impl(
    state: &AppState,
    kind: ExportValueKind,
    bins: u32,
) -> Result<ValueHistogram, String> {
    let mut options = ExportImageOptions::default();
    let views = image_views(state, &mut options)?;
    Ok(crate::export::survey::export_value_histogram(
        &views.project,
        views.adjusted.as_ref(),
        &views.raw,
        kind,
        bins as usize,
        options.include_reference_legs,
    ))
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn is_open_project_file_matches_same_path_and_rejects_others() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("cave.stalx");
        std::fs::write(&project, b"db").unwrap();

        // The exact open-project path (both exist) is caught.
        assert!(is_open_project_file(&project, &project));

        // A different, not-yet-existing export target in the same dir is allowed.
        let other = dir.path().join("export.svg");
        assert!(!is_open_project_file(&other, &project));

        // A not-yet-existing target that still resolves to the project file
        // (parent canonicalizes, file name matches) is caught via the fallback.
        std::fs::remove_file(&project).unwrap();
        let recreated = dir.path().join("cave.stalx");
        std::fs::write(&recreated, b"db").unwrap();
        let absent_same = dir.path().join("cave.stalx");
        assert!(is_open_project_file(&absent_same, &recreated));
    }

    /// The shared validator rejects the inputs the compute command rejects.
    ///
    /// ⚠ This tests the HELPER, not the export path's use of it. Named for what it shows: an
    /// earlier version claimed to prove "the export path rejects..." while never calling the export
    /// path, so deleting `validate_plan_inputs` from `render_profile_for` left it green. The wiring
    /// is covered by `the_export_path_actually_runs_the_validator` below - keep both.
    #[test]
    fn the_shared_plan_validator_rejects_bad_inputs() {
        use crate::commands::gas::validate_plan_inputs;
        use crate::core::state::dive_plan::MAX_WAYPOINTS;
        use crate::models::{CylRole, TankMaterial};

        let good_diver = Diver::new(20.0, None).unwrap();
        let cyl = Cylinder::new(
            12.0,
            232.0,
            200.0,
            TankMaterial::Steel,
            CylRole::BackGas,
            None,
        )
        .unwrap();
        let ok = |wp: &[StationRef], d: Diver| {
            validate_plan_inputs(wp, d, vec![cyl.clone()], GasConfig::default())
        };

        assert!(ok(&[], good_diver.clone()).is_ok());

        // Over the cap: each way point is a full extra Dijkstra, so an unbounded list is unbounded
        // compute on a blocking thread.
        let too_many = vec![StationRef::new(1, 0); MAX_WAYPOINTS + 1];
        let err = ok(&too_many, good_diver.clone()).unwrap_err();
        assert!(err.contains("Too many way points"), "{err}");
        assert!(ok(&vec![StationRef::new(1, 0); MAX_WAYPOINTS], good_diver).is_ok());

        // A NaN/zero RMV built straight by serde, bypassing `Diver::new`'s guard.
        let poisoned = Diver {
            rmv_l_per_min: f64::NAN,
            label: None,
        };
        assert!(ok(&[], poisoned).is_err());
        let zero = Diver {
            rmv_l_per_min: 0.0,
            label: None,
        };
        assert!(ok(&[], zero).is_err());
    }

    fn request(waypoints: Vec<StationRef>) -> DiveProfileRequest {
        DiveProfileRequest {
            origin: None,
            endpoint: None,
            waypoints,
            segments: Vec::new(),
            diver: Diver {
                rmv_l_per_min: 20.0,
                label: None,
            },
            cylinders: Vec::new(),
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
        }
    }

    /// The export path must actually CALL the validator, not merely have one available.
    ///
    /// `render_profile_for` is a second door into the gas engine, and the engine validates nothing -
    /// its contract says the COMMAND owns that. This is the test that fails if the call is dropped:
    /// an over-cap way-point list must come back as the CAP error. Without the validator the same
    /// request reaches the engine and fails later with the no-route message instead, so the two
    /// outcomes are distinguishable and the assertion cannot pass vacuously.
    ///
    /// Reachable as a unit test only because `render_profile_for` takes a `&Project` rather than an
    /// `AppHandle` - an empty survey is enough, since validation runs before any survey data is
    /// touched.
    #[test]
    fn the_export_path_actually_runs_the_validator() {
        use crate::core::state::dive_plan::MAX_WAYPOINTS;

        let project = Project::new("Cave", None);

        let err = render_profile_for(
            &project,
            request(vec![StationRef::new(1, 0); MAX_WAYPOINTS + 1]),
            ProfileFormat::PlainText,
        )
        .unwrap_err();
        assert!(
            err.contains("Too many way points"),
            "the export path skipped validate_plan_inputs: {err}"
        );

        // Same wiring proof for the segment cap - the second validator this door owes. Without the
        // call an over-long list is inert in the engine (it only reads waypoints+1 spans), so the
        // request would fail with the no-route message instead - distinguishable, not vacuous.
        let over_segments = DiveProfileRequest {
            segments: vec![
                PlanSegment {
                    speed_m_per_min: None,
                    on_scooter: false,
                };
                crate::core::state::dive_plan::MAX_SEGMENTS + 1
            ],
            ..request(Vec::new())
        };
        let err =
            render_profile_for(&project, over_segments, ProfileFormat::PlainText).unwrap_err();
        assert!(
            err.contains("Too many plan segments"),
            "the export path skipped validate_plan_segments: {err}"
        );

        // The control: an in-cap plan gets PAST validation and fails for the other reason, proving
        // the assertion above discriminates rather than matching any error at all.
        let err = render_profile_for(&project, request(Vec::new()), ProfileFormat::PlainText)
            .unwrap_err();
        assert!(err.contains("no route to export"), "{err}");
    }

    /// An empty project cannot produce an exportable profile, whichever door is used.
    ///
    /// The degenerate shape this pins is a route of ONE station: the engine picks the origin as its
    /// own turnaround whenever only the origin is in budget, and a one-point series renders as a
    /// dive of zero distance over zero duration - a syntactically valid Subsurface file for a dive
    /// that never happened.
    #[test]
    fn a_plan_with_no_swimmable_leg_is_refused_by_both_formats() {
        let project = Project::new("Cave", None);
        for format in [ProfileFormat::PlainText, ProfileFormat::SubsurfaceXml] {
            let err = render_profile_for(&project, request(Vec::new()), format).unwrap_err();
            assert!(err.contains("no route to export"), "{format:?}: {err}");
        }
    }
}

/// Every export command on a v2 project opened from its file: each writes
/// what `export::survey` writes for the open project, images and the Levels
/// histogram from the memoised Closures Computed View, and each refuses a
/// file the open project owns and a state with no project.
#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod v2_tests {
    use super::*;
    use crate::export::color_scale::ExportValueKind;
    use crate::export::ImageFormat;
    use crate::models::survey::{ClosureStrategy, LineMeta, StartAt, ZeroDatum};
    use crate::models::{CylRole, GpsCoordinates, LineId, ShotData, TankMaterial};
    use crate::test_helpers::{app_state_with_project_db, TestDb};
    use tempfile::TempDir;

    fn shot(length: f64, azimuth: f64, depth_in: f64, depth_out: f64) -> ShotData {
        ShotData {
            length,
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            depth_in,
            depth_out,
            ..ShotData::default()
        }
    }

    fn line_meta(name: &str) -> LineMeta {
        LineMeta {
            name: Some(name.to_string()),
            color: "#336699".to_string(),
            ..LineMeta::default()
        }
    }

    /// "Export Cave": Main, a square loop surveyed with a 1 m misclosure and
    /// closed by a Batch closure (so the Computed View moves it), its start
    /// held by a datum; Branch starts on Main's station 1, a junction with a
    /// station comment.
    fn cave() -> (Project, LineId) {
        let mut p = Project::new("Export Cave", None);
        let main = p
            .add_line(
                line_meta("Main"),
                StartAt::New,
                vec![
                    shot(10.0, 0.0, 1.0, 2.0),
                    shot(10.0, 90.0, 2.0, 3.0),
                    shot(10.0, 180.0, 3.0, 2.5),
                    shot(9.0, 270.0, 2.5, 1.0),
                ],
            )
            .unwrap();
        let chain: Vec<StationId> = p.line(main).unwrap().stations().collect();
        p.add_line(
            line_meta("Branch"),
            StartAt::Station(chain[1]),
            vec![shot(5.0, 45.0, 2.0, 4.0), shot(5.0, 45.0, 4.0, 6.0)],
        )
        .unwrap();
        p.datums.push(ZeroDatum {
            via: Some(main),
            ..ZeroDatum::new(chain[0], GpsCoordinates::new(20.5, -87.25, 0.0).unwrap())
        });
        p.push_closure(
            chain[4],
            Some(main),
            chain[0],
            Some(main),
            ClosureStrategy::Batch,
        );
        p.station_mut(chain[1]).unwrap().comment = Some("junction".to_string());
        assert!(p.validate().is_empty(), "{:?}", p.validate());
        (p, main)
    }

    /// The cave written as a v2 `.stalx` and opened from it, with the project
    /// as the app holds it.
    fn opened() -> (AppState, TestDb, Project, LineId) {
        let (project, main) = cave();
        let (state, db) = app_state_with_project_db(project).unwrap();
        let open = state.get_project_state().unwrap();
        assert_eq!(
            open.file_path.as_deref(),
            Some(db.path()),
            "opened from its file"
        );
        (state, db, open, main)
    }

    fn svg_options() -> ExportImageOptions {
        ExportImageOptions {
            format: ImageFormat::Svg,
            ..ExportImageOptions::default()
        }
    }

    /// The two views the image commands must draw: the open project's
    /// Computed View under the live config, and its measured layout.
    fn views_of(state: &AppState, open: &Project) -> (Arc<AdjustedPositions>, RawPositions) {
        let config = state.get_config();
        (
            Arc::new(open.adjusted_positions(&config.closure_adjustment)),
            open.raw_positions(),
        )
    }

    use crate::models::survey::{AdjustedPositions, RawPositions};
    use std::sync::Arc;

    fn plan(main: LineId, waypoints: Vec<StationRef>) -> DiveProfileRequest {
        DiveProfileRequest {
            origin: Some(StationRef::new(main, 0)),
            endpoint: Some(StationRef::new(main, 3)),
            waypoints,
            segments: Vec::new(),
            diver: Diver::new(20.0, None).unwrap(),
            cylinders: vec![Cylinder::new(
                11.1,
                207.0,
                200.0,
                TankMaterial::Aluminium,
                CylRole::BackGas,
                None,
            )
            .unwrap()],
            gas_config: GasConfig::default(),
            stage_drop_overrides: Vec::new(),
        }
    }

    #[test]
    fn csv_writes_the_survey_exporters_rows_of_the_open_project() {
        let (state, _db, open, _) = opened();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cave.csv");
        export_csv_impl(&state, path.clone()).unwrap();
        let mut expected = Vec::new();
        crate::export::survey::write_csv(&open, &mut expected).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        // The junction is one station: its Station ID ends Main's first row
        // and starts Branch's (declared change, plan 12.2). Columns: line,
        // from, to, from_id, to_id, ...
        let junction = open.lines()[0].shots[0].to.to_string();
        let text = String::from_utf8(expected).unwrap();
        let ids: Vec<(&str, &str, &str)> = text
            .lines()
            .skip(1)
            .map(|row| {
                let fields: Vec<&str> = row.split(',').collect();
                (fields[0], fields[3], fields[4])
            })
            .collect();
        let start = open.lines()[0].start.to_string();
        assert_eq!(
            ids[0],
            ("Main", start.as_str(), junction.as_str()),
            "{text}"
        );
        assert_eq!(ids[4].0, "Branch", "{text}");
        assert_eq!(ids[4].1, junction, "Branch starts on the junction: {text}");
    }

    #[test]
    fn json_writes_the_survey_exporters_records_of_the_open_project() {
        let (state, _db, open, _) = opened();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cave.json");
        export_json_impl(&state, path.clone()).unwrap();
        let written = std::fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&written).unwrap();
        assert_eq!(value["project"]["name"], "Export Cave");
        let at =
            chrono::DateTime::parse_from_rfc3339(value["project"]["exported_at"].as_str().unwrap())
                .unwrap()
                .with_timezone(&chrono::Utc);
        let mut expected = Vec::new();
        crate::export::survey::write_json(&open, at, &mut expected).unwrap();
        assert_eq!(written, expected);
    }

    #[test]
    fn tml_writes_the_survey_exporters_document_of_the_open_project() {
        let (state, _db, open, _) = opened();
        let dir = TempDir::new().unwrap();
        for name in ["cave.tmlu", "cave.tml"] {
            let path = dir.path().join(name);
            let notes = export_tml_impl(&state, path.clone()).unwrap();
            assert!(
                notes.is_empty(),
                "an ordinary project has no notes: {notes:?}"
            );
            let reference = dir.path().join(format!("reference-{name}"));
            crate::export::survey::export_tml(&open, &reference).unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                std::fs::read(&reference).unwrap(),
                "{name}"
            );
        }
        // Ariane ids are Station IDs (plan X1): the START is Main's start.
        let text = std::fs::read_to_string(dir.path().join("cave.tmlu")).unwrap();
        let start = open.lines()[0].start;
        assert!(text.contains(&format!("<ID>{start}</ID>")), "{text}");
    }

    #[test]
    fn the_image_and_its_preview_draw_the_memoised_computed_view() {
        let (state, _db, open, _) = opened();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cave.svg");
        // The frontend's reference-leg flag is ignored: the config's wins (#27).
        let include = state.get_config().gas.include_reference_legs;
        let sent = ExportImageOptions {
            include_reference_legs: !include,
            ..svg_options()
        };
        export_image_impl(&state, path.clone(), sent.clone()).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        let preview = preview_svg_export_impl(&state, sent).unwrap();
        assert_eq!(preview, written, "the preview is the file");

        let (adjusted, raw) = views_of(&state, &open);
        let options = ExportImageOptions {
            include_reference_legs: include,
            ..svg_options()
        };
        let expected = crate::export::survey::generate_svg_string_with_positions(
            &open,
            &options,
            adjusted.as_ref(),
            &raw,
        )
        .unwrap();
        assert_eq!(written, expected);
        let measured = crate::export::survey::generate_svg_string(&open, &options).unwrap();
        assert_ne!(
            written, measured,
            "the loop is adjusted, so the image is not the measured layout"
        );

        // The raster writer runs from the same views.
        let png = dir.path().join("cave.png");
        let options = ExportImageOptions {
            format: ImageFormat::Png,
            dpi: 72,
            ..ExportImageOptions::default()
        };
        export_image_impl(&state, png.clone(), options).unwrap();
        assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
    }

    #[test]
    fn the_levels_histogram_bins_the_computed_view() {
        let (state, _db, open, _) = opened();
        let (adjusted, raw) = views_of(&state, &open);
        let include = state.get_config().gas.include_reference_legs;
        for (kind, bins) in [(ExportValueKind::Depth, 16), (ExportValueKind::Distance, 4)] {
            let got = export_value_histogram_impl(&state, kind, bins).unwrap();
            let expected = crate::export::survey::export_value_histogram(
                &open,
                adjusted.as_ref(),
                &raw,
                kind,
                bins as usize,
                include,
            );
            assert_eq!(got, expected, "{kind:?}");
            assert_eq!(got.counts.len(), bins as usize);
        }
        let depth = export_value_histogram_impl(&state, ExportValueKind::Depth, 16).unwrap();
        assert_eq!(
            depth.counts.iter().sum::<u32>(),
            7,
            "one count per station: Main's 5 and Branch's 2 (its start is the junction)"
        );
    }

    #[test]
    fn the_dive_profile_renders_and_writes_the_route_of_the_open_project() {
        let (state, _db, _open, main) = opened();
        let text = render_dive_profile_text_impl(&state, plan(main, Vec::new())).unwrap();
        assert!(text.contains("Export Cave"), "{text}");

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("dive.txt");
        export_dive_profile_impl(
            &state,
            &path,
            ProfileFormat::PlainText,
            plan(main, Vec::new()),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        let xml = dir.path().join("dive.ssrf");
        export_dive_profile_impl(
            &state,
            &xml,
            ProfileFormat::SubsurfaceXml,
            plan(main, vec![StationRef::new(main, 2)]),
        )
        .unwrap();
        assert!(std::fs::read_to_string(&xml).unwrap().starts_with("<?xml"));
    }

    #[test]
    fn every_export_refuses_a_file_the_open_project_owns() {
        let (state, db, _open, main) = opened();
        let plnx = db.path().with_extension("plnx");
        std::fs::write(&plnx, b"dive plans").unwrap();
        for target in [db.path().to_path_buf(), plnx] {
            let before = std::fs::read(&target).unwrap();
            let refusals = [
                export_csv_impl(&state, target.clone()).err(),
                export_json_impl(&state, target.clone()).err(),
                export_tml_impl(&state, target.clone()).err(),
                export_image_impl(&state, target.clone(), svg_options()).err(),
                export_dive_profile_impl(
                    &state,
                    &target,
                    ProfileFormat::PlainText,
                    plan(main, Vec::new()),
                )
                .err(),
            ];
            for refusal in refusals {
                assert_eq!(refusal.as_deref(), Some(OWNED_FILE_REFUSAL), "{target:?}");
            }
            assert_eq!(
                std::fs::read(&target).unwrap(),
                before,
                "{target:?} untouched"
            );
        }
    }

    #[test]
    fn every_export_needs_an_open_project() {
        let state = AppState::new().unwrap();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out");
        let main = 1;
        let errors = [
            export_csv_impl(&state, path.clone()).err(),
            export_json_impl(&state, path.clone()).err(),
            export_tml_impl(&state, path.clone()).err(),
            export_image_impl(&state, path.clone(), svg_options()).err(),
            preview_svg_export_impl(&state, svg_options()).err(),
            export_value_histogram_impl(&state, ExportValueKind::Depth, 8).err(),
            render_dive_profile_text_impl(&state, plan(main, Vec::new())).err(),
            export_dive_profile_impl(
                &state,
                &path,
                ProfileFormat::PlainText,
                plan(main, Vec::new()),
            )
            .err(),
        ];
        for error in errors {
            assert_eq!(error.as_deref(), Some("No project open"));
        }
        assert!(!path.exists());
    }
}
