/**
 * [#576 W83] An exit waits for the edits queued before it.
 *
 * `guardQuit` decides from `isDirty`, which an edit sets only once its IPC and the
 * refresh after it have both returned. On a clean project, an edit still in flight
 * when the close request arrives (a grid cell committed on blur by the very click
 * on Restart or Quit) read as nothing to protect: the window went away, or the
 * update installed, with no prompt, and the edit was lost.
 *
 * These drive the App-level close listener with the REAL guard and the REAL store
 * (`updateShot` through the mutation queue), holding the edit's IPC open by hand.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render } from '@testing-library/react';

const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return vi.fn();
  }),
}));

const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
const closeMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: closeMock }),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: vi.fn(async () => true) }));

vi.mock('../../utils/ipc');

import { useBeforeUnload } from '../useBeforeUnload';
import {
  guardQuit,
  performQuit,
  quitApp,
  requestUpdateRestart,
  resetQuitConfirmationForTests,
  resetUpdateRestartForTests,
} from '../../utils/quit';
import { useAutosaveHoldStore } from '../../utils/autosaveHold';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useUpdateStore } from '../../store/updateStore';
import { deferred, type Deferred } from '../../test/factories/async';
import { mutationsSettled } from '../../store/mutationQueue';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

emptyBinaryRefresh();

const INSTALL = 'install_app_update_and_restart';
const EDIT = 'update_shot';

function App() {
  // As App.tsx wires it: nothing for tier 1, `guardQuit` as tier 2.
  useBeforeUnload(false, 'unused', undefined, guardQuit);
  return null;
}

/** The backend prevents the close and re-emits it; this is that re-emission. */
function closeRequested(): Promise<void> {
  return Promise.all(listeners.map((cb) => cb())).then(() => undefined);
}

function calls(cmd: string): number {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;
}

/**
 * Await `p` while letting fake time run in short steps: the edit's aftermath (the
 * refresh, the integrity re-check) waits on timers too. Fails rather than hangs,
 * and stays well inside the exit's settle timeout.
 */
async function settle<T>(p: Promise<T>): Promise<T> {
  let done = false;
  const tracked = p.finally(() => {
    done = true;
  });
  for (let i = 0; i < 100 && !done; i += 1) await vi.advanceTimersByTimeAsync(10);
  expect(done).toBe(true);
  return tracked;
}

/** The edit `holdEdit` is holding, so a failed test cannot leave the queue stuck for the next. */
let heldEdit: Deferred<unknown> | null = null;

/** Hold the edit's IPC open until the test settles it. */
function holdEdit() {
  const edit = deferred<unknown>();
  heldEdit = edit;
  invokeIpcMock.mockImplementation((cmd: string) => {
    if (cmd === EDIT) return edit.promise;
    if (cmd === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
    return Promise.resolve(null);
  });
  return edit;
}

/**
 * A grid cell committed on blur: the edit is queued, its IPC not yet answered.
 * Hands back the edit's promise boxed, so awaiting this does not await the edit.
 */
async function startEdit(): Promise<{ done: Promise<unknown> }> {
  const done = useProjectStore.getState().updateShot(1, 1, { length: 55 });
  await vi.advanceTimersByTimeAsync(0);
  expect(calls(EDIT)).toBe(1);
  expect(useProjectStore.getState().isDirty).toBe(false);
  return { done };
}

beforeEach(() => {
  vi.useFakeTimers();
  listeners.length = 0;
  destroyMock.mockClear();
  closeMock.mockClear();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  useAutosaveHoldStore.setState({ holds: 0 });
  useUpdateStore.getState().reset();
  useUIStore.setState({ activeDialog: null, dialogData: {}, toasts: [] });
  useSpeleoStore.setState({ link: null });
  useProjectStore.setState({ project: { name: 'CaveA' } as never, isDirty: false });
});

afterEach(async () => {
  heldEdit?.resolve({ remote_link: null });
  heldEdit = null;
  await settle(mutationsSettled());
  vi.useRealTimers();
});

describe('an exit waits for the edits queued before it', () => {
  it('a quit with an edit in flight on a clean project prompts once the edit lands', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    // Menu Quit / Ctrl+Q / the window X: all one close request.
    quitApp();
    const closing = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    // Still waiting on the edit: no decision yet either way.
    expect(destroyMock).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(closing);

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData.pendingQuit).toBe(true);
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('an update restart with an edit in flight on a clean project prompts instead of installing', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    await requestUpdateRestart();
    expect(calls('arm_app_update_restart')).toBe(1);
    const closing = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls(INSTALL)).toBe(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(closing);

    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(calls(INSTALL)).toBe(0);
    expect(destroyMock).not.toHaveBeenCalled();
    expect(useUpdateStore.getState().installing).toBe(false);
  });

  it('control: a clean project with nothing in flight quits at once, with no prompt', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);

    quitApp();
    await closeRequested();

    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });

  it('an edit that never settles counts as unsaved work once the wait runs out', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    quitApp();
    const closing = closeRequested();
    await vi.advanceTimersByTimeAsync(4_000);
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(destroyMock).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1_000);
    await settle(closing);
    // The project still reads clean, but the edit may yet land: ask.
    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(destroyMock).not.toHaveBeenCalled();

    // Let the queue drain so nothing is left hanging.
    edit.resolve({ remote_link: null });
    await settle(editDone);
  });

  it('a second close request during the wait is the same exit: one install, no destroy', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    await requestUpdateRestart();
    const first = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    // The X pressed while the restart is still waiting on the edit.
    const second = closeRequested();
    await vi.advanceTimersByTimeAsync(0);

    // The backend refuses the edit: the project stays clean, so the restart goes ahead.
    edit.reject({ code: 'VALIDATION', message: 'refused' });
    await settle(editDone);
    await settle(Promise.all([first, second]));

    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(calls(INSTALL)).toBe(1);
    expect(destroyMock).not.toHaveBeenCalled();
  });
});

