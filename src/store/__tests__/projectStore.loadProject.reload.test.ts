/**
 * Pre-PR delta review finding 2: the save prompt's "Use the file's version"
 * is `loadProject(path, { reload: true })`, ONE backend operation
 * (`reload_project_full_binary`): the open file is read and decoded, and
 * replaces the session only when it opened. The frontend never closes the
 * project first, so a refusal keeps the project and its unsaved changes, and
 * hands the reason back to the prompt instead of the load's own surfaces (the
 * recovery dialog, the loading panel). Injection: loading the open path with
 * `load_project_full_binary` is refused as already open and replaces
 * nothing; routing the refusal through the load's error handling opens the
 * recovery dialog over the prompt.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());
const loadBinaryMock = vi.hoisted(() => vi.fn());
const reloadBinaryMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: (e: unknown) => String(e) }));
vi.mock('../../utils/loadProjectBinary', () => ({
  loadProjectFullBinary: loadBinaryMock,
  reloadProjectFullBinary: reloadBinaryMock,
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

const PATH = '/caves/Main Sump.stalx';

/** The open session, with an unsaved edit the prompt is deciding about. */
const OURS = {
  name: 'Ours',
  file_path: PATH,
  created_at: '2026-01-01T00:00:00Z',
  modified_at: '2026-01-01T00:00:00Z',
  lines: [],
};

/** The file's version, as the backend's reload hands it over. */
const theirs = () => ({
  loadingId: 1n,
  isRemote: false,
  project: {
    name: 'Theirs',
    location: null,
    created_at: '2026-01-01T00:00:00Z',
    modified_at: '2026-02-02T00:00:00Z',
    lines: [],
    station_merges: [],
    zero_datums: [],
    loop_closures: [],
    file_path: PATH,
    next_line_id: 1,
    imported_file_hashes: [],
    imported_line_hashes: [],
    next_closure_id: 1n,
  },
  stations: [],
  surveyData: { shots: [], stations: [] },
  stats: null,
  warnings: [],
  frozen_shots: [],
  displaySettings: {},
});

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects' || command === 'take_unrecognised_shot_kinds') return [];
    return undefined;
  });
  useUIStore.setState({ activeDialog: 'save-conflict', dialogData: {}, view: 'single' });
  useProjectStore.setState({
    project: OURS as never,
    isDirty: true,
    loading: { loadingId: 0n, clientOpId: 0, isLoading: false, overallProgress: 0 },
  });
});

describe('loadProject with reload: the file read again in place of the session', () => {
  it('a reload the backend refuses keeps the project and its unsaved changes, and says why', async () => {
    reloadBinaryMock.mockRejectedValue({ code: 'CORRUPT', message: 'This is not a Stalagmight project file.', details: PATH });

    const outcome = await useProjectStore.getState().loadProject(PATH, { reload: true });

    expect(outcome).toEqual({ success: false, error: 'This is not a Stalagmight project file.' });
    expect(reloadBinaryMock).toHaveBeenCalledOnce();
    expect(loadBinaryMock).not.toHaveBeenCalled();
    const store = useProjectStore.getState();
    expect(store.project).toBe(OURS);
    expect(store.isDirty).toBe(true);
    expect(store.loading.isLoading).toBe(false);
    // The prompt keeps the screen: no recovery dialog, no loading panel error.
    expect(useUIStore.getState().activeDialog).toBe('save-conflict');
    expect(store.loading.error).toBeUndefined();
  });

  it('a reload that opened replaces the session with the file\'s version', async () => {
    reloadBinaryMock.mockResolvedValue(theirs());

    const outcome = await useProjectStore.getState().loadProject(PATH, { reload: true });

    expect(outcome).toEqual({ success: true });
    expect(loadBinaryMock).not.toHaveBeenCalled();
    const store = useProjectStore.getState();
    expect(store.project?.name).toBe('Theirs');
    expect(store.isDirty).toBe(false);
  });
});
