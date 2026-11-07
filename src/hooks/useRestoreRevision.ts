import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../store/uiStore';
import { speleoProjectSessionIssuedFor } from '../store/speleoStore';
import { expectedOpenProject } from '../store/expectedOpenProject';
import { invokeIpc } from '../utils/ipc';
import { applySpeleoTreeReplacement } from '../utils/speleoTreeReplacement';
import { afterQueuedEdits } from '../utils/projectTransition';
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
  /**
   * [#258] The dump entry the user picked, when this call is a retry from the
   * ambiguity picker. Absent on a first attempt, which is what makes the backend
   * classify normally.
   */
  entryPath?: string;
  /**
   * [#303] The "Unwind to previous SpeleoDB version" flavour: KEEP the downloaded
   * copy's undo history instead of dropping it, because that history is what the
   * action came for. Only the Unwind sets it, and the backend enforces its extra
   * preconditions (an empty undo stack, and any dirt being a previous Unwind's own
   * doing) rather than trusting this flag.
   */
  keepUndoHistory?: boolean;
  /**
   * [#303] The user was shown, and accepted, that this Unwind throws away un-pushed work -
   * the copy is dirty with no Unwind chain pointer to account for it, so the swap replaces
   * edits that were never pushed (typically ones the user undid, now on the redo stack).
   *
   * An acknowledgement, not a decision: the backend re-derives the same predicate that put
   * `discards_unpushed` on the unwind target, and without this flag it refuses exactly as
   * before. Only `SpeleoUnwindDialog` sets it, and only when it actually rendered the
   * warning.
   */
  discardUnpushed?: boolean;
  /**
   * [#303] This chain step walks FORWARD (toward BASE) rather than back. It selects which
   * stack the backend requires empty before the swap: going back, the undo stack, so the
   * tree sits at the parent's state; going forward, the redo stack, so every operation of
   * the commit being sat on has been re-applied and the child's history stacks on cleanly.
   * Only meaningful alongside `keepUndoHistory`, and clamped to it below.
   */
  chainForward?: boolean;
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
      entryPath,
      keepUndoHistory = false,
      discardUnpushed = false,
      chainForward = false,
    }: RestoreRevisionArgs) => {
      const issuedFor = speleoProjectSessionIssuedFor(projectId);
      // [#576 RVF-c-C1] The project the swap is asked for, before the queued-edits wait.
      const expectedProject = expectedOpenProject();
      // The backend hardcodes concurrent_session:false for an in-place revision swap
      // (the open project is unchanged), so unlike the open/adopt flow there is no
      // concurrent-session notice to surface here.
      // [#576 D23-SWAP] The swap waits for the edits queued before it.
      await afterQueuedEdits(() => invokeIpc('speleodb_restore_revision', {
        projectId,
        projectName,
        hexsha,
        restore,
        // Never waived for a View, whatever the caller passed.
        discardLocal: restore && discardLocal,
        entryPath: entryPath ?? null,
        // [#303] Likewise only meaningful for a Restore: a View leaves the project
        // read-only, so there would be nothing to undo with the history it kept.
        keepUndoHistory: restore && keepUndoHistory,
        // ...and the acknowledgement only means anything alongside that history-keeping
        // Restore, which is the only shape the backend consults it for.
        discardUnpushed: restore && keepUndoHistory && discardUnpushed,
        // ...and likewise the direction, which only selects a guard that a
        // history-keeping Restore is the one caller to reach.
        chainForward: restore && keepUndoHistory && chainForward,
        expectedProject,
      }));
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
