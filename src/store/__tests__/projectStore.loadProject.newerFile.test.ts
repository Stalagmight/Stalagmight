/**
 * [#576 B6-D1] `loadProject` settles the newer-version read-only gate from the load
 * result, in the same `set()` as the project - so a project a newer Stalagmight saved
 * is read-only on the frame it first appears, and the next ordinary project is
 * editable on ITS first frame. Sampled at the instant of the view switch, like the
 * #270 SpeleoDB pin (`projectStore.loadProject.speleoGate.test.ts`).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { useSpeleoStore } from '../speleoStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import type { DisplaySettings } from '../../types/loading';
import { invokeIpcMock } from '../../test/factories/ipc';

const loadBinaryMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));

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

/** Resolves the load with the backend's newer-version verdict for this file. */
const mockLoad = (newerSchemaVersion: number | null, path: string) =>
  loadBinaryMock.mockResolvedValue({
    loadingId: 1,
    isRemote: false,
    newerSchemaVersion,
    project: {
      name: 'CaveA',
      location: null,
      created_at: new Date().toISOString(),
      modified_at: new Date().toISOString(),
      sections: [],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
      file_path: path,
      next_section_id: 1,
      section_offsets: {},
      imported_file_hashes: [],
      imported_section_hashes: [],
      next_closure_id: 1,
    },
    stations: [],
    surveyData: { shots: [], stations: [] },
    stats: null,
    warnings: [],
    frozen_shots: [],
    displaySettings,
  });

/** The edit gate sampled at the moment the project view is switched in. */
let blockedAtViewSwitch: boolean | null;

beforeEach(() => {
  vi.clearAllMocks();
  blockedAtViewSwitch = null;

  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects') return [];
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
      if (view === 'single') blockedAtViewSwitch = isEditingBlockedNow();
      realSetView(view);
    },
  });

  useProjectStore.setState({
    project: null,
    newerSchemaVersion: null,
    stations: [],
    stats: null,
    frozenShots: [],
    warnings: [],
    recentProjects: [],
    autosaveFailure: null,
    isDirty: false,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
  });
});

describe('loadProject settles the newer-version gate with the project (#576 B6-D1)', () => {
  it('a file a newer build saved is read-only when the project view appears', async () => {
    mockLoad(19, '/mock/newer.stalx');

    await useProjectStore.getState().loadProject('/mock/newer.stalx');

    expect(useProjectStore.getState().newerSchemaVersion).toBe(19);
    expect(blockedAtViewSwitch).toBe(true);
    expect(isEditingBlockedNow()).toBe(true);
  });

  it('the next ordinary project is editable from its first frame', async () => {
    mockLoad(19, '/mock/newer.stalx');
    await useProjectStore.getState().loadProject('/mock/newer.stalx');
    expect(isEditingBlockedNow()).toBe(true);

    mockLoad(null, '/mock/ordinary.stalx');
    await useProjectStore.getState().loadProject('/mock/ordinary.stalx');

    expect(useProjectStore.getState().newerSchemaVersion).toBeNull();
    expect(blockedAtViewSwitch).toBe(false);
    expect(isEditingBlockedNow()).toBe(false);
  });
});
