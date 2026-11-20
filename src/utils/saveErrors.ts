/**
 * Marker the backend uses to flag a filesystem permission denial (e.g. macOS
 * forgot folder access after an OS/app update). Mirrors `PERMISSION_DENIED_CODE`
 * in `src-tauri/src/core/loading.rs`:
 * - save path: prefixed onto `CommandResult.error`
 * - open path: surfaced as `LoadError.code === 'PERMISSION'`
 */
export const FS_PERMISSION_DENIED = 'FS_PERMISSION_DENIED';

/**
 * Marker for a destination that is not reachable at all - the folder was
 * deleted or renamed, or its drive is not connected. Mirrors
 * `PATH_NOT_FOUND_CODE` (same file). Split out from the permission marker
 * because the two have no remediation in common: granting folder access cannot
 * bring back a drive that is unplugged (#429).
 */
export const FS_PATH_NOT_FOUND = 'FS_PATH_NOT_FOUND';

/**
 * Marker for a destination path longer than the OS will open - on Windows,
 * anything at or past the 260-character `MAX_PATH` budget SQLite's file layer
 * is bound by. Mirrors `PATH_TOO_LONG_CODE` (same file).
 */
export const FS_PATH_TOO_LONG = 'FS_PATH_TOO_LONG';

/**
 * Marker for a save refused because the project's ignore list could not be read
 * (#517) - saving would have rebuilt the file without it. Mirrors
 * `IGNORES_UNREADABLE_CODE` in `src-tauri/crates/core/src/core/loading.rs`.
 *
 * The message after the marker is already a full sentence, so the surfaces
 * strip the marker and show the backend's text (`saveFailureMessage`,
 * `autosaveFailureMessage`). (0.6.0 also needed it for routing, to keep this
 * refusal out of the integrity banner's Review & Repair, which is gone.)
 */
export const IGNORES_UNREADABLE = 'IGNORES_UNREADABLE';

/**
 * Marker for a save the external-modification guard refused (`.stalx` v2, plan
 * 6.5): the file on disk is not what the app last read or wrote, so another
 * program changed it. Mirrors `EXTERNALLY_MODIFIED_CODE` in
 * `src-tauri/crates/core/src/core/state/open_file.rs`. Routed to the save
 * prompt (reload / overwrite / save as), never to a bare error.
 */
export const EXTERNALLY_MODIFIED = 'EXTERNALLY_MODIFIED';

/**
 * Marker for a save refused because the project is open read-only: the file
 * was made with a newer Stalagmight (plan 6.4). Mirrors `READ_ONLY_FILE_CODE`
 * (same file). Edits and undo carry it too.
 */
export const READ_ONLY_FILE = 'READ_ONLY_FILE';

/** What a failed save's `CommandResult.error` says went wrong. */
export type SaveErrorKind =
  | 'permission'
  | 'path-not-found'
  | 'path-too-long'
  | 'ignores-unreadable'
  | 'externally-modified'
  | 'read-only-file'
  | 'other';

/**
 * Classify a save `CommandResult.error` by its backend marker. `other` covers
 * both unmarked errors (disk full, corruption) and no error at all - in both
 * cases there is no better wording than what the backend sent.
 */
export function classifySaveError(error?: string | null): SaveErrorKind {
  if (!error) return 'other';
  if (error.startsWith(FS_PERMISSION_DENIED)) return 'permission';
  if (error.startsWith(FS_PATH_NOT_FOUND)) return 'path-not-found';
  if (error.startsWith(FS_PATH_TOO_LONG)) return 'path-too-long';
  if (error.startsWith(IGNORES_UNREADABLE)) return 'ignores-unreadable';
  if (error.startsWith(EXTERNALLY_MODIFIED)) return 'externally-modified';
  if (error.startsWith(READ_ONLY_FILE)) return 'read-only-file';
  return 'other';
}

/**
 * The backend's own sentence, with its routing marker taken off the front. Used
 * for the markers whose message is already user-ready (unlike the `FS_*` ones,
 * which are reworded) - the marker itself must never reach a surface.
 */
export function stripSaveErrorMarker(error: string, marker: string): string {
  return error.startsWith(marker) ? error.slice(marker.length).replace(/^:\s*/, '') : error;
}

/** True if a save `CommandResult.error` carries the permission marker. */
export function isPermissionError(error?: string | null): boolean {
  return classifySaveError(error) === 'permission';
}

/** True if the destination itself is what failed, rather than the write. */
export function isPathError(error?: string | null): boolean {
  const kind = classifySaveError(error);
  return kind === 'path-not-found' || kind === 'path-too-long';
}
