use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::commands::event_sink::tauri_sink;
use crate::config::GasConfig;
use crate::core::loading::{LoadingPhase, ProgressEmitter};
use crate::core::AppState;
use crate::export::{ExportImageOptions, ProfileFormat};
use crate::models::{Cylinder, Diver, PlanSegment, Project, StationRef};

/// True when `target` resolves to the same file on disk as the open project's
/// `.stalx` at `existing`. Every export replaces its target wholesale
/// (`write_atomic`), so exporting over the live project database would destroy
/// it. `existing` always exists (it is the open DB); `target` may not exist yet
/// (a fresh export destination), so when it can't be canonicalized directly,
/// compare its canonicalized parent directory + file name against the
/// canonicalized project path. Errs toward catching the exact open-DB path
/// rather than rejecting unrelated destinations.
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
    // named to the open DB would overwrite it with CSV/JSON/TML, and one named to the `.plnx`
    // sibling would take every saved dive plan with it.
    if is_project_owned_file(&path, &project) {
        tracing::warn!(
            path = ?path,
            open_project = ?project.file_path,
            format = format,
            "Refused to export over a file the open project owns"
        );
        return Err("Cannot export over the open project file".to_string());
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
    export_format_impl(state, path, "csv", crate::export::export_csv)
}

/// Export project data to JSON
#[tauri::command]
pub fn export_json(state: State<AppState>, path: PathBuf) -> Result<(), String> {
    export_json_impl(state.inner(), path)
}

pub(crate) fn export_json_impl(state: &AppState, path: PathBuf) -> Result<(), String> {
    export_format_impl(state, path, "json", crate::export::export_json)
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
    export_format_with_notes(state, path, "tml", crate::export::export_tml)
}

