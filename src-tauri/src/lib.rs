// Extracted to the `stalagmight-models` workspace member; re-exported under the
// old names so every `crate::app_dirs::*` / `crate::models::*` path (and the
// `stalagmight_lib::…` paths the integration tests and gen_ipc_types example
// use) keeps compiling unchanged.
pub use stalagmight_models::app_dirs;
pub use stalagmight_persistence::cache;
pub mod commands;
// Extracted to the `stalagmight-core` workspace member; re-exported under the
// old names, same deal as `models` below (`tile_providers` and, for the test
// builds, `test_helpers` ride along further down).
pub use stalagmight_core::config;
pub use stalagmight_core::core;
// Extracted to the `stalagmight-export` workspace member; re-exported under
// the old name, same deal as `models` below.
pub use stalagmight_export::export;
// Extracted to the `stalagmight-telemetry` workspace member; re-exported under
// the old names, same deal as `models` above.
pub use stalagmight_models as models;
pub use stalagmight_telemetry::logging;
// Extracted to the `stalagmight-parsers` workspace member; re-exported under
// the old name, same deal as `models` above.
pub use stalagmight_parsers::parsers;
// Extracted to the `stalagmight-persistence` workspace member (together with
// `cache` and `tilemap_cache`); re-exported under the old names, same deal.
pub use stalagmight_persistence::persistence;
// Extracted to the `stalagmight-speleodb` workspace member; re-exported under
// the old name, same deal as `models` above.
pub use stalagmight_speleodb::speleodb;
pub use stalagmight_telemetry::telemetry;
// The core member compiles `test_helpers` under its `test-support` feature
// (enabled by this crate's [dev-dependencies]), so every
// `crate::test_helpers::*` path in the tests keeps resolving.
#[cfg(test)]
pub use stalagmight_core::test_helpers;
pub mod tile_attribution;

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod entry_point_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod ts_export_paths_tests;
pub use stalagmight_core::tile_providers;
pub use stalagmight_persistence::tilemap_cache;
/// Linux-only WebKit memory-pressure wiring (see the module docs and the
/// patched wry fork: <https://github.com/Stalagmight/wry>).
#[cfg(target_os = "linux")]
pub mod webkit_memory;

use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

use commands::event_sink::emit_event;
use core::events::{CloseRequested, TileTransportError, TileTransportSuccess};

// The tile engine is the `stalagmight-tiles` workspace member (#576 C8). This
// crate keeps the `stalagmight-tiles://` protocol registration in `run()` and
// the `fetch_tile` command glue (commands/cache.rs), which emit the Tauri
// events and telemetry the engine itself knows nothing about.
pub(crate) use stalagmight_tiles::tile_fetch_impl;
use stalagmight_tiles::{
    build_error_response, handle_tile_request, parse_tile_uri, TileServeSource,
};

// Moved to the `stalagmight-core` member (AppState owns the shared instance,
// and members stay tauri-free): the counters and the once-per-transition
// latch live there; THIS crate keeps the actual `tile-transport-*` event
// emission, at the call site in the tile protocol handler below.
pub use stalagmight_core::tile_stats::{TileFetchStats, TileFetchStatsSnapshot};

/// How long the app-exit handler waits for a best-effort SpeleoDB lock release
/// before exiting anyway (fix M2). Long enough for a healthy round trip, short
/// enough that a hung / unreachable server can't noticeably freeze quit; the
/// server-side mutex times out on its own if the release doesn't land.
const EXIT_LOCK_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

