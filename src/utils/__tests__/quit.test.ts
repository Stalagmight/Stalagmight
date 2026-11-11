/**
 * [#270] The exit guards.
 *
 * Two independent "you would lose work" conditions meet at quit time:
 *  - `projectStore.isDirty`            - unsaved to DISK;
 *  - `link.hasUnpushedEdits` + lock    - local edits NOT on the SpeleoDB server,
 *                                        which survive a save and a restart.
 * Before this, `quitApp` looked only at the first, so a saved-but-unpushed
 * project quit silently. These lock the guard, its ordering (SpeleoDB first, so
 * the prompt that can handle BOTH runs first and neither swallows the other) and
 * the one-shot confirmation that keeps the composed flow from looping.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

const closeMock = vi.hoisted(() => vi.fn(async () => {}));
const invokeIpcMock = vi.hoisted(() => vi.fn(async () => null));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: closeMock, destroy: vi.fn(async () => {}) }),
}));
vi.mock('../ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f,
}));

import {
  quitApp,
  guardQuit,
  performQuit,
  consumeQuitConfirmation,
  resetQuitConfirmationForTests,
  requestUpdateRestart,
  consumeUpdateRestartArmed,
  abandonUpdateRestart,
  resetUpdateRestartForTests,
} from '../quit';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { resetSpeleoExitPromptForTests, useSpeleoExitPrompt } from '../speleoExit';

const link = (o: Partial<SpeleoLinkState> = {}): SpeleoLinkState => ({
  permission: 'READ_AND_WRITE',
  editState: 'editing',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: true,
  unwound: false,

  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
  ...o,
});

const openDialog = vi.fn();

beforeEach(() => {
  closeMock.mockReset();
  closeMock.mockResolvedValue(undefined);
  openDialog.mockReset();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  useProjectStore.setState({ project: { name: 'p' } as never, isDirty: false });
  useSpeleoStore.setState({ link: null });
  useUIStore.setState({ openDialog: openDialog as never, activeDialog: null, dialogData: {} });
  resetSpeleoExitPromptForTests();
});

/**
 * [#270] `quitApp` decides NOTHING: it asks for a close, un-confirmed, and the
 * App-level `useBeforeUnload` listener runs the whole exit flow - the native
 * import / dive-plan confirm FIRST, then `guardQuit`.
 *
 * It used to call `guardQuit()` here and then `performQuit()`, which put the tiers
 * in the opposite order for menu Quit / Ctrl+Q: `SpeleoQuitDialog` ran and its
 * "Push, release & quit" released the server lock, and only then did the re-issued
 * close reach the native prompt - which the user could still cancel, keeping the
 * app open with the lock irreversibly surrendered. The guards below are therefore
 * asserted on `guardQuit` directly, which is where they live.
 */
describe('quitApp', () => {
  it('asks for a close and guards nothing itself, whatever the state', () => {
    // Both guard conditions are live, and quitApp still just requests the close -
    // the listener asks the native prompt first, THEN opens this dialog.
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link() });
    quitApp();
    expect(openDialog).not.toHaveBeenCalled();
    expect(closeMock).toHaveBeenCalledTimes(1);
  });

  it('does not arm the one-shot confirmation - that close must still be guarded', () => {
    quitApp();
    expect(consumeQuitConfirmation()).toBe(false);
  });
});

