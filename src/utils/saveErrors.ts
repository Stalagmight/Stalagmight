import type { CommandErrorCode } from '../types/ipc/CommandErrorCode';

/**
 * What a refused save says went wrong, read off its `CommandErrorCode` (#576 A6) -
 * never off its message, which is display-only. `other` covers both uncoded
 * failures (disk full, corruption) and no failure at all - in both cases there is no
 * better wording than what the backend sent.
 *
 * - `permission` (`FS_PERMISSION_DENIED`): a filesystem permission denial (e.g.
 *   macOS forgot folder access after an OS/app update). The open path gets the same
 *   code on its load error.
 * - `path-not-found` (`FS_PATH_NOT_FOUND`): the destination is not reachable at all
 *   - the folder was deleted or renamed, or its drive is not connected. Split out
 *   from the permission code because the two have no remediation in common:
 *   granting folder access cannot bring back a drive that is unplugged (#429).
 * - `path-too-long` (`FS_PATH_TOO_LONG`): a destination path longer than the OS will
 *   open - on Windows, anything at or past the 260-character `MAX_PATH` budget
 *   SQLite's file layer is bound by.
 * - `ignores-unreadable` (`IGNORES_UNREADABLE`): the project's ignore list could not
 *   be read (#517) - saving would have rebuilt the file without it. Unlike the
 *   `FS_*` codes this one exists for ROUTING, not wording: an uncoded failure is sent
 *   to Review & Repair whenever `integritySaveBlocked` is set, and Review & Repair
 *   repairs cross-references, not a broken ignore-list table - so without the code
 *   this refusal reaches no surface at all on a project that has both problems.
 * - `side-table-unreadable` (`SPELEODB_LINK_UNREADABLE`, `UNDO_MARKERS_UNREADABLE`):
 *   another side table the save has to carry could not be read from the open file
 *   (#576 A3-D2: these stop the save rather than rebuild the file without them) -
 *   the SpeleoDB link, and the SpeleoDB undo-history markers. Routed exactly like
 *   `ignores-unreadable`: Review & Repair cannot fix either.
 * - `newer-file` (`NEWER_FILE_READ_ONLY`): the open project was saved by a NEWER
 *   Stalagmight and is open read-only (#576 B6-D1). A refused save is a read-only
 *   notice, never an error dialog and never Review & Repair - nothing there can make
 *   this build write the file. (An EDIT of such a file is refused with
 *   `NEWER_FILE_EDIT_DISABLED`, an edit-gate code, which is not this kind.)
 * - `settings-unreadable` (`PROJECT_SETTINGS_UNREADABLE`): stored per-project
 *   settings could not be read when the project was opened (#576 B15): the session
 *   holds defaults in their place, and any save would write those over the stored
 *   values. Routed like `newer-file`: a blocked save on a project whose banner
 *   already says why.
 */
export type SaveErrorKind =
  | 'permission'
  | 'path-not-found'
  | 'path-too-long'
  | 'ignores-unreadable'
  | 'side-table-unreadable'
  | 'newer-file'
  | 'settings-unreadable'
  | 'other';

/** Classify a refused save by its code (see `SaveErrorKind`). */
export function saveErrorKind(code: CommandErrorCode | null | undefined): SaveErrorKind {
  switch (code) {
    case 'FS_PERMISSION_DENIED':
      return 'permission';
    case 'FS_PATH_NOT_FOUND':
      return 'path-not-found';
    case 'FS_PATH_TOO_LONG':
      return 'path-too-long';
    case 'IGNORES_UNREADABLE':
      return 'ignores-unreadable';
    case 'SPELEODB_LINK_UNREADABLE':
    case 'UNDO_MARKERS_UNREADABLE':
      return 'side-table-unreadable';
    case 'NEWER_FILE_READ_ONLY':
      return 'newer-file';
    case 'PROJECT_SETTINGS_UNREADABLE':
      return 'settings-unreadable';
    default:
      return 'other';
  }
}

/**
 * A refusal whose cause is a side table the open file could not give up (#517,
 * #576 A3-D2). Its message is already a full sentence, and Review & Repair -
 * which repairs cross-references - cannot fix it.
 */
export function isUnreadableRefusal(kind: SaveErrorKind | null | undefined): boolean {
  return kind === 'ignores-unreadable' || kind === 'side-table-unreadable';
}

/** True if a refused save's code is the permission denial. */
export function isPermissionError(code: CommandErrorCode | null | undefined): boolean {
  return saveErrorKind(code) === 'permission';
}

/** True if the destination itself is what failed, rather than the write. */
export function isPathError(code: CommandErrorCode | null | undefined): boolean {
  const kind = saveErrorKind(code);
  return kind === 'path-not-found' || kind === 'path-too-long';
}
