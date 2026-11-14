import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../store/uiStore';
import {
  speleoProjectSessionIssuedFor,
  speleoProjectSessionStillCurrent,
} from '../store/speleoStore';
import { invokeIpc } from '../utils/ipc';
import { applySpeleoTreeReplacement } from '../utils/speleoTreeReplacement';
import type { SpeleoOpenResult } from '../types/ipc.generated';
import { TOAST_KEYS } from '../utils/toastKeys';

export interface RestoreRevisionArgs {
  projectId: string;
  projectName: string;
  /** Full commit SHA to check out. */
  hexsha: string;
  /** Short SHA for the toast copy. */
  shortSha: string;
  /**
   * `true` = editable **Restore** (the user holds the lock; the tree becomes a
   * pushable unpushed edit). `false` = read-only **View** (a peek that Update
   * fast-forwards away).
   */
  restore: boolean;
  /**
   * Waive the backend's un-pushed-edits guard. Only ever `true` for a **Restore**
   * the user has confirmed a SECOND time, having been told which local changes it
   * discards (see `SpeleoRestoreDialog`'s armed step). A View never sets it - a
   * read-only peek must stay non-destructive.
   */
  discardLocal?: boolean;
  /**
   * Suppress the final toast. Set by the revert flow (`useRevertRevision`), where
   * this restore is a MIDDLE step: "Restored to revision X. Push to make it the
   * latest." misdirects when the same flow is about to push and release for you.
   * Everything else about the call is unchanged.
   */
  quiet?: boolean;
}

/**
 * Shared "check a SpeleoDB revision out into the working copy" flow (#218), used by
 * both the read-only View action and the editable Restore confirm.
 *
 * [#258] Deliberately KNOWS NOTHING about the ambiguous-dump picker. It only carries
 * `entryPath` and rethrows the typed rejection; each caller raises the picker itself,
 * because each owns a different retry. The revert composite in particular has already
 * handed the write lock back by the time it can ask, so its retry has to re-run the
 * whole thing rather than this one step - a picker raised from here offered a button
 * that the backend's edit gate would refuse. The backend
 * reopens the swapped tree in place, so this pulls it into the view via
 * `refreshProjectState` + `refreshLinkStatus` (loadProject would short-circuit on the
 * already-open project) and toasts. Throws on failure so the caller surfaces it.
 *
 * Resolves `false` - a quiet stale result, never a toast or a navigation - for any
 * reply, success or failure, that lands after the login or the open project
 * changed: the user is free to switch projects while the download runs, and the
 * backend refuses to touch whatever they switched to.
 */
export function useRestoreRevision() {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);

  return useCallback(
    async ({
      projectId,
      projectName,
      hexsha,
      shortSha,
      restore,
      discardLocal = false,
      quiet = false,
    }: RestoreRevisionArgs) => {
      const issuedFor = speleoProjectSessionIssuedFor(projectId);
      try {
        await invokeIpc<SpeleoOpenResult>('speleodb_restore_revision', {
          projectId,
          projectName,
          hexsha,
          restore,
          // Never waived for a View, whatever the caller passed.
          discardLocal: restore && discardLocal,
          // The history-keeping Unwind flags (`keepUndoHistory`, `discardUnpushed`,
          // `chainForward`) are left out: no surface offers that step since the Unwind
          // UI was removed (#668, #671), and the backend defaults each to false.
        });
      } catch (e) {
        // The download is unbounded and nothing blocks the workspace while it runs,
        // so the user may well have opened another project by now. The backend
        // leaves that project alone and refuses the swap; whatever the reply says,
        // it is about a project that is no longer on screen, so it ends quietly
        // as a stale result rather than as an error toast over the new one.
        if (!speleoProjectSessionStillCurrent(issuedFor)) return false;
        throw e;
      }
      // The backend closed and reopened the project at the swapped tree. Reconcile every
      // per-tree frontend store through the shared replacement seam. A false result means
      // this reply landed after the login or open project changed, so callers must stop.
      // [#473] `true`: a restore/view swaps the tree by definition - it is the flow that
      // reopens DIFFERENT bytes at the SAME path, which is why the discarded-history
      // re-read has to happen here and not in App.tsx's effect.
      if (!(await applySpeleoTreeReplacement(issuedFor, true))) return false;
      if (!quiet) {
        showToast(
          restore
            ? t('speleodb.restore.doneToast', { sha: shortSha })
            : t('speleodb.restore.viewToast', { sha: shortSha }),
          TOAST_KEYS.SPELEODB_RESTORE,
          // A completed restore rewrote the tree; "now viewing revision X" is just
          // a statement of where you are.
          restore ? 'success' : 'info',
        );
      }
      return true;
    },
    [t, showToast],
  );
}
