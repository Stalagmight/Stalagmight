import i18n from '../i18n';
import type { CommandErrorCode } from '../types/ipc/CommandErrorCode';

/**
 * The edit-gate refusals (#576 A6): every code with which the backend refuses an
 * edit because editing is blocked, not because the edit was wrong.
 * - `EDIT_DISABLED`: a remote SpeleoDB project opened read-only (no write lock).
 * - `WORKSPACE_READ_ONLY`: the Plan or Export workspace, which never edits the
 *   survey (#576 C5; the frontend reports the mode through `set_workspace_mode`).
 * - `NEWER_FILE_EDIT_DISABLED`: a project saved by a newer Stalagmight, open
 *   read-only (#576 B6-D1).
 * - `ADOPTION_IN_PROGRESS`: an Ariane adoption is handing the open project over to
 *   SpeleoDB; no edit until its push has landed (#576 NF-W4C-3-EDIT, RV5-F4-5).
 *
 * The full edit-state UX - an indicator that suppresses this toast and offers
 * "Start editing" (acquire the lock) - is owned by #204. Until then,
 * `executeCommand` surfaces a friendly read-only notice for any of them, so a
 * blocked edit isn't a silent no-op.
 */
const EDIT_GATE_CODES: ReadonlySet<CommandErrorCode> = new Set<CommandErrorCode>([
  'EDIT_DISABLED',
  'WORKSPACE_READ_ONLY',
  'NEWER_FILE_EDIT_DISABLED',
  'ADOPTION_IN_PROGRESS',
]);

/** True if a failure's code is an edit-gate refusal (see `EDIT_GATE_CODES`). */
export function isEditGateCode(code: CommandErrorCode | null | undefined): boolean {
  return code != null && EDIT_GATE_CODES.has(code);
}

/**
 * True if the lazy 20-min lock re-verify (#202) blocked an edit because the write
 * lock was lost or the server tip moved. Distinct from the edit gate so the
 * frontend routes it to the divergence prompt (reconcile) rather than the plain
 * read-only notice.
 */
export function isReverifyDivergedCode(code: CommandErrorCode | null | undefined): boolean {
  return code === 'EDIT_REVERIFY_DIVERGED';
}

/** User-facing toast shown when a read-only project rejects an edit. Says "write
    lock" like the rest of the SpeleoDB copy: the point is that someone else has
    exclusive write access, not that the file itself is locked.

    [#576 B6-D1] Unless the refusal is the OTHER read-only: a project saved by a
    newer Stalagmight (`NEWER_FILE_EDIT_DISABLED`). That one has nothing to do with
    the lock - Start editing is refused too - so it gets its own line. Decided from
    the refusal's code, not the store's flag, so it stays right even where the flag
    has not caught up.

    [#576 C5] Or the read-only WORKSPACE (`WORKSPACE_READ_ONLY`): Plan and Export
    never edit the survey, whatever the project, so its line points at Survey.

    [#576 RV5-F4-5] Or the adoption hand-over (`ADOPTION_IN_PROGRESS`): the
    project is being uploaded to SpeleoDB, so the generic line's "click Start
    editing" would invite a lock request in the middle of the adoption's own. */
export function READ_ONLY_EDIT_NOTICE(code?: CommandErrorCode | null): string {
  if (code === 'NEWER_FILE_EDIT_DISABLED') return i18n.t('editGate.newerFileNotice');
  if (code === 'WORKSPACE_READ_ONLY') return i18n.t('editGate.workspaceReadOnlyNotice');
  if (code === 'ADOPTION_IN_PROGRESS') return i18n.t('editGate.adoptionInProgressNotice');
  return i18n.t('editGate.readOnlyNotice');
}
