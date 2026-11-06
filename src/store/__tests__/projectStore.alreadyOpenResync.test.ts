/**
 * [#576 RVF-a-2] ALREADY_OPEN when the store does not show that file.
 *
 * The backend re-checks "already open" at its load commit, so ALREADY_OPEN can now
 * also answer a load after an earlier load of the same file committed whose answer
 * the store did not apply (it was superseded). The ALREADY_OPEN branch used to take
 * it as "the store already shows it": it flipped the view and reported success with
 * whatever the store had, and the store and the backend stayed on different
 * projects. Now, when the store does not show that file, it reads the backend's
 * state, as a new session.
 *
 * Store tier: the real store; the load and the state read are mocked. All data is
 * synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import { projectSessionEpoch } from '../projectSession';
import { useProjectStore } from '../projectStore';

const { loadBinaryMock, fetchStateMock } = vi.hoisted(() => ({ loadBinaryMock: vi.fn(), fetchStateMock: vi.fn() }));
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({
  loadProjectFullBinary: loadBinaryMock,
  fetchProjectStateBinary: fetchStateMock,
}));

const CAVE_A = '/caves/CaveA.stalx';
const CAVE_A_PROJECT = makeProject({ created_at: '2026-06-01T10:00:00Z', file_path: CAVE_A });

beforeEach(() => {
  vi.clearAllMocks();
  loadBinaryMock.mockRejectedValue({ code: 'ALREADY_OPEN', message: 'already open' });
  fetchStateMock.mockResolvedValue({
    project: CAVE_A_PROJECT,
    stations: [],
    surveyData: { shots: [], stations: [] },
    stats: null,
    frozen_shots: [],
  });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'get_project_warnings' || command === 'list_recent_projects') return [];
    return undefined;
  });
  useProjectStore.setState({
    project: null,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
});

describe('ALREADY_OPEN for a file the store does not show (#576 RVF-a-2)', () => {
  it("reads the backend's state, as a new session, instead of assuming it is shown", async () => {
    const session = projectSessionEpoch();

    const outcome = await useProjectStore.getState().loadProject(CAVE_A);

    expect(outcome).toEqual({ success: true });
    expect(fetchStateMock).toHaveBeenCalledTimes(1);
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_A);
    expect(projectSessionEpoch()).toBeGreaterThan(session);
  });

  it('reads nothing when the store already shows that file', async () => {
    useProjectStore.setState({ project: CAVE_A_PROJECT });
    const session = projectSessionEpoch();

    const outcome = await useProjectStore.getState().loadProject(CAVE_A);

    expect(outcome).toEqual({ success: true });
    expect(fetchStateMock).not.toHaveBeenCalled();
    expect(projectSessionEpoch()).toBe(session);
  });
});
