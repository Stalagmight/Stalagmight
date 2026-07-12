//! Canonical vocabulary of telemetry event and source names.
//!
//! The wire format sends event names verbatim (self-describing JSON), so these
//! are no longer a numeric ID map — they exist to back the coverage tests below
//! (every command / tool / dialog has a tracked event; typo detection).

/// Known interaction-source names (appended to an event as `:source`).
pub(super) const SOURCE_NAMES: &[&str] = &[
    "",               // no source suffix present
    "keyboard",       // 1
    "toolbar",        // 2
    "map",            // 3
    "data-table",     // 4
    "sections-panel", // 5
    "detail-panel",   // 6
    "display-menu",   // 7
    "context-menu",   // 8
    "dialog",         // 9
    "unknown",        // 10
    "mouse",          // 11
    "touch",          // 12
    "dblclick",       // 13
    // Project open/create/import entry-point attribution.
    "recent-list", // recent-projects list (card/table) double-click
    "recent-map",  // recent-projects map marker
    "hamburger",   // global-view hamburger application menu
    "empty-state", // empty-state cards (first run)
    "drag-drop",   // file dropped onto the window (import)
];

/// Known event base names (the `:source` suffix is appended at runtime). The
/// canonical vocabulary the coverage tests check against; order is irrelevant.
pub(super) const EVENT_NAMES: &[&str] = &[
    // ── Commands ──
    // `adjust_loop_closures` was removed when closure adjustment became a
    // non-destructive display mode. Its name is kept in the vocabulary for
    // historical continuity; it has no live handler, so the command/undo
    // coverage tests skip it via REMOVED_COMMANDS.
    "command.add_marker",
    "command.adjust_loop_closures",
    "command.attempt_repair",
    "command.calculate_file_hash",
    "command.can_redo",
    "command.can_undo",
    "command.cancel_loading",
    "command.check_duplicate_import",
    "command.check_merge_connectivity",
    "command.check_section_hashes",
    "command.clear_tile_cache",
    "command.close_project",
    "command.compute_gas_reach_field",
    "command.create_loop_closure",
    "command.create_project_full",
    "command.create_section",
    "command.create_section_and_merge",
    "command.create_shot",
    "command.debug_trigger_panic",
    "command.delete_dive_plan",
    "command.delete_last_shot_and_section",
    "command.delete_marker",
    "command.delete_section",
    "command.delete_shot",
    "command.delete_shot_and_split",
    "command.execute_batch",
    "command.execute_gpx_import",
    "command.execute_tml_import",
    "command.export_csv",
    "command.export_image",
    "command.export_json",
    "command.export_salvageable_data",
    "command.export_tml",
    "command.fetch_tile",
    "command.get_anchored_section_ids",
    "command.get_cache_stats",
    "command.get_config",
    "command.get_network_status",
    "command.get_project_state",
    "command.get_project_warnings",
    "command.get_recent_logs",
    "command.get_recent_logs_since",
    "command.get_redo_stack",
    "command.get_rigid_connected_section_ids",
    "command.get_section_relationships",
    "command.get_segment_geometry",
    "command.get_survey_geometry_for_file",
    "command.get_tile_fetch_stats",
    "command.get_tile_providers",
    "command.get_undo_stack",
    "command.import_dmp",
    "command.import_sections",
    "command.list_backups",
    "command.list_dive_plans",
    "command.list_recent_projects",
    "command.load_display_settings",
    "command.load_dive_plan",
    "command.load_project_full",
    "command.merge_projects",
    "command.merge_stations",
    "command.move_comment",
    "command.move_section",
    "command.move_segment",
    "command.move_station",
    "command.parse_gpx",
    "command.parse_tml",
    "command.preview_delete_shot",
    "command.push_frontend_log",
    "command.redo",
    "command.redo_to",
    "command.reload_config",
    "command.remove_from_recent_projects",
    "command.remove_loop_closure",
    "command.remove_zero_datum",
    // Retained for telemetry-ID stability; the command was removed (#57). See REMOVED_COMMANDS.
    "command.resolve_all_warnings",
    "command.resolve_warning",
    "command.restore_from_backup",
    "command.save_display_setting",
    "command.save_dive_plan",
    "command.save_project",
    "command.set_shot_reference",
    "command.set_station_comment",
    "command.set_zero_datum",
    "command.speleodb_add_collaborator",
    "command.speleodb_add_team",
    "command.speleodb_adopt_ariane",
    "command.speleodb_delete_project",
    "command.speleodb_done",
    "command.speleodb_force_edit",
    "command.speleodb_get_project_metadata",
    "command.speleodb_get_revisions",
    "command.speleodb_link_status",
    "command.speleodb_list_collaborators",
    "command.speleodb_list_projects",
    "command.speleodb_list_teams",
    "command.speleodb_login",
    "command.speleodb_logout",
    "command.speleodb_open_project",
    "command.speleodb_patch_project",
    "command.speleodb_publish",
    "command.speleodb_push",
    "command.speleodb_release_all_locks",
    "command.speleodb_release_current_lock",
    "command.speleodb_release_lock",
    "command.speleodb_remove_collaborator",
    "command.speleodb_remove_team",
    "command.speleodb_restore_revision",
    "command.speleodb_search_users",
    "command.speleodb_session",
    "command.speleodb_set_collaborator",
    "command.speleodb_set_exclude_geojson",
    "command.speleodb_set_team",
    "command.speleodb_start_editing",
    "command.speleodb_take_remote",
    "command.speleodb_unlink",
    "command.speleodb_update",
    "command.split_shot_at_distance",
    "command.suggest_all_import_placements",
    "command.suggest_import_placement",
    "command.suggest_orphan_placements",
    "command.undo",
    "command.undo_to",
    "command.unmerge_station_pair",
    "command.unmerge_stations",
    "command.unresolve_warning",
    "command.update_config",
    "command.update_marker",
    "command.update_panel_layout",
    "command.update_panel_sizes",
    "command.update_section",
    "command.update_shot",
    // ── Action: tools (93-104) ──
    "action.tool.comment",
    "action.tool.delete",
    "action.tool.draw",
    "action.tool.measure",
    "action.tool.merge",
    "action.tool.move_comment",
    "action.tool.move_segment",
    "action.tool.move_shot",
    "action.tool.move_station",
    "action.tool.pan",
    "action.tool.select",
    "action.tool.zoom",
    // ── Action: navigation (105-113) ──
    "action.nav.center-on-shot",
    "action.nav.center-on-station",
    "action.nav.fit-to-bounds",
    "action.nav.pan",
    "action.nav.pan-to-shot",
    "action.nav.zoom",
    "action.nav.zoom-in",
    "action.nav.zoom-out",
    "action.nav.zoom-to-section",
    // ── Action: display toggles (114-128) ──
    "action.toggle.gridSpacing",
    "action.toggle.showDepthGradient",
    "action.toggle.showDevStats",
    "action.toggle.showGrid",
    "action.toggle.showReferenceShots",
    "action.toggle.showSatellite",
    "action.toggle.showSectionNames",
    "action.toggle.showShotComments",
    "action.toggle.showShotData",
    "action.toggle.showShots",
    "action.toggle.showStationComments",
    "action.toggle.showStationLabels",
    "action.toggle.showStations",
    "action.toggle.showZeroDatum",
    // ── Action: selection (129-133) ──
    "action.select.section",
    "action.select.shot",
    "action.select.station",
    "action.select-add.shot",
    "action.select-add.station",
    // ── Action: dialogs (134-167) ──
    "action.dialog.adjust-closures",
    "action.dialog.closure",
    "action.dialog.confirm-delete",
    "action.dialog.confirm-overwrite",
    "action.dialog.context-menu",
    "action.dialog.corruption-recovery",
    "action.dialog.debug",
    "action.dialog.dmp-import-new",
    "action.dialog.dmp-selection",
    "action.dialog.draw-shot",
    "action.dialog.edit-section",
    "action.dialog.edit-shot",
    "action.dialog.edit-station",
    "action.dialog.error",
    "action.dialog.export",
    "action.dialog.gpx-placement",
    "action.dialog.gpx-selection",
    "action.dialog.held-lock-resume",
    "action.dialog.import",
    "action.dialog.insert-station",
    "action.dialog.keyboard-shortcuts",
    "action.dialog.merge",
    "action.dialog.merge-projects",
    "action.dialog.new-project",
    "action.dialog.new-project-for-import",
    "action.dialog.orphan-placement",
    "action.dialog.remove-closure",
    "action.dialog.section-adjust",
    "action.dialog.settings",
    "action.dialog.open-from-speleodb",
    "action.dialog.speleodb-done",
    "action.dialog.speleodb-divergence",
    "action.dialog.speleodb-force-edit",
    "action.dialog.speleodb-close",
    "action.dialog.speleodb-no-permission",
    "action.dialog.speleodb-restore",
    "action.dialog.publish-to-speleodb",
    "action.dialog.split-section",
    "action.dialog.swap-comments",
    "action.dialog.tml-import",
    "action.dialog.unmerge-stations",
    "action.dialog.unsaved-changes",
    "action.dialog.zero-datum-config",
    "action.dialog.zero-datum-selection",
    // ── Action: panels ──
    "action.panel-close.data-table",
    "action.panel-close.detail",
    "action.panel-close.sections",
    "action.panel-open.data-table",
    "action.panel-open.detail",
    "action.panel-open.sections",
    "action.panel-pin.data-table",
    "action.panel-pin.detail",
    "action.panel-pin.sections",
    "action.panel-unpin.data-table",
    "action.panel-unpin.detail",
    "action.panel-unpin.sections",
    // ── Action: context menu (174-176) ──
    "action.remove-closure",
    "action.toggle-reference",
    "action.unmerge",
    // ── Dialog cancellations (177-210) ──
    "dialog-cancel.adjust-closures",
    "dialog-cancel.closure",
    "dialog-cancel.confirm-delete",
    "dialog-cancel.confirm-overwrite",
    "dialog-cancel.context-menu",
    "dialog-cancel.corruption-recovery",
    "dialog-cancel.debug",
    "dialog-cancel.dmp-import-new",
    "dialog-cancel.dmp-selection",
    "dialog-cancel.draw-shot",
    "dialog-cancel.edit-section",
    "dialog-cancel.edit-shot",
    "dialog-cancel.edit-station",
    "dialog-cancel.error",
    "dialog-cancel.export",
    "dialog-cancel.gpx-placement",
    "dialog-cancel.gpx-selection",
    "dialog-cancel.held-lock-resume",
    "dialog-cancel.import",
    "dialog-cancel.insert-station",
    "dialog-cancel.keyboard-shortcuts",
    "dialog-cancel.merge",
    "dialog-cancel.merge-projects",
    "dialog-cancel.new-project",
    "dialog-cancel.new-project-for-import",
    "dialog-cancel.orphan-placement",
    "dialog-cancel.remove-closure",
    "dialog-cancel.section-adjust",
    "dialog-cancel.settings",
    "dialog-cancel.open-from-speleodb",
    "dialog-cancel.speleodb-done",
    "dialog-cancel.speleodb-divergence",
    "dialog-cancel.speleodb-force-edit",
    "dialog-cancel.speleodb-close",
    "dialog-cancel.speleodb-no-permission",
    "dialog-cancel.speleodb-restore",
    "dialog-cancel.publish-to-speleodb",
    "dialog-cancel.split-section",
    "dialog-cancel.swap-comments",
    "dialog-cancel.tml-import",
    "dialog-cancel.unmerge-stations",
    "dialog-cancel.unsaved-changes",
    "dialog-cancel.zero-datum-config",
    "dialog-cancel.zero-datum-selection",
    // ── Undo regret (211-237) ──
    "undo-after.add_marker",
    "undo-after.adjust_loop_closures",
    "undo-after.create_loop_closure",
    "undo-after.create_section",
    "undo-after.create_section_and_merge",
    "undo-after.create_shot",
    "undo-after.delete_last_shot_and_section",
    "undo-after.delete_marker",
    "undo-after.delete_section",
    "undo-after.delete_shot",
    "undo-after.delete_shot_and_split",
    "undo-after.execute_batch",
    "undo-after.execute_compass_import",
    "undo-after.execute_gpx_import",
    "undo-after.execute_tml_import",
    "undo-after.execute_walls_import",
    "undo-after.import_sections",
    "undo-after.merge_stations",
    "undo-after.move_comment",
    "undo-after.move_section",
    "undo-after.move_segment",
    "undo-after.move_station",
    "undo-after.remove_loop_closure",
    "undo-after.remove_zero_datum",
    "undo-after.set_shot_reference",
    "undo-after.set_station_comment",
    "undo-after.set_zero_datum",
    "undo-after.split_shot_at_distance",
    "undo-after.unmerge_station_pair",
    "undo-after.unmerge_stations",
    "undo-after.update_marker",
    "undo-after.update_section",
    "undo-after.update_shot",
    // ── Dwell: tools (238-249) ──
    "dwell.tool.comment",
    "dwell.tool.delete",
    "dwell.tool.draw",
    "dwell.tool.measure",
    "dwell.tool.merge",
    "dwell.tool.move_comment",
    "dwell.tool.move_segment",
    "dwell.tool.move_shot",
    "dwell.tool.move_station",
    "dwell.tool.pan",
    "dwell.tool.select",
    "dwell.tool.zoom",
    // ── Dwell: dialogs (250-283) ──
    "dwell.dialog.adjust-closures",
    "dwell.dialog.closure",
    "dwell.dialog.confirm-delete",
    "dwell.dialog.confirm-overwrite",
    "dwell.dialog.context-menu",
    "dwell.dialog.corruption-recovery",
    "dwell.dialog.debug",
    "dwell.dialog.dmp-import-new",
    "dwell.dialog.dmp-selection",
    "dwell.dialog.draw-shot",
    "dwell.dialog.edit-section",
    "dwell.dialog.edit-shot",
    "dwell.dialog.edit-station",
    "dwell.dialog.error",
    "dwell.dialog.export",
    "dwell.dialog.gpx-placement",
    "dwell.dialog.gpx-selection",
    "dwell.dialog.held-lock-resume",
    "dwell.dialog.import",
    "dwell.dialog.insert-station",
    "dwell.dialog.keyboard-shortcuts",
    "dwell.dialog.merge",
    "dwell.dialog.merge-projects",
    "dwell.dialog.new-project",
    "dwell.dialog.new-project-for-import",
    "dwell.dialog.orphan-placement",
    "dwell.dialog.remove-closure",
    "dwell.dialog.section-adjust",
    "dwell.dialog.settings",
    "dwell.dialog.open-from-speleodb",
    "dwell.dialog.speleodb-done",
    "dwell.dialog.speleodb-divergence",
    "dwell.dialog.speleodb-force-edit",
    "dwell.dialog.speleodb-close",
    "dwell.dialog.speleodb-no-permission",
    "dwell.dialog.speleodb-restore",
    "dwell.dialog.publish-to-speleodb",
    "dwell.dialog.split-section",
    "dwell.dialog.swap-comments",
    "dwell.dialog.tml-import",
    "dwell.dialog.unmerge-stations",
    "dwell.dialog.unsaved-changes",
    "dwell.dialog.zero-datum-config",
    "dwell.dialog.zero-datum-selection",
    // ── Dwell: other (284-285) ──
    "dwell.global-view",
    "dwell.placement",
    "command.track_imported_file",
    "action.dialog.delete-shot-choice",
    "dialog-cancel.delete-shot-choice",
    "dwell.dialog.delete-shot-choice",
    "action.select-add.section",
    "command.load_project_full_binary",
    "command.get_memory_stats",
    "command.parse_compass",
    "command.execute_compass_import",
    // compass-import dialog (action/cancel/dwell trio).
    "action.dialog.compass-import",
    "dialog-cancel.compass-import",
    "dwell.dialog.compass-import",
    // Collaboration-mode IPC commands (Ariane sibling .tml workflow).
    "command.set_collab_enabled",
    "command.set_collab_enabled_for_path",
    "command.set_project_name_for_path",
    "command.get_collab_state",
    "command.detect_collab_changes",
    "command.apply_collab_changes",
    "command.collab_sibling_when_disabled",
    // collab-review dialog (action/cancel/dwell trio).
    "action.dialog.collab-review",
    "dialog-cancel.collab-review",
    "dwell.dialog.collab-review",
    // SVG stick-map export preview command.
    "command.preview_svg_export",
    // Folder-access permission guidance dialog (action/cancel/dwell trio).
    "action.dialog.save-permission",
    "dialog-cancel.save-permission",
    "dwell.dialog.save-permission",
    // Structural-integrity IPC commands (check + repair).
    "command.check_project_integrity",
    "command.repair_project_integrity",
    // Closure-adjusted display mode (measured ↔ closure-adjusted) toggle + state.
    // Only the non-default state ("closureAdjusted") is emitted.
    "action.toggle.geometryMode",
    "display-state.geometryMode:closureAdjusted",
    // Lazy IPC query that returns the closure-adjusted station coordinates.
    "command.get_adjusted_stations",
    // Per-station along-cave distance query backing the distance-gradient overlay.
    "command.get_station_distances_from",
    // Data-table spreadsheet editing: persisted view config + raw-table browsing
    // (fullscreen data table).
    "command.update_data_table_view",
    "command.get_raw_table_names",
    "command.get_raw_table",
    // Ruler tool: along-cave-line distance + path shots between two endpoints (#26).
    "command.get_along_line_distance",
    // Per-project settings: validation thresholds stored in the .stalx (#57).
    "command.update_project_settings",
    // Project Settings dialog (#57).
    "action.dialog.project-settings",
    "dialog-cancel.project-settings",
    "dwell.dialog.project-settings",
    // Delete-project command + its confirm dialog (action/cancel/dwell trio) (#118).
    "command.delete_project",
    "action.dialog.delete-project",
    "dialog-cancel.delete-project",
    "dwell.dialog.delete-project",
    // Bulk shot deletion: contiguous-range preview + split/shift commands (#95).
    "command.preview_delete_shot_range",
    "command.delete_shot_range_split",
    "command.delete_shot_range_shift",
    "undo-after.delete_shot_range_split",
    "undo-after.delete_shot_range_shift",
    // Bulk-delete dialog (action/cancel/dwell trio) (#95).
    "action.dialog.bulk-delete",
    "dialog-cancel.bulk-delete",
    "dwell.dialog.bulk-delete",
    // Map-tile consumption per provider (#31): `tile.fetch.<provider>` = tiles pulled
    // fresh from the provider over the network; `tile.cache.<provider>` = tiles served
    // from the local cache. Add a `tile.fetch.<p>` + `tile.cache.<p>` pair here (and in
    // the worker registry) when a new provider is added to tile_providers.rs;
    // unregistered providers still report, via the overflow map.
    "tile.fetch.esri",
    "tile.cache.esri",
    // Active-dwell mirror of every dwell state (#31): like `dwell.<state>` but
    // only accumulates while the user interacted within the last 15s (active vs
    // merely open). Keep one per dwell entry (the mirror is asserted by a test).
    "active-dwell.tool.comment",
    "active-dwell.tool.delete",
    "active-dwell.tool.draw",
    "active-dwell.tool.measure",
    "active-dwell.tool.merge",
    "active-dwell.tool.move_comment",
    "active-dwell.tool.move_segment",
    "active-dwell.tool.move_shot",
    "active-dwell.tool.move_station",
    "active-dwell.tool.pan",
    "active-dwell.tool.select",
    "active-dwell.tool.zoom",
    "active-dwell.dialog.adjust-closures",
    "active-dwell.dialog.closure",
    "active-dwell.dialog.confirm-delete",
    "active-dwell.dialog.confirm-overwrite",
    "active-dwell.dialog.context-menu",
    "active-dwell.dialog.corruption-recovery",
    "active-dwell.dialog.debug",
    "active-dwell.dialog.dmp-import-new",
    "active-dwell.dialog.dmp-selection",
    "active-dwell.dialog.draw-shot",
    "active-dwell.dialog.edit-section",
    "active-dwell.dialog.edit-shot",
    "active-dwell.dialog.edit-station",
    "active-dwell.dialog.error",
    "active-dwell.dialog.export",
    "active-dwell.dialog.gpx-placement",
    "active-dwell.dialog.gpx-selection",
    "active-dwell.dialog.held-lock-resume",
    "active-dwell.dialog.import",
    "active-dwell.dialog.insert-station",
    "active-dwell.dialog.keyboard-shortcuts",
    "active-dwell.dialog.merge",
    "active-dwell.dialog.merge-projects",
    "active-dwell.dialog.new-project",
    "active-dwell.dialog.new-project-for-import",
    "active-dwell.dialog.orphan-placement",
    "active-dwell.dialog.remove-closure",
    "active-dwell.dialog.section-adjust",
    "active-dwell.dialog.settings",
    "active-dwell.dialog.open-from-speleodb",
    "active-dwell.dialog.speleodb-done",
    "active-dwell.dialog.speleodb-divergence",
    "active-dwell.dialog.speleodb-force-edit",
    "active-dwell.dialog.speleodb-close",
    "active-dwell.dialog.speleodb-no-permission",
    "active-dwell.dialog.speleodb-restore",
    "active-dwell.dialog.publish-to-speleodb",
    "active-dwell.dialog.split-section",
    "active-dwell.dialog.swap-comments",
    "active-dwell.dialog.tml-import",
    "active-dwell.dialog.unmerge-stations",
    "active-dwell.dialog.unsaved-changes",
    "active-dwell.dialog.zero-datum-config",
    "active-dwell.dialog.zero-datum-selection",
    "active-dwell.global-view",
    "active-dwell.placement",
    "active-dwell.dialog.delete-shot-choice",
    "active-dwell.dialog.compass-import",
    "active-dwell.dialog.collab-review",
    "active-dwell.dialog.save-permission",
    // Active-dwell mirrors for the dialogs master added (#57/#95/#118).
    "active-dwell.dialog.project-settings",
    "active-dwell.dialog.delete-project",
    "active-dwell.dialog.bulk-delete",
    // Walls (.wpj/.srv) survey import.
    "command.parse_walls",
    "command.execute_walls_import",
    // walls-import dialog (action/cancel/dwell + active-dwell mirror).
    "action.dialog.walls-import",
    "dialog-cancel.walls-import",
    "dwell.dialog.walls-import",
    "active-dwell.dialog.walls-import",
    // SpeleoDB per-endpoint traffic metrics (#210, F1). For each endpoint kind
    // (`speleodb::metrics::Endpoint`) the client emits four outcome counters
    // (`ok`/`client-error`/`server-error`/`network-error`, which sum to the call
    // count) plus a latency histogram (`latency.<bucket>`) the backend
    // reconstructs p50/p95 from. NO PII: only the endpoint kind, the outcome
    // class, and a latency bucket ever leave the app - never a host, project id,
    // email, token, or file bytes. Recorded per HTTP attempt at the
    // `client::with_retry` / `client::send_json_request` chokepoints (a retried
    // 5xx re-hits the server, so counting per attempt is what makes a retry storm
    // legible). `every_speleodb_endpoint_has_traffic_events` regenerates this
    // exact set from `Endpoint::ALL` x `Outcome::ALL` x `LATENCY_BUCKETS`: add an
    // endpoint kind and CI fails until its ten names are listed here.
    "speleodb-api.login.ok",
    "speleodb-api.login.client-error",
    "speleodb-api.login.server-error",
    "speleodb-api.login.network-error",
    "speleodb-api.login.latency.le-100",
    "speleodb-api.login.latency.le-500",
    "speleodb-api.login.latency.le-1000",
    "speleodb-api.login.latency.le-5000",
    "speleodb-api.login.latency.le-15000",
    "speleodb-api.login.latency.gt-15000",
    "speleodb-api.list-projects.ok",
    "speleodb-api.list-projects.client-error",
    "speleodb-api.list-projects.server-error",
    "speleodb-api.list-projects.network-error",
    "speleodb-api.list-projects.latency.le-100",
    "speleodb-api.list-projects.latency.le-500",
    "speleodb-api.list-projects.latency.le-1000",
    "speleodb-api.list-projects.latency.le-5000",
    "speleodb-api.list-projects.latency.le-15000",
    "speleodb-api.list-projects.latency.gt-15000",
    "speleodb-api.get-project.ok",
    "speleodb-api.get-project.client-error",
    "speleodb-api.get-project.server-error",
    "speleodb-api.get-project.network-error",
    "speleodb-api.get-project.latency.le-100",
    "speleodb-api.get-project.latency.le-500",
    "speleodb-api.get-project.latency.le-1000",
    "speleodb-api.get-project.latency.le-5000",
    "speleodb-api.get-project.latency.le-15000",
    "speleodb-api.get-project.latency.gt-15000",
    "speleodb-api.get-revisions.ok",
    "speleodb-api.get-revisions.client-error",
    "speleodb-api.get-revisions.server-error",
    "speleodb-api.get-revisions.network-error",
    "speleodb-api.get-revisions.latency.le-100",
    "speleodb-api.get-revisions.latency.le-500",
    "speleodb-api.get-revisions.latency.le-1000",
    "speleodb-api.get-revisions.latency.le-5000",
    "speleodb-api.get-revisions.latency.le-15000",
    "speleodb-api.get-revisions.latency.gt-15000",
    "speleodb-api.fetch-user.ok",
    "speleodb-api.fetch-user.client-error",
    "speleodb-api.fetch-user.server-error",
    "speleodb-api.fetch-user.network-error",
    "speleodb-api.fetch-user.latency.le-100",
    "speleodb-api.fetch-user.latency.le-500",
    "speleodb-api.fetch-user.latency.le-1000",
    "speleodb-api.fetch-user.latency.le-5000",
    "speleodb-api.fetch-user.latency.le-15000",
    "speleodb-api.fetch-user.latency.gt-15000",
    "speleodb-api.download-dump.ok",
    "speleodb-api.download-dump.client-error",
    "speleodb-api.download-dump.server-error",
    "speleodb-api.download-dump.network-error",
    "speleodb-api.download-dump.latency.le-100",
    "speleodb-api.download-dump.latency.le-500",
    "speleodb-api.download-dump.latency.le-1000",
    "speleodb-api.download-dump.latency.le-5000",
    "speleodb-api.download-dump.latency.le-15000",
    "speleodb-api.download-dump.latency.gt-15000",
    "speleodb-api.upload.ok",
    "speleodb-api.upload.client-error",
    "speleodb-api.upload.server-error",
    "speleodb-api.upload.network-error",
    "speleodb-api.upload.latency.le-100",
    "speleodb-api.upload.latency.le-500",
    "speleodb-api.upload.latency.le-1000",
    "speleodb-api.upload.latency.le-5000",
    "speleodb-api.upload.latency.le-15000",
    "speleodb-api.upload.latency.gt-15000",
    "speleodb-api.acquire.ok",
    "speleodb-api.acquire.client-error",
    "speleodb-api.acquire.server-error",
    "speleodb-api.acquire.network-error",
    "speleodb-api.acquire.latency.le-100",
    "speleodb-api.acquire.latency.le-500",
    "speleodb-api.acquire.latency.le-1000",
    "speleodb-api.acquire.latency.le-5000",
    "speleodb-api.acquire.latency.le-15000",
    "speleodb-api.acquire.latency.gt-15000",
    "speleodb-api.release.ok",
    "speleodb-api.release.client-error",
    "speleodb-api.release.server-error",
    "speleodb-api.release.network-error",
    "speleodb-api.release.latency.le-100",
    "speleodb-api.release.latency.le-500",
    "speleodb-api.release.latency.le-1000",
    "speleodb-api.release.latency.le-5000",
    "speleodb-api.release.latency.le-15000",
    "speleodb-api.release.latency.gt-15000",
    "speleodb-api.release-all.ok",
    "speleodb-api.release-all.client-error",
    "speleodb-api.release-all.server-error",
    "speleodb-api.release-all.network-error",
    "speleodb-api.release-all.latency.le-100",
    "speleodb-api.release-all.latency.le-500",
    "speleodb-api.release-all.latency.le-1000",
    "speleodb-api.release-all.latency.le-5000",
    "speleodb-api.release-all.latency.le-15000",
    "speleodb-api.release-all.latency.gt-15000",
    "speleodb-api.patch-project.ok",
    "speleodb-api.patch-project.client-error",
    "speleodb-api.patch-project.server-error",
    "speleodb-api.patch-project.network-error",
    "speleodb-api.patch-project.latency.le-100",
    "speleodb-api.patch-project.latency.le-500",
    "speleodb-api.patch-project.latency.le-1000",
    "speleodb-api.patch-project.latency.le-5000",
    "speleodb-api.patch-project.latency.le-15000",
    "speleodb-api.patch-project.latency.gt-15000",
    "speleodb-api.create-project.ok",
    "speleodb-api.create-project.client-error",
    "speleodb-api.create-project.server-error",
    "speleodb-api.create-project.network-error",
    "speleodb-api.create-project.latency.le-100",
    "speleodb-api.create-project.latency.le-500",
    "speleodb-api.create-project.latency.le-1000",
    "speleodb-api.create-project.latency.le-5000",
    "speleodb-api.create-project.latency.le-15000",
    "speleodb-api.create-project.latency.gt-15000",
    "speleodb-api.delete-project.ok",
    "speleodb-api.delete-project.client-error",
    "speleodb-api.delete-project.server-error",
    "speleodb-api.delete-project.network-error",
    "speleodb-api.delete-project.latency.le-100",
    "speleodb-api.delete-project.latency.le-500",
    "speleodb-api.delete-project.latency.le-1000",
    "speleodb-api.delete-project.latency.le-5000",
    "speleodb-api.delete-project.latency.le-15000",
    "speleodb-api.delete-project.latency.gt-15000",
    "speleodb-api.list-permissions.ok",
    "speleodb-api.list-permissions.client-error",
    "speleodb-api.list-permissions.server-error",
    "speleodb-api.list-permissions.network-error",
    "speleodb-api.list-permissions.latency.le-100",
    "speleodb-api.list-permissions.latency.le-500",
    "speleodb-api.list-permissions.latency.le-1000",
    "speleodb-api.list-permissions.latency.le-5000",
    "speleodb-api.list-permissions.latency.le-15000",
    "speleodb-api.list-permissions.latency.gt-15000",
    "speleodb-api.mutate-permission.ok",
    "speleodb-api.mutate-permission.client-error",
    "speleodb-api.mutate-permission.server-error",
    "speleodb-api.mutate-permission.network-error",
    "speleodb-api.mutate-permission.latency.le-100",
    "speleodb-api.mutate-permission.latency.le-500",
    "speleodb-api.mutate-permission.latency.le-1000",
    "speleodb-api.mutate-permission.latency.le-5000",
    "speleodb-api.mutate-permission.latency.le-15000",
    "speleodb-api.mutate-permission.latency.gt-15000",
    "speleodb-api.list-teams.ok",
    "speleodb-api.list-teams.client-error",
    "speleodb-api.list-teams.server-error",
    "speleodb-api.list-teams.network-error",
    "speleodb-api.list-teams.latency.le-100",
    "speleodb-api.list-teams.latency.le-500",
    "speleodb-api.list-teams.latency.le-1000",
    "speleodb-api.list-teams.latency.le-5000",
    "speleodb-api.list-teams.latency.le-15000",
    "speleodb-api.list-teams.latency.gt-15000",
    "speleodb-api.autocomplete.ok",
    "speleodb-api.autocomplete.client-error",
    "speleodb-api.autocomplete.server-error",
    "speleodb-api.autocomplete.network-error",
    "speleodb-api.autocomplete.latency.le-100",
    "speleodb-api.autocomplete.latency.le-500",
    "speleodb-api.autocomplete.latency.le-1000",
    "speleodb-api.autocomplete.latency.le-5000",
    "speleodb-api.autocomplete.latency.le-15000",
    "speleodb-api.autocomplete.latency.gt-15000",
];

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Bare names of commands removed from the app but kept in the vocabulary.
    /// They intentionally have no live handler and no frontend tracking, so the
    /// `command.*` and `undo-after.*` coverage checks skip them.
    const REMOVED_COMMANDS: &[&str] = &["adjust_loop_closures", "resolve_all_warnings"];

    /// Dialog keys removed from `App.tsx` whose registry slots are likewise
    /// retained. The `action.dialog.*` / `dialog-cancel.*` / `dwell.dialog.*`
    /// coverage check skips them.
    // Dialogs whose 4 event slots are retained but excluded from the
    // `activeDialog`-derived stale check: either genuinely removed
    // (`adjust-closures`) or relocated OUT of the single `activeDialog` slot into an
    // independent overlay (`held-lock-resume`, #201 - so it can layer over another
    // dialog during login). The slots are kept for event-numbering stability.
    const REMOVED_DIALOGS: &[&str] = &["adjust-closures", "held-lock-resume"];

    /// `action.toggle.*` events that are intentionally NOT derived from
    /// `DISPLAY_DEFAULTS`. `geometryMode` is an enum display mode tracked via an
    /// explicit `trackAction('toggle.geometryMode')`, not a boolean default.
    const EXTRA_TOGGLES: &[&str] = &["action.toggle.geometryMode"];

    #[test]
    fn no_duplicate_event_names() {
        let mut seen = HashSet::new();
        for (i, &name) in EVENT_NAMES.iter().enumerate() {
            assert!(
                seen.insert(name),
                "Duplicate event name at index {i}: {name}"
            );
        }
    }

    #[test]
    fn no_duplicate_source_names() {
        let mut seen = HashSet::new();
        for (i, &name) in SOURCE_NAMES.iter().enumerate() {
            if name.is_empty() {
                continue;
            }
            assert!(
                seen.insert(name),
                "Duplicate source name at index {i}: {name}"
            );
        }
    }

    #[test]
    fn event_count() {
        // 401 on the pre-feature master
        //   + 7 walls-import events + 1 compass-import active-dwell mirror (#223)
        //   + 9 SpeleoDB events (#183)        = 418
        //   + 1 gas event (#38):
        //       command.compute_gas_reach_field = 419
        //   + 3 SpeleoDB sync events (#194/#197):
        //       speleodb_{start_editing,done,update} = 422.
        //   + 1 SpeleoDB link-status event (#204):
        //       command.speleodb_link_status = 423.
        //   + 4 open-from-speleodb dialog events (#203):
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .open-from-speleodb = 427.
        //   + 1 SpeleoDB push event (T2d): command.speleodb_push = 428.
        //   + 4 speleodb-divergence dialog events (#198): = 432.
        //   + 4 speleodb-close dialog events (T2d): = 436.
        //   + 1 SpeleoDB unlink event (#204 convert-to-unmanaged):
        //       command.speleodb_unlink = 437.
        //   + 4 speleodb-no-permission dialog events (#204): = 441.
        //   + 1 SpeleoDB take-remote event (#198):
        //       command.speleodb_take_remote = 442.
        //   + 1 SpeleoDB release-all-locks event (#201):
        //       command.speleodb_release_all_locks = 443.
        //   + 4 held-lock-resume dialog events (#201): = 447.
        //   + 1 SpeleoDB set-exclude-geojson event (#200):
        //       command.speleodb_set_exclude_geojson = 448.
        //   + 1 SpeleoDB per-project release event (#201 individual release):
        //       command.speleodb_release_lock = 449.
        //   + 3 SpeleoDB Project Settings + publish events (#206/#207):
        //       command.speleodb_{get_project_metadata,patch_project,publish} = 452.
        //   + 4 publish-to-speleodb dialog events (#207):
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .publish-to-speleodb = 456.
        //   + 10 SpeleoDB full Project Settings commands (#215):
        //       command.speleodb_{list_collaborators,add_collaborator,set_collaborator,
        //       remove_collaborator,add_team,set_team,remove_team,list_teams,
        //       release_current_lock,delete_project} = 466.
        //   + 1 SpeleoDB collaborator autocomplete command (#215 follow-up):
        //       command.speleodb_search_users = 467.
        //   + 170 SpeleoDB per-endpoint traffic events (#210, F1): 17 endpoint
        //       kinds x (4 outcome counters + 6 latency buckets) = 170. Asserted
        //       exhaustively by `every_speleodb_endpoint_has_traffic_events`:
        //       467 + 170 = 637.
        //   + 1 SpeleoDB adopt-Ariane command (#214, C2):
        //       command.speleodb_adopt_ariane = 638.
        //   + 1 SpeleoDB force-edit command (#213, E2):
        //       command.speleodb_force_edit = 639.
        //   + 4 speleodb-force-edit dialog events (#213):
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-force-edit = 643.
        //   + 1 SpeleoDB revision-history command (#218):
        //       command.speleodb_get_revisions = 644.
        //   + 1 SpeleoDB revision-restore command (#218):
        //       command.speleodb_restore_revision = 645.
        //   + 4 speleodb-restore dialog events (#218):
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-restore = 649.
        //   + 4 dive-plan (.plnx) commands (#244):
        //       command.{list,load,save,delete}_dive_plan = 653.
        //   + 2 agent-harness observability commands:
        //       command.get_recent_logs_since + command.debug_trigger_panic = 655.
        //   - 1 retired DepthPreset command (#275, no caller on either side):
        //       command.get_depth_preset_colors removed = 654.
        //   - 5 retired dead commands (#275, no frontend caller): command.
        //       {create_project, get_connected_section_ids, get_display_setting,
        //       get_section, get_survey_geometry} removed = 649.
        //   - 1 retired dead command (#275, superseded by load_project_full,
        //       zero frontend invokers left after the #275 batch removed the
        //       last caller): command.open_project removed = 648.
        //   + 6 insert-station split-shot events (#59):
        //       command.split_shot_at_distance, undo-after.split_shot_at_distance,
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .insert-station = 654.
        //   + 6 marker commands (#256):
        //       command.{add,update,delete}_marker + undo-after.{add,update,delete}_marker = 660.
        assert_eq!(EVENT_NAMES.len(), 660);
    }

    /// Active-dwell mirrors every dwell state 1:1 (#31): for each `dwell.<key>` there
    /// must be a matching `active-dwell.<key>`. Catches a dwell state added without
    /// its active-dwell twin (which would otherwise silently land in the overflow map).
    #[test]
    fn active_dwell_mirrors_dwell() {
        for name in EVENT_NAMES {
            if let Some(key) = name.strip_prefix("dwell.") {
                let mirror = format!("active-dwell.{key}");
                assert!(
                    EVENT_NAMES.iter().any(|&e| e == mirror),
                    "dwell state '{name}' has no '{mirror}' — add it here and in \
                     telemetry-worker/src/registry.ts"
                );
            }
        }
    }

    /// Every tile provider must have its per-provider consumption events registered
    /// (#31), so tile counts land as registered events rather than the overflow map.
    /// Catches "added a provider but forgot the telemetry events".
    #[test]
    fn every_tile_provider_has_consumption_events() {
        for info in crate::tile_providers::get_all_provider_info() {
            for kind in ["fetch", "cache"] {
                let event = format!("tile.{kind}.{}", info.name);
                assert!(
                    EVENT_NAMES.iter().any(|&e| e == event),
                    "provider '{}' missing telemetry event '{event}' — add it to EVENT_NAMES \
                     here and in telemetry-worker/src/registry.ts",
                    info.name
                );
            }
        }
    }

    /// Every SpeleoDB endpoint kind (#210, F1) must have its full traffic-event
    /// set registered: four outcome counters + one counter per latency bucket.
    /// Mirrors `every_tile_provider_has_consumption_events` - add an `Endpoint`
    /// variant and this fails until its ten names land in `EVENT_NAMES`. Also
    /// asserts the reverse: no orphan `speleodb-api.*` name the generator cannot
    /// produce (a typo, or a renamed/removed kind), so the vocabulary can't drift
    /// away from `speleodb::metrics::Endpoint`.
    #[test]
    fn every_speleodb_endpoint_has_traffic_events() {
        use crate::speleodb::metrics::{Endpoint, Outcome, LATENCY_BUCKETS};

        let registered: HashSet<&str> = EVENT_NAMES.iter().copied().collect();
        let mut expected: HashSet<String> = HashSet::new();
        for ep in Endpoint::ALL {
            let kind = ep.kind();
            for outcome in Outcome::ALL {
                let event = format!("speleodb-api.{kind}.{outcome}");
                assert!(
                    registered.contains(event.as_str()),
                    "endpoint '{kind}' missing outcome event '{event}' - add it to EVENT_NAMES"
                );
                expected.insert(event);
            }
            for bucket in LATENCY_BUCKETS {
                let event = format!("speleodb-api.{kind}.latency.{bucket}");
                assert!(
                    registered.contains(event.as_str()),
                    "endpoint '{kind}' missing latency event '{event}' - add it to EVENT_NAMES"
                );
                expected.insert(event);
            }
        }
        for name in EVENT_NAMES {
            if name.starts_with("speleodb-api.") {
                assert!(
                    expected.contains(*name),
                    "orphan speleodb-api event '{name}' - not produced by Endpoint::ALL \
                     x Outcome::ALL x LATENCY_BUCKETS; remove it or fix the kind/label"
                );
            }
        }
    }

    #[test]
    fn source_count() {
        assert_eq!(SOURCE_NAMES.len(), 19);
    }

    /// Commands registered in `generate_handler![]` that are excluded from
    /// the `command.*` exhaustiveness check (they are telemetry internals).
    const TELEMETRY_COMMANDS: &[&str] = &[
        "track_events",
        "flush_telemetry",
        "is_telemetry_available",
        "set_telemetry_enabled",
    ];

    #[test]
    fn registry_covers_all_commands() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let lib_path = format!("{manifest_dir}/src/lib.rs");
        let lib_src = std::fs::read_to_string(&lib_path)
            .unwrap_or_else(|e| panic!("Failed to read {lib_path}: {e}"));

        // Extract the generate_handler![...] block
        let start = lib_src
            .find("generate_handler![")
            .expect("generate_handler![ not found in lib.rs")
            + "generate_handler![".len();
        let end = start
            + lib_src[start..]
                .find(']')
                .expect("closing ] not found for generate_handler!");
        let block = &lib_src[start..end];

        // Parse command names from each line
        let mut handler_commands: HashSet<String> = HashSet::new();
        for line in block.lines() {
            let trimmed = line.trim();
            // Skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            // Extract the last :: segment, e.g. "commands::project::create_project,"
            let name = trimmed
                .rsplit("::")
                .next()
                .unwrap()
                .trim_end_matches(',')
                .trim();
            if !TELEMETRY_COMMANDS.contains(&name) {
                handler_commands.insert(format!("command.{name}"));
            }
        }

        // Build set of command.* entries from EVENT_NAMES, excluding the retained
        // slots of removed commands (which have no live handler by design).
        let removed: HashSet<String> = REMOVED_COMMANDS
            .iter()
            .map(|c| format!("command.{c}"))
            .collect();
        let registry_commands: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("command."))
            .map(|n| (*n).to_string())
            .filter(|n| !removed.contains(n))
            .collect();

        let missing: Vec<_> = handler_commands.difference(&registry_commands).collect();
        let stale: Vec<_> = registry_commands.difference(&handler_commands).collect();

        assert!(
            missing.is_empty() && stale.is_empty(),
            "EVENT_NAMES command.* entries out of sync with generate_handler![]:\n  \
             Missing from registry: {missing:?}\n  \
             Stale in registry: {stale:?}"
        );
    }

    /// Helper: read a frontend source file relative to the project root.
    fn read_frontend_file(rel_path: &str) -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = format!("{manifest_dir}/../{rel_path}");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {path}: {e}"))
    }

    #[test]
    fn registry_covers_all_tools() {
        let src = read_frontend_file("src/store/uiStore.ts");

        // Extract ToolType union members
        let start = src
            .find("type ToolType =")
            .expect("ToolType not found in uiStore.ts");
        let block = &src[start..];
        let mut tools: HashSet<String> = HashSet::new();
        for line in block.lines().skip(1) {
            let trimmed = line.trim();
            if !trimmed.starts_with('|') {
                break;
            }
            if let Some(q1) = trimmed.find('\'') {
                if let Some(q2) = trimmed[q1 + 1..].find('\'') {
                    tools.insert(trimmed[q1 + 1..q1 + 1 + q2].to_string());
                }
            }
        }
        assert!(!tools.is_empty(), "Failed to parse any ToolType variants");

        let code_action: HashSet<String> =
            tools.iter().map(|t| format!("action.tool.{t}")).collect();
        let code_dwell: HashSet<String> = tools.iter().map(|t| format!("dwell.tool.{t}")).collect();

        let reg_action: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("action.tool."))
            .map(|n| (*n).to_string())
            .collect();
        let reg_dwell: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("dwell.tool."))
            .map(|n| (*n).to_string())
            .collect();

        // Tools removed from the UI whose event names are kept in the vocabulary
        // (their events simply never fire anymore). The registry is allowed to
        // retain these even though they're gone from ToolType — but every
        // *current* tool must still be covered.
        const RETIRED_TOOLS: &[&str] = &["draw", "delete"];
        let retired_action: HashSet<String> = RETIRED_TOOLS
            .iter()
            .map(|t| format!("action.tool.{t}"))
            .collect();
        let retired_dwell: HashSet<String> = RETIRED_TOOLS
            .iter()
            .map(|t| format!("dwell.tool.{t}"))
            .collect();

        let missing_action: Vec<_> = code_action.difference(&reg_action).collect();
        let stale_action: Vec<_> = reg_action
            .difference(&code_action)
            .filter(|n| !retired_action.contains(*n))
            .collect();
        let missing_dwell: Vec<_> = code_dwell.difference(&reg_dwell).collect();
        let stale_dwell: Vec<_> = reg_dwell
            .difference(&code_dwell)
            .filter(|n| !retired_dwell.contains(*n))
            .collect();

        assert!(
            missing_action.is_empty()
                && stale_action.is_empty()
                && missing_dwell.is_empty()
                && stale_dwell.is_empty(),
            "Tool registry out of sync with ToolType:\n  \
             Missing action.tool.*: {missing_action:?}\n  \
             Stale action.tool.*: {stale_action:?}\n  \
             Missing dwell.tool.*: {missing_dwell:?}\n  \
             Stale dwell.tool.*: {stale_dwell:?}"
        );
    }

    #[test]
    fn registry_covers_all_undoable_commands() {
        let src = read_frontend_file("src/utils/telemetry.ts");

        // Extract UNDOABLE_COMMANDS set members
        let start = src
            .find("UNDOABLE_COMMANDS")
            .expect("UNDOABLE_COMMANDS not found");
        let bracket = src[start..]
            .find('[')
            .expect("[ not found after UNDOABLE_COMMANDS");
        let end_bracket = src[start + bracket..]
            .find(']')
            .expect("] not found after UNDOABLE_COMMANDS [");
        let block = &src[start + bracket..start + bracket + end_bracket];

        let mut commands: HashSet<String> = HashSet::new();
        for cap in block.split('\'').collect::<Vec<_>>().chunks(2) {
            if cap.len() == 2 && !cap[1].is_empty() {
                // cap[1] is the string between quotes
                commands.insert(cap[1].to_string());
            }
        }
        // Better approach: extract all single-quoted strings
        let mut commands2: HashSet<String> = HashSet::new();
        let mut chars = block.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                let s: String = chars.by_ref().take_while(|&ch| ch != '\'').collect();
                if !s.is_empty() {
                    commands2.insert(s);
                }
            }
        }
        let commands = commands2;
        assert!(
            !commands.is_empty(),
            "Failed to parse any UNDOABLE_COMMANDS"
        );

        let code_undo: HashSet<String> =
            commands.iter().map(|c| format!("undo-after.{c}")).collect();

        // Exclude the retained undo-after slots of removed commands.
        let removed: HashSet<String> = REMOVED_COMMANDS
            .iter()
            .map(|c| format!("undo-after.{c}"))
            .collect();
        let reg_undo: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("undo-after."))
            .map(|n| (*n).to_string())
            .filter(|n| !removed.contains(n))
            .collect();

        let missing: Vec<_> = code_undo.difference(&reg_undo).collect();
        let stale: Vec<_> = reg_undo.difference(&code_undo).collect();

        assert!(
            missing.is_empty() && stale.is_empty(),
            "undo-after.* registry out of sync with UNDOABLE_COMMANDS:\n  \
             Missing: {missing:?}\n  \
             Stale: {stale:?}"
        );
    }

    #[test]
    fn registry_covers_all_display_toggles() {
        let src = read_frontend_file("src/store/displaySelectors.ts");

        // Extract DISPLAY_DEFAULTS keys
        let start = src
            .find("DISPLAY_DEFAULTS = {")
            .expect("DISPLAY_DEFAULTS not found");
        let brace = start + "DISPLAY_DEFAULTS = {".len();
        let end_brace = src[brace..]
            .find('}')
            .expect("} not found after DISPLAY_DEFAULTS");
        let block = &src[brace..brace + end_brace];

        let mut keys: HashSet<String> = HashSet::new();
        for line in block.lines() {
            let trimmed = line.trim();
            if let Some(colon_pos) = trimmed.find(':') {
                let key = trimmed[..colon_pos].trim();
                if !key.is_empty() && !key.starts_with("//") {
                    keys.insert(key.to_string());
                }
            }
        }
        assert!(
            !keys.is_empty(),
            "Failed to parse any DISPLAY_DEFAULTS keys"
        );

        let code_toggles: HashSet<String> =
            keys.iter().map(|k| format!("action.toggle.{k}")).collect();

        // Exclude toggles that are intentionally not DISPLAY_DEFAULTS booleans.
        let extra: HashSet<String> = EXTRA_TOGGLES.iter().map(|s| (*s).to_string()).collect();
        let reg_toggles: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("action.toggle."))
            .map(|n| (*n).to_string())
            .filter(|n| !extra.contains(n))
            .collect();

        let missing: Vec<_> = code_toggles.difference(&reg_toggles).collect();
        let stale: Vec<_> = reg_toggles.difference(&code_toggles).collect();

        assert!(
            missing.is_empty() && stale.is_empty(),
            "action.toggle.* registry out of sync with DISPLAY_DEFAULTS:\n  \
             Missing: {missing:?}\n  \
             Stale: {stale:?}"
        );
    }

    #[test]
    fn registry_covers_all_dialogs() {
        let src = read_frontend_file("src/App.tsx");

        // Extract all activeDialog === '...' patterns
        let re_pattern = "activeDialog === '";
        let mut dialogs: HashSet<String> = HashSet::new();
        for line in src.lines() {
            let mut search = line;
            while let Some(pos) = search.find(re_pattern) {
                let after = &search[pos + re_pattern.len()..];
                if let Some(end) = after.find('\'') {
                    dialogs.insert(after[..end].to_string());
                }
                search = &search[pos + re_pattern.len()..];
            }
        }
        assert!(
            !dialogs.is_empty(),
            "Failed to parse any dialogs from App.tsx"
        );

        // Build expected sets
        let code_action: HashSet<String> = dialogs
            .iter()
            .map(|d| format!("action.dialog.{d}"))
            .collect();
        let code_cancel: HashSet<String> = dialogs
            .iter()
            .map(|d| format!("dialog-cancel.{d}"))
            .collect();
        let code_dwell: HashSet<String> = dialogs
            .iter()
            .map(|d| format!("dwell.dialog.{d}"))
            .collect();

        // Retained slots of removed dialogs are excluded from the stale checks.
        let removed_action: HashSet<String> = REMOVED_DIALOGS
            .iter()
            .map(|d| format!("action.dialog.{d}"))
            .collect();
        let removed_cancel: HashSet<String> = REMOVED_DIALOGS
            .iter()
            .map(|d| format!("dialog-cancel.{d}"))
            .collect();
        let removed_dwell: HashSet<String> = REMOVED_DIALOGS
            .iter()
            .map(|d| format!("dwell.dialog.{d}"))
            .collect();
        let reg_action: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("action.dialog."))
            .map(|n| (*n).to_string())
            .filter(|n| !removed_action.contains(n))
            .collect();
        let reg_cancel: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("dialog-cancel."))
            .map(|n| (*n).to_string())
            .filter(|n| !removed_cancel.contains(n))
            .collect();
        let reg_dwell: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("dwell.dialog."))
            .map(|n| (*n).to_string())
            .filter(|n| !removed_dwell.contains(n))
            .collect();

        let missing_action: Vec<_> = code_action.difference(&reg_action).collect();
        let stale_action: Vec<_> = reg_action.difference(&code_action).collect();
        let missing_cancel: Vec<_> = code_cancel.difference(&reg_cancel).collect();
        let stale_cancel: Vec<_> = reg_cancel.difference(&code_cancel).collect();
        let missing_dwell: Vec<_> = code_dwell.difference(&reg_dwell).collect();
        let stale_dwell: Vec<_> = reg_dwell.difference(&code_dwell).collect();

        assert!(
            missing_action.is_empty()
                && stale_action.is_empty()
                && missing_cancel.is_empty()
                && stale_cancel.is_empty()
                && missing_dwell.is_empty()
                && stale_dwell.is_empty(),
            "Dialog registry out of sync with App.tsx activeDialog values:\n  \
             Missing action.dialog.*: {missing_action:?}\n  \
             Stale action.dialog.*: {stale_action:?}\n  \
             Missing dialog-cancel.*: {missing_cancel:?}\n  \
             Stale dialog-cancel.*: {stale_cancel:?}\n  \
             Missing dwell.dialog.*: {missing_dwell:?}\n  \
             Stale dwell.dialog.*: {stale_dwell:?}"
        );
    }

    #[test]
    fn registry_covers_all_sources() {
        let src = read_frontend_file("src/utils/telemetry.ts");
        let sources = extract_type_union(&src, "SourceName");

        // SourceName excludes "" (no source) and "unknown" (default fallback)
        // which are registry-only entries. Add them to get the full set.
        let excluded: HashSet<&str> = ["", "unknown"].into();

        let reg_sources: HashSet<String> = SOURCE_NAMES
            .iter()
            .filter(|s| !excluded.contains(*s))
            .map(|s| (*s).to_string())
            .collect();

        let code_sources: HashSet<String> = sources.into_iter().collect();

        let missing: Vec<_> = code_sources.difference(&reg_sources).collect();
        let stale: Vec<_> = reg_sources.difference(&code_sources).collect();

        assert!(
            missing.is_empty() && stale.is_empty(),
            "SOURCE_NAMES out of sync with SourceName type union:\n  \
             Missing from registry: {missing:?}\n  \
             Stale in registry: {stale:?}"
        );
    }

    /// Parse a TS type union like `type Foo = | 'a' | 'b' | 'c';`.
    /// Extracts single-quoted strings from `|`-prefixed lines after
    /// `type {name} =`.
    fn extract_type_union(src: &str, name: &str) -> Vec<String> {
        let marker = format!("type {name} =");
        let start = src
            .find(&marker)
            .unwrap_or_else(|| panic!("type {name} not found in source"));
        let block = &src[start + marker.len()..];
        let mut items = Vec::new();
        for line in block.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('|') {
                if !items.is_empty() {
                    break; // end of union
                }
                continue; // skip the `=` line itself if no `|` yet
            }
            if let Some(q1) = trimmed.find('\'') {
                if let Some(q2) = trimmed[q1 + 1..].find('\'') {
                    items.push(trimmed[q1 + 1..q1 + 1 + q2].to_string());
                }
            }
        }
        assert!(
            !items.is_empty(),
            "Failed to parse any variants from type {name}"
        );
        items
    }

    #[test]
    fn registry_covers_all_nav_events() {
        let src = read_frontend_file("src/utils/telemetry.ts");
        let navs = extract_type_union(&src, "NavigationAction");

        let code_action: HashSet<String> = navs.iter().map(|n| format!("action.nav.{n}")).collect();

        let reg_action: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("action.nav."))
            .map(|n| (*n).to_string())
            .collect();

        let missing: Vec<_> = code_action.difference(&reg_action).collect();
        let stale: Vec<_> = reg_action.difference(&code_action).collect();

        assert!(
            missing.is_empty() && stale.is_empty(),
            "action.nav.* registry out of sync with NavigationAction:\n  \
             Missing: {missing:?}\n  \
             Stale: {stale:?}"
        );
    }

    #[test]
    fn registry_covers_all_panel_events() {
        let src = read_frontend_file("src/utils/telemetry.ts");
        let panels = extract_type_union(&src, "PanelName");

        // Each panel emits open/close (visibility) and pin/unpin (mode) events.
        let verbs = ["open", "close", "pin", "unpin"];
        let mut missing: Vec<String> = Vec::new();
        let mut stale: Vec<String> = Vec::new();
        for verb in verbs {
            let prefix = format!("action.panel-{verb}.");
            let code: HashSet<String> = panels.iter().map(|p| format!("{prefix}{p}")).collect();
            let reg: HashSet<String> = EVENT_NAMES
                .iter()
                .filter(|n| n.starts_with(&prefix))
                .map(|n| (*n).to_string())
                .collect();
            missing.extend(code.difference(&reg).cloned());
            stale.extend(reg.difference(&code).cloned());
        }

        assert!(
            missing.is_empty() && stale.is_empty(),
            "Panel registry out of sync with PanelName:\n  \
             Missing: {missing:?}\n  \
             Stale: {stale:?}"
        );
    }

    #[test]
    fn registry_covers_all_selections() {
        let src = read_frontend_file("src/utils/telemetry.ts");
        let types = extract_type_union(&src, "SelectionType");

        let code_select: HashSet<String> =
            types.iter().map(|t| format!("action.select.{t}")).collect();
        let code_add: HashSet<String> = types
            .iter()
            .map(|t| format!("action.select-add.{t}"))
            .collect();

        let reg_select: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("action.select."))
            .map(|n| (*n).to_string())
            .collect();
        let reg_add: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| n.starts_with("action.select-add."))
            .map(|n| (*n).to_string())
            .collect();

        let missing_select: Vec<_> = code_select.difference(&reg_select).collect();
        let stale_select: Vec<_> = reg_select.difference(&code_select).collect();
        let missing_add: Vec<_> = code_add.difference(&reg_add).collect();
        let stale_add: Vec<_> = reg_add.difference(&code_add).collect();

        assert!(
            missing_select.is_empty()
                && stale_select.is_empty()
                && missing_add.is_empty()
                && stale_add.is_empty(),
            "Selection registry out of sync with SelectionType:\n  \
             Missing select.*: {missing_select:?}\n  \
             Stale select.*: {stale_select:?}\n  \
             Missing select-add.*: {missing_add:?}\n  \
             Stale select-add.*: {stale_add:?}"
        );
    }

    #[test]
    fn registry_covers_all_context_menu_actions() {
        let src = read_frontend_file("src/utils/telemetry.ts");
        let actions = extract_type_union(&src, "ContextMenuAction");

        let code_actions: HashSet<String> = actions.iter().map(|a| format!("action.{a}")).collect();

        let reg_actions: HashSet<String> = EVENT_NAMES
            .iter()
            .filter(|n| {
                n.starts_with("action.")
                    && !n.starts_with("action.tool.")
                    && !n.starts_with("action.nav.")
                    && !n.starts_with("action.toggle.")
                    && !n.starts_with("action.select")
                    && !n.starts_with("action.dialog.")
                    && !n.starts_with("action.panel-")
            })
            .map(|n| (*n).to_string())
            .collect();

        let missing: Vec<_> = code_actions.difference(&reg_actions).collect();
        let stale: Vec<_> = reg_actions.difference(&code_actions).collect();

        assert!(
            missing.is_empty() && stale.is_empty(),
            "Context-menu action registry out of sync with ContextMenuAction:\n  \
             Missing: {missing:?}\n  \
             Stale: {stale:?}"
        );
    }

    /// Helper: walk directory recursively collecting .ts/.tsx files.
    fn walk_ts_files(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk_ts_files(&path, files);
                } else if let Some(ext) = path.extension() {
                    if ext == "ts" || ext == "tsx" {
                        files.push(path);
                    }
                }
            }
        }
    }

    #[test]
    fn dialog_complete_calls_match_dialogs() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let src_dir = std::path::Path::new(manifest_dir).join("../src");
        let mut ts_files = Vec::new();
        walk_ts_files(&src_dir, &mut ts_files);
        assert!(!ts_files.is_empty(), "No .ts/.tsx files found in src/");

        // Collect all dialog names from trackDialogComplete('...') calls
        let pattern = "trackDialogComplete('";
        let mut complete_names: HashSet<String> = HashSet::new();
        for file in &ts_files {
            let content = std::fs::read_to_string(file).unwrap();
            let mut search = content.as_str();
            while let Some(pos) = search.find(pattern) {
                let after = &search[pos + pattern.len()..];
                if let Some(end) = after.find('\'') {
                    complete_names.insert(after[..end].to_string());
                }
                search = &search[pos + pattern.len()..];
            }
        }
        assert!(
            !complete_names.is_empty(),
            "No trackDialogComplete() calls found in src/"
        );

        // All dialog names from App.tsx (canonical list)
        let app_src = read_frontend_file("src/App.tsx");
        let dialog_pattern = "activeDialog === '";
        let mut app_dialogs: HashSet<String> = HashSet::new();
        let mut search = app_src.as_str();
        while let Some(pos) = search.find(dialog_pattern) {
            let after = &search[pos + dialog_pattern.len()..];
            if let Some(end) = after.find('\'') {
                app_dialogs.insert(after[..end].to_string());
            }
            search = &search[pos + dialog_pattern.len()..];
        }

        // Every trackDialogComplete name must be a real dialog
        let invalid: Vec<_> = complete_names.difference(&app_dialogs).collect();
        assert!(
            invalid.is_empty(),
            "trackDialogComplete() called with names not in App.tsx dialog list:\n  \
             {invalid:?}\n  \
             These would silently produce unregistered dialog-cancel events."
        );
    }
}
