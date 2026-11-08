// @vitest-environment jsdom
/**
 * [#576 W84 x FZ-FZ-INT-1] The exits' wait for queued edits (W83 quit, W84 Close,
 * New Project, opening a `.stalx`) against the fuzz branch's own guarantees.
 *
 * The wait is where the two meet: a request that leaves the project now waits for
 * the edits queued before it, and for that wait
 * - an edit queued DURING it must be waited for too, or the guard reads a clean
 *   project, the switch waits for that edit and closes the project under it;
 * - two requests can be waiting at once, and the one made LAST is the one that goes
 *   ahead (latest wins, as `runProjectSwitch` decides between switches);
 * - a dialog that took the screen meanwhile is not replaced, and an update that
 *   began installing meanwhile holds the project still;
 * - the import-as-new hand-over asks with the parsed file, not without it.
 *
 * The REAL guards and the REAL store (`updateShot` through the mutation queue, the
 * real `closeProject`), each edit's IPC held open by hand. Synthetic data only.
 */
import { describe, it, expect, vi, beforeEach, afterEach, type Mock } from 'vitest';
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
  confirm: vi.fn(async () => false),
}));
vi.mock('../../utils/ipc');

import { useProject } from '../useProject';
import { useBeforeUnload } from '../useBeforeUnload';
import { newProjectGuarded } from '../../utils/projectActions';
import { routeOpenedFile } from '../../utils/openOrImport';
import { editsSettledForExit, guardQuit, resetQuitConfirmationForTests } from '../../utils/quit';
import { readPendingImport } from '../../utils/pendingImport';
import { TOAST_KEYS } from '../../utils/toastKeys';
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
/** The projects the "open another .stalx" exit is asked for. */
const OTHER = '/cave/other.stalx';
const NEWER = '/cave/newer.stalx';
/** A new-project-only format, for the import-as-new hand-over. */
const TML = '/cave/CaveB.tml';

/** The edit's IPC answers: it lands (the project is dirty), or the backend refuses it (still clean). */
const LANDS = { remote_link: null };
const REFUSED = { code: 'FAILED', message: 'length out of range' };

let loadProject: Mock;
let parseTml: Mock;
/** Every dialog that took the slot, in order, with its data. */
let opened: Array<{ dialog: string; data: Record<string, unknown> }>;
let stopRecording: () => void;

const ui = () => useUIStore.getState();

function calls(cmd: string): number {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;
}

/**
 * Await `p` while letting fake time run in short steps: an edit's aftermath (the
 * refresh, the integrity re-check) waits on timers too. Fails rather than hangs,
 * and stays well inside the exits' 5 s settle bound.
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

/** Each `update_shot` IPC, held open on its own deferred, in the order the queue sends them. */
let heldEdits: Array<Deferred<unknown>>;

/** Queue entries the unit cases hold open, released after each case so a failure cannot wedge the queue. */
let heldWork: Array<Deferred<void>>;

/** Queue a bare entry on the mutation queue, held open until the case resolves it. */
function holdQueueEntry(): Deferred<void> {
  const held = deferred<void>();
  heldWork.push(held);
  void runSerialized(() => held.promise);
  return held;
}

/** The IPC of the edit the queue sent `n`-th (0-based). */
function edit(n: number): Deferred<unknown> {
  const held = heldEdits[n];
  if (!held) throw new Error(`edit ${n} has not reached the backend`);
  return held;
}

/**
 * A grid cell committed on blur: the edit is queued through the real store, its IPC
 * held. Boxed, so awaiting this does not await the edit.
 */
function queueEdit(length = 55): { done: Promise<unknown> } {
  return { done: useProjectStore.getState().updateShot(1, 1, { length }) };
}

function App() {
  // As App.tsx wires it: nothing for tier 1, `guardQuit` as tier 2.
  useBeforeUnload(false, 'unused', undefined, guardQuit);
  return null;
}

/** The backend prevents the window close and re-emits it; this is that re-emission. */
function closeRequested(): Promise<void> {
  return Promise.all(listeners.map((cb) => cb())).then(() => undefined);
}

