// @vitest-environment jsdom
/**
 * Never strand a SpeleoDB lock silently, on every route that leaves a project:
 * Close, a project switch (file open / New Project / open from SpeleoDB all end in
 * `loadProject` or `createProject`), a quit the app drives (`guardQuit`), and a
 * quit that first waited for a transfer to finish. A clean release goes on with
 * no question; a release the backend reports as failed or uncertain
 * (`lock_release: 'failed'`) raises "Couldn't hand back the lock" and nothing
 * leaves until the user answers: Retry, Close and keep the lock, or Cancel.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, renderHook, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
const loadBinaryMock = vi.hoisted(() => vi.fn());
const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
const destroyMock = vi.hoisted(() => vi.fn(async () => {}));
const closeMock = vi.hoisted(() => vi.fn());

vi.mock('../ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: (e: unknown) => String(e) }));
vi.mock('../loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: vi.fn(async () => true), open: vi.fn(), save: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return vi.fn();
  }),
}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: destroyMock, close: closeMock }),
}));

import SpeleoReleaseFailedDialog from '../../components/Dialogs/SpeleoReleaseFailedDialog';
import { useBeforeUnload } from '../../hooks/useBeforeUnload';
import { useProject } from '../../hooks/useProject';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoNetworkStore } from '../../store/speleoNetworkStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import type { CommandResult } from '../../types/commands';
import {
  guardQuit,
  requestUpdateRestart,
  resetQuitConfirmationForTests,
  resetUpdateRestartForTests,
} from '../quit';
import { QUIT_SETTLE_MS, resetQuitWaitForTests } from '../quitAfterWork';
import { closeAfterUpload, resetSpeleoExitPromptForTests } from '../speleoExit';
import { resetReleaseFailurePromptForTests, useReleaseFailurePrompt } from '../speleoReleaseFailure';

const ok: CommandResult = { success: true, warnings: [], error: null, section_ids: [] };
const released: CommandResult = { ...ok, lock_release: 'released' };
const failed = (error = 'The SpeleoDB lock could not be handed back: SpeleoDB returned HTTP 503'): CommandResult =>
  ({ success: false, warnings: [], error, section_ids: [], lock_release: 'failed' });

const editing: SpeleoLinkState = {
  permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: 'me@example.com', lockedByOther: false,
  hasUnpushedEdits: false, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
  noAccess: false, serverError: null,
};

/** Answer each IPC command from a per-command queue (last answer repeats). */
let replies: Record<string, CommandResult[]>;
const calls = (command: string) => invokeIpcMock.mock.calls.filter(([cmd]) => cmd === command);

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

async function waitForPrompt() {
  await vi.waitFor(() => expect(useReleaseFailurePrompt.getState().request).not.toBeNull());
  return useReleaseFailurePrompt.getState().request!;
}

beforeEach(() => {
  vi.clearAllMocks();
  listeners.length = 0;
  closeMock.mockReset().mockImplementation(async () => {
    for (const cb of [...listeners]) await cb();
  });
  replies = {};
  invokeIpcMock.mockImplementation(async (command: string) => {
    const queue = replies[command];
    if (queue && queue.length > 0) return queue.length > 1 ? queue.shift() : queue[0];
    if (command === 'list_recent_projects' || command === 'take_unrecognised_shot_kinds') return [];
    if (command === 'speleodb_link_status') return new Promise(() => {});
    return ok;
  });
  resetReleaseFailurePromptForTests();
  resetSpeleoExitPromptForTests();
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  resetQuitWaitForTests();
  useSpeleoNetworkStore.getState().reset();
  useSpeleoStore.setState({ link: editing, projectLinkKind: 'remote', remoteProjectId: 'p1', busy: false });
  useProjectStore.setState({
    project: { name: 'Synthetic cave', file_path: '/synthetic/cache/p1.stalx' } as never,
    isDirty: false, isClosing: false,
  } as never);
  useUIStore.setState({ openDialog: vi.fn() as never, setView: vi.fn() as never, activeDialog: null, dialogData: {} });
});

afterEach(() => resetQuitWaitForTests());

