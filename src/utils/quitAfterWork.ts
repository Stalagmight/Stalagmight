/**
 * One "quit after in-flight work" path (lock#6, frontend#4).
 *
 * A SpeleoDB upload, acquire, download or release can take minutes on a slow
 * connection, and quitting used to cut it: the window went away, the backend's
 * exit release queued behind the upload for 5 seconds and the process exited -
 * outcome unknown, the lock left held. The in-app quit routes come through the
 * `useBeforeUnload` close listener (menu Quit, Ctrl+Q, the window X, an OS close
 * and the update restart all request a window close), and this is its FIRST
 * gate, ahead of every question. Not every route is known to: on macOS the
 * native app-menu Quit / Cmd+Q may end the app through `ExitRequested` without
 * a close request (#640, unverified), and then only the backend's exit hook
 * (`commands/exit_wait.rs`) holds the exit for a transfer in flight - with no
 * prompt and no waiting dialog. For the routes that do arrive here:
 *
 * - nothing in flight: the quit goes on as before;
 * - something in flight: the window stays, a toast says the app will close when
 *   the work finishes, and the waiting prompt (`QuitWaitDialog`) shows progress
 *   with two choices - keep Stalagmight open, or (behind a warning) quit now.
 *   When the work drains the quit is re-issued from the top, so every guard
 *   (unsaved changes, the SpeleoDB exit prompt for an upload that failed) sees
 *   the settled state.
 *
 * "Quit now" is never the default. It saves unsaved edits on this computer, then
 * quits without waiting; the backend is told (`arm_forced_exit`) right before
 * the window goes, so its own exit hook (`commands/exit_wait.rs`) cuts the
 * operation instead of holding the exit for it. When the window stays after all
 * (its destroy or the update install failed), the arm is taken back
 * (`disarm_forced_exit`).
 */