describe('guardQuit', () => {
  it('lets a clean project quit immediately', () => {
    expect(guardQuit()).toBe(false);
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('quits immediately with no project open at all', () => {
    useProjectStore.setState({ project: null, isDirty: true });
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(false);
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('blocks on unsaved-to-disk changes', () => {
    useProjectStore.setState({ isDirty: true });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', { pendingQuit: true });
  });

  // Never strand a lock silently: a SpeleoDB project with nothing un-uploaded asks
  // no question, but hands its lock back (holder-checked) BEFORE the quit goes on -
  // the backend decides from its own claim, so a read-only one is a no-op there.
  it.each([
    { hasUnpushedEdits: false }, { editState: 'read-only' as const },
    { editState: 'read-only' as const, hasUnpushedEdits: false },
  ])('hands the lock back without a prompt, then quits: %j', async (state) => {
    useSpeleoStore.setState({ link: link(state) });
    invokeIpcMock.mockResolvedValueOnce({ success: true, warnings: [], error: null, section_ids: [] } as never);
    expect(guardQuit()).toBe(true);
    await vi.waitFor(() => expect(closeMock).toHaveBeenCalledTimes(1));
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_settle_open_lock', { releaseLock: true }, expect.anything());
    expect(consumeQuitConfirmation()).toBe(true);
    expect(openDialog).not.toHaveBeenCalled();
    expect(useSpeleoExitPrompt.getState().request).toBeNull();
  });

  // The bug this guards: saved (autosaved) work under our lock used to quit with
  // no question, and the exit released the lock over it.
  it('asks the exit prompt for SAVED but un-uploaded work under our lock', () => {
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(true);
    expect(useSpeleoExitPrompt.getState().request?.reason).toBe('quit');
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('read-only with unsaved work gets the ordinary unsaved-changes prompt', () => {
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link({ editState: 'read-only' }) });
    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', { pendingQuit: true });
    expect(useSpeleoExitPrompt.getState().request).toBeNull();
  });

  // Composition: BOTH dirty. The SpeleoDB prompt is the only one raised: its upload
  // and its keep both save the unsaved part first.
  it('raises only the SpeleoDB prompt when the project is both disk-dirty and unpushed', () => {
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(true);
    expect(openDialog).not.toHaveBeenCalled();
    expect(useSpeleoExitPrompt.getState().request?.reason).toBe('quit');
  });

  it('continues the quit once the prompt has carried out an answer', async () => {
    useSpeleoStore.setState({ link: link() });
    guardQuit();
    useSpeleoExitPrompt.getState().request?.settle(true);
    await vi.waitFor(() => expect(closeMock).toHaveBeenCalledTimes(1));
    expect(consumeQuitConfirmation()).toBe(true);
  });

  it('a cancelled prompt does not quit', async () => {
    useSpeleoStore.setState({ link: link() });
    guardQuit();
    useSpeleoExitPrompt.getState().request?.settle(false);
    await Promise.resolve();
    await Promise.resolve();
    expect(closeMock).not.toHaveBeenCalled();
    expect(consumeQuitConfirmation()).toBe(false);
  });
});

/**
 * Restart-to-update asks nothing: un-uploaded work under our lock is KEPT, lock
 * and all (unsaved edits saved first), and the restart goes ahead - the same end
 * state as the prompt's "Keep the lock". The backend releases a clean project's
 * lock before installing (`install_app_update_and_restart`).
 */
describe('guardQuit on an update restart', () => {
  const saveProject = vi.fn();
  const closeProject = vi.fn();
  beforeEach(() => {
    saveProject.mockReset();
    closeProject.mockReset();
    saveProject.mockResolvedValue({ success: true, warnings: [], error: null, section_ids: [] });
    closeProject.mockResolvedValue({ success: true, warnings: [], error: null, section_ids: [] });
    useProjectStore.setState({ saveProject, closeProject } as never);
  });

  it('saves, closes KEEPING the lock, then restarts - without a prompt', async () => {
    await requestUpdateRestart();
    closeMock.mockClear();
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(true);
    expect(useSpeleoExitPrompt.getState().request).toBeNull();
    await vi.waitFor(() => expect(closeMock).toHaveBeenCalledTimes(1));
    expect(saveProject).toHaveBeenCalledTimes(1);
    expect(closeProject).toHaveBeenCalledWith({ releaseLock: false });
    expect(saveProject.mock.invocationCallOrder[0]).toBeLessThan(closeProject.mock.invocationCallOrder[0]!);
    expect(consumeUpdateRestartArmed()).toBe(true);
  });

  it('a failed save abandons the restart and says why, leaving the project open', async () => {
    await requestUpdateRestart();
    closeMock.mockClear();
    invokeIpcMock.mockClear();
    saveProject.mockResolvedValue({ success: false, warnings: [], error: 'disk full', section_ids: [] });
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link() });
    expect(guardQuit()).toBe(true);
    await vi.waitFor(() => expect(openDialog).toHaveBeenCalledWith('error', expect.objectContaining({ message: 'disk full' })));
    expect(closeProject).not.toHaveBeenCalled();
    expect(closeMock).not.toHaveBeenCalled();
    expect(consumeUpdateRestartArmed()).toBe(false);
    expect(invokeIpcMock).toHaveBeenCalledWith('unarm_app_update_restart', undefined, expect.anything());
  });
});

/**
 * The exit prompt is an independent overlay: it layers OVER whatever dialog was
 * on screen (an import wizard mid-placement, say) instead of evicting it from the
 * single `activeDialog` slot, so cancelling puts the user straight back where they
 * were with nothing to restore.
 */
describe('guardQuit leaves the dialog on screen alone', () => {
  it('does not touch the dialog slot when raising the SpeleoDB prompt', () => {
    useSpeleoStore.setState({ link: link() });
    useProjectStore.setState({ isDirty: true });
    useUIStore.setState({
      activeDialog: 'dmp-selection',
      dialogData: { dmpFile: { sections: 3 } },
    });

    expect(guardQuit()).toBe(true);
    expect(openDialog).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('dmp-selection');
    expect(useSpeleoExitPrompt.getState().request?.reason).toBe('quit');
  });

  it('carries the displaced dialog into the unsaved-changes prompt alongside pendingQuit', () => {
    useProjectStore.setState({ isDirty: true });
    useUIStore.setState({ activeDialog: 'dmp-import-new', dialogData: { dmpFile: {} } });

    expect(guardQuit()).toBe(true);
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', {
      pendingQuit: true,
      cancelReturnsTo: 'dmp-import-new',
      cancelReturnsData: { dmpFile: {} },
    });
  });

  it('a second X while the prompt is up replaces it rather than stacking a second one', () => {
    useSpeleoStore.setState({ link: link() });
    guardQuit();
    const first = useSpeleoExitPrompt.getState().request;
    guardQuit();
    const second = useSpeleoExitPrompt.getState().request;
    expect(second).not.toBe(first);
    expect(second?.reason).toBe('quit');
  });
});

