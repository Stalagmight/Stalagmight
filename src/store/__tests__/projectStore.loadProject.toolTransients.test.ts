/**
 * #275 B1 wiring: the real loadProject action must clear armed tool
 * transients (moveCommentSource, merge picks, activeTool) after the new
 * project commits. The uiStore method has its own unit test; this pins the
 * call site inside loadProject so removing it fails a test.
 *
 * Same argument, same call site, for the Export-mode option set: it resets in
 * `applyDisplaySettings`, whose own test calls that method DIRECTLY, so
 * nothing pinned that a load still routes through it. The options are a
 * rendering choice made for the project that was open, and a 600-DPI PNG
 * framed for one survey staying selected while the preview draws the next one
 * is exactly the failure this funnel exists to prevent.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { useExportStore, DEFAULT_IMAGE_OPTIONS } from '../exportStore';
import type { DisplaySettings } from '../../types/loading';

const invokeIpcMock = vi.hoisted(() => vi.fn());
const loadBinaryMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

vi.mock('../../utils/loadProjectBinary', () => ({
  loadProjectFullBinary: loadBinaryMock,
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

describe('projectStore.loadProject tool-transient wiring (#275 B1)', () => {
  beforeEach(() => {
    vi.clearAllMocks();

    invokeIpcMock.mockResolvedValue(undefined);
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'list_recent_projects') return [];
      return undefined;
    });

    loadBinaryMock.mockResolvedValue({
      loadingId: 1n,
      // A local project (#270): the load result carries the backend's is_remote.
      isRemote: false,
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
      // Complete payload (satisfies pins it): applyDisplaySettings merges with
      // ??, so an omitted field would silently keep prior in-memory state and
      // under-exercise the real load path.
      displaySettings: {
        showStations: true,
        showShots: true,
        showSectionNames: true,
        showStationLabels: false,
        showStationNames: false,
        showShotData: false,
        showShotErrors: false,
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
      } satisfies DisplaySettings,
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
      loading: {
        loadingId: 0n,
        clientOpId: 0,
        isLoading: false,
        overallProgress: 0,
      },
    });
  });

  it('clears armed tool transients when a project load commits', async () => {
    useUIStore.setState({
      activeTool: 'merge',
      moveCommentSource: null,
      mergeStationA: { section_id: 4, station_index: 2 },
      mergeStationB: null,
      commentTarget: null,
    });

    await useProjectStore.getState().loadProject('/mock/loaded.stalx');

    const ui = useUIStore.getState();
    expect(useProjectStore.getState().project?.name).toBe('Loaded Cave');
    expect(ui.activeTool).toBe('select');
    expect(ui.mergeStationA).toBeNull();
  });

  it('drops the previous project Export-mode options when a load commits', async () => {
    const before = useExportStore.getState();
    before.setExportType('data');
    before.setDataFormat('tml');
    before.setImageOptions({ ...DEFAULT_IMAGE_OPTIONS, format: 'pdf', dpi: 600 });

    await useProjectStore.getState().loadProject('/mock/loaded.stalx');

    const after = useExportStore.getState();
    expect(useProjectStore.getState().project?.name).toBe('Loaded Cave');
    expect(after.exportType).toBe('image');
    expect(after.dataFormat).toBe('csv');
    expect(after.imageOptions).toEqual(DEFAULT_IMAGE_OPTIONS);
  });
});