describe('Close', () => {
  it('a clean release closes with no question', async () => {
    replies.close_project = [released];
    const result = await useProjectStore.getState().closeProject();
    expect(result.success).toBe(true);
    expect(useReleaseFailurePrompt.getState().request).toBeNull();
    expect(useProjectStore.getState().project).toBeNull();
  });

  it('a failed release keeps the project open, pauses editing and asks; Retry closes once it settles', async () => {
    replies.close_project = [failed(), released];
    const closing = useProjectStore.getState().closeProject();
    const prompt = await waitForPrompt();
    expect(prompt.reason).toBe('close');
    expect(prompt.detail).toContain('HTTP 503');
    expect(useProjectStore.getState().project).not.toBeNull();
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    await expect(prompt.retry()).resolves.toEqual({ ok: true });
    prompt.settle('settled');
    const result = await closing;
    expect(result.success).toBe(true);
    expect(useProjectStore.getState().project).toBeNull();
    expect(calls('close_project').map(([, args]) => args)).toEqual([
      { releaseLock: true, discardLocal: false },
      { releaseLock: true, discardLocal: false },
    ]);
  });

  it('a Retry that fails again stays on the question with the new reason', async () => {
    replies.close_project = [failed(), failed('still unreachable')];
    const closing = useProjectStore.getState().closeProject();
    const prompt = await waitForPrompt();
    await expect(prompt.retry()).resolves.toEqual({ ok: false, detail: 'still unreachable' });
    expect(useProjectStore.getState().project).not.toBeNull();
    prompt.settle('cancel');
    await closing;
  });

  it('Close and keep the lock closes without releasing', async () => {
    replies.close_project = [failed(), ok];
    const closing = useProjectStore.getState().closeProject();
    (await waitForPrompt()).settle('keep');
    expect((await closing).success).toBe(true);
    expect(calls('close_project')[1]?.[1]).toEqual({ releaseLock: false, discardLocal: false });
    expect(useProjectStore.getState().project).toBeNull();
  });

  it('Cancel stays in the project, read-only, and the close reports Cancelled', async () => {
    replies.close_project = [failed()];
    const closing = useProjectStore.getState().closeProject();
    (await waitForPrompt()).settle('cancel');
    const result = await closing;
    expect(result).toMatchObject({ success: false, error: 'Cancelled' });
    expect(calls('close_project')).toHaveLength(1);
    expect(useProjectStore.getState().project).not.toBeNull();
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    expect(useProjectStore.getState().isClosing).toBe(false);
  });

  it('an exit-prompt close the user stays at says the project is still open, not "Cancelled"', async () => {
    replies.close_project = [failed()];
    const outcome = closeAfterUpload();
    (await waitForPrompt()).settle('cancel');
    expect(await outcome).toEqual({
      ok: false,
      error: 'The SpeleoDB lock was not handed back, so the project is still open.',
    });
  });

  it('the Close Project menu shows no error for Cancel, and a proper one for any other failure', async () => {
    const { result } = renderHook(() => useProject());
    const openDialog = useUIStore.getState().openDialog as ReturnType<typeof vi.fn>;
    replies.close_project = [failed()];
    const closing = result.current.handleCloseProject();
    (await waitForPrompt()).settle('cancel');
    await closing;
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.anything());

    replies.close_project = [{ ...ok, success: false, error: 'Synthetic: the database is locked' }];
    await result.current.handleCloseProject();
    expect(openDialog).toHaveBeenCalledWith('error', {
      title: 'Close Project failed',
      message: 'Synthetic: the database is locked',
    });
    expect(useReleaseFailurePrompt.getState().request).toBeNull();
  });
});

const loadResult = () => ({
  loadingId: 1n, isRemote: false,
  project: { name: 'Other', sections: [], station_merges: [], zero_datums: [], loop_closures: [], file_path: '/synthetic/other.stalx' },
  stations: [], surveyData: { shots: [], stations: [] }, stats: null, warnings: [], frozen_shots: [],
  displaySettings: { showStations: true },
});

