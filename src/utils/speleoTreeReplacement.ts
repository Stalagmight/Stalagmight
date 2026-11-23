import { useGasPlanStore } from '../store/gasPlanStore';
import { useProjectStore } from '../store/projectStore';
import {
  speleoProjectSessionStillCurrent,
  type SpeleoProjectSessionIdentity,
  useSpeleoStore,
} from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { runTreeReplacement } from './projectTransition';
import { HISTORY_BANNER_CLEARED } from '../store/projectHistoryBannerActions';
import { TOAST_KEYS } from './toastKeys';
import { FILE_STATUS_CLEARED } from '../store/projectFileStatusActions';

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
    useGasPlanStore.getState().reset();
    ui.setView('single');

    await useProjectStore.getState().refreshProjectState();
    if (issuedFor && !speleoProjectSessionStillCurrent(issuedFor)) return false;

    useProjectStore.setState({ isDirty: false });
    // [#473] Re-ask whether the SWAPPED tree could read its history.
    //
    // This seam is the reason the check belongs here rather than in the App effect that
    // normally drives it. That effect keys on `file_path` + `isLoading`, and an in-place
    // revision swap changes NEITHER: the backend closes and reopens DIFFERENT bytes at
    // the SAME path (`speleodb_revisions.rs`), and the frontend deliberately does not
    // call `loadProject` (it would short-circuit on the already-open project, and
    // its `cancel_loading` would move the backend's session on and end the edit
    // session the reload kept: sdbdemo2 issues 1 and 2), so the effect never re-fires. Without this a restored revision with an unreadable history
    // showed no banner at all, and restoring a clean revision left the previous tree's
    // banner standing over a healthy project.
    //
    // Both fields are reset in ONE synchronous write, before the async re-read. Clearing
    // only the dismissal would leave the PREVIOUS tree's `historyDiscarded` on screen
    // until the reply lands, which flashes a false data-loss banner over a healthy
    // project - the reply then corrects it, but the flash is the same wrong claim the
    // banner exists to avoid making.
    if (treeReplaced) {
      // A revision from SpeleoDB replaced the copy: the open's "copy may be out of
      // date" / "SpeleoDB has a newer revision ... which is older" toast no longer
      // describes what is on screen (sdbdemo2 P3: it stayed up after Resume
      // editing had downloaded that revision).
      ui.dismissToastsByKey(TOAST_KEYS.SPELEODB_OFFLINE_OPEN);
      useProjectStore.setState({ ...HISTORY_BANNER_CLEARED, ...FILE_STATUS_CLEARED });
      void useProjectStore.getState().checkHistoryDiscarded();
      // The new bytes may be a version 2 file a newer Stalagmight wrote, or an
      // old-format revision the backend converted: ask about them as a load does.
      void useProjectStore.getState().checkFileStatus();
    }
    await useSpeleoStore.getState().refreshLinkStatus();
    if (issuedFor && !speleoProjectSessionStillCurrent(issuedFor)) return false;
    return true;
  });
}