beforeEach(() => {
  vi.useFakeTimers();
  listeners.length = 0;
  destroyMock.mockClear();
  invokeIpcMock.mockReset();
  heldEdits = [];
  heldWork = [];
  invokeIpcMock.mockImplementation((cmd: string) => {
    if (cmd === EDIT) {
      const held = deferred<unknown>();
      heldEdits.push(held);
      return held.promise;
    }
    if (cmd === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
    return Promise.resolve(null);
  });
  resetQuitConfirmationForTests();
  loadProject = vi.fn(async () => ({ success: true }));
  parseTml = vi.fn(async () => ({ cave_name: 'CaveB', sections: [] }));
  useUpdateStore.getState().reset();
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
  // A saved, local project: nothing for any guard to protect yet.
  useProjectStore.setState({
    project: { name: 'CaveA', file_path: '/cave/a.stalx' } as never,
    isDirty: false,
    isClosing: false,
    loadProject: loadProject as never,
    parseTml: parseTml as never,
  });
  opened = [];
  stopRecording = useUIStore.subscribe((s, prev) => {
    if (s.activeDialog && (s.activeDialog !== prev.activeDialog || s.dialogData !== prev.dialogData)) {
      opened.push({ dialog: s.activeDialog, data: s.dialogData });
    }
  });
});

afterEach(async () => {
  stopRecording();
  for (const held of heldWork) held.resolve();
  for (const held of heldEdits) held.resolve(LANDS);
  await settle(mutationsSettled());
  useUpdateStore.getState().reset();
  vi.useRealTimers();
});

describe('an edit queued while an exit waits is waited for too', () => {
  it('editsSettledForExit resolves only once the queue is quiet, not when the chain it was called on settles', async () => {
    const first = holdQueueEntry();
    let answer: boolean | undefined;
    const waiting = editsSettledForExit().then((settled) => {
      answer = settled;
    });
    // Queued during the wait.
    const second = holdQueueEntry();

    first.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(answer).toBeUndefined();

    second.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(answer).toBe(true);
    await waiting;
  });

  it('editsSettledForExit gives up 5 s after the call, however many edits it waited for', async () => {
    const first = holdQueueEntry();
    let answer: boolean | undefined;
    const waiting = editsSettledForExit().then((settled) => {
      answer = settled;
    });
    await vi.advanceTimersByTimeAsync(3_000);
    const second = holdQueueEntry();
    first.resolve();

    await vi.advanceTimersByTimeAsync(1_999);
    expect(answer).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1);
    // 5 s from the call, not from the edit queued later.
    expect(answer).toBe(false);

    second.resolve();
    await waiting;
  });

  it('Close on a clean project: an edit queued once its wait began prompts, instead of being closed away', async () => {
    const { result } = renderHook(() => useProject());
    const closing = result.current.handleCloseProject();
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls(EDIT)).toBe(1);
    expect(useProjectStore.getState().isDirty).toBe(false);

    edit(0).resolve(LANDS);
    await settle(done);
    const outcome = await settle(closing);

    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(calls(CLOSE)).toBe(0);
    expect(outcome.success).toBe(false);
    expect(useProjectStore.getState().project).not.toBeNull();
    expect(useProjectStore.getState().isDirty).toBe(true);
  });

  it('Quit on a clean project: an edit queued once its wait began prompts, instead of the window going', async () => {
    render(<App />);
    await vi.advanceTimersByTimeAsync(0);
    const closing = closeRequested();
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls(EDIT)).toBe(1);

    edit(0).resolve(LANDS);
    await settle(done);
    await settle(closing);

    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingQuit: true });
    expect(destroyMock).not.toHaveBeenCalled();
  });
});

