// @vitest-environment jsdom
/**
 * [#576 W86 x FZ-FZ-INT-1] The exit gate on the fuzz branch: latest wins among the
 * hand-back exits and project switches, and a quit is never lost.
 *
 * #594 (W85, W86) let one hand-back exit decide at a time and DROPPED a later one,
 * and made a quit wait for the hand-back exit at the gate before it decides. This
 * branch's switches are serialised with the latest winning (`runProjectSwitch`), and
 * its hand-back exits follow that (`settleEditsBeforeHandBack`'s ticket): a newer
 * Close / New Project / `.stalx` open, or a switch request, makes an earlier exit
 * still waiting do nothing. The close listener's requests (quit, the window X, the
 * update restart) are not part of that race: a quit waits for every hand-back exit
 * at the gate to decide, then decides itself, whatever was asked for meanwhile.
 *
 * The REAL guards, the REAL store and the REAL close listener; each edit's IPC held
 * open by hand. Synthetic data only.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, renderHook } from '@testing-library/react';

const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return vi.fn();
  }),
}));
const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: destroyMock }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
  confirm: vi.fn(async () => true),
}));
vi.mock('../../utils/ipc');

import { useBeforeUnload } from '../useBeforeUnload';
import { useProject } from '../useProject';
import { newProjectGuarded } from '../../utils/projectActions';
import { routeOpenedFile } from '../../utils/openOrImport';
import { runProjectSwitch } from '../../utils/projectTransition';
import { guardQuit, quitApp, resetQuitConfirmationForTests, resetUpdateRestartForTests } from '../../utils/quit';
import { useAutosaveHoldStore } from '../../utils/autosaveHold';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useUpdateStore } from '../../store/updateStore';
import { mutationsSettled, runSerialized } from '../../store/mutationQueue';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

emptyBinaryRefresh();

const EDIT = 'update_shot';
const CLOSE = 'close_project';
const OTHER = '/cave/other.stalx';
/** The edit's IPC answers: it lands (the project is dirty), or the backend refuses it (still clean). */
const LANDS = { remote_link: null };
const REFUSED = { code: 'VALIDATION', message: 'refused' };

const ui = () => useUIStore.getState();

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

let heldEdit: Deferred<unknown> | null = null;

/** A grid cell committed on blur: the edit is queued, its IPC not yet answered. */
async function startEdit(): Promise<{ edit: Deferred<unknown>; done: Promise<unknown> }> {
  const edit = deferred<unknown>();
  heldEdit = edit;
  invokeIpcMock.mockImplementation((cmd: string) => {
    if (cmd === EDIT) return edit.promise;
    if (cmd === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
    return Promise.resolve(null);
  });
  const done = useProjectStore.getState().updateShot(1, 1, { length: 55 });
  await vi.advanceTimersByTimeAsync(0);
  expect(calls(EDIT)).toBe(1);
  expect(useProjectStore.getState().isDirty).toBe(false);
  return { edit, done };
}

/** Every dialog that took the slot, in order, with its data. */
let opened: Array<{ dialog: string; data: Record<string, unknown> }>;
let stopRecording: (() => void) | null = null;

let loadProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  listeners.length = 0;
  destroyMock.mockClear();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  useAutosaveHoldStore.setState({ holds: 0 });
  useUpdateStore.getState().reset();
  useUIStore.setState({ activeDialog: null, dialogData: {}, toasts: [], screenOccupants: 0 });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  loadProject = vi.fn(async () => ({ success: true }));
  useProjectStore.setState({
    project: { name: 'CaveA', file_path: '/cave/a.stalx' } as never,
    isDirty: false,
    isClosing: false,
    loadProject: loadProject as never,
  });
  opened = [];
  stopRecording = useUIStore.subscribe((s, prev) => {
    if (s.activeDialog && (s.activeDialog !== prev.activeDialog || s.dialogData !== prev.dialogData)) {
      opened.push({ dialog: s.activeDialog, data: s.dialogData });
    }
  });
});

afterEach(async () => {
  stopRecording?.();
  stopRecording = null;
  heldEdit?.resolve(LANDS);
  heldEdit = null;
  await settle(mutationsSettled());
  vi.useRealTimers();
});

