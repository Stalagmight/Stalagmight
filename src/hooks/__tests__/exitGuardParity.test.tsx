// @vitest-environment jsdom
/**
 * The ways to leave an open project must agree about which SpeleoDB state deserves
 * the exit prompt - and since the lock-release fix, the answer is:
 *
 *   we hold the write lock (editState 'editing') AND there is work SpeleoDB has not
 *   seen (unsaved edits OR has_unpushed_edits).
 *
 * The previous contract here was "saved work never prompts", which is exactly the
 * bug: autosave clears the unsaved flag, the exit then released the lock over the
 * saved-but-unpushed edits, and the next open downloaded, refused to overwrite the
 * local copy and offered only "take the server copy and discard my changes".
 *
 * A read-only project (no lock) never gets the lock prompt - there is nothing to
 * keep or release - only the ordinary unsaved-changes handling. A clean project is
 * left silently and its lock released as before.
 *
 * Every exit routes through `needsSpeleoExitPrompt` and the awaited
 * `askSpeleoExit`, and the matrix below keeps them from drifting apart again.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
  confirm: vi.fn(async () => false),
}));
vi.mock('../../utils/ipc', () => ({
  invokeIpc: vi.fn(),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { useProject } from '../useProject';
import { guardQuit, resetQuitConfirmationForTests } from '../../utils/quit';
import { routeOpenedFile } from '../../utils/openOrImport';
import { newProjectGuarded } from '../../utils/projectActions';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { needsSpeleoExitPrompt, resetSpeleoExitPromptForTests, useSpeleoExitPrompt } from '../../utils/speleoExit';
import { useExitConfirm } from '../../utils/exitConfirm';
import { registerWorkspaceDraft } from '../../utils/workspaceDrafts';

const link = (o: Partial<SpeleoLinkState>): SpeleoLinkState => ({
  permission: 'READ_AND_WRITE',
  editState: 'read-only',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: false,
  unwound: false,

  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
  ...o,
});

/** The other project the "open another .stalx" exit is asked for. */
const OTHER = '/cave/other.stalx';

let openDialog: ReturnType<typeof vi.fn>;
let closeProject: ReturnType<typeof vi.fn>;
let loadProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  openDialog = vi.fn();
  closeProject = vi.fn(async () => ({ success: true, warnings: [], error: null, section_ids: [] }));
  loadProject = vi.fn(async () => ({ success: true }));
  resetQuitConfirmationForTests();
  resetSpeleoExitPromptForTests();
  useProjectStore.setState({
    project: { name: 'Cave A', file_path: '/cache/p1.stalx' } as never,
    isDirty: false,
    closeProject,
    loadProject,
    saveProject: vi.fn(async () => ({ success: true, warnings: [], error: null, section_ids: [] })),
  });
  useUIStore.setState({ openDialog: openDialog as never, activeDialog: null, dialogData: {} });
});

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/**
 * Start one exit, and if it raised the SpeleoDB exit prompt answer it with
 * Cancel. Returns whether it was raised. The exit is awaited to the end either way,
 * so "prompted" always also means "and, cancelled, did NOT leave".
 */
async function prompted(exit: () => unknown): Promise<boolean> {
  const running = Promise.resolve(exit());
  await tick();
  const request = useSpeleoExitPrompt.getState().request;
  request?.settle(false);
  await running;
  return request !== null;
}

/**
 * `routeOpenedFile` also raises an UNRELATED confirm before a clean, non-SpeleoDB
 * project gets swapped out (#424). Answer it the way its "Open" button does, so the
 * exit's actual outcome still shows up.
 */
async function openOther(): Promise<void> {
  await routeOpenedFile(OTHER);
  if (openDialog.mock.calls.some((c) => c[0] === 'launch-open-confirm')) {
    await routeOpenedFile(OTHER, 'Open Failed', { swapConfirmed: true });
  }
}

async function allExits(): Promise<{ quit: boolean; close: boolean; open: boolean; new: boolean }> {
  const { result } = renderHook(() => useProject());
  const quit = await prompted(() => guardQuit());
  const close = await prompted(() => result.current.handleCloseProject());
  const open = await prompted(openOther);
  const nu = await prompted(() => newProjectGuarded());
  return { quit, close, open, new: nu };
}

const NONE = { quit: false, close: false, open: false, new: false };
const ALL = { quit: true, close: true, open: true, new: true };

describe('the exit decision table', () => {
  it.each([
    { isDirty: false, link: { editState: 'editing' as const, hasUnpushedEdits: true }, ask: true },
    { isDirty: true, link: { editState: 'editing' as const, hasUnpushedEdits: true }, ask: true },
    { isDirty: true, link: { editState: 'editing' as const, hasUnpushedEdits: false }, ask: true },
    { isDirty: false, link: { editState: 'editing' as const, hasUnpushedEdits: false }, ask: false },
    { isDirty: false, link: { editState: 'read-only' as const, hasUnpushedEdits: true }, ask: false },
    { isDirty: true, link: { editState: 'read-only' as const, hasUnpushedEdits: true }, ask: false },
    { isDirty: true, link: null, ask: false },
  ])('needsSpeleoExitPrompt(%j)', ({ isDirty, link: l, ask }) => {
    expect(needsSpeleoExitPrompt(l ? link(l) : null, isDirty)).toBe(ask);
  });
});

