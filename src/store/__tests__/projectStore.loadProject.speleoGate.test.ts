/**
 * #270 render-order pin: `loadProject` must settle the SpeleoDB read-only gate
 * BEFORE `setView('single')` puts the project on screen.
 *
 * The original bug was ordering, not just defaulting: the load fired
 * `refreshLinkStatus()` unawaited AFTER the view switch, so a remote project
 * rendered with `link === null` for as long as that round trip took (and it is
 * not a cheap local read - for a remote project `speleodb_link_status` also makes
 * best-effort `get_project` / `get_revisions` HTTP calls). These tests sample the
 * gate at the exact instant of the view switch, so re-introducing the gap fails
 * here rather than only in the app.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { handBackProject } from '../../utils/quit';
import { useUIStore } from '../uiStore';
import { useSpeleoStore, selectSpeleoReadOnly } from '../speleoStore';
import type { DisplaySettings } from '../../types/loading';

const invokeIpcMock = vi.hoisted(() => vi.fn());
const loadBinaryMock = vi.hoisted(() => vi.fn());
const confirmMock = vi.hoisted(() => vi.fn().mockResolvedValue(true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: confirmMock }));

vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

const displaySettings: DisplaySettings = {
  showStations: true,
  showShots: true,
  showSectionNames: true,
  showStationLabels: false,
  showShotData: false,
  showReferenceShots: true,
  showJumps: true,
  showZeroDatum: true,
  showGrid: false,
  showSatellite: false,
  showDepthGradient: false,
  showStationComments: false,
  showShotComments: false,
  gridSpacing: 10,
  geometryMode: 'measured',
};

/** Resolves the load with the backend's `is_remote` verdict for this project. */
const mockLoad = (isRemote: boolean) =>
  loadBinaryMock.mockResolvedValue({
    loadingId: 1n,
    isRemote,
    project: {
      name: 'Loaded Cave',
      location: null,
      created_at: new Date().toISOString(),
      modified_at: new Date().toISOString(),
      sections: [],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
      file_path: '/mock/loaded.stalx',
      next_section_id: 1,
      section_offsets: {},
      imported_file_hashes: [],
      imported_section_hashes: [],
      next_closure_id: 1n,
    },
    stations: [],
    surveyData: { shots: [], stations: [] },
    stats: null,
    warnings: [],
    frozen_shots: [],
    displaySettings,
  });

/** The read-only verdict sampled at the moment the project view is switched in. */
let readOnlyAtViewSwitch: boolean | null;
/** Whether the SpeleoDB shell had a link by then (only the synchronous
 *  `syncProjectLink` can have seeded one this early). */
let hadLinkAtViewSwitch: boolean | null;

beforeEach(() => {
  vi.clearAllMocks();
  confirmMock.mockResolvedValue(true);
  readOnlyAtViewSwitch = null;
  hadLinkAtViewSwitch = null;

  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects') return [];
    // Never answers: stands in for the slow/hanging status probe the gate must
    // no longer depend on. (`refreshLinkStatus` swallows the rejection.)
    if (command === 'speleodb_link_status') return new Promise(() => {});
    return undefined;
  });

  useSpeleoStore.setState({
    link: null,
    projectLinkKind: 'unknown',
    remoteProjectId: null,
    divergence: null,
    banner: null,
    session: null,
  });

  const realSetView = useUIStore.getState().setView;
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    setView: (view) => {
      if (view === 'single') {
        readOnlyAtViewSwitch = selectSpeleoReadOnly(useSpeleoStore.getState());
        hadLinkAtViewSwitch = useSpeleoStore.getState().link !== null;
      }
      realSetView(view);
    },
  });

  useProjectStore.setState({
    project: null,
    stations: [],
    stats: null,
    frozenShots: [],
    warnings: [],
    recentProjects: [],
    autosaveFailure: null,
    isDirty: false,
    loading: { loadingId: 0n, clientOpId: 0, isLoading: false, overallProgress: 0 },
  });
});

describe('loadProject resolves the SpeleoDB gate before the view renders (#270)', () => {
  it('a remote project is already read-only when the project view appears', async () => {
    mockLoad(true);

    await useProjectStore.getState().loadProject('/mock/loaded.stalx');

    expect(readOnlyAtViewSwitch).toBe(true);
    // Proves the gate was RESOLVED at that point rather than merely unresolved
    // (the pessimistic default would also read `true`): only the synchronous
    // open-time resolution can have seeded the shell this early.
    expect(hadLinkAtViewSwitch).toBe(true);
    // ... and stays read-only afterwards: the status probe never answered.
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(true);
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
  });

  it('a local project is already editable when the project view appears (no read-only flash)', async () => {
    mockLoad(false);

    await useProjectStore.getState().loadProject('/mock/loaded.stalx');

    expect(readOnlyAtViewSwitch).toBe(false);
    expect(hadLinkAtViewSwitch).toBe(false);
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);
    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
  });
});


