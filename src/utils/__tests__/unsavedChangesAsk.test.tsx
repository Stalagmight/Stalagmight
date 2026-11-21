// @vitest-environment jsdom
/**
 * Pre-PR delta review finding 1: every exit asks the backend's unsaved-changes
 * flag (`askOpenProjectUnsaved`), an IPC that had no timeout, and the close
 * guard asked it while it held the close listener's latch, so a backend that
 * never answered made the window X dead for good. The ask is bounded now: when
 * the bound runs out it answers "unsaved" (ask the user, fail-safe).
 *
 * And the answer belongs to the project it was asked about (the same file,
 * `created_at` and load): a project that moved during the ask (a load or a
 * close landed) gets no prompt, no save and no close on the old project's
 * answer. Each exit decides again for what is open now, or steps aside.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
  confirm: vi.fn(async () => false),
}));
vi.mock('../ipc', () => ({
  invokeIpc: vi.fn(),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { invokeIpc } from '../ipc';
import { useProject } from '../../hooks/useProject';
import { guardQuit, resetQuitConfirmationForTests } from '../quit';
import { routeOpenedFile } from '../openOrImport';
import { newProjectGuarded } from '../projectActions';
import { closeKeepingLock, resetSpeleoExitPromptForTests } from '../speleoExit';
import { protectAccountTransition } from '../protectAccountTransition';
import { forceQuitNow, resetQuitWaitForTests, useQuitWait } from '../quitAfterWork';
import { UNSAVED_ASK_BOUND_MS } from '../unsavedChanges';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

const OTHER = '/cave/other.stalx';

let openDialog: ReturnType<typeof vi.fn>;
let closeProject: ReturnType<typeof vi.fn>;
let loadProject: ReturnType<typeof vi.fn>;
let saveProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.mocked(invokeIpc).mockReset();
  openDialog = vi.fn();
  closeProject = vi.fn(async () => ({ success: true, warnings: [], error: null, line_ids: [] }));
  loadProject = vi.fn(async () => ({ success: true }));
  saveProject = vi.fn(async () => ({ success: true, warnings: [], error: null, line_ids: [] }));
  resetQuitConfirmationForTests();
  resetSpeleoExitPromptForTests();
  resetQuitWaitForTests();
  useProjectStore.setState({
    project: { name: 'Cave A', file_path: '/cave/a.stalx', created_at: '2026-01-01T00:00:00Z' } as never,
    isDirty: false,
    closeProject,
    loadProject,
    saveProject,
  });
  useSpeleoStore.setState({ link: null });
  useUIStore.setState({ openDialog: openDialog as never, activeDialog: null, dialogData: {} });
});

afterEach(() => {
  vi.useRealTimers();
  resetQuitWaitForTests();
});

const unsavedPrompts = () => openDialog.mock.calls.filter((c) => c[0] === 'unsaved-changes');

describe('a backend that never answers the unsaved-changes flag', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(invokeIpc).mockImplementation(((command: string) =>
      command === 'has_unsaved_changes' ? new Promise(() => {}) : Promise.resolve(null)) as never);
  });

  /**
   * Injection: awaiting `has_unsaved_changes` without the bound leaves every
   * exit pending forever, and this test times out.
   */
  it('every exit asks the user once the bound runs out', async () => {
    const { result } = renderHook(() => useProject());
    const exits = [
      () => guardQuit(),
      () => result.current.handleCloseProject(),
      () => routeOpenedFile(OTHER),
      () => newProjectGuarded(),
    ];
    for (const exit of exits) {
      const running = Promise.resolve(exit());
      await vi.advanceTimersByTimeAsync(UNSAVED_ASK_BOUND_MS - 1);
      expect(unsavedPrompts(), 'not before the bound').toHaveLength(exits.indexOf(exit));
      await vi.advanceTimersByTimeAsync(1);
      await running;
    }
    expect(unsavedPrompts()).toEqual([
      ['unsaved-changes', { pendingQuit: true }],
      ['unsaved-changes'],
      ['unsaved-changes', { pendingOpenPath: OTHER }],
      ['unsaved-changes', { pendingNewProject: true }],
    ]);
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
  });
});

describe('a project that moved during the ask', () => {
  /** The first ask waits for `release`; every later one answers "clean". */
  let release: (unsaved: boolean) => void;
  let asks: number;

  beforeEach(() => {
    asks = 0;
    vi.mocked(invokeIpc).mockImplementation(((command: string) => {
      if (command !== 'has_unsaved_changes') return Promise.resolve(null);
      asks += 1;
      if (asks > 1) return Promise.resolve(false);
      return new Promise<boolean>((resolve) => {
        release = resolve;
      });
    }) as never);
  });

  /** Start `exit`, open another project while its first ask is pending, then answer "unsaved". */
  async function swappedDuringTheAsk<T>(exit: () => Promise<T> | T): Promise<T> {
    const running = Promise.resolve(exit());
    await vi.waitFor(() => expect(asks).toBe(1));
    const { loading } = useProjectStore.getState();
    useProjectStore.setState({
      project: { name: 'Cave B', file_path: '/cave/b.stalx', created_at: '2026-02-02T00:00:00Z' } as never,
      loading: { ...loading, clientOpId: loading.clientOpId + 1 },
    });
    release(true);
    const outcome = await running;
    await new Promise((resolve) => setTimeout(resolve, 0));
    return outcome;
  }

  /**
   * Injection: acting on the answer without `stillOpen` (any one exit) opens
   * the unsaved-changes prompt, saves or closes over the project that
   * replaced the one the answer was about.
   */
  it('quit decides again for the project open now', async () => {
    expect(await swappedDuringTheAsk(() => guardQuit())).toBe(false);
    expect(asks).toBe(2);
    expect(unsavedPrompts()).toEqual([]);
  });

  it('close steps aside: no prompt, nothing closed', async () => {
    const { result } = renderHook(() => useProject());
    expect(await swappedDuringTheAsk(() => result.current.handleCloseProject())).toEqual({
      success: false,
      error: 'Superseded',
    });
    expect(unsavedPrompts()).toEqual([]);
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('opening another file is routed again against the project open now', async () => {
    await swappedDuringTheAsk(() => routeOpenedFile(OTHER));
    expect(asks).toBe(2);
    expect(unsavedPrompts()).toEqual([]);
    // The project open now is clean and local: the ordinary swap question.
    expect(openDialog).toHaveBeenCalledWith('launch-open-confirm', expect.objectContaining({ path: OTHER }));
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('New Project decides again for the project open now', async () => {
    await swappedDuringTheAsk(() => newProjectGuarded());
    expect(unsavedPrompts()).toEqual([]);
    expect(openDialog).toHaveBeenCalledWith('new-project');
  });

  it('"Keep the lock" saves and closes nothing', async () => {
    const outcome = await swappedDuringTheAsk(() => closeKeepingLock());
    expect(outcome.ok).toBe(false);
    expect(saveProject).not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('"Quit now" asks again and does not save the project that was just opened', async () => {
    useQuitWait.setState({ waiting: true, step: 'force', error: null });
    await swappedDuringTheAsk(() => forceQuitNow());
    expect(asks).toBe(2);
    expect(saveProject).not.toHaveBeenCalled();
  });

  it('an account change protects the project open now, not the one that left', async () => {
    expect(await swappedDuringTheAsk(() => protectAccountTransition())).toBe(true);
    expect(asks).toBe(2);
    expect(saveProject).not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
  });
});
