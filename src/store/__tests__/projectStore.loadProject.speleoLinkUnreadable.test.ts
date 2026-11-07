/**
 * [#576 B35(b) review RV1-RUST-3] A file whose SpeleoDB link cannot be read opens as
 * a local project, but the backend refuses every save over it (the rebuilt file
 * would lose the link row). The load result says so (`speleoLinkUnreadable`), and
 * the store keeps that verdict for the open project so the banner can explain the
 * refusal before the first save fails. It ends with the project: the next load, a
 * new project, or a Save As that moves the session to another file clears it.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
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

/** Resolves the load with the backend's unreadable-link verdict for this file. */
const mockLoad = (speleoLinkUnreadable: boolean, path: string) =>
  loadBinaryMock.mockResolvedValue({
    loadingId: 1,
    isRemote: false,
    newerSchemaVersion: null,
    speleoLinkUnreadable,
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

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects') return [];
    if (command === 'speleodb_link_status') return new Promise(() => {});
    if (command === 'save_project') return { success: true, saved_path: '/mock/rescue.stalx' };
    return undefined;
  });
  useProjectStore.setState({
    project: null,
    speleoLinkUnreadable: false,
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

describe('loadProject keeps the unreadable-link verdict for the open project (#576 B35(b))', () => {
  it('a file whose link cannot be read is flagged when it opens', async () => {
    mockLoad(true, '/mock/CaveA.stalx');
    await useProjectStore.getState().loadProject('/mock/CaveA.stalx');
    expect(useProjectStore.getState().speleoLinkUnreadable).toBe(true);
  });

  it('the next ordinary project is not', async () => {
    mockLoad(true, '/mock/CaveA.stalx');
    await useProjectStore.getState().loadProject('/mock/CaveA.stalx');
    mockLoad(false, '/mock/CaveB.stalx');
    await useProjectStore.getState().loadProject('/mock/CaveB.stalx');
    expect(useProjectStore.getState().speleoLinkUnreadable).toBe(false);
  });

  it('a Save As that moves the session to another file ends it', async () => {
    mockLoad(true, '/mock/CaveA.stalx');
    await useProjectStore.getState().loadProject('/mock/CaveA.stalx');
    expect(useProjectStore.getState().speleoLinkUnreadable).toBe(true);

    await useProjectStore.getState().saveProject('/mock/rescue.stalx');

    expect(useProjectStore.getState().project?.file_path).toBe('/mock/rescue.stalx');
    expect(useProjectStore.getState().speleoLinkUnreadable).toBe(false);
  });
});