// The mobile entry point is `run()` itself (#576 NF-W4T-2): the attribute sat on
// the constant above, which a mobile build would have tried to export instead.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Export the WebKit memory-pressure environment as the FIRST thing `run()`
    // does, while the process is still genuinely single-threaded: the `set_var`
    // calls inside are a data race once any other thread is live, and
    // `logging::init()` below spawns a `tracing_appender` writer thread. It also
    // must run before the Tauri builder: on Linux the config-defined window's
    // WebContext is constructed inside Tauri's setup step (before our own
    // `.setup()` hook), and `memory-pressure-settings` is a construct-only
    // property. No-op unless enabled (default on). The summary is logged after
    // `logging::init()` (the logger does not exist yet here). See `webkit_memory`
    // and the patched wry fork (https://github.com/Stalagmight/wry).
    #[cfg(target_os = "linux")]
    let webkit_mem_cfg = webkit_memory::apply_env_from_config();

    let _guard = logging::init();
    // Route panics through tracing + a crash-marker file so a crash oracle can
    // detect a backend panic even when a thread panic leaves the app alive.
    logging::panic_hook::install();

    // Now that logging is initialized, emit the WebKit memory summary that
    // `apply_env_from_config` computed before any subscriber existed.
    #[cfg(target_os = "linux")]
    webkit_memory::log_applied_config(&webkit_mem_cfg);

    #[cfg(not(feature = "e2e"))]
    let builder = tauri::Builder::default();
    #[cfg(feature = "e2e")]
    let builder = tauri::Builder::default().plugin(tauri_plugin_webdriver_automation::init());

    // Single instance (#424), registered before every other plugin as the
    // plugin requires. A second launch - Explorer's "Open with", a Linux
    // `.desktop` handler - is killed and its argv forwarded here, so the file
    // opens in the window the user already has. macOS is not built with the
    // plugin at all (see Cargo.toml): LaunchServices routes the second open to
    // the running instance as an Apple event, handled in the `RunEvent::Opened`
    // arm below. Off under `e2e` so the WebDriver harness can still spawn its
    // own instance while a dev build is running, and off in a DEBUG build the
    // agent harness started (`STALAGMIGHT_TEST_HOOKS=1`): parallel harness
    // sessions on one session bus otherwise kill each other (#576 HFI-7). A
    // release binary ignores that variable - see `single_instance_enabled`.
    //
    // `desktop` (not a hand-rolled `not(android)`/`not(ios)` pair) so this cfg
    // cannot drift from Cargo.toml's `[target.'cfg(...)'.dependencies]` gate on
    // the crate itself - `not(macos)` alone used to be WIDER than that gate
    // (true on Android/iOS too), so a mobile build would try to reference a
    // dependency Cargo never pulled in and fail to resolve it (#445 review).
    #[cfg(all(not(feature = "e2e"), desktop, not(target_os = "macos")))]
    let builder = if commands::launch::single_instance_enabled(
        cfg!(debug_assertions),
        std::env::var_os("STALAGMIGHT_TEST_HOOKS").as_deref(),
    ) {
        builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            // `cwd` is the SENDER's working directory - a relative argument there
            // means nothing against ours, so it is what relative paths resolve
            // against (and without it they are dropped, not guessed at).
            let cwd = std::path::PathBuf::from(cwd);
            let base = cwd.is_absolute().then_some(cwd.as_path());
            let paths = commands::launch::launch_paths_from_args(
                argv.into_iter().map(std::ffi::OsString::from),
                base,
            );
            if paths.is_empty() {
                return;
            }
            // Raise the window BEFORE queueing: whatever the frontend does with
            // this - a confirm, an open - has to happen somewhere the user can see,
            // or "Open with" on a backgrounded app reads as nothing happening.
            commands::launch::focus_main_window(app);
            commands::launch::queue_and_notify(app, paths);
        }))
    } else {
        builder
    };

    let builder = builder
        .manage(commands::launch::PendingLaunchFiles::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol(
            "stalagmight-tiles",
            |ctx, request, responder| {
                let uri = request.uri().clone();
                let app_handle = ctx.app_handle().clone();
                let state = ctx.app_handle().state::<core::state::AppState>();
                let tile_cache = state.tile_cache();
                let tilemap_cache = state.tilemap_cache();
                let tile_fetch_stats = state.tile_fetch_stats();
                let tile_fetch_stats_notify = Arc::clone(&tile_fetch_stats);
                let cache_config = state.get_config().cache;

                // Use Tauri's async runtime with spawn_blocking for bounded concurrency
                // This uses a threadpool instead of spawning unbounded threads
                tauri::async_runtime::spawn(async move {
                    // Parse the tile coordinates from the URI
                    let Some((provider, z, y, x)) = parse_tile_uri(&uri) else {
                        tracing::warn!(
                            uri = %uri,
                            purpose = "handle map tile request",
                            "Invalid tile URI format; rejecting request"
                        );
                        responder.respond(build_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "Invalid tile URI format. Expected: /esri/{z}/{y}/{x} or esri/{z}/{y}/{x}",
                    ));
                        return;
                    };

                    tracing::debug!(
                        provider = %provider,
                        z = z,
                        y = y,
                        x = x,
                        purpose = "render map tiles",
                        "Processing tile request"
                    );

                    // Provider for tile-consumption telemetry (#31), kept past the fetch
                    // closure that moves `provider`.
                    let provider_for_telemetry = provider.clone();

                    // Run the blocking tile fetch in the blocking threadpool. The global
                    // stats snapshot drives the (concurrency-tolerant) connectivity
                    // notifications; `network_hit` is the per-request flag used for the
                    // exact tile-consumption telemetry below.
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        let snap_before = tile_fetch_stats.snapshot();
                        let (response, serve_source) = handle_tile_request(
                            &provider,
                            z,
                            y,
                            x,
                            &tile_cache,
                            &tilemap_cache,
                            &tile_fetch_stats,
                            &cache_config,
                        );
                        let snap_after = tile_fetch_stats.snapshot();
                        let had_network_success =
                            snap_after.total_fetched > snap_before.total_fetched;
                        let had_network_error = snap_after.total_errors > snap_before.total_errors;
                        (
                            response,
                            had_network_success,
                            had_network_error,
                            serve_source,
                        )
                    })
                    .await;

                    let (response, had_network_success, had_network_error, serve_source) = result
                        .unwrap_or_else(|e| {
                            tracing::error!(
                                error = %e,
                                purpose = "render map tiles",
                                "Tile request task panicked"
                            );
                            (
                                build_error_response(
                                    http::StatusCode::INTERNAL_SERVER_ERROR,
                                    "Internal error processing tile request",
                                ),
                                false,
                                false,
                                TileServeSource::Cache,
                            )
                        });

                    // Notify connectivity changes (debounced - only emits on state
                    // transitions; the once-per-transition latch lives on
                    // `TileFetchStats` in the core member, the emission stays here
                    // with Tauri).
                    if had_network_success {
                        // Emit `tile-transport-success` once per offline→online transition.
                        if tile_fetch_stats_notify.transition_to_online() {
                            let _ = emit_event::<TileTransportSuccess, _>(&app_handle, ());
                        }
                    } else if had_network_error && tile_fetch_stats_notify.transition_to_offline() {
                        // Emit `tile-transport-error` once per online→offline transition.
                        let _ = emit_event::<TileTransportError, _>(&app_handle, ());
                    }

                    // Tile-consumption telemetry (#31): one event per SERVED tile, tagged by
                    // provider and by where its bytes came from - `tile.fetch.<p>` for
                    // imagery pulled from the provider, `tile.revalidate.<p>` for a 304
                    // (a round trip, but the bytes came off our disk), `tile.cache.<p>`
                    // for no network call at all, stale-if-error included. Uses the
                    // PER-REQUEST source (not the shared stats diff, which concurrent
                    // requests would cross-contaminate). Errors (no tile served) aren't
                    // counted; "tiles used by a session for provider P" is the sum of the
                    // three. No-op unless telemetry is compiled in and enabled.
                    if response.status().is_success() {
                        let event = format!(
                            "tile.{}.{provider_for_telemetry}",
                            serve_source.telemetry_kind()
                        );
                        app_handle
                            .state::<telemetry::TelemetryCollector>()
                            .track(&event);
                    }

                    responder.respond(response);
                });
            },
        );

    // Desktop-only: intercept window close to allow frontend confirmation dialogs.
    // Android has no window close button, so this handler is dead code on mobile.
    #[cfg(desktop)]
    let builder = builder.on_window_event(|window, event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Err(e) = emit_event::<CloseRequested, _>(window, ()) {
                tracing::error!(
                    error = %e,
                    purpose = "allow frontend to confirm close",
                    "Failed to emit close-requested event"
                );
                if let Err(destroy_err) = window.destroy() {
                    tracing::error!(
                        error = %destroy_err,
                        purpose = "close window after emit failure",
                        "Failed to destroy window after close-requested emit failed"
                    );
                }
            }
        }
    });

    builder
        .setup(|app| {
            tracing::info!("Stalagmight starting up");

            // Resolve platform directories before anything else needs them.
            // On Android, the `directories` crate returns None; Tauri's
            // PathResolver delegates to the platform-native APIs correctly.
            commands::event_sink::init_app_dirs(app.handle());

            // Initialize app state
            let app_state = core::state::AppState::new()?;
            app.manage(app_state);

            // Set event sink on state for event emission
            let state = app.state::<core::state::AppState>();
            state.set_event_sink(commands::event_sink::tauri_sink(app.handle().clone()));
            // [#576 A11] The process-wide lock cells (log ring buffer, telemetry,
            // tile caches, the launch queue) report a crash through the managed
            // state's monitor, so the user gets the same single internal-error
            // banner whichever lock a crash poisoned - including one poisoned
            // before this line ran.
            models::poison_policy::global_monitor().forward_to(state.poison_monitor());

            // Initialize telemetry (no-op when feature is disabled)
            let telemetry_config = state.get_config();
            let collector =
                telemetry::TelemetryCollector::new(telemetry_config.general.telemetry_enabled);
            app.manage(collector.clone());
            // #210 (F1): give the free-function SpeleoDB client a handle to emit
            // per-endpoint traffic metrics through (it has no AppHandle to reach
            // Tauri-managed state, so it records via this write-once sink).
            speleodb::metrics::install(collector.clone());
            // Also drains what earlier sessions buffered, as that thread's
            // first job: draining here, on the setup thread, let a slow
            // telemetry endpoint hold up startup (#576 B17).
            telemetry::start_periodic_flush(collector);

            // Bound the log directory once per launch (#319). `cleanup_logs`
            // existed but had no caller, so logs and crash markers grew
            // forever. Runs here because the config is only readable after
            // `AppState::new`; it sweeps the directory `logging::init()`
            // recorded, not `log_directory()` (which now resolves elsewhere -
            // see `ACTIVE_LOG_DIR`). Inline rather than spawned: the work is
            // one stat pass over a directory holding a handful of log files,
            // and a failure must never block startup.
            if let Err(err) =
                logging::cleanup_logs(state.get_config().logging.max_directory_size_mb)
            {
                tracing::warn!(error = %err, "Failed to prune the log directory");
            }

            // Launch-with-file (#424): a cold launch from Explorer or a Linux
            // `.desktop` handler puts the path in argv. Queue it now; the
            // frontend drains the queue once its stores are up. macOS passes
            // nothing here (Finder sends an Apple event instead - see the
            // `RunEvent::Opened` arm below), so this is simply empty there.
            // `args_os`, not `args`: the latter PANICS on a non-Unicode
            // argument, which would make a latin-1 filename crash the launch.
            let launch_cwd = std::env::current_dir().ok();
            commands::launch::queue_and_notify(
                app.handle(),
                commands::launch::launch_paths_from_args(
                    std::env::args_os(),
                    launch_cwd.as_deref(),
                ),
            );

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Cache commands
            commands::cache::get_cache_stats,
            commands::cache::clear_tile_cache,
            commands::cache::get_network_status,
            commands::cache::get_tile_fetch_stats,
            commands::cache::fetch_tile,
            commands::cache::get_tile_providers,
            commands::cache::get_tile_attribution,
            // Gas planning (#38)
            commands::gas::compute_gas_reach_field,
            // Dive-plan persistence (#244, .plnx sibling)
            commands::dive_plan::list_dive_plans,
            commands::dive_plan::load_dive_plan,
            commands::dive_plan::save_dive_plan,
            commands::dive_plan::delete_dive_plan,
            commands::dive_plan::plan_anchors_for_refs,
            commands::dive_plan::plan_refs_for_anchors,
            // Project commands
            commands::project::update_project_settings,
            commands::project::set_surface_elevation,
            commands::project::save_project,
            commands::project::close_project,
            commands::project::take_unrecognised_shot_kinds,
            commands::project::history_discarded,
            commands::project::set_workspace_mode,
            commands::project::get_project_state,
            commands::project::get_project_state_binary,
            commands::project::get_adjusted_stations,
            commands::project::get_station_distances_from,
            commands::project::get_along_line_distance,
            commands::project::list_recent_projects,
            commands::project::remove_from_recent_projects,
            commands::project::delete_project,
            commands::project::set_project_name_for_path,
            commands::project::merge_projects,
            commands::project::load_project_full,
            commands::project::load_project_full_binary,
            commands::project::create_project_full,
            commands::project::check_create_target,
            commands::project::cancel_loading,
            // SpeleoDB commands (read-only spike, #183)
            commands::speleodb::speleodb_login,
            commands::speleodb::speleodb_list_projects,
            commands::speleodb::speleodb_open_project,
            commands::speleodb::speleodb_open_project_discarding_local,
            commands::speleodb::speleodb_adopt_ariane,
            commands::speleodb::speleodb_logout,
            commands::speleodb::speleodb_session,
            commands::speleodb::speleodb_set_token_storage,
            commands::speleodb::speleodb_retry_keychain,
            // SpeleoDB linked-project status for the action shell (#204)
            commands::speleodb::speleodb_link_status,
            // [#576 NF-W2F-1] Re-ask the open file's flags after an in-place tree swap.
            commands::speleodb::speleodb_open_file_flags,
            commands::speleodb::speleodb_unlink,
            commands::speleodb::speleodb_remove_link_at,
            // SpeleoDB single-writer hand-off (#194/#197)
            commands::speleodb::speleodb_start_editing,
            commands::speleodb::speleodb_force_edit,
            commands::speleodb::speleodb_done,
            commands::speleodb::speleodb_update,
            commands::speleodb::speleodb_push,
            // SpeleoDB divergence resolution - take remote (#198)
            commands::speleodb::speleodb_take_remote,
            // SpeleoDB held-lock resume/release prompt - release all + per-project (#201)
            commands::speleodb::speleodb_release_all_locks,
            commands::speleodb::speleodb_release_lock,
            commands::speleodb::speleodb_release_lock_acquired_here,
            // SpeleoDB per-project LOCAL state (cached copy + un-pushed edits), the
            // local half of the held-lock / GlobalView project joins
            commands::speleodb::speleodb_local_project_states,
            // SpeleoDB revision-history read + restore (#218)
            commands::speleodb::speleodb_get_revisions,
            commands::speleodb::speleodb_restore_revision,
            commands::speleodb::speleodb_dump_candidates,
            // SpeleoDB hybrid undo stack: the Unwind target + the per-project policy (#303)
            commands::speleodb::speleodb_unwind_target,
            commands::speleodb::speleodb_rewind_target,
            commands::speleodb::speleodb_discard_changes,
            commands::speleodb::speleodb_set_undo_mode,
            // SpeleoDB Enable-rendering PATCH (#200)
            commands::speleodb::speleodb_set_exclude_geojson,
            // SpeleoDB Project Settings metadata (#206) + create-on-SDB publish (#207)
            commands::speleodb::speleodb_get_project_metadata,
            commands::speleodb::speleodb_patch_project,
            commands::speleodb::speleodb_publish,
            // SpeleoDB full Project Settings: collaborators + lock release + delete (#215)
            commands::speleodb::speleodb_list_collaborators,
            commands::speleodb::speleodb_add_collaborator,
            commands::speleodb::speleodb_set_collaborator,
            commands::speleodb::speleodb_remove_collaborator,
            commands::speleodb::speleodb_add_team,
            commands::speleodb::speleodb_set_team,
            commands::speleodb::speleodb_remove_team,
            commands::speleodb::speleodb_list_teams,
            commands::speleodb::speleodb_search_users,
            commands::speleodb::speleodb_release_current_lock,
            commands::speleodb::speleodb_delete_project,
            // Section commands
            commands::section::create_section,
            commands::section::update_section,
            commands::section::delete_section,
            commands::section::reverse_section,
            commands::section::move_section,
            commands::section::move_segment,
            commands::section::execute_batch,
            commands::section::get_rigid_connected_section_ids,
            commands::section::get_anchored_section_ids,
            commands::section::create_section_and_merge,
            commands::section::get_section_relationships,
            commands::section::get_segment_geometry,
            // Shot commands
            commands::shot::create_shot,
            commands::shot::update_shot,
            commands::shot::preview_delete_shot,
            commands::shot::delete_shot,
            commands::shot::set_shot_kind,
            commands::shot::split_shot_at_distance,
            commands::shot::delete_shot_and_split,
            commands::shot::delete_last_shot_and_section,
            commands::shot::preview_delete_shot_range,
            commands::shot::delete_shot_range_split,
            commands::shot::delete_shot_range_shift,
            // Station commands
            commands::station::merge_stations,
            commands::station::unmerge_stations,
            commands::station::unmerge_station_pair,
            commands::station::set_zero_datum,
            commands::station::remove_zero_datum,
            commands::station::set_station_comment,
            commands::station::move_comment,
            commands::station::move_station,
            commands::station::check_merge_connectivity,
            commands::station::create_loop_closure,
            commands::station::remove_loop_closure,
            // Undo/Redo
            commands::history::undo,
            commands::history::redo,
            commands::history::can_undo,
            commands::history::can_redo,
            commands::history::get_undo_stack,
            commands::history::get_redo_stack,
            commands::history::undo_to,
            commands::history::redo_to,
            // Import/Export
            commands::import::import_dmp,
            commands::import::calculate_file_hash,
            commands::import::check_duplicate_import,
            commands::import::check_section_hashes,
            commands::import::import_sections,
            commands::import::parse_tml,
            commands::import::execute_tml_import,
            commands::import::parse_compass,
            commands::import::execute_compass_import,
            commands::import::parse_walls,
            commands::import::execute_walls_import,
            commands::import::parse_gpx,
            commands::import::execute_gpx_import,
            commands::import::suggest_import_placement,
            commands::import::suggest_all_import_placements,
            commands::import::suggest_orphan_placements,
            commands::import::count_unanchored_sections,
            commands::export::export_csv,
            commands::export::export_json,
            commands::export::export_tml,
            commands::export::export_image,
            commands::export::preview_svg_export,
            commands::export::export_dive_profile,
            commands::export::render_dive_profile_text,
            // Config
            commands::config::get_config,
            commands::config::update_config,
            commands::config::patch_config,
            commands::config::reload_config,
            commands::config::update_panel_sizes,
            commands::config::update_panel_layout,
            commands::config::update_data_table_view,
            commands::config::update_new_section_view,
            commands::config::update_edit_section_view,
            commands::config::update_import_shots_view,
            commands::config::update_gas_config,
            // Display settings (per-project)
            commands::config::load_display_settings,
            commands::config::save_display_setting,
            // Raw table browsing (data table fullscreen)
            commands::raw_data::get_raw_table_names,
            commands::raw_data::get_raw_table,
            // Validation
            commands::validation::get_project_warnings,
            commands::validation::check_project_integrity,
            commands::validation::repair_project_integrity,
            commands::validation::resolve_warning,
            commands::validation::unresolve_warning,
            // Survey geometry
            commands::survey_geometry::get_survey_geometry_for_file,
            // OS file association / launch-with-file (#424)
            commands::launch::take_pending_launch_files,
            // Recovery (SPEC 14.4)
            commands::recovery::attempt_repair,
            commands::recovery::list_backups,
            commands::recovery::restore_from_backup,
            commands::recovery::export_salvageable_data,
            commands::recovery::write_salvaged_data,
            // [#576 HFI-2] The overwrite prompts' existence probe
            commands::save_target::save_target_exists,
            // Logging
            commands::logging::get_recent_logs,
            commands::logging::get_recent_logs_since,
            commands::logging::push_frontend_log,
            commands::debug::debug_trigger_panic,
            // [#576 RV5-PI-3] The internal-error banner's pull path
            commands::internal_error::get_internal_error_status,
            // Memory probe (debug)
            commands::memory::get_memory_stats,
            // Telemetry
            commands::telemetry::track_events,
            commands::telemetry::flush_telemetry,
            commands::telemetry::is_telemetry_available,
            commands::telemetry::set_telemetry_enabled,
        ])
        .build(tauri::generate_context!())
        .map(|app| {
            app.run(|app_handle, event| {
                // Best-effort release of a held SpeleoDB write lock on app exit so
                // quitting / closing the window doesn't strand the server mutex
                // locked for every other editor (fix 1). This covers the quit +
                // window-close paths that destroy the window WITHOUT routing through
                // `close_project` (which releases on its own). No-op when no remote
                // edit session is active. Bounded (fix M2): this closure runs on the
                // main event-loop thread, so a synchronous release against a hung /
                // unreachable server would freeze quit for the full HTTP timeout - cap
                // the wait and exit anyway (the server lock times out on its own).
                if let tauri::RunEvent::ExitRequested { .. } = event {
                    if let Some(state) = app_handle.try_state::<core::state::AppState>() {
                        state.speleodb_release_held_lock_bounded(EXIT_LOCK_RELEASE_TIMEOUT);
                    }
                }

                // macOS "open with" (#424). Finder never uses argv: both a cold
                // launch and a double-click while the app runs arrive as an
                // Apple event, which Tauri surfaces here. On a cold launch this
                // fires long before the webview exists, so the paths are queued
                // rather than pushed - the frontend drains them when ready.
                // `focus_main_window` mirrors the single-instance path above (a
                // no-op with no webview yet, so the cold-launch case above is
                // unaffected): without it, a double-click on a backgrounded app
                // queues the file and raises the confirm in a window the user
                // cannot see - the exact failure raising it exists to prevent
                // (#445 review).
                #[cfg(target_os = "macos")]
                if let tauri::RunEvent::Opened { urls } = &event {
                    commands::launch::focus_main_window(app_handle);
                    commands::launch::queue_and_notify(
                        app_handle,
                        commands::launch::launch_paths_from_urls(urls),
                    );
                }
            });
        })
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "Fatal: Tauri application failed to start");
        });
}
