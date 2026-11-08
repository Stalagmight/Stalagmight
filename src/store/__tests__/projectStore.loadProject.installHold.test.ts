/**
 * [#576 W86] A project load does not run while an update installs; it runs once a
 * failed install gives the app back.
 *
 * `loadProject` had no install check. The loads that need no click or key press -
 * the reload after a SpeleoDB Update or a start-editing fast-forward answered from
 * the network, an open the user asked for whose project close was still running -
 * opened the project under the install, after the user had passed the exit prompt
 * for the one on screen.
 *
 * Queued, not refused: in those cases the backend has usually moved already (the
 * fast-forward swapped the tree in place, and the reload is how the frontend catches
 * up), so a refused reload left the frontend showing a tree the backend no longer
 * had once a failed install gave the app back. A successful install ends the
 * process, so a queued load never runs; a failed one runs it, and the caller's
 * continuation (the tree reconciliation, the toast) with it.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { useSpeleoStore } from '../speleoStore';
import { useUpdateStore } from '../updateStore';
import { runSerialized } from '../mutationQueue';
import { invokeIpcMock } from '../../test/factories/ipc';
import { deferred } from '../../test/factories/async';

const loadBinaryMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));

const PATH = '/cache/p1.stalx';

/** Let every queued microtask and zero-delay step run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 20; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  vi.clearAllMocks();
  useUpdateStore.getState().reset();
  // The backend's "already open" answer: the reload after a fast-forward lands here.
  loadBinaryMock.mockRejectedValue({ code: 'ALREADY_OPEN', message: 'project already open' });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects') return [];
    return undefined;
  });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', refreshLinkStatus: vi.fn(async () => {}) });
  useUIStore.setState({ appMode: 'survey', view: 'global', activeDialog: null, dialogData: {} });
  useProjectStore.setState({
    project: null,
    isDirty: false,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  });
});

describe('a project load asked for during an update install', () => {
  it('does not run under the install; a failed install runs it, and the caller gets its answer', async () => {
    useUpdateStore.getState().setInstalling(true);
    let settled = false;
    const loading = useProjectStore.getState().loadProject(PATH).finally(() => {
      settled = true;
    });
    await flush();

    expect(loadBinaryMock).not.toHaveBeenCalled();
    expect(useUIStore.getState().view).toBe('global');
    expect(settled).toBe(false);

    useUpdateStore.getState().setInstalling(false);
    const outcome = await loading;

    expect(loadBinaryMock).toHaveBeenCalledTimes(1);
    // The fuzz branch's load names itself (its load token, #576 W44-NF3).
    expect(loadBinaryMock).toHaveBeenCalledWith(PATH, expect.any(String));
    expect(outcome.success).toBe(true);
    expect(useUIStore.getState().view).toBe('single');
  });

  it('a load asked for just before the install, still waiting for the edits queued before it, waits for the install too', async () => {
    const edit = deferred<void>();
    void runSerialized(() => edit.promise);
    const loading = useProjectStore.getState().loadProject(PATH);
    await flush();
    expect(loadBinaryMock).not.toHaveBeenCalled();

    // The restart's close request raises the hold; then the edit lands.
    useUpdateStore.getState().setInstalling(true);
    edit.resolve();
    await flush();
    expect(loadBinaryMock).not.toHaveBeenCalled();

    useUpdateStore.getState().setInstalling(false);
    expect((await loading).success).toBe(true);
    expect(loadBinaryMock).toHaveBeenCalledTimes(1);
  });

  it('a newer load asked for while one waits for the hold wins: only the newer one opens, and no loading window shows meanwhile', async () => {
    // [#576 W86 x FZ-FZ-INT-1] Loads are switches, serialised with the latest winning.
    // The waiting load has started its turn, so it is not dropped, only told it was
    // superseded; it must then end at the hold instead of opening its project first.
    const OLDER = '/cache/older.stalx';
    useUpdateStore.getState().setInstalling(true);
    const older = useProjectStore.getState().loadProject(OLDER);
    await flush();
    const newer = useProjectStore.getState().loadProject(PATH);
    await flush();

    expect(loadBinaryMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().loading.isLoading).toBe(false);

    useUpdateStore.getState().setInstalling(false);
    const [olderOutcome, newerOutcome] = await Promise.all([older, newer]);

    expect(olderOutcome).toMatchObject({ success: false, superseded: true });
    expect(newerOutcome.success).toBe(true);
    expect(loadBinaryMock).toHaveBeenCalledTimes(1);
    expect(loadBinaryMock).toHaveBeenCalledWith(PATH, expect.any(String));
  });

  it('control: with no install running, the load runs at once', async () => {
    const outcome = await useProjectStore.getState().loadProject(PATH);

    expect(loadBinaryMock).toHaveBeenCalledTimes(1);
    expect(outcome.success).toBe(true);
  });
});