/// Export map image with progress events via shared loading-progress infrastructure.
///
/// This command runs heavy export work (SVG generation, PNG rasterization, PDF conversion)
/// in a blocking thread pool to prevent freezing the UI and ensure progress events propagate.
/// Progress is emitted using the shared ProgressEmitter (loading-progress events) for
/// integration with the LoadingProgress component.
#[tauri::command]
pub async fn export_image(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    path: PathBuf,
    mut options: ExportImageOptions,
) -> Result<(), String> {
    // Server-populated (#27): the export must colour its distance ramp and
    // label its penetration with the same graph the app is showing, so this
    // comes from the live config, never from whatever the frontend sent.
    options.include_reference_legs = state.get_config().gas.include_reference_legs;
    let project = state.get_project_state().ok_or_else(|| {
        tracing::warn!(
            path = ?path,
            purpose = "export map image for sharing",
            format = ?options.format,
            "Export requested with no project open"
        );
        "No project open".to_string()
    })?;

    // Never let an image export clobber a file the open project owns: the export
    // renamed into place by `export/image.rs` would replace the live database, or the
    // `.plnx` sibling holding every saved dive plan.
    //
    // `is_project_owned_file`, not `is_open_project_file`: this arm used the
    // narrower `.stalx`-only predicate while the text exports and the dive
    // profile used the wider one, so a PNG/SVG/PDF aimed at `<project>.plnx`
    // was written - and the doc comment on `is_project_owned_file` already
    // claimed the gap was closed.
    if is_project_owned_file(&path, &project) {
        tracing::warn!(
            path = ?path,
            open_project = ?project.file_path,
            purpose = "protect the open project database",
            "Refusing to export an image over a file the open project owns"
        );
        return Err("Cannot export over the open project file".to_string());
    }

    let format = options.format;
    let log_path = path.clone();

    // Run heavy export work in spawn_blocking to avoid freezing UI
    // and ensure progress events are delivered during processing
    tauri::async_runtime::spawn_blocking(move || {
        tracing::info!(
            path = ?path,
            purpose = "export map image for sharing",
            format = ?options.format,
            "Starting image export"
        );
        // Render from the memoized closure-adjusted solve so the saved image
        // matches the on-screen (adjusted) map; falls back to measured
        // coordinates when there are no eligible closures. Fetch before the
        // emitter consumes `app_handle`.
        let adjusted = app_handle
            .state::<AppState>()
            .inner()
            .get_adjusted_stations();

        // Use shared ProgressEmitter for consistent progress events
        let mut emitter = ProgressEmitter::new_for_export(tauri_sink(app_handle));

        // Start preparing phase
        emitter.start_phase(LoadingPhase::ExportPreparing);

        // Generate phase (SVG generation, rasterization)
        emitter.start_phase(LoadingPhase::ExportGenerating);

        // Perform the actual export
        let result = match adjusted {
            Some(stations) => {
                crate::export::export_image_with_stations(&project, &path, &options, stations)
            }
            None => crate::export::export_image(&project, &path, &options),
        };

        if result.is_ok() {
            // Writing phase
            emitter.start_phase(LoadingPhase::ExportWriting);
            emitter.complete_phase(LoadingPhase::ExportWriting);

            // Complete phase
            emitter.start_phase(LoadingPhase::ExportComplete);
            emitter.complete_phase(LoadingPhase::ExportComplete);
            tracing::info!(
                path = ?path,
                purpose = "export map image for sharing",
                format = ?options.format,
                "Image export completed"
            );
        } else if let Err(ref e) = result {
            tracing::error!(
                path = ?path,
                purpose = "export map image for sharing",
                format = ?options.format,
                reason = %format_args!("{e:#}"),
                "Image export failed"
            );
        }

        result.map_err(|e| format!("{e:#}"))
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

#[cfg(test)]
pub(crate) fn export_image_impl(
    state: &AppState,
    path: PathBuf,
    mut options: ExportImageOptions,
) -> Result<(), String> {
    // See the `export_image` command: config wins over the wire (#27).
    options.include_reference_legs = state.get_config().gas.include_reference_legs;
    let project = state.get_project_state().ok_or_else(|| {
        tracing::warn!(
            path = ?path,
            purpose = "export map image for sharing",
            format = ?options.format,
            "Export requested with no project open"
        );
        "No project open".to_string()
    })?;

    // Mirror the live command's guard: never clobber a file the open project owns.
    if is_project_owned_file(&path, &project) {
        return Err("Cannot export over the open project file".to_string());
    }

    tracing::info!(
        path = ?path,
        purpose = "export map image for sharing",
        format = ?options.format,
        "Exporting map image"
    );
    match crate::export::export_image(&project, &path, &options) {
        Ok(()) => {
            tracing::info!(
                path = ?path,
                purpose = "export map image for sharing",
                format = ?options.format,
                "Map image export completed"
            );
            Ok(())
        }
        Err(e) => {
            tracing::error!(
                path = ?path,
                purpose = "export map image for sharing",
                format = ?options.format,
                reason = %format_args!("{e:#}"),
                "Map image export failed"
            );
            Err(format!("{e:#}"))
        }
    }
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

/// Recompute the reach field for `request` and render its profile in `format`.
///
/// Runs `validate_plan_inputs` and `validate_plan_segments` exactly as `compute_gas_reach_field`
/// does. Skipping them would make
/// this the one entry point where a NaN RMV or an uncapped way-point list reaches the engine - and
/// unlike the compute, which would just show nothing, this path renders a document the diver
/// carries underwater.
/// Takes the project by reference so ONE snapshot serves the whole command.
///
/// `AppState::get_project_state` is a whole-`Project` deep clone, which is multi-second on a large
/// survey; going through `AppState::gas_reach_field` (which clones again internally) on top of a
/// snapshot taken for the name and another for the clobber guard meant three clones per export
/// click. Calling the `Project` method directly on one clone also removes a real correctness wart:
/// with three snapshots, the name, the guard and the computed field could each come from a
/// different project state if a load landed between them.
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
    let field = project.compute_gas_reach_field_with_overrides(
        request.origin.as_ref(),
        request.endpoint.as_ref(),
        &request.waypoints,
        &request.segments,
        &diver,
        &cylinders,
        &gas_config,
        &request.stage_drop_overrides,
    );
    crate::export::render_dive_profile(
        &field,
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
    // branch below records it with Display - a newline-bearing value would forge log records.
    let format = ProfileFormat::from_wire(&format).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let project = state
            .get_project_state()
            .ok_or_else(|| "No project open".to_string())?;
        let refuse = |path: &PathBuf| {
            tracing::warn!(
                path = ?path,
                open_project = ?project.file_path,
                format = format.label(),
                "Refused to export a dive profile over a file the open project owns"
            );
            "Cannot export over the open project file".to_string()
        };
        if is_project_owned_file(&path, &project) {
            return Err(refuse(&path));
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
        if is_project_owned_file(&path, &project) {
            return Err(refuse(&path));
        }
        crate::export::write_atomic(&path, |w| {
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
    })
    .await
    .map_err(|e| format!("Dive profile export task panicked: {e}"))?
}

/// Render the planned dive profile as text WITHOUT writing it, for the clipboard (#299).
#[tauri::command]
pub async fn render_dive_profile_text(
    app_handle: AppHandle,
    request: DiveProfileRequest,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let project = state
            .get_project_state()
            .ok_or_else(|| "No project open".to_string())?;
        render_profile_for(&project, request, ProfileFormat::PlainText)
    })
    .await
    .map_err(|e| format!("Dive profile render task panicked: {e}"))?
}

/// Generate the SVG markup that `export_image` would write, without
/// touching the filesystem. Used by the export dialog so the preview is
/// pixel-for-pixel identical to what export will produce.
///
/// Runs the (potentially heavy: bounds + Dijkstra + polyline emission)
/// work in `spawn_blocking` so the slider can drag without stalling the
/// Tauri main thread on large surveys - mirroring `export_image`.
#[tauri::command]
pub async fn preview_svg_export(
    app_handle: AppHandle,
    mut options: ExportImageOptions,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        // Same config-wins rule as the export itself, so the preview cannot
        // disagree with the file it is previewing (#27).
        options.include_reference_legs = state.get_config().gas.include_reference_legs;
        let project = state
            .get_project_state()
            .ok_or_else(|| "No project open".to_string())?;
        // Render from the memoized closure-adjusted solve so the preview matches
        // the on-screen (adjusted) map; falls back to measured coordinates when
        // there are no eligible closures.
        match state.inner().get_adjusted_stations() {
            Some(stations) => {
                crate::export::generate_svg_string_with_stations(&project, &options, stations)
            }
            None => crate::export::generate_svg_string(&project, &options),
        }
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Preview task panicked: {e}"))?
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

    /// The export path must actually CALL the validator, not merely have one available.
    ///
    /// `render_profile_for` is a second door into the gas engine, and the engine validates nothing -
    /// its contract says the COMMAND owns that. This is the test that fails if the call is dropped:
    /// an over-cap way-point list must come back as the CAP error. Without the validator the same
    /// request reaches the engine and fails later with the no-route message instead, so the two
    /// outcomes are distinguishable and the assertion cannot pass vacuously.
    ///
    /// Reachable as a unit test only because `render_profile_for` takes a `&Project` rather than an
    /// `AppHandle` - the tuple `Project::new` builds is enough, since validation runs before any
    /// survey data is touched.
    #[test]
    fn the_export_path_actually_runs_the_validator() {
        use crate::core::state::dive_plan::MAX_WAYPOINTS;

        let project = Project::new("Cave".to_string(), None);
        let request = |waypoints: Vec<StationRef>| DiveProfileRequest {
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
        };

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
        let project = Project::new("Cave".to_string(), None);
        for format in [ProfileFormat::PlainText, ProfileFormat::SubsurfaceXml] {
            let err = render_profile_for(
                &project,
                DiveProfileRequest {
                    origin: None,
                    endpoint: None,
                    waypoints: Vec::new(),
                    segments: Vec::new(),
                    diver: Diver {
                        rmv_l_per_min: 20.0,
                        label: None,
                    },
                    cylinders: Vec::new(),
                    gas_config: GasConfig::default(),
                    stage_drop_overrides: Vec::new(),
                },
                format,
            )
            .unwrap_err();
            assert!(err.contains("no route to export"), "{format:?}: {err}");
        }
    }
}
