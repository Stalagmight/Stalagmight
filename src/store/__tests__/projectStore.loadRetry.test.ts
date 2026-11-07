/**
 * [#576 FZ-FZ-INT-R2] A load whose file a save renamed over the path while the load
 * read it is refused by the backend with FILE_REPLACED_DURING_LOAD, nothing
 * committed (`core/state/project_load.rs`, the stamp checks). `loadProject` repeats
 * that load once, silently - no error UI, no dialog, no toast - and a second
 * FILE_REPLACED_DURING_LOAD is handled like any other load failure.
 *
 * Store tier: the real store; the load IPC and the rest of the IPC are mocked. All
 * data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const loadBinaryMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));

const PATH = '/caves/CaveA.stalx';
const REPLACED = { code: 'FILE_REPLACED_DURING_LOAD', message: 'The file was replaced while it was being opened.' };

function loadResult() {
  return {
    loadingId: 7,
    isRemote: false,
    newerSchemaVersion: null,
    speleoLinkUnreadable: false,
    unreadableProjectSettings: [],
    loadRepairs: null,
    project: makeProject({ created_at: '2026-04-04T10:00:00Z', file_path: PATH }),
    stations: [],
    surveyData: { shots: [], stations: [] },
    stats: null,
    warnings: [],
    frozen_shots: [],
    displaySettings: {},
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects' || command === 'take_unrecognised_shot_kinds') return [];
    return undefined;
  });
  useUIStore.setState({ activeDialog: null, toasts: [] });
  useProjectStore.setState({
    project: null,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
});

describe('a load refused because a save replaced its file is repeated once (#576 FZ-FZ-INT-R2)', () => {
  it('repeats the load once, silently, and opens the file', async () => {
    loadBinaryMock.mockRejectedValueOnce(REPLACED).mockResolvedValueOnce(loadResult());

    const outcome = await useProjectStore.getState().loadProject(PATH);

    expect(loadBinaryMock).toHaveBeenCalledTimes(2);
    expect(outcome).toEqual({ success: true });
    expect(useProjectStore.getState().project?.file_path).toBe(PATH);
    expect(useProjectStore.getState().loading.error).toBeUndefined();
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(useUIStore.getState().toasts).toEqual([]);
  });

  it('handles a second refusal like any other load failure', async () => {
    loadBinaryMock.mockRejectedValue(REPLACED);

    const outcome = await useProjectStore.getState().loadProject(PATH);

    expect(loadBinaryMock).toHaveBeenCalledTimes(2);
    expect(outcome).toEqual({ success: false });
    expect(useProjectStore.getState().loading.error?.message).toBe(REPLACED.message);
  });

  it('never repeats a load the user cancelled, and opens nothing (#576 RVF-c-A2-1)', async () => {
    // The refusal arrives after the user pressed Cancel on the loading overlay.
    let refuse!: (e: unknown) => void;
    loadBinaryMock
      .mockImplementationOnce(() => new Promise((_resolve, reject) => { refuse = reject; }))
      .mockResolvedValueOnce(loadResult());

    const pending = useProjectStore.getState().loadProject(PATH);
    await vi.waitFor(() => expect(loadBinaryMock).toHaveBeenCalledTimes(1));
    await useProjectStore.getState().cancelLoading();
    expect(useProjectStore.getState().loading.isLoading).toBe(false);
    refuse(REPLACED);
    const outcome = await pending;

    expect(loadBinaryMock, 'a cancelled load is not repeated').toHaveBeenCalledTimes(1);
    expect(outcome.success).toBe(false);
    expect(useProjectStore.getState().project).toBeNull();
  });
});