/**
 * [#576 W85] A close request that waits for the queued edits ends once another pass
 * has run the one-way tail in the meantime - also when that pass failed and gave
 * the window back.
 *
 * The scenario: an edit is stuck, so the first quit's wait ran out and the
 * unsaved-changes prompt is up. The user presses the window X again (that pass waits
 * on the same edit), then answers the prompt: the confirmed quit takes the tail, and
 * its destroy - or, for an update restart, its install - fails, which releases
 * `isClosing` so the app stays quittable. The waiting pass used to check `isClosing`
 * alone, found it released, and ran the guard again: a second prompt the user never
 * asked for, or, with the project clean, a destroy straight after the failed install.
 */
describe('a waiting close request ends when a confirmed quit took the tail meanwhile', () => {
  const guard = vi.fn((exit: { editsSettled: boolean }) => guardQuit(exit));
  function GuardedApp() {
    useBeforeUnload(false, 'unused', undefined, guard);
    return null;
  }

  /** The stuck edit, the prompt its timed-out wait raised, and the X pressed again during a second wait. */
  async function promptUpThenXAgain(): Promise<{ edit: Deferred<unknown>; editDone: Promise<unknown>; waiting: Promise<void> }> {
    render(<GuardedApp />);
    await vi.advanceTimersByTimeAsync(0);
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const first = closeRequested();
    await vi.advanceTimersByTimeAsync(5_000);
    await settle(first);
    expect(guard).toHaveBeenCalledTimes(1);
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');

    const waiting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    expect(guard).toHaveBeenCalledTimes(1);
    return { edit, editDone, waiting };
  }

  /** "Discard & Quit": the prompt closes itself and re-issues the close, confirmed. */
  async function discardAndQuit(): Promise<void> {
    useUIStore.getState().closeDialog();
    performQuit();
    await settle(closeRequested());
  }

  beforeEach(() => {
    guard.mockClear();
  });

  it('a quit whose destroy failed: the waiting pass does not prompt again', async () => {
    quitApp();
    const { edit, editDone, waiting } = await promptUpThenXAgain();
    destroyMock.mockRejectedValueOnce(new Error('destroy denied'));

    await discardAndQuit();
    expect(destroyMock).toHaveBeenCalledTimes(1);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(waiting);

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect.soft(guard).toHaveBeenCalledTimes(1);
    expect.soft(useUIStore.getState().activeDialog).toBeNull();
    expect.soft(destroyMock).toHaveBeenCalledTimes(1);
  });

  it('an update restart whose install failed: the waiting pass neither prompts, destroys nor installs again', async () => {
    await requestUpdateRestart();
    const { edit, editDone, waiting } = await promptUpThenXAgain();
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === EDIT) return edit.promise;
      if (cmd === INSTALL) return Promise.reject({ code: 'FAILED', message: 'disk full' });
      if (cmd === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
      return Promise.resolve(null);
    });

    await discardAndQuit();
    expect(calls(INSTALL)).toBe(1);
    expect(useUpdateStore.getState().installing).toBe(false);

    // The backend refuses the stuck edit: the project stays clean.
    edit.reject({ code: 'VALIDATION', message: 'refused' });
    await settle(editDone);
    await settle(waiting);

    expect(useProjectStore.getState().isDirty).toBe(false);
    expect.soft(guard).toHaveBeenCalledTimes(1);
    expect.soft(calls(INSTALL)).toBe(1);
    expect.soft(destroyMock).not.toHaveBeenCalled();
    expect.soft(useUpdateStore.getState().installing).toBe(false);
    expect.soft(useUIStore.getState().activeDialog).toBeNull();
  });
});