describe('a later hand-back exit wins, and a quit after them is still not lost', () => {
  it('Close, then a .stalx opened, then Quit, all during the wait: the open asks, the close does nothing, and the quit prompt is the one left', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const { edit, done } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    const opening = routeOpenedFile(OTHER);
    await vi.advanceTimersByTimeAsync(0);
    quitApp();
    const quitting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    expect(opened).toEqual([]);

    edit.resolve(LANDS);
    await settle(done);
    await settle(Promise.all([closing, opening, quitting]));

    // The newer hand-back exit decided (the earlier one did nothing), then the quit.
    expect(opened.map((o) => o.dialog)).toEqual(['unsaved-changes', 'unsaved-changes']);
    expect(opened[0]?.data).toMatchObject({ pendingOpenPath: OTHER });
    expect(opened[0]?.data.pendingQuit).toBeUndefined();
    expect(opened[1]?.data).toMatchObject({ pendingQuit: true });
    expect(ui().dialogData.pendingQuit).toBe(true);
    expect((await closing).success).toBe(false);
    expect(calls(CLOSE)).toBe(0);
    expect(loadProject).not.toHaveBeenCalled();
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('an edit queued between Close and Quit: the close still decides first, and the quit prompt is the one left', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const { edit, done } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    // A second cell committed on blur: the queue the quit then waits on is a newer
    // one than the close's, and the close only reaches it once the first edit lands.
    const second = deferred<void>();
    void runSerialized(() => second.promise);
    quitApp();
    const quitting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve(LANDS);
    await settle(done);
    second.resolve();
    await settle(Promise.all([closing, quitting]));

    expect(opened.map((o) => o.data.pendingQuit === true)).toEqual([false, true]);
    expect(ui().dialogData.pendingQuit).toBe(true);
    expect(calls(CLOSE)).toBe(0);
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('with nothing to protect: the open asks to swap, the close does nothing, and the quit quits', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const { edit, done } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    const opening = routeOpenedFile(OTHER);
    await vi.advanceTimersByTimeAsync(0);
    quitApp();
    const quitting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);

    // The backend refuses the edit: the project stays clean.
    edit.reject(REFUSED);
    await settle(done.catch(() => undefined));
    await settle(Promise.all([closing, opening, quitting]));

    expect(opened.map((o) => o.dialog)).toEqual(['launch-open-confirm']);
    expect(calls(CLOSE)).toBe(0);
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });
});

/** The three hand-back exits, each started as its entry point starts it. */
const HAND_BACK_EXITS: Array<[string, (close: () => Promise<unknown>) => Promise<unknown>]> = [
  ['Close', (close) => close()],
  ['New Project', () => newProjectGuarded()],
  ['opening another .stalx', () => routeOpenedFile(OTHER)],
];

describe('a project switch requested while an exit waits', () => {
  it.each(HAND_BACK_EXITS)('cancels a waiting %s: no prompt, no close, and the switch runs', async (_name, start) => {
    const { result } = renderHook(() => useProject());
    const { edit, done } = await startEdit();

    const exiting = start(() => result.current.handleCloseProject());
    await vi.advanceTimersByTimeAsync(0);
    // A switch with no click behind it: the reload after a SpeleoDB Update, say.
    const switched = vi.fn(async () => 'switched');
    const switching = runProjectSwitch(switched);
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve(LANDS);
    await settle(done);
    await settle(Promise.all([exiting, switching]));

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(opened).toEqual([]);
    expect(calls(CLOSE)).toBe(0);
    expect(loadProject).not.toHaveBeenCalled();
    expect(switched).toHaveBeenCalledTimes(1);
  });

  it('does not drop a waiting quit: the quit still prompts', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { edit, done } = await startEdit();

    quitApp();
    const quitting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    const switching = runProjectSwitch(async () => 'switched');
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve(LANDS);
    await settle(done);
    await settle(Promise.all([quitting, switching]));

    expect(opened.map((o) => o.dialog)).toEqual(['unsaved-changes']);
    expect(ui().dialogData.pendingQuit).toBe(true);
    expect(destroyMock).not.toHaveBeenCalled();
  });

  it('control: an exit asked for once the switch has settled is asked as usual', async () => {
    await runProjectSwitch(async () => 'switched');
    useProjectStore.setState({ isDirty: true });

    await newProjectGuarded();

    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingNewProject: true });
  });
});
