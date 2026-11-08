// @vitest-environment jsdom
/**
 * [#576 W86] Quit, the window close and the update restart share one gate with the
 * three hand-back exits (Close, New Project, opening another `.stalx`).
 *
 * Before it, the hand-back exits and the close listener each waited for the queued
 * edits on their own. A quit and a hand-back exit waiting at the same time both
 * prompted, and the later prompt replaced the earlier one's data in the single dialog
 * slot. The close listener resumes a few microtasks earlier than a hand-back caller,
 * so the hand-back's prompt landed last whichever was asked for first: the quit's
 * `pendingQuit` was gone, and for an update restart its arm stayed set behind a
 * close prompt.
 *
 * Now a hand-back exit asked for while a close request is at the gate is dropped. A
 * close request asked for while hand-back exits wait is NOT dropped (a quit must not
 * be lost): it waits for their decisions, then waits for the edits and decides itself
 * against the state those decisions left. Its prompt then replaces the exit's (the
 * app is going away, which subsumes the close), and with nothing to protect it quits.
 * Among the hand-back exits themselves the latest wins, as between project switches
 * on this branch (`exitGate.latestWins.test.tsx`).
 *
 * Driven with the REAL guard, the REAL store and the REAL close listener, holding the
 * edit's IPC open by hand.
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
  getCurrentWindow: () => ({ destroy: destroyMock, close: vi.fn(async () => {}) }),
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
import { mutationsSettled } from '../../store/mutationQueue';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

emptyBinaryRefresh();

const EDIT = 'update_shot';
const CLOSE = 'close_project';
const INSTALL = 'install_app_update_and_restart';
const OTHER = '/cave/other.stalx';

const guard = vi.fn((exit: { editsSettled: boolean }) => guardQuit(exit));
function App() {
  // As App.tsx wires it: nothing for tier 1, `guardQuit` as tier 2.
  useBeforeUnload(false, 'unused', undefined, guard);
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

/** A grid cell committed on blur: the edit is queued, its IPC not yet answered. */
async function startEdit(): Promise<{ done: Promise<unknown> }> {
  const done = useProjectStore.getState().updateShot(1, 1, { length: 55 });
  await vi.advanceTimersByTimeAsync(0);
  expect(calls(EDIT)).toBe(1);
  expect(useProjectStore.getState().isDirty).toBe(false);
  return { done };
}

/** Every `unsaved-changes` prompt that opens (`openDialog` installs fresh data each time). */
let unsubscribe: (() => void) | null = null;
function recordPrompts(): Array<Record<string, unknown>> {
  const opened: Array<Record<string, unknown>> = [];
  unsubscribe = useUIStore.subscribe((s, prev) => {
    if (s.activeDialog === 'unsaved-changes' && s.dialogData !== prev.dialogData) opened.push(s.dialogData);
  });
  return opened;
}

const ui = () => useUIStore.getState();

/** The three hand-back exits, each started as its entry point starts it. */
const HAND_BACK_EXITS: Array<[string, (close: () => Promise<unknown>) => Promise<unknown>, Record<string, unknown>]> = [
  ['Close', (close) => close(), {}],
  ['New Project', () => newProjectGuarded(), { pendingNewProject: true }],
  ['opening another .stalx', () => routeOpenedFile(OTHER), { pendingOpenPath: OTHER }],
];

beforeEach(() => {
  vi.useFakeTimers();
  listeners.length = 0;
  guard.mockClear();
  destroyMock.mockClear();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  useAutosaveHoldStore.setState({ holds: 0 });
  useUpdateStore.getState().reset();
  useUIStore.setState({ activeDialog: null, dialogData: {}, toasts: [] });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useProjectStore.setState({
    project: { name: 'CaveA', file_path: '/cave/a.stalx' } as never,
    isDirty: false,
    isClosing: false,
    loadProject: vi.fn(async () => ({ success: true })) as never,
  });
});

afterEach(async () => {
  unsubscribe?.();
  unsubscribe = null;
  heldEdit?.resolve({ remote_link: null });
  heldEdit = null;
  await settle(mutationsSettled());
  vi.useRealTimers();
});

describe('a quit asked for while a hand-back exit waits waits for it, then decides', () => {
  it.each(HAND_BACK_EXITS)('%s first, then Quit: the quit prompt is the one left', async (_name, start, exitData) => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();
    const prompts = recordPrompts();

    const exiting = start(() => result.current.handleCloseProject());
    await vi.advanceTimersByTimeAsync(0);
    quitApp();
    const quitting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    expect(guard).not.toHaveBeenCalled();

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([exiting, quitting]));

    // The exit decided first, then the quit, which was not lost.
    expect(prompts.map((p) => p.pendingQuit === true)).toEqual([false, true]);
    expect(prompts[0]).toMatchObject(exitData);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData.pendingQuit).toBe(true);
    expect(guard).toHaveBeenCalledTimes(1);
    expect(destroyMock).not.toHaveBeenCalled();
    expect(calls(CLOSE)).toBe(0);
  });

  it('an update restart while Close waits: the restart prompts, nothing installs, the hold is down', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    await requestUpdateRestart();
    const restarting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([closing, restarting]));

    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData.pendingQuit).toBe(true);
    expect(calls(INSTALL)).toBe(0);
    expect(useUpdateStore.getState().installing).toBe(false);
  });

  it('control: with nothing to protect, Close closes and the quit then quits', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    quitApp();
    const quitting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);

    // The backend refuses the edit: the project stays clean.
    edit.reject({ code: 'VALIDATION', message: 'refused' });
    await settle(editDone);
    await settle(Promise.all([closing, quitting]));

    expect(ui().activeDialog).toBeNull();
    expect(calls(CLOSE)).toBe(1);
    expect(destroyMock).toHaveBeenCalledTimes(1);
  });

  it('a second close request while the first waits for a hand-back exit is dropped', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    const first = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    const second = closeRequested();
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([closing, first, second]));

    expect(guard).toHaveBeenCalledTimes(1);
    expect(ui().dialogData.pendingQuit).toBe(true);
  });
});

describe('a hand-back exit asked for while a close request waits is dropped', () => {
  it.each(HAND_BACK_EXITS)('Quit first, then %s: one prompt, the quit one', async (_name, start) => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();
    const prompts = recordPrompts();

    quitApp();
    const quitting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    const exiting = start(() => result.current.handleCloseProject());
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([quitting, exiting]));

    expect(prompts).toHaveLength(1);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData.pendingQuit).toBe(true);
    expect(calls(CLOSE)).toBe(0);
    expect(useProjectStore.getState().loadProject).not.toHaveBeenCalled();
  });

  it('Close while an update restart waits is dropped: the restart prompts, and its arm is not left behind a close prompt', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    await requestUpdateRestart();
    const restarting = closeRequested();
    await vi.advanceTimersByTimeAsync(0);
    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([restarting, closing]));

    expect((await closing).success).toBe(false);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData.pendingQuit).toBe(true);
    expect(calls(INSTALL)).toBe(0);
  });
});
