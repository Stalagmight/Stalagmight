import i18n from '../i18n';

/**
 * Marker the backend prefixes onto `CommandResult.error` when a mutation is
 * rejected because the open project is a remote SpeleoDB project opened
 * read-only (no write lock). Mirrors `EDIT_DISABLED_CODE` in
 * `src-tauri/src/core/command.rs`.
 *
 * The full edit-state UX - an indicator that suppresses this toast and offers
 * "Start editing" (acquire the lock) - is owned by #204. Until then,
 * `executeCommand` surfaces a friendly read-only notice instead of the raw
 * coded error so a blocked edit isn't a silent no-op.
 */
export const EDIT_DISABLED = 'EDIT_DISABLED';

/** True if a `CommandResult.error` carries the read-only edit-gate marker. */
export function isEditDisabledError(error?: string | null): boolean {
  return !!error && error.startsWith(EDIT_DISABLED);
}

/**
 * Marker the backend prefixes when the lazy 20-min lock re-verify (#202) blocks an
 * edit because the write lock was lost or the server tip moved. Mirrors
 * `EDIT_REVERIFY_DIVERGED_CODE` in `src-tauri/src/core/command.rs`. Distinct from
 * `EDIT_DISABLED` so the frontend routes it to the divergence prompt (reconcile)
 * rather than the plain read-only notice.
 */
export const EDIT_REVERIFY_DIVERGED = 'EDIT_REVERIFY_DIVERGED';

/** True if a `CommandResult.error` carries the #202 re-verify divergence marker. */
export function isReverifyDivergedError(error?: string | null): boolean {
  return !!error && error.startsWith(EDIT_REVERIFY_DIVERGED);
}

/** User-facing toast shown when a read-only project rejects an edit. Says "write
    lock" like the rest of the SpeleoDB copy: the point is that someone else has
    exclusive write access, not that the file itself is locked. */
export function READ_ONLY_EDIT_NOTICE(): string {
  return i18n.t('editGate.readOnlyNotice');
}

/**
 * Marker the backend prefixes when the open file was made with a newer
 * Stalagmight and is read-only (`READ_ONLY_FILE_CODE`, plan 6.4): every edit,
 * undo and save is refused. Its notice replaces the raw coded string.
 */
export const READ_ONLY_FILE = 'READ_ONLY_FILE';

/** True if a `CommandResult.error` carries the read-only file marker. */
export function isReadOnlyFileError(error?: string | null): boolean {
  return !!error && error.startsWith(READ_ONLY_FILE);
}

/** User-facing toast when a read-only (newer-version) file refuses an edit. */
export function READ_ONLY_FILE_NOTICE(): string {
  return i18n.t('editGate.readOnlyFileNotice');
}
