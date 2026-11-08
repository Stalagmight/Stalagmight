// @vitest-environment jsdom
/**
 * [#576 W84 x W62-Q1-D1] A file the OS launches while an in-app Close or Open is
 * waiting for queued edits waits in the launch queue until that request has
 * decided, as it waits while a project loads from the list.
 *
 * The wait (W84) has nothing on screen, so the launch queue read the screen as idle
 * and routed the launched file at once: it then overtook the user's own request
 * (latest wins) or replaced the prompt that request raised. The fuzz branch decided
 * the other way round - a launched file waits for an in-app pick - so the wait
 * holds the screen (`isScreenBusy`) until the guard has decided.
 *
 * Real `launchWithFile.ts` + `openOrImport.ts` + `useProject` + the store (the edit
 * through the mutation queue, the real `closeProject`). Mocked: the backend IPC
 * (the edit is held; the launch queue answers `take_pending_launch_files`) and the
 * Tauri event listener (the test fires the nudge). Synthetic data only.
 */
import { describe, it, expect, vi, beforeEach, afterEach, type Mock } from 'vitest';
import { renderHook } from '@testing-library/react';

const listenMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
  confirm: vi.fn(async () => false),
}));
vi.mock('../../utils/ipc');

import { useProject } from '../useProject';
import { useLaunchWithFile, resetLaunchFileQueueForTest } from '../../utils/launchWithFile';
import { routeOpenedFile } from '../../utils/openOrImport';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore, isScreenBusy } from '../../store/uiStore';
import { mutationsSettled } from '../../store/mutationQueue';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

emptyBinaryRefresh();

const EDIT = 'update_shot';
const CLOSE = 'close_project';
const TAKE = 'take_pending_launch_files';
/** The project the user opens in-app, and the one the OS launches meanwhile. */
const PICKED = '/cave/picked.stalx';
const LAUNCHED = '/dives/launched.stalx';

let loadProject: Mock;
/** The backend's launch queue, handed over (and emptied) on the next fetch. */
let queued: string[];
/** The backend nudge, captured from the hook's listener. */
let nudge: (() => void) | null;
let heldEdit: Deferred<unknown> | null;

const ui = () => useUIStore.getState();

function calls(cmd: string): number {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;
}

/** Await `p` while letting fake time run in short steps; fails rather than hangs. */
async function settle<T>(p: Promise<T>): Promise<T> {
  let done = false;
  const tracked = p.finally(() => {
    done = true;
  });
  for (let i = 0; i < 100 && !done; i += 1) await vi.advanceTimersByTimeAsync(10);
  expect(done).toBe(true);
  return tracked;
}

/** The OS hands the running app a file: queue it, then nudge. */
function launch(path: string): void {
  queued.push(path);
  if (!nudge) throw new Error('the launch listener is not registered');
  nudge();
}

/** Mount the launch channel, ready, and let its listener register and its first drain run. */
async function mountLaunchChannel(): Promise<void> {
  renderHook(() => useLaunchWithFile(true));
  await vi.advanceTimersByTimeAsync(0);
  expect(nudge).not.toBeNull();
}

/** A grid cell committed on blur: queued through the real store, its IPC held. */
async function startEdit(): Promise<{ done: Promise<unknown> }> {
  const done = useProjectStore.getState().updateShot(1, 1, { length: 55 });
  await vi.advanceTimersByTimeAsync(0);
  expect(calls(EDIT)).toBe(1);
  expect(useProjectStore.getState().isDirty).toBe(false);
  return { done };
}

beforeEach(() => {
  vi.useFakeTimers();
  resetLaunchFileQueueForTest();
  queued = [];
  nudge = null;
  heldEdit = null;
  listenMock.mockReset();
  listenMock.mockImplementation(async (_event: string, handler: () => void) => {
    nudge = handler;
    return () => {};
  });
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((cmd: string) => {
    if (cmd === EDIT) {
      heldEdit = deferred<unknown>();
      return heldEdit.promise;
    }
    if (cmd === TAKE) {
      const batch = queued;
      queued = [];
      return Promise.resolve(batch);
    }
    if (cmd === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
    return Promise.resolve(null);
  });
  loadProject = vi.fn(async () => ({ success: true }));
  useUIStore.setState({
    activeDialog: null,
    dialogData: {},
    toasts: [],
    screenOccupants: 0,
    importParse: null,
    importContext: null,
    placementMode: null,
  });
  useSpeleoStore.setState({ link: null });
  useProjectStore.setState({
    project: { name: 'CaveA', file_path: '/cave/a.stalx' } as never,
    isDirty: false,
    isClosing: false,
    loadProject: loadProject as never,
  });
});

afterEach(async () => {
  heldEdit?.resolve({ remote_link: null });
  await settle(mutationsSettled());
  useUIStore.setState({ activeDialog: null, dialogData: {}, screenOccupants: 0 });
  vi.useRealTimers();
});

describe('an OS-launched file waits while an in-app exit waits for queued edits', () => {
  it('Close: the launched file is held until the close has asked, and routes once the prompt is answered', async () => {
    await mountLaunchChannel();
    const { result } = renderHook(() => useProject());
    const { done } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    expect(isScreenBusy(ui())).toBe(true);

    const fetches = calls(TAKE);
    launch(LAUNCHED);
    await vi.advanceTimersByTimeAsync(0);
    // Held: not even fetched while the screen is spoken for.
    expect(calls(TAKE)).toBe(fetches);

    heldEdit?.resolve({ remote_link: null });
    await settle(done);
    const outcome = await settle(closing);

    // The close's own prompt is up: the launched file did not overtake it.
    expect(outcome.success).toBe(false);
    expect(calls(CLOSE)).toBe(0);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).not.toHaveProperty('pendingOpenPath');
    expect(loadProject).not.toHaveBeenCalled();

    // Answered (Cancel): the screen is free, and the launched file routes now.
    ui().closeDialog();
    await vi.advanceTimersByTimeAsync(50);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingOpenPath: LAUNCHED });
  });

  it('an in-app open of a .stalx: its prompt is the one that shows, and the launched file waits behind it', async () => {
    await mountLaunchChannel();
    const { done } = await startEdit();

    const opening = routeOpenedFile(PICKED);
    await vi.advanceTimersByTimeAsync(0);
    expect(isScreenBusy(ui())).toBe(true);

    launch(LAUNCHED);
    await vi.advanceTimersByTimeAsync(0);

    heldEdit?.resolve({ remote_link: null });
    await settle(done);
    await settle(opening);
    await vi.advanceTimersByTimeAsync(50);

    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingOpenPath: PICKED });
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('control: with nothing in flight the wait is over at once, and the screen is free again after it', async () => {
    await mountLaunchChannel();

    await routeOpenedFile(PICKED);

    expect(ui().activeDialog).toBe('launch-open-confirm');
    expect(ui().screenOccupants).toBe(0);
  });
});