describe('two exits waiting on the same edits: the one asked for last goes ahead', () => {
  it('Close, then a .stalx opened during the wait: the open asks, the close does nothing', async () => {
    const { result } = renderHook(() => useProject());
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const closing = result.current.handleCloseProject();
    const opening = routeOpenedFile(OTHER);
    await vi.advanceTimersByTimeAsync(0);
    expect(ui().activeDialog).toBeNull();

    // Refused: the project is still clean, so nothing but the order decides.
    edit(0).reject(REFUSED);
    await settle(done);
    const outcome = await settle(closing);
    await settle(opening);

    expect(calls(CLOSE)).toBe(0);
    expect(outcome.success).toBe(false);
    expect(ui().activeDialog).toBe('launch-open-confirm');
    expect(ui().dialogData).toMatchObject({ path: OTHER });
    expect(useProjectStore.getState().project).not.toBeNull();
  });

  it('a .stalx opened, then Close during the wait: the close goes ahead, the open does nothing', async () => {
    const { result } = renderHook(() => useProject());
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const opening = routeOpenedFile(OTHER);
    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);

    edit(0).reject(REFUSED);
    await settle(done);
    await settle(opening);
    const outcome = await settle(closing);

    expect(opened.map((o) => o.dialog)).not.toContain('launch-open-confirm');
    expect(ui().activeDialog).toBeNull();
    expect(loadProject).not.toHaveBeenCalled();
    expect(calls(CLOSE)).toBe(1);
    expect(outcome.success).toBe(true);
  });

  it('two .stalx opened during the wait: only the newer one asks', async () => {
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const first = routeOpenedFile(OTHER);
    const second = routeOpenedFile(NEWER);
    await vi.advanceTimersByTimeAsync(0);

    edit(0).resolve(LANDS);
    await settle(done);
    await settle(first);
    await settle(second);

    expect(opened.filter((o) => o.data.pendingOpenPath === OTHER)).toEqual([]);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingOpenPath: NEWER });
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('Close, then New Project during the wait: only New Project asks', async () => {
    const { result } = renderHook(() => useProject());
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const closing = result.current.handleCloseProject();
    const starting = newProjectGuarded();
    await vi.advanceTimersByTimeAsync(0);

    edit(0).resolve(LANDS);
    await settle(done);
    const outcome = await settle(closing);
    await settle(starting);

    expect(opened.filter((o) => o.dialog === 'unsaved-changes' && o.data.pendingNewProject !== true)).toEqual([]);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingNewProject: true });
    expect(outcome.success).toBe(false);
    expect(calls(CLOSE)).toBe(0);
  });

  it('an update that begins installing during the wait: neither Close nor the open does anything', async () => {
    const { result } = renderHook(() => useProject());
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const closing = result.current.handleCloseProject();
    const opening = routeOpenedFile(OTHER);
    await vi.advanceTimersByTimeAsync(0);
    useUpdateStore.setState({ installing: true });

    edit(0).resolve(LANDS);
    await settle(done);
    const outcome = await settle(closing);
    await settle(opening);

    expect(opened).toEqual([]);
    expect(outcome.success).toBe(false);
    expect(calls(CLOSE)).toBe(0);
    expect(loadProject).not.toHaveBeenCalled();
  });
});