describe('Project switch', () => {
  it('opening another file settles the lock first: released, then the load goes on', async () => {
    loadBinaryMock.mockResolvedValue(loadResult());
    replies.speleodb_settle_open_lock = [released];
    const outcome = await useProjectStore.getState().loadProject('/synthetic/other.stalx', { accessConfirmed: true });
    expect(outcome.success).toBe(true);
    expect(calls('speleodb_settle_open_lock')[0]?.[1]).toEqual({ releaseLock: true });
    expect(useReleaseFailurePrompt.getState().request).toBeNull();
    expect(loadBinaryMock).toHaveBeenCalledOnce();
  });

  it('a failed release asks before opening; Cancel stays in the project and opens nothing', async () => {
    loadBinaryMock.mockResolvedValue(loadResult());
    replies.speleodb_settle_open_lock = [failed()];
    const opening = useProjectStore.getState().loadProject('/synthetic/other.stalx', { accessConfirmed: true });
    const prompt = await waitForPrompt();
    expect(prompt.reason).toBe('switch');
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    prompt.settle('cancel');
    expect((await opening).success).toBe(false);
    expect(loadBinaryMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().project?.file_path).toBe('/synthetic/cache/p1.stalx');
  });

  it('Close and keep the lock keeps it on purpose, then opens the other project', async () => {
    loadBinaryMock.mockResolvedValue(loadResult());
    replies.speleodb_settle_open_lock = [failed(), ok];
    const opening = useProjectStore.getState().loadProject('/synthetic/other.stalx', { accessConfirmed: true });
    (await waitForPrompt()).settle('keep');
    expect((await opening).success).toBe(true);
    expect(calls('speleodb_settle_open_lock').map(([, args]) => args)).toEqual([{ releaseLock: true }, { releaseLock: false }]);
    expect(loadBinaryMock).toHaveBeenCalledOnce();
  });

  it('New Project asks the same question; Retry that settles goes on to create', async () => {
    replies.speleodb_settle_open_lock = [failed(), released];
    replies.create_project_full = [{ loadingId: 1n, project: { name: 'New', sections: [], file_path: null } } as never];
    const creating = useProjectStore.getState().createProject('New');
    const prompt = await waitForPrompt();
    expect(prompt.reason).toBe('switch');
    expect(calls('create_project_full')).toHaveLength(0);
    await expect(prompt.retry()).resolves.toEqual({ ok: true });
    prompt.settle('settled');
    await creating;
    expect(calls('create_project_full')).toHaveLength(1);
  });

  it('New Project cancelled at the question creates nothing', async () => {
    replies.speleodb_settle_open_lock = [failed()];
    const creating = useProjectStore.getState().createProject('New');
    (await waitForPrompt()).settle('cancel');
    expect(await creating).toMatchObject({ success: false, error: 'Cancelled' });
    expect(calls('create_project_full')).toHaveLength(0);
  });

  it('a local project switches without asking the backend anything', async () => {
    loadBinaryMock.mockResolvedValue(loadResult());
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
    await useProjectStore.getState().loadProject('/synthetic/other.stalx', { accessConfirmed: true });
    expect(calls('speleodb_settle_open_lock')).toHaveLength(0);
  });
});

describe('Quit', () => {
  it('a clean release quits with no question', async () => {
    replies.speleodb_settle_open_lock = [released];
    expect(guardQuit()).toBe(true);
    await vi.waitFor(() => expect(closeMock).toHaveBeenCalledOnce());
    expect(useReleaseFailurePrompt.getState().request).toBeNull();
  });

  it('a failed release asks; Cancel abandons the quit and any update restart riding on it', async () => {
    await requestUpdateRestart();
    closeMock.mockReset();
    replies.speleodb_settle_open_lock = [failed()];
    expect(guardQuit()).toBe(true);
    const prompt = await waitForPrompt();
    expect(prompt.reason).toBe('quit');
    prompt.settle('cancel');
    await flush();
    expect(closeMock).not.toHaveBeenCalled();
    expect(calls('unarm_app_update_restart')).toHaveLength(1);
  });

  it.each(['retry', 'keep'] as const)('quits only once the user settles it (%s)', async (answer) => {
    replies.speleodb_settle_open_lock = [failed(), answer === 'retry' ? released : ok];
    expect(guardQuit()).toBe(true);
    const prompt = await waitForPrompt();
    await flush();
    expect(closeMock).not.toHaveBeenCalled();
    if (answer === 'retry') {
      await prompt.retry();
      prompt.settle('settled');
    } else {
      prompt.settle('keep');
    }
    await vi.waitFor(() => expect(closeMock).toHaveBeenCalledOnce());
    expect(calls('speleodb_settle_open_lock')[1]?.[1]).toEqual({ releaseLock: answer === 'retry' });
  });
});

function Harness() {
  useBeforeUnload(false, 'Import in progress', undefined, guardQuit);
  return <SpeleoReleaseFailedDialog />;
}

describe('Quit after a transfer', () => {
  it('a quit that waited for an upload, then hits a failed release, asks instead of exiting', async () => {
    render(<Harness />);
    await flush();
    const endUpload = useSpeleoNetworkStore.getState().begin('exclusive');
    replies.speleodb_settle_open_lock = [failed(), ok];
    await act(async () => { for (const cb of [...listeners]) await cb(); });
    expect(destroyMock).not.toHaveBeenCalled();
    act(() => endUpload());
    await act(() => new Promise((resolve) => setTimeout(resolve, QUIT_SETTLE_MS + 60)));
    expect(await screen.findByTestId('speleodb-release-failed-dialog')).toBeVisible();
    expect(screen.getByTestId('speleodb-release-failed-body')).toHaveTextContent('Stalagmight closes');
    expect(destroyMock).not.toHaveBeenCalled();
    await userEvent.click(screen.getByTestId('speleodb-release-failed-keep'));
    await vi.waitFor(() => expect(destroyMock).toHaveBeenCalledOnce());
  });
});