import { create } from 'zustand';
import i18n from '../i18n';
import { isSpeleoOperationBusy } from '../hooks/useSpeleoOperationBusy';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoNetworkStore } from '../store/speleoNetworkStore';
import { useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { invokeIpc } from './ipc';
import { abandonUpdateRestart, performQuit, quitApp } from './quit';
import { saveOpenProjectLocally } from './saveOpenProjectLocally';
import { useSpeleoExitPrompt } from './speleoExit';
import { useReleaseFailurePrompt } from './speleoReleaseFailure';
import { TOAST_KEYS } from './toastKeys';

/**
 * Multi-step flows that must finish as a whole (Save Changes: save, upload,
 * flush, close; Publish: save, create, upload, reopen), registered for their
 * full span by {@link holdQuitDuring}: between two of their calls nothing may be
 * in flight at the IPC boundary, yet quitting there still abandons the flow.
 */
const useQuitHolds = create<{ count: number }>(() => ({ count: 0 }));

/** Keep a quit waiting until `work` settles (see {@link useQuitHolds}). */
export async function holdQuitDuring<T>(work: () => Promise<T>): Promise<T> {
  useQuitHolds.setState((s) => ({ count: s.count + 1 }));
  try {
    return await work();
  } finally {
    useQuitHolds.setState((s) => ({ count: Math.max(0, s.count - 1) }));
  }
}

/**
 * In-flight work a quit must not cut: a SpeleoDB lock / upload / download /
 * account call or hand-off verb, an exit-prompt answer being carried out (its
 * close can be releasing the lock), a "Couldn't hand back the lock" Retry, or a
 * flow held by {@link holdQuitDuring}.
 */
export function quitBlockingWorkInFlight(): boolean {
  return isSpeleoOperationBusy() || useSpeleoExitPrompt.getState().working ||
    useReleaseFailurePrompt.getState().working || useQuitHolds.getState().count > 0;
}

/**
 * How long "nothing in flight" must hold before the quit goes on. One verb can
 * be several calls in a row (save, then upload; upload, then the flush that
 * follows it): the next one starts within a tick or two, and must not be raced.
 */
export const QUIT_SETTLE_MS = 300;

export type QuitWaitStep = 'wait' | 'force' | 'forcing';

interface QuitWaitState {
  /** A quit is waiting for in-flight work. Drives `QuitWaitDialog`. */
  waiting: boolean;
  step: QuitWaitStep;
  /** Why "Quit now" did not go through (its local save failed). */
  error: string | null;
}

export const useQuitWait = create<QuitWaitState>(() => ({ waiting: false, step: 'wait', error: null }));

let stopWatching: (() => void) | null = null;
let settleTimer: ReturnType<typeof setTimeout> | null = null;

function clearSettleTimer(): void {
  if (settleTimer !== null) clearTimeout(settleTimer);
  settleTimer = null;
}

function endWait(): void {
  stopWatching?.();
  stopWatching = null;
  clearSettleTimer();
  useQuitWait.setState({ waiting: false, step: 'wait', error: null });
}

/** Watch the in-flight work; once it has drained (and stayed drained), quit again. */
function watchUntilDrained(): void {
  const check = () => {
    if (quitBlockingWorkInFlight()) {
      clearSettleTimer();
      return;
    }
    if (settleTimer !== null) return;
    settleTimer = setTimeout(() => {
      settleTimer = null;
      if (quitBlockingWorkInFlight() || !useQuitWait.getState().waiting) return;
      endWait();
      // Re-issued UN-confirmed, so every guard asks again about the settled state.
      quitApp();
    }, QUIT_SETTLE_MS);
  };
  const unsubscribers = [
    useSpeleoNetworkStore.subscribe(check),
    useSpeleoStore.subscribe(check),
    useSpeleoExitPrompt.subscribe(check),
    useReleaseFailurePrompt.subscribe(check),
    useQuitHolds.subscribe(check),
  ];
  stopWatching = () => unsubscribers.forEach((unsubscribe) => unsubscribe());
  check();
}

/**
 * Set by "Quit now" ({@link forceQuitNow}) for the quit attempt it starts: that
 * close goes through without waiting, and the backend is told it may cut the
 * operation in flight.
 *
 * A one-shot, NOT a deadline. The attempt still asks the unsent-input question
 * and the import / dive-plan question, and the user may take as long as they like
 * over either; a wall-clock bound ran out under them, and the close that came
 * back after a slow "yes" fell into the waiting prompt again instead of quitting.
 * Like the update restart's arm (`updateRestartArmed` in `utils/quit`), it is
 * consumed at the point of no return ({@link consumeForcedQuit}) and dropped by
 * every path that abandons the attempt ({@link abandonForcedQuit}): either
 * question declined, a guard taking the close over, the close request refused.
 */
let forcedQuitArmed = false;

/**
 * The close listener's first gate. Returns true when it took the close over
 * (work is in flight); the quit is then re-issued once the work has finished.
 * A forced quit ("Quit now") passes. A close that passes ends any wait still
 * showing, since the quit is going ahead anyway.
 */
export function deferQuitUntilWorkDone(): boolean {
  if (forcedQuitArmed || !quitBlockingWorkInFlight()) {
    if (useQuitWait.getState().waiting) endWait();
    return false;
  }
  // frontend#4: say so even when the waiting prompt is already up - the user just
  // asked again.
  useUIStore.getState().showToast(i18n.t('quitWait.toast'), TOAST_KEYS.QUIT_AFTER_WORK, 'info');
  if (!useQuitWait.getState().waiting) {
    useQuitWait.setState({ waiting: true, step: 'wait', error: null });
    watchUntilDrained();
  }
  return true;
}

/** "Keep Stalagmight open": the quit is called off; the work carries on. */
export function cancelPendingQuit(): void {
  endWait();
  // An update restart riding on this quit is called off with it.
  abandonUpdateRestart();
}

/** Step between the waiting prompt and its "Quit now" warning. */
export function setQuitWaitStep(step: 'wait' | 'force'): void {
  useQuitWait.setState({ step, error: null });
}

/**
 * Read AND clear the forced-quit arm, at the point of no return in the close
 * listener: when it was set, the backend is told the exit may cut the operation
 * in flight (`arm_forced_exit`) right before the window goes - not when "Quit
 * now" was pressed, so the backend's own short arm cannot run out while a
 * question is still on screen.
 */
export function consumeForcedQuit(): boolean {
  const forced = forcedQuitArmed;
  forcedQuitArmed = false;
  return forced;
}

/** A forced quit that did not happen after all (a question declined, a guard took over). */
export function abandonForcedQuit(): void {
  forcedQuitArmed = false;
}

/**
 * "Quit now", after its warning. Unsaved edits are saved on this computer first
 * (a failed save stops here and says why); then the quit goes on WITHOUT
 * waiting and without the exit questions the wait would have re-asked (the
 * SpeleoDB exit prompt, unsaved changes, the lock hand-back). The unsent SpeleoDB
 * input question and the import / dive-plan question still run, for as long as
 * the user takes to answer, and declining either calls the quit off. Saved but
 * un-uploaded SpeleoDB work keeps its lock (the backend never releases over it).
 */
export async function forceQuitNow(): Promise<void> {
  if (!useQuitWait.getState().waiting) return;
  stopWatching?.();
  stopWatching = null;
  clearSettleTimer();
  useQuitWait.setState({ step: 'forcing', error: null });
  if (useProjectStore.getState().isDirty) {
    const saved = await saveOpenProjectLocally();
    if (!saved?.success) {
      useQuitWait.setState({ step: 'force', error: saved?.error ?? i18n.t('quitWait.saveFailed') });
      watchUntilDrained();
      return;
    }
  }
  forcedQuitArmed = true;
  useQuitWait.setState({ waiting: false, step: 'wait', error: null });
  // A refused close never reaches the listener, so nothing else would drop the arm.
  performQuit(abandonForcedQuit);
}

/** Tell the backend this exit may cut the operation in flight. Best-effort. */
export async function armForcedExit(): Promise<void> {
  try {
    await invokeIpc('arm_forced_exit', undefined, { purpose: 'quit without waiting for a SpeleoDB transfer' });
  } catch (error) {
    // Without the arm the backend waits for the operation instead of cutting it,
    // which is the safe direction.
    console.error('[quit] arm_forced_exit failed:', error);
  }
}

/**
 * Take the backend's forced-exit arm back: the window survived the close it was
 * armed for (its destroy, or the update install, failed), so the app goes on
 * running, and a later exit must wait for a transfer again instead of cutting
 * it. Best-effort; the backend arm also runs out on its own after 10 seconds.
 */
export async function disarmForcedExit(): Promise<void> {
  try {
    await invokeIpc('disarm_forced_exit', undefined, { purpose: 'a forced quit whose window stayed open' });
  } catch (error) {
    console.error('[quit] disarm_forced_exit failed:', error);
  }
}

/**
 * Whether a quit is still pending here: the waiting prompt is up, the in-flight
 * work is being watched, or the settle step is scheduled. False means nothing in
 * this module will re-issue a quit - the deterministic signal an end-to-end test
 * waits for before asserting that no quit happened (instead of sleeping past
 * {@link QUIT_SETTLE_MS}).
 */
export function quitAfterWorkPending(): boolean {
  return useQuitWait.getState().waiting || stopWatching !== null || settleTimer !== null;
}

/** Test-only reset. */
export function resetQuitWaitForTests(): void {
  stopWatching?.();
  stopWatching = null;
  clearSettleTimer();
  forcedQuitArmed = false;
  useQuitHolds.setState({ count: 0 });
  useQuitWait.setState({ waiting: false, step: 'wait', error: null });
}
