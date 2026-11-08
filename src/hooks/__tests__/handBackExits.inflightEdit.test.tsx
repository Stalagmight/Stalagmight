// @vitest-environment jsdom
/**
 * [#576 W84] The three hand-back exits wait for the edits queued before them, as
 * quitting does (W83, `useBeforeUnload.inflightEdit.test.tsx`).
 *
 * Close, New Project and opening another `.stalx` decide from `isDirty`, which an
 * edit sets only once its IPC and the refresh after it have both returned. The
 * click on the exit is also the blur that commits a grid cell being edited, so on
 * a clean project that edit is still in flight when the guard reads the flag. The
 * project switch then waits for the edit (`runProjectSwitch`), so it lands on the
 * project being replaced, which goes with the edit unsaved and no prompt.
 *
 * These run each exit with the REAL guard and the REAL store (`updateShot` through
 * the mutation queue, the real `closeProject`), holding the edit's IPC open by hand.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook } from '@testing-library/react';

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
import { newProjectGuarded } from '../../utils/projectActions';
import { routeOpenedFile } from '../../utils/openOrImport';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { mutationsSettled } from '../../store/mutationQueue';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

emptyBinaryRefresh();

const EDIT = 'update_shot';
const CLOSE = 'close_project';
/** The other project the "open another .stalx" exit is asked for. */
const OTHER = '/cave/other.stalx';

let loadProject: ReturnType<typeof vi.fn>;

function calls(cmd: string): number {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;
}

/**
 * Await `p` while letting fake time run in short steps: the edit's aftermath (the
 * refresh, the integrity re-check) waits on timers too. Fails rather than hangs,
 * and stays well inside the exits' settle timeout.
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

const ui = () => useUIStore.getState();

beforeEach(() => {
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  loadProject = vi.fn(async () => ({ success: true }));
  useUIStore.setState({ activeDialog: null, dialogData: {}, toasts: [] });
  useSpeleoStore.setState({ link: null });
  // A saved, local project: nothing for any guard to protect yet.
  useProjectStore.setState({
    project: { name: 'CaveA', file_path: '/cave/a.stalx' } as never,
    isDirty: false,
    isClosing: false,
    loadProject: loadProject as never,
  });
});

afterEach(async () => {
  heldEdit?.resolve({ remote_link: null });
  heldEdit = null;
  await settle(mutationsSettled());
  vi.useRealTimers();
});

describe('Close waits for the edits queued before it', () => {
  it('an edit in flight on a clean project: Close prompts once the edit lands, and does not close', async () => {
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    // Still waiting on the edit: no decision yet either way.
    expect(ui().activeDialog).toBeNull();
    expect(calls(CLOSE)).toBe(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    const outcome = await settle(closing);

    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(calls(CLOSE)).toBe(0);
    expect(outcome.success).toBe(false);
    expect(useProjectStore.getState().project).not.toBeNull();
    expect(useProjectStore.getState().isDirty).toBe(true);
  });

  it('control: a clean project with nothing in flight closes at once, with no prompt', async () => {
    const { result } = renderHook(() => useProject());

    // No fake time passes: the close does not sit out the settle timeout.
    const outcome = await result.current.handleCloseProject();

    expect(outcome.success).toBe(true);
    expect(calls(CLOSE)).toBe(1);
    expect(ui().activeDialog).toBeNull();
  });

  it('an edit that never settles counts as unsaved work once the wait runs out', async () => {
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(4_000);
    expect(ui().activeDialog).toBeNull();

    await vi.advanceTimersByTimeAsync(1_000);
    // The project still reads clean, but the edit may yet land: ask.
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(calls(CLOSE)).toBe(0);
    expect(useProjectStore.getState().isDirty).toBe(false);
    await settle(closing);

    edit.resolve({ remote_link: null });
    await settle(editDone);
  });
});

describe('New Project waits for the edits queued before it', () => {
  it('an edit in flight on a clean project: New Project prompts once the edit lands', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const starting = newProjectGuarded();
    await vi.advanceTimersByTimeAsync(0);
    expect(ui().activeDialog).toBeNull();

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(starting);

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingNewProject: true });
  });

  it('control: a clean project with nothing in flight opens the new-project dialog at once', async () => {
    await newProjectGuarded();

    expect(ui().activeDialog).toBe('new-project');
  });

  it('an edit that never settles counts as unsaved work once the wait runs out', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const starting = newProjectGuarded();
    await vi.advanceTimersByTimeAsync(4_000);
    expect(ui().activeDialog).toBeNull();

    await vi.advanceTimersByTimeAsync(1_000);
    await settle(starting);
    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingNewProject: true });

    edit.resolve({ remote_link: null });
    await settle(editDone);
  });
});

describe('Opening another .stalx waits for the edits queued before it', () => {
  it('an edit in flight on a clean project: the open prompts about unsaved changes once the edit lands', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const opening = routeOpenedFile(OTHER);
    await vi.advanceTimersByTimeAsync(0);
    expect(ui().activeDialog).toBeNull();

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(opening);

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingOpenPath: OTHER });
    expect(loadProject).not.toHaveBeenCalled();
  });

  // The confirmed swap re-enters with `swapConfirmed`, which used to fall straight
  // through to the load: an edit still in flight then landed on the project the
  // load replaced.
  it('an edit in flight when the swap is confirmed: prompts instead of loading', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();
    // LaunchOpenConfirmDialog routes the confirmed open before it closes itself.
    useUIStore.setState({ activeDialog: 'launch-open-confirm', dialogData: { path: OTHER } });

    const opening = routeOpenedFile(OTHER, 'Open Failed', { swapConfirmed: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(loadProject).not.toHaveBeenCalled();

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(opening);

    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingOpenPath: OTHER });
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('control: a clean project with nothing in flight asks to swap at once, and a confirmed swap loads at once', async () => {
    await routeOpenedFile(OTHER);
    expect(ui().activeDialog).toBe('launch-open-confirm');
    expect(loadProject).not.toHaveBeenCalled();

    await routeOpenedFile(OTHER, 'Open Failed', { swapConfirmed: true });
    expect(loadProject).toHaveBeenCalledWith(OTHER);
  });

  it('an edit that never settles counts as unsaved work once the wait runs out', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const opening = routeOpenedFile(OTHER);
    await vi.advanceTimersByTimeAsync(4_000);
    expect(ui().activeDialog).toBeNull();

    await vi.advanceTimersByTimeAsync(1_000);
    await settle(opening);
    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingOpenPath: OTHER });
    expect(loadProject).not.toHaveBeenCalled();

    edit.resolve({ remote_link: null });
    await settle(editDone);
  });
});

/**
 * [#576 W85] "Import as new project" decides after a good parse whether the close
 * must ask first, and a prompt it raises carries the parsed file (`pendingImport`,
 * #576 HFI-4-D2). It read `isDirty` without waiting, so an edit in flight read as a
 * clean project and the close was asked for at once: that close waits for the edit,
 * finds the project dirty and asks WITHOUT the file, and the import ended there.
 */
