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
  /** An edit refused while a project switch is under way (#576 Q-RVFc-1-D1). */
  EDIT_DURING_SWITCH: 'edit-during-switch',
  /** Save & Quit refused because an edit queued before it has not settled (#576 PREPR-594-3-LOW). */
  EXIT_EDIT_PENDING: 'exit-edit-pending',
  /** A save refused because a newer Stalagmight saved the project (#576 B6-D1). */
  NEWER_FILE_READ_ONLY: 'newer-file-read-only',
  /*
   * PREFIX for what a successful save went without (#576 A3-D2): the shown key is
   * `save-warning:<code>`, one per warning code, so repeated autosaves collapse
   * while two different losses never replace each other (the EXPORT_NOTE idiom).
   */
  SAVE_WARNING: 'save-warning',
  /** A save refused because stored project settings could not be read on open (#576 B15). */
  SETTINGS_UNREADABLE: 'settings-unreadable',
  /** Catch-all for a command failure no specific handler claims (docs/notifications.md). */
  COMMAND_FAILED: 'command-failed',
  /** A user-started verb refused because the project changed under it (#576 Q-W44-7-D1). */
  PROJECT_CHANGED: 'project-changed',
  UNDO_FAILED: 'undo-failed',
  REDO_FAILED: 'redo-failed',

  // Map
  STATIONS_HIDDEN: 'stations-hidden',
  /** The distance-gradient origin's station was deleted; the ramp is back on the entrance (#576 D39(g)). */
  DISTANCE_ORIGIN_RESET: 'distance-origin-reset',
  /** The distance-gradient field could not be computed (it rejected); once per failure burst (#576 C7-MSG). */
  DISTANCE_GRADIENT_FAILED: 'distance-gradient-failed',
  SHOTS_HIDDEN: 'shots-hidden',
  RENDER_FATAL: 'render-fatal',
  /** The map's WebGL context was lost and not restored; carries "Reload map" (#576 D38-LOST). */
  RENDER_CONTEXT_LOST: 'render-context-lost',
  MERGE_CHECK: 'merge-check',
  MERGE_BLOCKED: 'merge-blocked',
  MERGE_ERROR: 'merge-error',
  /** A successful project merge whose CommandResult carries a warning (e.g. #448 entrance-elevation no-clobber). */
  MERGE_WARNING: 'merge-warning',
  GEOMETRY_COMPUTE_FAILED: 'geometry-compute-failed',
  ANCHORED_IDS_ERROR: 'anchored-ids-error',
  MOVE_COMMENT_EMPTY: 'move-comment-empty',
  /** A carried move ghost was cancelled by another edit, a survey change or read-only (GHOST-2, src/store/moveCarry.ts). */
  MOVE_CANCELLED: 'move-cancelled',

  // Global view
  SPELEODB_OPEN_REMOTE_ERROR: 'speleodb-open-remote-error',
  RENAME: 'rename',

  // Data table
  SHOT_EDITOR_PASTE: 'shot-editor-paste',
  DATA_TABLE_PASTE: 'data-table-paste',
  DATA_TABLE_DELETE: 'data-table-delete',
  DATA_TABLE_VIEW_SAVE: 'data-table-view-save',
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
  SPELEODB_KEYCHAIN_RESIDUE: 'speleodb-keychain-residue',
  // [#576 Q-W61-3-D1] A logout signed out but could not release a held write lock.
  SPELEODB_LOGOUT_LOCKS_HELD: 'speleodb-logout-locks-held',
  // [#576 NF-W72-1] A sign-in replaced a session but could not release its write lock.
  SPELEODB_SIGNIN_LOCKS_HELD: 'speleodb-signin-locks-held',
  SPELEODB_TOKEN_RESIDUE: 'speleodb-token-residue',
  SPELEODB_ENABLE_RENDERING_ERROR: 'speleodb-enable-rendering-error',
  SPELEODB_PROJECT_DIALOG_STALE: 'speleodb-project-dialog-stale',
  SPELEODB_RELEASE_ONE_ERROR: 'speleodb-release-one-error',
  SPELEODB_CONTINUE_EDITING_ERROR: 'speleodb-continue-editing-error',
  SPELEODB_CONTINUE_EDITING_DIRTY: 'speleodb-continue-editing-dirty',
  SPELEODB_CONTINUE_EDITING_UNPUSHED: 'speleodb-continue-editing-unpushed',
  SPELEODB_PUBLISH: 'speleodb-publish',
  SPELEODB_DISCARDED: 'speleodb-discarded',
  SPELEODB_LOCK: 'speleodb-lock',
  SPELEODB_DELETE: 'speleodb-delete',
  SPELEODB_STORAGE_CHOICE: 'speleodb-storage-choice',
  SPELEODB_RESTORE: 'speleodb-restore',
  SPELEODB_REVERT: 'speleodb-revert',
  SPELEODB_COLLABORATORS: 'speleodb-collaborators',
  SPELEODB_WEB: 'speleodb-web',
  SPELEODB_PROJECT_SETTINGS: 'speleodb-project-settings',
  SPELEODB_CONCURRENT_SESSION: 'speleodb-concurrent-session',
  SPELEODB_ADOPTED: 'speleodb-adopted',
  SPELEODB_ACTION_ERROR: 'speleodb-action-error',
  SPELEODB_EDITING: 'speleodb-editing',
  SPELEODB_LOCKED_BY: 'speleodb-locked-by',
  SPELEODB_FORCE_EDIT: 'speleodb-force-edit',
  SPELEODB_UP_TO_DATE: 'speleodb-up-to-date',
  SPELEODB_PUSHED: 'speleodb-pushed',
  SPELEODB_NOT_EDITING: 'speleodb-not-editing',
  SPELEODB_RELEASED: 'speleodb-released',
  SPELEODB_TOOK_REMOTE: 'speleodb-took-remote',

  // Dialogs
  NEW_SECTION_PASTE_CAP: 'new-section-paste-cap',
  NEW_SECTION_PASTE: 'new-section-paste',
  EDIT_SECTION_FROZEN_DELETE: 'edit-section-frozen-delete',
  EDIT_SECTION_MID_RUN_DELETE: 'edit-section-mid-run-delete',
  DMP_WIZARD_FROZEN_DELETE: 'dmp-wizard-frozen-delete',
  DMP_WIZARD_EMPTY_SECTIONS: 'dmp-wizard-empty-sections',
  DMP_WIZARD_INCOMPLETE: 'dmp-wizard-incomplete',
  DMP_MANUAL_PLACEMENT_HINT: 'dmp-manual-placement-hint',
  /** A DMP import landed but the orphan-placement lookup after it failed (#576 NF-AN4-N4). */
  DMP_ORPHAN_LOOKUP_FAILED: 'dmp-orphan-lookup-failed',
  /** A TML / Compass / Walls import landed but the orphan-placement lookup after it failed (#576 NF-DMPS-1). */
  ORPHAN_LOOKUP_FAILED: 'orphan-lookup-failed',
  PROJECT_SETTINGS: 'project-settings',
  DELETE_PROJECT_WARNING: 'delete-project-warning',
  /** Warning-free import receipt, surfaced as a toast instead of a modal (#411/#420). */
  IMPORT_RECEIPT: 'import-receipt',

  // Open / import routing
  IMPORT_IN_PROGRESS: 'import-in-progress',
  /** An import the loading overlay's Cancel stopped before it applied anything (#576 NF-FIX3A-2). */
  IMPORT_CANCELLED: 'import-cancelled',

  // In-app updater (#579)
  APP_UPDATE_DOWNLOAD_FAILED: 'app-update-download-failed',
  // Its own key, not a reuse of the download one: the copy has to name a
  // DIFFERENT recovery. "Try again from the update banner" is right after a
  // failed download (the banner still offers Download); after a failed install
  // the banner is on "Restart to update", where that instruction points at a
  // button that does not exist.
  APP_UPDATE_INSTALL_FAILED: 'app-update-install-failed',

  // Debug
  DEBUG_REFRESH: 'debug-refresh',
  DEBUG_COPY: 'debug-copy',
  DEBUG_COPY_FAILED: 'debug-copy-failed',
} as const;

/** Templated dedup key for keyboard-shortcut confirmation toasts. */
export function shortcutToastKey(action: string): string {
  return `shortcut-${action}`;
}
