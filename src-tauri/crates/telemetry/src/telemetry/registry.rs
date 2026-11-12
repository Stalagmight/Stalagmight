//! Canonical vocabulary of telemetry event and source names.
//!
//! The wire format sends event names verbatim (self-describing JSON), so these
//! are no longer a numeric ID map - they exist to back the coverage tests below
//! (every command / tool / dialog has a tracked event; typo detection).

/// Known interaction-source names (appended to an event as `:source`).
pub const SOURCE_NAMES: &[&str] = &[
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
    // File handed to us by the OS: double-click / "Open with" (#424).
    "os-file-association",
];

/// Known event base names (the `:source` suffix is appended at runtime). The
/// canonical vocabulary the coverage tests check against; order is irrelevant.
pub const EVENT_NAMES: &[&str] = &[
    // ── Commands ──
    // `adjust_loop_closures` was removed when closure adjustment became a
    // non-destructive display mode. Its name is kept in the vocabulary for
    // historical continuity; it has no live handler, so the command/undo
    // coverage tests skip it via REMOVED_COMMANDS.
    "command.adjust_loop_closures",
    "command.arm_app_update_restart",
    "command.arm_forced_exit",
    "command.attempt_repair",
    "command.calculate_file_hash",
    "command.can_redo",
    "command.can_undo",
    "command.cancel_loading",
    "command.check_duplicate_import",
    "command.check_for_app_update",
    "command.check_merge_connectivity",
    "command.check_section_hashes",
    "command.clear_tile_cache",
    "command.close_project",
    "command.take_unrecognised_shot_kinds",
    "command.take_pending_launch_files",
    "command.compute_gas_reach_field",
    "command.count_unanchored_sections",
    "command.create_loop_closure",
    "command.create_project_full",
    "command.create_section",
    "command.create_section_and_merge",
    "command.create_shot",
    "command.debug_trigger_panic",
    "command.delete_dive_plan",
    "command.delete_last_shot_and_section",
    "command.delete_section",
    "command.delete_shot",
    "command.delete_shot_and_split",
    "command.disarm_forced_exit",
    "command.discard_app_update_download",
    "command.download_app_update",
    "command.execute_batch",
    "command.execute_gpx_import",
    "command.execute_tml_import",
    "command.export_csv",
    "command.export_dive_profile",
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
    "command.get_tile_attribution",
    "command.get_tile_fetch_stats",
    "command.get_tile_providers",
    "command.get_undo_stack",
    "command.history_discarded",
    "command.import_dmp",
    "command.import_sections",
    "command.add_project_file",
    "command.install_app_update_and_restart",
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
    "command.plan_anchors_for_refs",
    "command.plan_refs_for_anchors",
    "command.preview_delete_shot",
    "command.push_frontend_log",
    "command.redo",
    "command.redo_to",
    "command.reload_config",
    "command.remove_from_recent_projects",
    "command.remove_loop_closure",
    "command.remove_zero_datum",
    "command.render_dive_profile_text",
    // Retained for telemetry-ID stability; the command was removed (#57). See REMOVED_COMMANDS.
    "command.resolve_all_warnings",
    "command.resolve_warning",
    "command.restore_from_backup",
    "command.reverse_section",
    "command.save_display_setting",
    "command.save_dive_plan",
    "command.save_project",
    // Recent-projects rename-by-path command (recent-projects metadata panel;
    // project need not be open).
    "command.set_project_name_for_path",
    "command.set_shot_kind",
    "command.set_station_comment",
    "command.set_zero_datum",
    "command.speleodb_done",
    "command.speleodb_get_project_metadata",
    "command.speleodb_get_revisions",
    "command.speleodb_link_status",
    "command.speleodb_list_projects",
    "command.speleodb_local_project_states",
    "command.speleodb_login",
    "command.speleodb_logout",
    "command.speleodb_open_project",
    "command.speleodb_open_project_discarding_local",
    "command.speleodb_project_access_at",
    "command.speleodb_publish",
    "command.speleodb_push",
    "command.speleodb_release_current_lock",
    "command.speleodb_release_lock",
    "command.speleodb_release_lock_acquired_here",
    "command.speleodb_settle_open_lock",
    "command.speleodb_restore_revision",
    "command.speleodb_session",
    "command.speleodb_start_editing",
    "command.speleodb_take_remote",
    "command.speleodb_unlink",
    "command.speleodb_remove_link_at",
    "command.speleodb_unwind_target",
    "command.speleodb_rewind_target",
    "command.speleodb_discard_changes",
    "command.speleodb_discard_cached_copy",
    "command.speleodb_cached_projects",
    "command.speleodb_remove_cached_copy",
    "command.speleodb_purge_cached_copies",
    "command.speleodb_update",
    "command.split_shot_at_distance",
    "command.suggest_all_import_placements",
    "command.suggest_import_placement",
    "command.suggest_orphan_placements",
    "command.unarm_app_update_restart",
    "command.undo",
    "command.undo_to",
    "command.unmerge_station_pair",
    "command.unmerge_stations",
    "command.unresolve_warning",
    "command.update_config",
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
    // ── Action: display toggles (114-131) ──
    "action.toggle.gridSpacing",
    "action.toggle.showDepthGradient",
    "action.toggle.showDevStats",
    "action.toggle.showMemoryStats",
    "action.toggle.showMapStats",
    "action.toggle.showZoomStats",
    "action.toggle.showGrid",
    "action.toggle.showReferenceShots",
    "action.toggle.showJumps",
    "action.toggle.showSatellite",
    "action.toggle.showSectionNames",
    "action.toggle.showShotComments",
    "action.toggle.showShotData",
    "action.toggle.showShotErrors",
    "action.toggle.showShots",
    "action.toggle.showStationComments",
    "action.toggle.showStationLabels",
    "action.toggle.showStationNames",
    "action.toggle.showStations",
    "action.toggle.showZeroDatum",
    // ── Action: selection (132-136) ──
    "action.select.section",
    "action.select.shot",
    "action.select.station",
    "action.select-add.shot",
    "action.select-add.station",
    // ── Action: dialogs (137-170) ──
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
    "action.dialog.import-blocked",
    "action.dialog.launch-open-confirm",
    "action.dialog.insert-station",
    "action.dialog.keyboard-shortcuts",
    "action.dialog.merge",
    "action.dialog.merge-projects",
    "action.dialog.new-project",
    "action.dialog.new-project-for-import",
    "action.dialog.orphan-placement",
    "action.dialog.remove-closure",
    "action.dialog.reverse-section",
    "action.dialog.section-adjust",
    "action.dialog.settings",
    "action.dialog.open-from-speleodb",
    "action.dialog.speleodb-done",
    "action.dialog.speleodb-divergence",
    "action.dialog.speleodb-close",
    "action.dialog.speleodb-quit",
    "action.dialog.speleodb-no-permission",
    "action.dialog.speleodb-unpushed-local",
    "action.dialog.speleodb-managed-file",
    "action.dialog.speleodb-restore",
    "action.dialog.speleodb-unwind",
    "action.dialog.speleodb-unwound-push",
    "action.dialog.speleodb-discard-changes",
    "action.dialog.speleodb-revert",
    "action.dialog.publish-to-speleodb",
    "action.dialog.split-section",
    "action.dialog.swap-comments",
    "action.dialog.unrecognised-shot-kind",
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
    // ── Action: context menu (177-179) ──
    "action.remove-closure",
    "action.set-shot-kind",
    "action.cycle-shot-kind",
    "action.unmerge",
    // ── Dialog cancellations (180-213) ──
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
    "dialog-cancel.import-blocked",
    "dialog-cancel.launch-open-confirm",
    "dialog-cancel.insert-station",
    "dialog-cancel.keyboard-shortcuts",
    "dialog-cancel.merge",
    "dialog-cancel.merge-projects",
    "dialog-cancel.new-project",
    "dialog-cancel.new-project-for-import",
    "dialog-cancel.orphan-placement",
    "dialog-cancel.remove-closure",
    "dialog-cancel.reverse-section",
    "dialog-cancel.section-adjust",
    "dialog-cancel.settings",
    "dialog-cancel.open-from-speleodb",
    "dialog-cancel.speleodb-done",
    "dialog-cancel.speleodb-divergence",
    "dialog-cancel.speleodb-close",
    "dialog-cancel.speleodb-quit",
    "dialog-cancel.speleodb-no-permission",
    "dialog-cancel.speleodb-unpushed-local",
    "dialog-cancel.speleodb-managed-file",
    "dialog-cancel.speleodb-restore",
    "dialog-cancel.speleodb-unwind",
    "dialog-cancel.speleodb-unwound-push",
    "dialog-cancel.speleodb-discard-changes",
    "dialog-cancel.speleodb-revert",
    "dialog-cancel.publish-to-speleodb",
    "dialog-cancel.split-section",
    "dialog-cancel.swap-comments",
    "dialog-cancel.unrecognised-shot-kind",
    "dialog-cancel.tml-import",
    "dialog-cancel.unmerge-stations",
    "dialog-cancel.unsaved-changes",
    "dialog-cancel.zero-datum-config",
    "dialog-cancel.zero-datum-selection",
    // ── Undo regret (214-240) ──
    "undo-after.adjust_loop_closures",
    "undo-after.create_loop_closure",
    "undo-after.create_section",
    "undo-after.create_section_and_merge",
    "undo-after.create_shot",
    "undo-after.delete_last_shot_and_section",
    "undo-after.delete_section",
    "undo-after.delete_shot",
    "undo-after.delete_shot_and_split",
    "undo-after.execute_batch",
    "undo-after.execute_compass_import",
    "undo-after.execute_gpx_import",
    "undo-after.execute_tml_import",
    "undo-after.execute_walls_import",
    "undo-after.import_sections",
    "undo-after.add_project_file",
    "undo-after.merge_stations",
    "undo-after.move_comment",
    "undo-after.move_section",
    "undo-after.move_segment",
    "undo-after.move_station",
    "undo-after.remove_loop_closure",
    "undo-after.remove_zero_datum",
    "undo-after.reverse_section",
    "undo-after.set_shot_kind",
    "undo-after.set_station_comment",
    "undo-after.set_zero_datum",
    "undo-after.split_shot_at_distance",
    "undo-after.unmerge_station_pair",
    "undo-after.unmerge_stations",
    "undo-after.update_section",
    "undo-after.update_shot",
    // ── Dwell: tools (241-252) ──
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
    // ── Dwell: dialogs (253-286) ──
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
    "dwell.dialog.import-blocked",
    "dwell.dialog.launch-open-confirm",
    "dwell.dialog.insert-station",
    "dwell.dialog.keyboard-shortcuts",
    "dwell.dialog.merge",
    "dwell.dialog.merge-projects",
    "dwell.dialog.new-project",
    "dwell.dialog.new-project-for-import",
    "dwell.dialog.orphan-placement",
    "dwell.dialog.remove-closure",
    "dwell.dialog.reverse-section",
    "dwell.dialog.section-adjust",
    "dwell.dialog.settings",
    "dwell.dialog.open-from-speleodb",
    "dwell.dialog.speleodb-done",
    "dwell.dialog.speleodb-divergence",
    "dwell.dialog.speleodb-close",
    "dwell.dialog.speleodb-quit",
    "dwell.dialog.speleodb-no-permission",
    "dwell.dialog.speleodb-unpushed-local",
    "dwell.dialog.speleodb-managed-file",
    "dwell.dialog.speleodb-restore",
    "dwell.dialog.speleodb-unwind",
    "dwell.dialog.speleodb-unwound-push",
    "dwell.dialog.speleodb-discard-changes",
    "dwell.dialog.speleodb-revert",
    "dwell.dialog.publish-to-speleodb",
    "dwell.dialog.split-section",
    "dwell.dialog.swap-comments",
    "dwell.dialog.unrecognised-shot-kind",
    "dwell.dialog.tml-import",
    "dwell.dialog.unmerge-stations",
    "dwell.dialog.unsaved-changes",
    "dwell.dialog.zero-datum-config",
    "dwell.dialog.zero-datum-selection",
    // ── Dwell: other (287-288) ──
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
    // Same, for the new-section dialog's grid (its own column layout).
    "command.update_new_section_view",
    // Same again for the edit-section dialog's shot grid.
    "command.update_edit_section_view",
    "command.update_import_shots_view",
    "command.update_gas_config",
    // Depth / distance colour palette + pins, written from the map legend cog
    // and the export panel (partial `[map_palette]` write).
    "command.update_map_palette",
    // Export panel Levels slider: histogram of the depths / distances the
    // image export colours.
    "command.export_value_histogram",
    "command.get_raw_table_names",
    "command.get_raw_table",
    // Ruler tool: along-cave-line distance + path shots between two endpoints (#26).
    "command.get_along_line_distance",
    // Per-project settings: validation thresholds stored in the .stalx (#57).
    "command.update_project_settings",
    // Entrance elevation edit, project settings dialog (#465).
    "command.set_surface_elevation",
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
    // Map-tile consumption per provider (#31), one event per SERVED tile:
    //   `tile.fetch.<p>`      imagery bytes pulled from the provider
    //   `tile.revalidate.<p>` served from cache after a 304 - a round trip, but
    //                         no imagery crossed the wire
    //   `tile.cache.<p>`      served from cache with no network call at all
    // The three are disjoint and sum to tiles used. Splitting revalidations out
    // is what keeps `tile.fetch` meaning "consumed from the provider": past the
    // first day nearly every request revalidates, so folding those into `fetch`
    // would report near-total consumption for a session that downloaded nothing.
    // Add all three here (and in the worker registry) when a provider is added
    // to tile_providers.rs; unregistered providers still report, via the
    // overflow map.
    "tile.fetch.esri",
    "tile.revalidate.esri",
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
    "active-dwell.dialog.import-blocked",
    "active-dwell.dialog.launch-open-confirm",
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
    "active-dwell.dialog.reverse-section",
    "active-dwell.dialog.section-adjust",
    "active-dwell.dialog.settings",
    "active-dwell.dialog.open-from-speleodb",
    "active-dwell.dialog.speleodb-done",
    "active-dwell.dialog.speleodb-divergence",
    "active-dwell.dialog.speleodb-close",
    "active-dwell.dialog.speleodb-quit",
    "active-dwell.dialog.speleodb-no-permission",
    "active-dwell.dialog.speleodb-unpushed-local",
    "active-dwell.dialog.speleodb-managed-file",
    "active-dwell.dialog.speleodb-restore",
    "active-dwell.dialog.speleodb-unwind",
    "active-dwell.dialog.speleodb-unwound-push",
    "active-dwell.dialog.speleodb-discard-changes",
    "active-dwell.dialog.speleodb-revert",
    "active-dwell.dialog.publish-to-speleodb",
    "active-dwell.dialog.split-section",
    "active-dwell.dialog.swap-comments",
    "active-dwell.dialog.unrecognised-shot-kind",
    "active-dwell.dialog.tml-import",
    "active-dwell.dialog.unmerge-stations",
    "active-dwell.dialog.unsaved-changes",
    "active-dwell.dialog.zero-datum-config",
    "active-dwell.dialog.zero-datum-selection",
    "active-dwell.global-view",
    "active-dwell.placement",
    "active-dwell.dialog.delete-shot-choice",
    "active-dwell.dialog.compass-import",
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
    // (`speleodb::metrics::Endpoint`) the client emits five outcome counters
    // (`ok`/`client-error`/`server-error`/`parse-error`/`network-error`, which
    // sum to the call count) plus a latency histogram (`latency.<bucket>`) the backend
    // reconstructs p50/p95 from. NO PII: only the endpoint kind, the outcome
    // class, and a latency bucket ever leave the app - never a host, project id,
    // email, token, or file bytes. Recorded per HTTP attempt at the
    // `client::SdkClient::call` chokepoint (a retried 5xx re-hits the server, so
    // counting per attempt is what makes a retry storm legible).
    // `every_speleodb_endpoint_has_traffic_events` regenerates this exact set from
    // `Endpoint::ALL` x `Outcome::ALL` x `LATENCY_BUCKETS`: add an endpoint kind
    // and CI fails until its eleven names are listed here.
    "speleodb-api.login.ok",
    "speleodb-api.login.client-error",
    "speleodb-api.login.server-error",
    "speleodb-api.login.parse-error",
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
    "speleodb-api.list-projects.parse-error",
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
    "speleodb-api.get-project.parse-error",
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
    "speleodb-api.get-revisions.parse-error",
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
    "speleodb-api.fetch-user.parse-error",
    "speleodb-api.fetch-user.network-error",
    "speleodb-api.fetch-user.latency.le-100",
    "speleodb-api.fetch-user.latency.le-500",
    "speleodb-api.fetch-user.latency.le-1000",
    "speleodb-api.fetch-user.latency.le-5000",
    "speleodb-api.fetch-user.latency.le-15000",
    "speleodb-api.fetch-user.latency.gt-15000",
    "speleodb-api.download-stalx.ok",
    "speleodb-api.download-stalx.client-error",
    "speleodb-api.download-stalx.server-error",
    "speleodb-api.download-stalx.parse-error",
    "speleodb-api.download-stalx.network-error",
    "speleodb-api.download-stalx.latency.le-100",
    "speleodb-api.download-stalx.latency.le-500",
    "speleodb-api.download-stalx.latency.le-1000",
    "speleodb-api.download-stalx.latency.le-5000",
    "speleodb-api.download-stalx.latency.le-15000",
    "speleodb-api.download-stalx.latency.gt-15000",
    "speleodb-api.upload.ok",
    "speleodb-api.upload.client-error",
    "speleodb-api.upload.server-error",
    "speleodb-api.upload.parse-error",
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
    "speleodb-api.acquire.parse-error",
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
    "speleodb-api.release.parse-error",
    "speleodb-api.release.network-error",
    "speleodb-api.release.latency.le-100",
    "speleodb-api.release.latency.le-500",
    "speleodb-api.release.latency.le-1000",
    "speleodb-api.release.latency.le-5000",
    "speleodb-api.release.latency.le-15000",
    "speleodb-api.release.latency.gt-15000",
    "speleodb-api.patch-project.ok",
    "speleodb-api.patch-project.client-error",
    "speleodb-api.patch-project.server-error",
    "speleodb-api.patch-project.parse-error",
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
    "speleodb-api.create-project.parse-error",
    "speleodb-api.create-project.network-error",
    "speleodb-api.create-project.latency.le-100",
    "speleodb-api.create-project.latency.le-500",
    "speleodb-api.create-project.latency.le-1000",
    "speleodb-api.create-project.latency.le-5000",
    "speleodb-api.create-project.latency.le-15000",
    "speleodb-api.create-project.latency.gt-15000",
    // Post-import receipt dialog (#391): summarises a foreign-format import
    // (source, sections/shots/loops, unit conversion, warnings) so the user
    // sees what landed without having to open the data table.
    "action.dialog.import-receipt",
    "dialog-cancel.import-receipt",
    "dwell.dialog.import-receipt",
    "active-dwell.dialog.import-receipt",
    // Import-as-new-project confirm dialog (#391): replaces the old close-first
    // dead end when a new-project-only format is picked while a project is open.
    "action.dialog.import-as-new-project",
    "dialog-cancel.import-as-new-project",
    "dwell.dialog.import-as-new-project",
    "active-dwell.dialog.import-as-new-project",
    // Open-or-add prompt (#674): a `.stalx` picked through an Import door while a
    // project is open asks whether to open it or add it to the open project.
    "action.dialog.add-or-open-project",
    "dialog-cancel.add-or-open-project",
    "dwell.dialog.add-or-open-project",
    "active-dwell.dialog.add-or-open-project",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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
        //   + 1 net for the Jump/Reference split (#27): action.toggle-reference
        //       retired, action.{set-shot-kind,cycle-shot-kind} added = 655.
        //   + 1 split display toggle (#27): action.toggle.showJumps = 656.
        //   + 1 unreadable-kind prompt (#27):
        //       command.take_unrecognised_shot_kinds = 657.
        //   + 4 unrecognised-shot-kind dialog events (#27):
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .unrecognised-shot-kind = 661.
        //   + 1 SpeleoDB per-project local-state command (the local half of the
        //       held-lock / GlobalView joins):
        //       command.speleodb_local_project_states = 662.
        //   + 1 SpeleoDB discard-local re-pull command (the "take remote and
        //       discard local" branch of the refused-open prompt):
        //       command.speleodb_open_project_discarding_local = 663.
        //   + 4 quit-with-unpushed-edits dialog events:
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-quit = 667.
        //   + 4 refused-open (unpushed local edits) dialog events:
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-unpushed-local = 671.
        //   + 4 SpeleoDB project dialog events (the per-project administration
        //       promoted out of Project Settings):
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-project = 675.
        //   + 4 revert-a-revision dialog events (the read-only route that takes the
        //       lock, uploads the chosen revision and releases it again):
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-revert = 679.
        //   + 2 dive-profile export commands (#299):
        //       command.{export_dive_profile,render_dive_profile_text} = 683.
        //   + 6 section-reverse events (#24):
        //       command.reverse_section, undo-after.reverse_section,
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .reverse-section = 689.
        //   + 2 plan-anchor round-trip commands (#310, the live-pin half of what the
        //       `.plnx` already did on save):
        //       command.plan_{anchors_for_refs,refs_for_anchors} = 691.
        //   + 4 ambiguous-dump picker dialog events (#258). The condition is
        //       EXOGENOUS - only a web-UI upload or a non-Stalagmight client can
        //       produce a multi-file dump - so incidence and cancel rate are the
        //       only way to learn whether the picker earns its upkeep:
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-dump-picker = 695.
        //   + 1 lock-free dump probe (#258), so the revert composite can ask which
        //       file to use BEFORE it acquires the write lock rather than after:
        //       command.speleodb_dump_candidates = 696.
        //   + 1 provenance-scoped lock unwind (#258 review), so a composite that
        //       finds its context stale after acquiring can hand back what IT took
        //       without surrendering a lock inherited from a sibling session on the
        //       same account: command.speleodb_release_lock_acquired_here = 697.
        //   - 10 retired collaboration-mode events (Ariane sibling .tml workflow
        //       removed entirely, not via REMOVED_COMMANDS/REMOVED_DIALOGS):
        //       command.{set_collab_enabled, set_collab_enabled_for_path,
        //       get_collab_state, detect_collab_changes, apply_collab_changes,
        //       collab_sibling_when_disabled},
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .collab-review removed = 687.
        //   + 1 SpeleoDB foreign-copy de-link command (canonical local storage:
        //       the strip half of the managed-file prompt):
        //       command.speleodb_remove_link_at = 688.
        //   + 4 managed-file dialog events (canonical local storage: the prompt a
        //       refused non-canonical open raises):
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-managed-file = 692.
        //   + 1 gas-only config write (#357 item 2), so Plan mode's gas controls
        //       stop persisting a whole `Config` spread from a store snapshot that
        //       could already be stale, and silently reverting another partial
        //       save: command.update_gas_config = 693.
        //   + 2 hybrid-undo commands (#303). `speleodb_unwind_target` is asked when the
        //       undo dropdown opens, so its rate says how often anyone LOOKS for
        //       cross-commit history; `speleodb_set_undo_mode` says whether the
        //       per-project Disabled escape hatch is ever reached for:
        //       command.speleodb_{unwind_target,set_undo_mode} = 695.
        //   + 4 speleodb-unwind dialog events (#303). The confirmation in front of the
        //       one action that walks history backwards: its cancel rate is how we learn
        //       whether the two wordings (reload this commit's history vs move the copy
        //       back to an ancestor) actually land:
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-unwind = 699.
        //   + 1 speleodb_rewind_target (#303). The forward half of the same walk, and
        //       the pair is the point: comparing it against unwind_target says whether
        //       people actually step BACK out of a version once they have stepped into
        //       one, or only ever go deeper:
        //       command.speleodb_rewind_target = 700.
        //   + 1 speleodb_discard_changes (#303). The escape hatch from a copy the user
        //       does not want. How often it is reached says whether the chain walk is
        //       leaving people somewhere they did not mean to be:
        //       command.speleodb_discard_changes = 701.
        //   + 8 = 4 each for the two dialogs guarding it (#303). Both stand between a
        //       user and a destructive push, so their CANCEL rates are the measurement
        //       that matters: a high one means people are reaching a push they never
        //       wanted, which is the bug that produced them:
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .speleodb-{unwound-push,discard-changes} = 709.
        //   + 4 post-import receipt dialog events (#391), the (source, counts, unit
        //       conversion, warnings) summary shown after a foreign-format import:
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .import-receipt = 713.
        //   + 4 import-as-new-project confirm dialog events (#391), replacing the
        //       old close-first dead end for a new-project-only format picked while
        //       a project is open:
        //       {action.dialog,dialog-cancel,dwell.dialog,active-dwell.dialog}
        //       .import-as-new-project = 717.
        //   + 1 get_tile_attribution. Reads the imagery provider's live source names,
        //       which the map is required to display. Counts how often a surface
        //       ASKED, not how often we fetched: the renderer shares only the
        //       in-flight request, so every mount of an imagery surface calls in
        //       and the backend answers from its own 24h cache (or a 5-minute
        //       failure backoff). So the shape to watch is the ratio to sessions
        //       and to satellite toggles, not an absolute rate - a count far
        //       above the number of imagery surfaces opened would mean the
        //       in-flight sharing is broken, and each true miss is a blocking
        //       fetch behind a map that is already drawing:
        //       command.get_tile_attribution = 718.
        //   + 1 tile.revalidate.esri. Splits a 304 out of `tile.fetch.<p>`: the
        //       network was reached, but no imagery crossed the wire, and past
        //       the first day nearly every request revalidates. Folded together
        //       they would report near-total provider consumption for a session
        //       that downloaded nothing, which is the one thing #31 measures:
        //       tile.revalidate.esri = 719.
        //   + 1 new-section grid view write. Its own command rather than more
        //       arguments on update_data_table_view, so the rate answers a question
        //       the data table's cannot: whether anyone reshapes the columns while
        //       ENTERING a survey, as opposed to while reviewing one already typed:
        //       command.update_new_section_view = 720.
        //   + 1 the same write for the EDIT-section dialog's shot grid. Kept
        //       apart from the one above on purpose: comparing the two rates is
        //       how we learn whether column shaping is something people do while
        //       entering a survey, while correcting one, or both:
        //       command.update_edit_section_view = 721.
        //   + 1 the same write for the import-time shot grids (the mNemo wizard
        //       and zero-datum config), kept apart from the edit-section one so
        //       reshaping columns mid-import cannot overwrite the layout used to
        //       review a saved section:
        //       command.update_import_shots_view = 722.
        //   + 4 the import-blocked prompt (#407 follow-up): the Plan/Export
        //       mode-only DMP rejection is a choice ("Switch to Survey" vs
        //       stay), not an error dialog, so it gets the full per-dialog set:
        //       action.dialog + dialog-cancel + dwell.dialog +
        //       active-dwell.dialog for import-blocked = 726.
        //   + 1 the OS file-association drain (#424):
        //       command.take_pending_launch_files = 727.
        //   + 4 the launch-open confirm (#424): consent for a `.stalx` the OS
        //       handed us while a clean project is open, so it gets the full
        //       per-dialog set: action.dialog + dialog-cancel + dwell.dialog +
        //       active-dwell.dialog for launch-open-confirm = 731.
        //   + 3 the hamburger menu's Debug submenu split the single showDevStats
        //       overlay toggle into four independent ones:
        //       action.toggle.{showMemoryStats,showMapStats,showZoomStats} = 734.
        //   + 1 entrance elevation became editable in Project Settings (#465):
        //       command.set_surface_elevation = 735.
        //   + 1 command.count_unanchored_sections (#446) = 736.
        //   + 1 the discarded-history banner (#473) asks the backend whether the
        //       project it just opened could read its edit history at all:
        //       command.history_discarded = 737.
        //   + 5 the in-app updater (#579), one per step of the deliberately
        //       split cycle: command.check_for_app_update,
        //       command.download_app_update, command.arm_app_update_restart,
        //       command.unarm_app_update_restart,
        //       command.install_app_update_and_restart = 742. The arm/unarm
        //       pair is the backend's one-shot consent for the install, so a
        //       webview `invoke` cannot skip the exit guards.
        // Native SDK migration removes retired commands and picker events.
        // Removed force-edit command/dialog (5) and project settings dialog (4).
        // Retired workspace metadata mutation commands (3).
        //   + 1 sign-out's discard of a cached copy that is not open (the exit
        //       prompt work): how often a sign-out has to throw un-uploaded work
        //       away says whether people are leaving work stranded between
        //       sessions: command.speleodb_discard_cached_copy = 654.
        //   The speleodb-close / speleodb-quit dialogs became one independent
        //   overlay (like held-lock-resume); their slots are retained.
        //   + 11 SpeleoDB traffic outcome `parse-error`, one per endpoint kind
        //       (sdk#8): a reply that does not match the SDK contract was
        //       counted as `ok`, hiding every contract drift = 665.
        //   + 1 the user's explicit "Quit now" while a SpeleoDB transfer is still
        //       running (lock#6): how often people refuse to wait says whether the
        //       quit-after-work wait is too long to bear:
        //       command.arm_forced_exit = 666.
        //   + 3 cached SpeleoDB copies on the home screen: the owner-aware
        //       listing, removing one copy, and Settings' purge:
        //       command.speleodb_cached_projects, command.speleodb_remove_cached_copy,
        //       command.speleodb_purge_cached_copies = 669.
        //   + 1 a project switch or a quit hands the open project's lock back
        //       before leaving it, and asks when that fails (never strand a lock
        //       silently): command.speleodb_settle_open_lock = 670.
        //   + 1 a "Quit now" whose window close failed takes its forced-exit arm
        //       back, so the app that stays open does not cut a later transfer:
        //       command.disarm_forced_exit = 671.
        //   + 6 "Add .stalx to current project" (#674): command.add_project_file,
        //       undo-after.add_project_file, and the full per-dialog set for the
        //       open-or-add prompt: action.dialog + dialog-cancel + dwell.dialog +
        //       active-dwell.dialog for add-or-open-project = 677.
        //   + 1 the Display menu's Station names toggle (station names as the
        //       map's station labels): action.toggle.showStationNames = 678.
        //   + 1 the depth / distance colour-palette picker's partial config
        //       write (map legend cog, export panel): how often anyone moves off
        //       the theme ramp: command.update_map_palette = 679.
        //   + 1 the export panel's Levels histogram (depth / penetration
        //       distance over the export's own range):
        //       command.export_value_histogram = 680.
        //   + 1 the "Errors" display toggle: action.toggle.showShotErrors = 681.
        assert_eq!(EVENT_NAMES.len(), 681);
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
                    "dwell state '{name}' has no '{mirror}' - add it here and in \
                     telemetry-worker/src/registry.ts"
                );
            }
        }
    }

    #[test]
    fn source_count() {
        assert_eq!(SOURCE_NAMES.len(), 20);
    }
}