describe('Import as new project waits for the edits queued before it', () => {
  const TML = '/cave/CaveA.tml';
  const PARSED = { cave_name: 'CaveA', sections: [] };

  /** The confirm dialog as the router opened it, then "Close and import" with the real guarded close. */
  function confirmImportAsNew(closeCurrent: () => Promise<{ success: boolean }>): Promise<void> {
    useProjectStore.setState({ parseTml: vi.fn(async () => PARSED) as never });
    useUIStore.setState({ activeDialog: 'import-as-new-project', dialogData: { path: TML, fileName: 'CaveA.tml' } });
    return routeOpenedFile(TML, 'Open Failed', { importAsNew: { closeCurrent } });
  }

  it('an edit in flight when the parse lands: the prompt carries the parsed file, and the project stays open', async () => {
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const importing = confirmImportAsNew(() => result.current.handleCloseProject());
    await vi.advanceTimersByTimeAsync(0);
    expect(calls(CLOSE)).toBe(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(importing);

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData.pendingImport).toMatchObject({ filePath: TML, importType: 'tml', tmlFile: PARSED });
    expect(calls(CLOSE)).toBe(0);
    expect(useProjectStore.getState().project).not.toBeNull();
  });

  it('an edit that never settles counts as unsaved work, and the prompt still carries the parsed file', async () => {
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const importing = confirmImportAsNew(() => result.current.handleCloseProject());
    await vi.advanceTimersByTimeAsync(5_000);
    await settle(importing);

    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData.pendingImport).toMatchObject({ filePath: TML, importType: 'tml' });
    expect(calls(CLOSE)).toBe(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
  });

  it('a confirm cancelled while the edit is still landing drops the file: no prompt, the project stays', async () => {
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const importing = confirmImportAsNew(() => result.current.handleCloseProject());
    await vi.advanceTimersByTimeAsync(0);
    // The confirm's Cancel, while the router still waits on the edit.
    useUIStore.getState().closeDialog();

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(importing);

    expect(ui().activeDialog).toBeNull();
    expect(calls(CLOSE)).toBe(0);
    expect(useProjectStore.getState().project).not.toBeNull();
  });

  it('control: a clean project with nothing in flight closes and opens the new-project dialog with the file', async () => {
    const { result } = renderHook(() => useProject());

    await settle(confirmImportAsNew(() => result.current.handleCloseProject()));

    expect(calls(CLOSE)).toBe(1);
    expect(ui().activeDialog).toBe('new-project-for-import');
    expect(ui().dialogData).toMatchObject({ filePath: TML, importType: 'tml' });
  });
});

