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
