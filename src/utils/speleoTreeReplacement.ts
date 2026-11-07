import { useGasPlanStore } from '../store/gasPlanStore';
import { useProjectStore } from '../store/projectStore';
import {
  speleoProjectSessionStillCurrent,
  type SpeleoProjectSessionIdentity,
  useSpeleoStore,
} from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import type { OpenFileFlags } from '../types/ipc.generated';
import { dropDormantDistanceOrigin } from './distanceOriginAnchor';
import { invokeIpc } from './ipc';
import { runTreeReplacement } from './projectTransition';

/**
 * The swapped file's flags, or `null` when they could not be read - which keeps
 * the previous values rather than clearing a read-only gate on a failed read.
 */
async function readOpenFileFlags(): Promise<OpenFileFlags | null> {
  try {
    return await invokeIpc('speleodb_open_file_flags', undefined, {
      purpose: 're-read the open file flags after an in-place tree swap',
    });
  } catch (e) {
    console.error('Failed to re-read the open file flags after a tree swap:', e);
    return null;
  }
}

/**
 * Reconcile every frontend store after the backend replaces the open survey tree in place.
 * Returns false when the request belongs to a project or login session that is no longer current.
 *
 * [#473] `treeReplaced` says whether the bytes on disk actually changed. Only then is the
 * discarded-history flag re-read, because that re-read also clears the user's dismissal:
 * doing it on a caller that swapped nothing would resurrect a banner they had dismissed
 * for a project whose history is exactly as it was.
 *
 * **Every current caller passes `true`** - the last one that did not,
 * `discardChanges`, was wrong to: its backend fast-forwards to the server tip BEFORE
 * deciding whether to release the lock, so even its `null` result means the tree moved.
 * Determine this from the BACKEND verb, never from what the frontend result looks like.
 *
 * The parameter stays REQUIRED, with no default, precisely because it is currently
 * constant. A `= false` default would read like the safe choice and be the opposite:
 * unreachable today, so its only possible effect is to let a FUTURE caller omit it and
 * silently skip the re-read - the silent history loss this issue exists to prevent,
 * reintroduced with no type error to catch it.
 */
export async function applySpeleoTreeReplacement(
  issuedFor: SpeleoProjectSessionIdentity | undefined,
  treeReplaced: boolean,
): Promise<boolean> {
  return runTreeReplacement(async () => {
    if (issuedFor && !speleoProjectSessionStillCurrent(issuedFor)) return false;

    const ui = useUIStore.getState();
    ui.clearSelection();
    ui.resetToolTransientsForProjectSwitch();
    ui.setDistanceGradientOrigin(null);
    dropDormantDistanceOrigin();
    useGasPlanStore.getState().reset();
    ui.setView('single');

    await useProjectStore.getState().refreshProjectState();
    if (issuedFor && !speleoProjectSessionStillCurrent(issuedFor)) return false;

    // [#576 NF-W2F-1] Re-ask the open FILE's flags. The backend reopened DIFFERENT
    // bytes at the same path, and the newer-format stamp and the unreadable-link flag
    // are file properties the frontend otherwise reads only from `loadProject`, which
    // this seam deliberately does not call - so the read-only banner and gate kept
    // describing the previous tree until the next open. Applied only while the swap
    // is still the current session's, like everything else here.
    const flags = await readOpenFileFlags();
    if (issuedFor && !speleoProjectSessionStillCurrent(issuedFor)) return false;
    if (flags) {
      useProjectStore.setState({
        newerSchemaVersion: flags.newerSchemaVersion,
        speleoLinkUnreadable: flags.speleoLinkUnreadable,
        // [#576 RV4-R-S5] The third file property: the stored settings this file's
        // open could not read, which refuse every save and drive the banner.
        unreadableProjectSettings: flags.unreadableProjectSettings,
      });
    }

    useProjectStore.setState({ isDirty: false });
    void useProjectStore.getState().checkProjectIntegrity();
    // [#473] Re-ask whether the SWAPPED tree could read its history.
    //
    // This seam is the reason the check belongs here rather than in the App effect that
    // normally drives it. That effect keys on `file_path` + `isLoading`, and an in-place
    // revision swap changes NEITHER: the backend closes and reopens DIFFERENT bytes at
    // the SAME path (`speleodb_revisions.rs`), and the frontend deliberately does not
    // call `loadProject` (it would short-circuit on the already-open project), so the
    // effect never re-fires. Without this a restored revision with an unreadable history
    // showed no banner at all, and restoring a clean revision left the previous tree's
    // banner standing over a healthy project.
    //
    // Both fields are reset in ONE synchronous write, before the async re-read. Clearing
    // only the dismissal would leave the PREVIOUS tree's `historyDiscarded` on screen
    // until the reply lands, which flashes a false data-loss banner over a healthy
    // project - the reply then corrects it, but the flash is the same wrong claim the
    // banner exists to avoid making.
    if (treeReplaced) {
      useProjectStore.setState({ historyDiscarded: false, historyDiscardedDismissed: false });
      void useProjectStore.getState().checkHistoryDiscarded();
    }
    await useSpeleoStore.getState().refreshLinkStatus();
    if (issuedFor && !speleoProjectSessionStillCurrent(issuedFor)) return false;
    useSpeleoStore.getState().invalidateChainTargets();
    return true;
  });
}