/**
 * [#576 W85] One hand-back exit decides at a time. Each one waits for the queued
 * edits and decides after the wait, but the screen-owned checks in front of it (a
 * dialog open, the shortcut hooks' modal gate) were all passed before it. A second
 * exit asked for during the wait - Ctrl+N twice, a double-click on a Recents entry,
 * Close then New - passed them too, and its prompt replaced the first one's in the
 * single dialog slot, with the first call's request gone. The first exit owns the
 * answer; a later one is dropped, as a shortcut is while a dialog owns the screen.
 */
describe('a second hand-back exit asked for during the wait is dropped', () => {
  const OTHER_2 = '/cave/other-2.stalx';

  /** Every `unsaved-changes` prompt opened from here on (`openDialog` installs fresh data each time). */
  function recordPrompts(): Array<Record<string, unknown>> {
    const opened: Array<Record<string, unknown>> = [];
    unsubscribe = useUIStore.subscribe((s, prev) => {
      if (s.activeDialog === 'unsaved-changes' && s.dialogData !== prev.dialogData) opened.push(s.dialogData);
    });
    return opened;
  }
  let unsubscribe: (() => void) | null = null;
  afterEach(() => {
    unsubscribe?.();
    unsubscribe = null;
  });

  it('New Project twice (Ctrl+N, Ctrl+N): one prompt', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();
    const prompts = recordPrompts();

    const first = newProjectGuarded();
    await vi.advanceTimersByTimeAsync(0);
    const second = newProjectGuarded();
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([first, second]));

    expect(prompts).toHaveLength(1);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData).toMatchObject({ pendingNewProject: true });
  });

  it('two opens during the wait (a double-click on Recents): one prompt, for the file opened first', async () => {
    const edit = holdEdit();
    const { done: editDone } = await startEdit();
    const prompts = recordPrompts();

    const first = routeOpenedFile(OTHER, 'Open Failed', { allow: 'project-only' });
    await vi.advanceTimersByTimeAsync(0);
    const second = routeOpenedFile(OTHER_2, 'Open Failed', { allow: 'project-only' });
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([first, second]));

    expect(prompts).toHaveLength(1);
    expect(ui().dialogData).toMatchObject({ pendingOpenPath: OTHER });
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('Close, then New Project during the wait: one prompt, the close one', async () => {
    const { result } = renderHook(() => useProject());
    const edit = holdEdit();
    const { done: editDone } = await startEdit();
    const prompts = recordPrompts();

    const closing = result.current.handleCloseProject();
    await vi.advanceTimersByTimeAsync(0);
    const starting = newProjectGuarded();
    await vi.advanceTimersByTimeAsync(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    await settle(Promise.all([closing, starting]));

    expect(prompts).toHaveLength(1);
    expect(ui().activeDialog).toBe('unsaved-changes');
    expect(ui().dialogData.pendingNewProject).toBeUndefined();
    expect(calls(CLOSE)).toBe(0);
  });

  it('control: once the first exit has decided, the next one is asked as usual', async () => {
    await newProjectGuarded();
    expect(ui().activeDialog).toBe('new-project');
    useUIStore.getState().closeDialog();

    await routeOpenedFile(OTHER);
    expect(ui().activeDialog).toBe('launch-open-confirm');
  });
});