describe('the one-shot quit confirmation', () => {
  it('is armed by performQuit and consumed exactly once', () => {
    expect(consumeQuitConfirmation()).toBe(false);
    performQuit();
    expect(consumeQuitConfirmation()).toBe(true);
    expect(consumeQuitConfirmation()).toBe(false);
  });

  it('is NOT armed by a guarded quit that never reached performQuit', () => {
    useProjectStore.setState({ isDirty: true });
    expect(guardQuit()).toBe(true);
    expect(consumeQuitConfirmation()).toBe(false);
  });

  // Terminates: the dialog's own action calls performQuit while the store is still
  // dirty/unpushed, and the close listener must NOT re-guard it into a loop.
  it('lets a dialog quit past a still-dirty store', async () => {
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link() });
    performQuit();
    expect(closeMock).toHaveBeenCalledTimes(1);
    expect(consumeQuitConfirmation()).toBe(true);
  });

  // Hardening, not a fix for a reachable trigger. Every un-arming path is on the
  // emitting side (a rejected `close()`) or the receiving side (a consume), so a
  // request that IS emitted but whose event is never DELIVERED would leave the flag
  // armed forever - and the next genuine close would consume it and skip `guardQuit`
  // entirely, quitting silently over unpushed edits.
  it('expires instead of arming a much later close request', () => {
    vi.useFakeTimers();
    try {
      performQuit();
      vi.advanceTimersByTime(60_000);
      expect(consumeQuitConfirmation()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('survives the round trip it was armed for', () => {
    vi.useFakeTimers();
    try {
      performQuit();
      // A close request -> Rust prevent -> re-emit -> listener round trip is a single
      // IPC hop; the deadline must not be tight enough to lose a real one.
      vi.advanceTimersByTime(250);
      expect(consumeQuitConfirmation()).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('un-arms itself when the window close is rejected, so the next X is guarded again', async () => {
    closeMock.mockRejectedValueOnce(new Error('capability denied'));
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    performQuit();
    await Promise.resolve();
    await Promise.resolve();
    expect(consumeQuitConfirmation()).toBe(false);
    err.mockRestore();
  });
});

/**
 * [#579] `requestUpdateRestart` - unlike `performQuit`, it does NOT pre-confirm
 * the close: an update restart still runs the same exit guards a plain quit
 * does, so `updateRestartArmedUntil` is a SEPARATE one-shot flag consumed only
 * in `useBeforeUnload`'s final branch (see the guard-integration tests in
 * `useBeforeUnload.guard.test.tsx` for that half).
 */
describe('the update-restart arm (#579)', () => {
  it('requests a close, unconfirmed - guardQuit must still run for it', async () => {
    await requestUpdateRestart();
    expect(closeMock).toHaveBeenCalledTimes(1);
    expect(consumeQuitConfirmation()).toBe(false);
  });

  it('is armed by requestUpdateRestart and consumed exactly once', async () => {
    expect(consumeUpdateRestartArmed()).toBe(false);
    await requestUpdateRestart();
    expect(consumeUpdateRestartArmed()).toBe(true);
    expect(consumeUpdateRestartArmed()).toBe(false);
  });

  /**
   * The BACKEND arm is what actually gates the install command, so it has to be
   * taken before the window is asked to close - the frontend flag alone is a
   * convention any webview script could ignore.
   */
  it('arms the backend before asking for the close', async () => {
    await requestUpdateRestart();
    expect(invokeIpcMock).toHaveBeenCalledWith(
      'arm_app_update_restart',
      undefined,
      expect.anything(),
    );
    expect(invokeIpcMock.mock.invocationCallOrder[0]).toBeLessThan(
      closeMock.mock.invocationCallOrder[0]!,
    );
  });

  it('does not arm, or close, when the backend refuses', async () => {
    invokeIpcMock.mockRejectedValueOnce(new Error('nothing downloaded'));
    await expect(requestUpdateRestart()).rejects.toThrow('nothing downloaded');
    expect(closeMock).not.toHaveBeenCalled();
    expect(consumeUpdateRestartArmed()).toBe(false);
  });

  /**
   * [review] The arm was already taken when the close was requested, so a
   * REFUSED close (a revoked `core:window:allow-close` capability, an IPC
   * failure) left it set with no guard chain coming to consume it - and the
   * next ordinary quit the user made installed the update instead of quitting,
   * without them ever pressing Restart.
   */
  it('releases both arms when the window refuses to close', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    closeMock.mockRejectedValueOnce(new Error('close denied'));

    await expect(requestUpdateRestart()).rejects.toThrow('close denied');

    expect(consumeUpdateRestartArmed()).toBe(false);
    expect(invokeIpcMock).toHaveBeenCalledWith(
      'unarm_app_update_restart',
      undefined,
      expect.anything(),
    );
    err.mockRestore();
  });

  /**
   * [review] It used to be a 10-second TTL, and this test used to assert the
   * expiry as if it were the desired behaviour. It is not: the arm spans the
   * unsaved-changes dialog, which is HUMAN time - reading it, saving, then
   * confirming is routinely more than ten seconds. Past the deadline the
   * restart silently became a plain quit and the downloaded update was lost
   * with nothing said. Only an explicit abandon may clear it.
   */
  it('survives arbitrarily long guard dialogs - no wall clock', async () => {
    vi.useFakeTimers();
    try {
      await requestUpdateRestart();
      vi.advanceTimersByTime(60 * 60_000);
      expect(consumeUpdateRestartArmed()).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('abandonUpdateRestart clears both halves', async () => {
    await requestUpdateRestart();
    invokeIpcMock.mockClear();
    abandonUpdateRestart();
    expect(consumeUpdateRestartArmed()).toBe(false);
    expect(invokeIpcMock).toHaveBeenCalledWith(
      'unarm_app_update_restart',
      undefined,
      expect.anything(),
    );
  });

  /**
   * Every ordinary quit runs through the same exit dialogs, whose Cancel calls
   * this unconditionally - so it must be free when no restart is in flight,
   * rather than sending an unarm for every cancelled quit in the app.
   */
  it('abandoning when nothing is armed touches no IPC', () => {
    abandonUpdateRestart();
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });
});
