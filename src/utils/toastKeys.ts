/**
 * Central registry of `showToast` dedup keys.
 *
 * Policy (see docs/notifications.md): one key per event class. Callsites
 * reporting the same event share a key so the dedup window collapses them
 * into a single toast instead of stacking duplicates. Never pass a bare
 * string literal to `showToast` - register the key here first, then import
 * it from `TOAST_KEYS`.
 */
export const TOAST_KEYS = {
  // Integrity / undo-redo
  INTEGRITY_REPAIR: 'integrity-repair',
  EDIT_DISABLED: 'edit-disabled',
  /** Catch-all for a command failure no specific handler claims (docs/notifications.md). */
  COMMAND_FAILED: 'command-failed',
  UNDO_FAILED: 'undo-failed',
  REDO_FAILED: 'redo-failed',

  // Map
  STATIONS_HIDDEN: 'stations-hidden',
  SHOTS_HIDDEN: 'shots-hidden',
  RENDER_FATAL: 'render-fatal',
  MERGE_CHECK: 'merge-check',
  MERGE_BLOCKED: 'merge-blocked',
  MERGE_ERROR: 'merge-error',
  /** A successful project merge whose CommandResult carries a warning (e.g. #448 entrance-elevation no-clobber). */
  MERGE_WARNING: 'merge-warning',
  /**
   * A delete with shift removed loop closures it left spanning a single shot
   * (#737). One toast lists every closure the delete removed.
   */
  CLOSURE_REMOVED: 'closure-removed',
  GEOMETRY_COMPUTE_FAILED: 'geometry-compute-failed',
  ANCHORED_IDS_ERROR: 'anchored-ids-error',
  MOVE_COMMENT_EMPTY: 'move-comment-empty',
  /** The distance-gradient origin's station is gone after an edit; the ramp starts at the entrance. */
  DISTANCE_ORIGIN_RESET: 'distance-origin-reset',

  // Global view
  SPELEODB_OPEN_REMOTE_ERROR: 'speleodb-open-remote-error',
  /** A cached SpeleoDB copy opened read-only because there is no usable session. */
  SPELEODB_READ_ONLY_OPEN: 'speleodb-read-only-open',
  /** The outcome of removing a downloaded SpeleoDB copy from the home screen. */
  SPELEODB_CACHED_COPY: 'speleodb-cached-copy',
  RENAME: 'rename',

  // Data table
  SHOT_EDITOR_PASTE: 'shot-editor-paste',
  DATA_TABLE_PASTE: 'data-table-paste',
  DATA_TABLE_DELETE: 'data-table-delete',
  DATA_TABLE_VIEW_SAVE: 'data-table-view-save',
  MAP_PALETTE_SAVE: 'map-palette-save',
  NEW_SECTION_VIEW_SAVE: 'new-section-view-save',
  EDIT_SECTION_VIEW_SAVE: 'edit-section-view-save',
  IMPORT_SHOTS_VIEW_SAVE: 'import-shots-view-save',
  PROFILE_EXPORT_FAILED: 'profile-export-failed',
  PROFILE_EXPORT_OK: 'profile-export-ok',
  /** Paste outcome on the Plan-mode Segments/Cylinders grids (#590). */
  PLAN_SEGMENTS_PASTE: 'plan-segments-paste',
  PLAN_CYLINDERS_PASTE: 'plan-cylinders-paste',
  /**
   * A CSV/JSON/TML/image export the user asked for did not produce a file.
   *
   * One key for all six formats and both surfaces (the modal Export dialog and
   * the Export-mode panel): they are one event class - "the export you asked
   * for failed" - and `ExportControls` is literally one component rendered
   * twice, so a per-format key would only ever split the dedup window without
   * telling the user anything the message does not.
   *
   * Distinct from `COMMAND_FAILED`: the four export actions in `projectStore`
   * do NOT go through `executeCommand`, so the catch-all never fires for them.
   */
  EXPORT_FAILED: 'export-failed',
  /*
   * PREFIX for export notes: the shown key is `export-note:<code>`, one per
   * note code. An export can return more than one code since #378 (an azimuth
   * note plus the GPS-anchors note), and a shared literal key would let the
   * second toast replace the first inside the burst window - silencing one
   * file-level compromise. Same code across two exports still collapses.
   */
  EXPORT_NOTE: 'export-note',
  WARNING_RESOLVE: 'warning-resolve',
  ZERO_DATUM_INCOMPLETE: 'zero-datum-incomplete',

  // SpeleoDB
  SPELEODB_UNLINKED_COPY: 'speleodb-unlinked-copy',
  SPELEODB_PROJECT_DIALOG_STALE: 'speleodb-project-dialog-stale',
  SPELEODB_RELEASE_ONE_ERROR: 'speleodb-release-one-error',
  SPELEODB_CONTINUE_EDITING_ERROR: 'speleodb-continue-editing-error',
  SPELEODB_CONTINUE_EDITING_DIRTY: 'speleodb-continue-editing-dirty',
  SPELEODB_CONTINUE_EDITING_UNPUSHED: 'speleodb-continue-editing-unpushed',
  SPELEODB_PUBLISH: 'speleodb-publish',
  SPELEODB_DISCARDED: 'speleodb-discarded',
  SPELEODB_LOCK: 'speleodb-lock',
  SPELEODB_STORAGE_CHOICE: 'speleodb-storage-choice',
  SPELEODB_RESTORE: 'speleodb-restore',
  SPELEODB_REVERT: 'speleodb-revert',
  SPELEODB_WEB: 'speleodb-web',
  SPELEODB_PROJECT_SETTINGS: 'speleodb-project-settings',
  SPELEODB_ACTION_ERROR: 'speleodb-action-error',
  SPELEODB_EDITING: 'speleodb-editing',
  SPELEODB_LOCKED_BY: 'speleodb-locked-by',
  SPELEODB_UP_TO_DATE: 'speleodb-up-to-date',
  SPELEODB_PUSHED: 'speleodb-pushed',
  SPELEODB_NOT_EDITING: 'speleodb-not-editing',
  SPELEODB_RELEASED: 'speleodb-released',
  SPELEODB_TOOK_REMOTE: 'speleodb-took-remote',
  SPELEODB_LATE_UPLOAD: 'speleodb-late-upload',
  SPELEODB_SAVED_LOCALLY: 'speleodb-saved-locally',
  /** A quit waits for an in-flight SpeleoDB transfer, then closes (lock#6). */
  QUIT_AFTER_WORK: 'quit-after-work',

  // Dialogs
  NEW_SECTION_PASTE_CAP: 'new-section-paste-cap',
  NEW_SECTION_PASTE: 'new-section-paste',
  EDIT_SECTION_FROZEN_DELETE: 'edit-section-frozen-delete',
  EDIT_SECTION_MID_RUN_DELETE: 'edit-section-mid-run-delete',
  DMP_WIZARD_FROZEN_DELETE: 'dmp-wizard-frozen-delete',
  DMP_WIZARD_EMPTY_SECTIONS: 'dmp-wizard-empty-sections',
  DMP_WIZARD_INCOMPLETE: 'dmp-wizard-incomplete',
  DMP_MANUAL_PLACEMENT_HINT: 'dmp-manual-placement-hint',
  PROJECT_SETTINGS: 'project-settings',
  DELETE_PROJECT_WARNING: 'delete-project-warning',
  /** Warning-free import receipt, surfaced as a toast instead of a modal (#411/#420). */
  IMPORT_RECEIPT: 'import-receipt',

  // Open / import routing
  IMPORT_IN_PROGRESS: 'import-in-progress',

  // In-app updater (#579)
  APP_UPDATE_DOWNLOAD_FAILED: 'app-update-download-failed',
  // Its own key, not a reuse of the download one: the copy has to name a
  // DIFFERENT recovery. "Try again from the update banner" is right after a
  // failed download (the banner still offers Download); after a failed install
  // the banner is on "Restart to update", where that instruction points at a
  // button that does not exist.
  APP_UPDATE_INSTALL_FAILED: 'app-update-install-failed',
  // The backend refused the restart because a SpeleoDB transfer is still running
  // (lock#6): nothing failed, the download is kept and Restart works again later.
  APP_UPDATE_RESTART_WAITS_FOR_TRANSFER: 'app-update-restart-waits-for-transfer',

  // Debug
  DEBUG_REFRESH: 'debug-refresh',
  DEBUG_COPY: 'debug-copy',
  DEBUG_COPY_FAILED: 'debug-copy-failed',
} as const;

/** Templated dedup key for keyboard-shortcut confirmation toasts. */
export function shortcutToastKey(action: string): string {
  return `shortcut-${action}`;
}