describe('restricted native file preflight', () => {
  it.each([
    { permission: 'READ_ONLY', lock_holder: null },
    { permission: 'READ_AND_WRITE', lock_holder: 'other@example.com' },
  ])('warns on a cold direct open and preserves the current project on cancel: %o', async (access) => {
    const current = { file_path: '/current.stalx', name: 'Current' };
    useProjectStore.setState({ project: current as never });
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'speleodb_project_access_at') return { project: { id: 'p', name: 'Restricted', ...access }, endpoint: 'https://example.com', verification: 'verified' };
      return [];
    });
    confirmMock.mockResolvedValue(false);
    const first = useProjectStore.getState().loadProject('/cold.stalx');
    const duplicate = useProjectStore.getState().loadProject('/cold.stalx');
    expect(await first).toEqual({ success: false });
    await duplicate;
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(loadBinaryMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().project).toBe(current);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_project_access_at', { path: '/cold.stalx' });
    expect(invokeIpcMock).not.toHaveBeenCalledWith('cancel_loading', undefined, expect.anything());
  });

  it('does not repeat a remote-browser access confirmation', async () => {
    mockLoad(true);
    await useProjectStore.getState().loadProject('/confirmed.stalx', { accessConfirmed: true });
    expect(invokeIpcMock.mock.calls.some(([command]) => command === 'speleodb_project_access_at')).toBe(false);
    expect(confirmMock).not.toHaveBeenCalled();
  });
});

it('settles an already-dispatched load when a newer restricted-file warning is cancelled', async () => {
  mockLoad(false);
  const payload = await loadBinaryMock();
  let finish!: (value: unknown) => void;
  loadBinaryMock.mockReset().mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  invokeIpcMock.mockImplementation(async (command, args) => {
    if (command === 'speleodb_project_access_at' && args.path === '/restricted.stalx') {
      return { project: { name: 'Restricted B', permission: 'READ_ONLY', lock_holder: null }, verification: 'verified', endpoint: 'https://example.com' };
    }
    if (command === 'list_recent_projects' || command === 'take_unrecognised_shot_kinds') return [];
    return undefined;
  });
  const opening = useProjectStore.getState().loadProject('/mock/loaded.stalx');
  await vi.waitFor(() => expect(loadBinaryMock).toHaveBeenCalledOnce());
  expect(useProjectStore.getState().loading.isLoading).toBe(true);
  confirmMock.mockResolvedValue(false);
  expect(await useProjectStore.getState().loadProject('/restricted.stalx')).toEqual({ success: false });
  finish(payload);
  expect(await opening).toEqual({ success: true });
  expect(useProjectStore.getState().loading.isLoading).toBe(false);
  expect(useProjectStore.getState().project?.file_path).toBe('/mock/loaded.stalx');
  expect(loadBinaryMock).toHaveBeenCalledExactlyOnceWith('/mock/loaded.stalx');
});


it('reconciles a dispatched close after a newer file warning is cancelled', async () => {
  const current = { file_path: '/current.stalx', name: 'Current' };
  useProjectStore.setState({ project: current as never, isClosing: false });
  let finishClose!: (result: { success: boolean }) => void;
  const closeResult = new Promise<{ success: boolean }>((resolve) => { finishClose = resolve; });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'close_project') return closeResult;
    if (command === 'speleodb_project_access_at') return {
      project: { id: 'p', name: 'Restricted', permission: 'READ_ONLY', lock_holder: null },
      endpoint: 'https://example.com', verification: 'verified',
    };
    return [];
  });
  const closing = useProjectStore.getState().closeProject({ releaseLock: false });
  await vi.waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith(
    'close_project', { releaseLock: false }, expect.anything(),
  ));
  confirmMock.mockResolvedValue(false);
  expect(await useProjectStore.getState().loadProject('/restricted.stalx')).toEqual({ success: false });
  expect(useProjectStore.getState().project).toBe(current);
  finishClose({ success: true });
  await closing;
  expect(useProjectStore.getState().project).toBeNull();
  expect(useProjectStore.getState().isClosing).toBe(false);
  expect(loadBinaryMock).not.toHaveBeenCalled();
});


it('carries pending New through a dispatched close and a cancelled newer warning', async () => {
  useProjectStore.setState({ project: { file_path: '/current.stalx', name: 'Current' } as never, isClosing: false });
  let finishClose!: (result: { success: boolean }) => void;
  const closeResult = new Promise<{ success: boolean }>((resolve) => { finishClose = resolve; });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'close_project') return closeResult;
    if (command === 'speleodb_project_access_at') return {
      project: { id: 'p', name: 'Restricted', permission: 'READ_ONLY', lock_holder: null },
      endpoint: 'https://example.com', verification: 'verified',
    };
    return [];
  });
  const closing = handBackProject({ pendingNewProject: true }, { releaseLock: false });
  await vi.waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith(
    'close_project', { releaseLock: false }, expect.anything(),
  ));
  confirmMock.mockResolvedValue(false);
  await useProjectStore.getState().loadProject('/restricted.stalx');
  finishClose({ success: true });
  await closing;
  expect(useProjectStore.getState().project).toBeNull();
  expect(useUIStore.getState().activeDialog).toBe('new-project');
});