describe('every exit agrees', () => {
  it('saved but un-uploaded work under our lock prompts on every exit', async () => {
    useSpeleoStore.setState({ link: link({ editState: 'editing', hasUnpushedEdits: true }) });
    expect(await allExits()).toEqual(ALL);
    // Cancelled: nothing was closed, nothing else was opened.
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
    expect(openDialog).not.toHaveBeenCalledWith('new-project');
  });

  it('unsaved work under our lock prompts on every exit, and the disk prompt never runs', async () => {
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link({ editState: 'editing', hasUnpushedEdits: false }) });
    expect(await allExits()).toEqual(ALL);
    expect(openDialog).not.toHaveBeenCalledWith('unsaved-changes', expect.anything());
    expect(openDialog).not.toHaveBeenCalledWith('unsaved-changes');
    expect(closeProject).not.toHaveBeenCalled();
  });

  it.each([
    { editState: 'editing' as const, hasUnpushedEdits: false },
    { editState: 'read-only' as const, hasUnpushedEdits: true },
    { editState: 'read-only' as const, hasUnpushedEdits: false },
  ])('a clean lock, or work we hold no lock over, leaves without the prompt: %j', async (state) => {
    useSpeleoStore.setState({ link: link(state) });
    expect(await allExits()).toEqual(NONE);
    expect(closeProject).toHaveBeenCalled();
    expect(loadProject).toHaveBeenCalledWith(OTHER);
  });

  it('read-only unsaved work gets the ordinary unsaved-changes prompt on every exit', async () => {
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: link({ editState: 'read-only', hasUnpushedEdits: true }) });
    expect(await allExits()).toEqual(NONE);
    const unsaved = openDialog.mock.calls.filter((c) => c[0] === 'unsaved-changes');
    expect(unsaved).toHaveLength(4);
    expect(closeProject).not.toHaveBeenCalled();
  });
});

describe('an answered prompt continues the exit the user asked for', () => {
  beforeEach(() => {
    useSpeleoStore.setState({ link: link({ editState: 'editing', hasUnpushedEdits: true }) });
  });

  /** Raise the prompt from `exit`, answer it as carried out, and wait for the exit. */
  async function answered(exit: () => unknown): Promise<void> {
    const running = Promise.resolve(exit());
    await tick();
    const request = useSpeleoExitPrompt.getState().request;
    expect(request).not.toBeNull();
    // The prompt closes the project itself before it settles `true`.
    useProjectStore.setState({ project: null });
    request?.settle(true);
    await running;
    await tick();
  }

  it('close lands on the home screen', async () => {
    const setView = vi.fn();
    useUIStore.setState({ setView: setView as never });
    const { result } = renderHook(() => useProject());
    await answered(() => result.current.handleCloseProject());
    expect(setView).toHaveBeenCalledWith('global');
  });

  it('opening another file then opens it', async () => {
    await answered(() => routeOpenedFile(OTHER));
    // The destination preflight ran before the prompt, so the load skips it.
    expect(loadProject).toHaveBeenCalledWith(OTHER, { accessConfirmed: true });
  });

  it('New Project then opens the New Project dialog', async () => {
    await answered(() => newProjectGuarded());
    expect(openDialog).toHaveBeenCalledWith('new-project');
  });
});

describe('the disk-dirty guard still covers projects with no SpeleoDB lock', () => {
  beforeEach(() => {
    useProjectStore.setState({ isDirty: true });
  });

  it('falls back to unsaved-changes for a local project', async () => {
    useSpeleoStore.setState({ link: null });

    await routeOpenedFile(OTHER);
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', { pendingOpenPath: OTHER });

    openDialog.mockClear();
    newProjectGuarded();
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', { pendingNewProject: true });
  });
});

/**
 * frontend#10 - unsent SpeleoDB input (an upload description, the project form) is
 * asked about in-app FIRST on every exit, before the exit prompt can upload,
 * release or close anything. Declining stops the exit with nothing done.
 */
describe('unsent SpeleoDB input is asked about before the exit prompt', () => {
  let discard: ReturnType<typeof vi.fn>;
  let unregister: () => void;
  beforeEach(() => {
    useExitConfirm.setState({ request: null });
    useSpeleoStore.setState({ link: link({ editState: 'editing', hasUnpushedEdits: true }) });
    discard = vi.fn();
    unregister = registerWorkspaceDraft({
      label: () => 'the synthetic form', dirty: () => discard.mock.calls.length === 0, discard,
    });
  });
  afterEach(() => unregister());

  function exits(): Record<'close' | 'open' | 'new', () => unknown> {
    const { result } = renderHook(() => useProject());
    return {
      close: () => result.current.handleCloseProject(),
      open: openOther,
      new: () => newProjectGuarded(),
    };
  }

  it.each(['close', 'open', 'new'] as const)('%s: declining stops before the exit prompt', async (exit) => {
    const running = Promise.resolve(exits()[exit]());
    await vi.waitFor(() => expect(useExitConfirm.getState().request).not.toBeNull());
    expect(useExitConfirm.getState().request?.message).toContain('the synthetic form');
    expect(useSpeleoExitPrompt.getState().request).toBeNull();
    useExitConfirm.getState().request?.answer(false);
    await running;
    await tick();
    expect(useSpeleoExitPrompt.getState().request).toBeNull();
    expect(discard).not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
    expect(openDialog).not.toHaveBeenCalledWith('new-project');
  });

  it.each(['close', 'open', 'new'] as const)('%s: discarding goes on to the exit prompt', async (exit) => {
    const running = Promise.resolve(exits()[exit]());
    await vi.waitFor(() => expect(useExitConfirm.getState().request).not.toBeNull());
    useExitConfirm.getState().request?.answer(true);
    await vi.waitFor(() => expect(useSpeleoExitPrompt.getState().request).not.toBeNull());
    expect(discard).toHaveBeenCalledOnce();
    useSpeleoExitPrompt.getState().request?.settle(false);
    await running;
  });
});
