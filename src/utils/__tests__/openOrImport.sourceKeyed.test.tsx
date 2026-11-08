// @vitest-environment jsdom
/**
 * [#576 FUZZ-MERGE-NF] A dropped `.stalx` route clears only its own entry-point source.
 *
 * Each open/import names where it was asked from (`setProjectActionSource`: keyboard,
 * drag-drop, ...), and the route drops that source on every way out that hands nothing
 * on. Over an open project a `.stalx` route waits for the queued edits first, and a newer
 * request asked for meanwhile wins (latest wins). The older route then ends with nothing
 * handed on, and its `finally` cleared the source - the NEWER request's, set just before
 * it: the newer open went on, and its prompt or load was attributed to nothing. Now the
 * clear is keyed to the setting the route started with.
 *
 * The REAL store (`updateShot` through the mutation queue), the real router and the real
 * telemetry source; the IPC is the shared mock, the edit's reply held by hand.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));
vi.mock('../ipc');

import { routeOpenedFile } from '../openOrImport';
import { clearProjectActionSource, peekProjectActionSource, setProjectActionSource } from '../telemetry';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { mutationsSettled } from '../../store/mutationQueue';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

emptyBinaryRefresh();

const EDIT = 'update_shot';
const FIRST = '/cave/first.stalx';
const SECOND = '/cave/second.stalx';

const calls = (cmd: string) => invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;

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

/** A grid cell committed on blur: queued, its IPC not answered. Boxed, so awaiting this does not await the edit. */
async function startEdit(): Promise<{ done: Promise<unknown> }> {
  const done = useProjectStore.getState().updateShot(1, 1, { length: 55 });
  await vi.advanceTimersByTimeAsync(0);
  expect(calls(EDIT)).toBe(1);
  return { done };
}

beforeEach(() => {
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  clearProjectActionSource();
  useUIStore.setState({ activeDialog: null, dialogData: {}, toasts: [] });
  useSpeleoStore.setState({ link: null });
  useProjectStore.setState({
    project: { name: 'CaveA', file_path: '/cave/a.stalx' } as never,
    isDirty: false,
    isClosing: false,
    loadProject: vi.fn(async () => ({ success: true })) as never,
  });
});

afterEach(async () => {
  heldEdit?.resolve({ remote_link: null });
  heldEdit = null;
  await settle(mutationsSettled());
  clearProjectActionSource();
  vi.useRealTimers();
});

describe('a dropped .stalx route keeps a newer request\'s entry-point source (#576 FUZZ-MERGE-NF)', () => {
  it('Ctrl+O, then a file dropped during the wait: the drop prompts with its own source still set', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    setProjectActionSource('keyboard');
    const first = routeOpenedFile(FIRST);
    await vi.advanceTimersByTimeAsync(0);
    setProjectActionSource('drag-drop');
    const second = routeOpenedFile(SECOND);
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([first, second]));

    // The newer request won: its prompt is up, carrying its file ...
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData).toEqual({ pendingOpenPath: SECOND });
    // ... and the source it set is still there for that prompt's flow to apply.
    expect(peekProjectActionSource()).toBe('drag-drop');
  });

  it('control: a route dropped with nothing newer asked for still clears its own source', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    setProjectActionSource('keyboard');
    const first = routeOpenedFile(FIRST);
    await vi.advanceTimersByTimeAsync(0);
    // The screen is taken during the wait: the route does not replace it, and ends.
    useUIStore.getState().openDialog('settings');

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(first);

    expect(useUIStore.getState().activeDialog).toBe('settings');
    expect(peekProjectActionSource()).toBeUndefined();
  });
});