describe('a dialog that took the screen during the wait is not replaced', () => {
  it('Close: the dialog stays, and the close does nothing', async () => {
    const { result } = renderHook(() => useProject());
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    ui().openDialog('settings');

    edit(0).resolve(LANDS);
    await settle(done);
    const outcome = await settle(closing);

    expect(ui().activeDialog).toBe('settings');
    expect(outcome.success).toBe(false);
    expect(calls(CLOSE)).toBe(0);
  });

  it('New Project: the dialog stays', async () => {
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const starting = newProjectGuarded();
    await vi.advanceTimersByTimeAsync(0);
    ui().openDialog('settings');

    edit(0).resolve(LANDS);
    await settle(done);
    await settle(starting);

    expect(ui().activeDialog).toBe('settings');
    expect(opened.map((o) => o.dialog)).toEqual(['settings']);
  });

  it('opening a .stalx: the dialog stays, and the router says another flow has the screen', async () => {
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const opening = routeOpenedFile(OTHER);
    await vi.advanceTimersByTimeAsync(0);
    ui().openDialog('settings');

    edit(0).resolve(LANDS);
    await settle(done);
    await settle(opening);

    expect(ui().activeDialog).toBe('settings');
    expect(ui().toasts.map((t) => t.key)).toContain(TOAST_KEYS.IMPORT_IN_PROGRESS);
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('a confirmed swap the user cancels while it waits is not carried out', async () => {
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);
    // LaunchOpenConfirmDialog routes the confirmed open before it closes itself.
    useUIStore.setState({ activeDialog: 'launch-open-confirm', dialogData: { path: OTHER, fileName: 'other.stalx' } });

    const opening = routeOpenedFile(OTHER, 'Open Failed', { swapConfirmed: true });
    await vi.advanceTimersByTimeAsync(0);
    ui().closeDialog();

    edit(0).reject(REFUSED);
    await settle(done);
    await settle(opening);

    expect(loadProject).not.toHaveBeenCalled();
    expect(ui().activeDialog).toBeNull();
  });

  it('control: the confirmed swap, still on screen when the wait ends, loads', async () => {
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);
    useUIStore.setState({ activeDialog: 'launch-open-confirm', dialogData: { path: OTHER, fileName: 'other.stalx' } });

    const opening = routeOpenedFile(OTHER, 'Open Failed', { swapConfirmed: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(loadProject).not.toHaveBeenCalled();

    edit(0).reject(REFUSED);
    await settle(done);
    await settle(opening);

    expect(loadProject).toHaveBeenCalledWith(OTHER);
  });
});

describe('the import-as-new hand-over waits before it asks, and asks with the parsed file', () => {
  /** "Close and import" on the import-as-new confirm, as ImportAsNewProjectDialog makes it. */
  function confirmImportAsNew(closeCurrent: () => Promise<{ success: boolean }>): Promise<void> {
    useUIStore.setState({ activeDialog: 'import-as-new-project', dialogData: { path: TML, fileName: 'CaveB.tml' } });
    return routeOpenedFile(TML, 'Open Failed', { importAsNew: { closeCurrent } });
  }

  it('an edit in flight when the parse lands: the unsaved-changes prompt carries the parsed file', async () => {
    const { result } = renderHook(() => useProject());
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const importing = confirmImportAsNew(result.current.handleCloseProject);
    await vi.advanceTimersByTimeAsync(0);
    expect(parseTml).toHaveBeenCalledWith(TML);
    expect(ui().activeDialog).toBe('import-as-new-project');

    edit(0).resolve(LANDS);
    await settle(done);
    await settle(importing);

    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(readPendingImport(ui().dialogData.pendingImport)).toMatchObject({ filePath: TML, importType: 'tml' });
    expect(calls(CLOSE)).toBe(0);
    expect(useProjectStore.getState().project).not.toBeNull();
  });

  it('the confirm cancelled while the hand-over waits: nothing opens, and the project stays', async () => {
    const { result } = renderHook(() => useProject());
    const { done } = queueEdit();
    await vi.advanceTimersByTimeAsync(0);

    const importing = confirmImportAsNew(result.current.handleCloseProject);
    await vi.advanceTimersByTimeAsync(0);
    ui().closeDialog();

    edit(0).resolve(LANDS);
    await settle(done);
    await settle(importing);

    expect(ui().activeDialog).toBeNull();
    expect(calls(CLOSE)).toBe(0);
    expect(useProjectStore.getState().project).not.toBeNull();
  });

  it('control: nothing in flight on a clean project: the hand-over closes it and opens the new-project dialog', async () => {
    const { result } = renderHook(() => useProject());

    const importing = confirmImportAsNew(result.current.handleCloseProject);
    await settle(importing);

    expect(calls(CLOSE)).toBe(1);
    expect(ui().activeDialog).toBe('new-project-for-import');
    expect(ui().dialogData).toMatchObject({ filePath: TML, importType: 'tml' });
  });
});
