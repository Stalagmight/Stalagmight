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
import { registerWorkspaceDraft } from '../../utils/workspaceDrafts';
import { useExitConfirm } from '../../utils/exitConfirm';

const invokeIpcMock = vi.hoisted(() => vi.fn());
const loadBinaryMock = vi.hoisted(() => vi.fn());
const confirmMock = vi.hoisted(() => vi.fn().mockResolvedValue(true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: confirmMock }));

vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: (e: unknown) => String(e) }));
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

const displaySettings: DisplaySettings = {
  showStations: true,
  showShots: true,
  showSectionNames: true,
  showStationLabels: false,
  showStationNames: false,
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
    if (command === 'speleodb_start_editing') return { kind: 'locked_by', email: 'editor@example.com' };
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
  ])('opens without an advance warning and attempts the lock once: %o', async (access) => {
    mockLoad(true);
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'speleodb_project_access_at') return { project: { id: 'p', name: 'Restricted', ...access } };
      if (command === 'speleodb_start_editing') return { kind: 'locked_by', email: access.lock_holder };
      return [];
    });
    const first = useProjectStore.getState().loadProject('/mock/loaded.stalx');
    const duplicate = useProjectStore.getState().loadProject('/mock/loaded.stalx');
    expect(await first).toEqual({ success: true });
    await duplicate;
    expect(confirmMock).not.toHaveBeenCalled();
    expect(loadBinaryMock).toHaveBeenCalledOnce();
    expect(invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'speleodb_start_editing')).toHaveLength(1);
  });

  it('does not acquire when intentionally reloading after releasing', async () => {
    mockLoad(true);
    await useProjectStore.getState().loadProject('/mock/loaded.stalx', { accessConfirmed: true, acquireLock: false });
    expect(invokeIpcMock.mock.calls.some(([cmd]) => cmd === 'speleodb_start_editing')).toBe(false);
  });
});

it('settles an already-dispatched load when a newer file preflight fails', async () => {
  mockLoad(false);
  const payload = await loadBinaryMock();
  let finish!: (value: unknown) => void;
  loadBinaryMock.mockReset().mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  invokeIpcMock.mockImplementation(async (command, args) => {
    if (command === 'speleodb_project_access_at' && args.path === '/restricted.stalx') {
      throw new Error('File preflight failed');
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


it('reconciles a dispatched close after a newer file preflight fails', async () => {
  const current = { file_path: '/current.stalx', name: 'Current' };
  useProjectStore.setState({ project: current as never, isClosing: false });
  let finishClose!: (result: { success: boolean }) => void;
  const closeResult = new Promise<{ success: boolean }>((resolve) => { finishClose = resolve; });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'close_project') return closeResult;
    if (command === 'speleodb_project_access_at') throw new Error('File preflight failed');
    return [];
  });
  const closing = useProjectStore.getState().closeProject({ releaseLock: false });
  await vi.waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith(
    'close_project', { releaseLock: false, discardLocal: false }, expect.anything(),
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


it('carries pending New through a dispatched close and a failed newer preflight', async () => {
  useProjectStore.setState({ project: { file_path: '/current.stalx', name: 'Current' } as never, isClosing: false });
  let finishClose!: (result: { success: boolean }) => void;
  const closeResult = new Promise<{ success: boolean }>((resolve) => { finishClose = resolve; });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'close_project') return closeResult;
    if (command === 'speleodb_project_access_at') throw new Error('File preflight failed');
    return [];
  });
  const closing = handBackProject({ pendingNewProject: true }, { releaseLock: false });
  await vi.waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith(
    'close_project', { releaseLock: false, discardLocal: false }, expect.anything(),
  ));
  confirmMock.mockResolvedValue(false);
  await useProjectStore.getState().loadProject('/restricted.stalx');
  finishClose({ success: true });
  await closing;
  expect(useProjectStore.getState().project).toBeNull();
  expect(useUIStore.getState().activeDialog).toBe('new-project');
});

describe('unsent SpeleoDB input on a load (frontend#10)', () => {
  it('asks before another project replaces it, and declining loads nothing', async () => {
    mockLoad(false);
    useProjectStore.setState({ project: { file_path: '/current.stalx', name: 'Current' } as never });
    const unregister = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => true, discard: vi.fn() });
    const pending = useProjectStore.getState().loadProject('/mock/loaded.stalx', { accessConfirmed: true });
    await vi.waitFor(() => expect(useExitConfirm.getState().request).not.toBeNull());
    useExitConfirm.getState().request?.answer(false);
    expect((await pending).success).toBe(false);
    expect(loadBinaryMock).not.toHaveBeenCalled();
    unregister();
  });

  it('reloading the project already open is not leaving it: nothing is asked', async () => {
    mockLoad(false);
    useProjectStore.setState({ project: { file_path: '/mock/loaded.stalx', name: 'Loaded Cave' } as never });
    const unregister = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => true, discard: vi.fn() });
    await useProjectStore.getState().loadProject('/mock/loaded.stalx', { accessConfirmed: true });
    expect(useExitConfirm.getState().request).toBeNull();
    unregister();
  });
});
