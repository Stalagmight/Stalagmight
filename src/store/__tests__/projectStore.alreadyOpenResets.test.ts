/**
 * [#576 W71-NF3] An ALREADY_OPEN answer for a file the store does not show re-reads
 * the backend's project as a new session, so it owes that project every per-project
 * reset a load runs - not only the dive plan's (0c5eb0f5b).
 *
 * A load resets, for the project it opens: the display settings it stores (and,
 * through the same `applyDisplaySettings` funnel, the previous project's pre-Plan
 * snapshots, session-only overlays and Export options), the map's fit, the
 * selection, the armed tool state, the dormant distance origin, and the
 * project-store state no file carries over (unsaved changes, integrity verdict,
 * history and load-repair banners, the open database). The ALREADY_OPEN branch's
 * re-read (`refreshProjectState`, then `setView('single')`) reached none of them, so
 * each stayed as the PREVIOUS project left it.
 *
 * Every reset is pinned twice: on a normal load (the control: what the re-read owes)
 * and on the ALREADY_OPEN re-read. A third block pins the guard: ALREADY_OPEN for the
 * project already shown resets nothing.
 *
 * Store tier: the real stores; the load, the state read and the display-settings read
 * are mocked. All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject, stationRef } from '../../test/factories/survey';
import type { DisplaySettings } from '../../types/loading';
import type { Project } from '../../types/project';
import { captureDistanceOrigin, dropDormantDistanceOrigin, remapDistanceOrigin } from '../../utils/distanceOriginAnchor';
import { DEFAULT_IMAGE_OPTIONS, useExportStore } from '../exportStore';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const { loadBinaryMock, fetchStateMock } = vi.hoisted(() => ({ loadBinaryMock: vi.fn(), fetchStateMock: vi.fn() }));
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({
  loadProjectFullBinary: loadBinaryMock,
  fetchProjectStateBinary: fetchStateMock,
}));

const CAVE_A = '/caves/CaveA.stalx';
const CAVE_B = '/caves/CaveB.stalx';
const CAVE_A_PROJECT = makeProject({ name: 'CaveA', created_at: '2026-06-01T10:00:00Z', file_path: CAVE_A });
const CAVE_B_PROJECT = makeProject({ name: 'CaveB', created_at: '2026-07-01T10:00:00Z', file_path: CAVE_B });

const ALREADY_OPEN = { code: 'ALREADY_OPEN', message: 'already open' };

/** CaveB's stored display rows, as `load_display_settings` answers them. */
const CAVE_B_ROWS: Record<string, string> = {
  showStations: 'true',
  showGrid: 'true',
  gridSpacing: '25',
  showStationLabels: 'true',
  showSatellite: 'false',
  showDepthGradient: 'true',
  geometryMode: 'measured',
};

/** The same settings as a load's `display_settings` (`DisplaySettings::from_stored`). */
const CAVE_B_DISPLAY: DisplaySettings = {
  showStations: true,
  showShots: true,
  showSectionNames: true,
  showStationLabels: true,
  showShotData: false,
  showReferenceShots: true,
  showJumps: true,
  showZeroDatum: true,
  showGrid: true,
  showSatellite: false,
  showDepthGradient: true,
  showStationComments: false,
  showShotComments: false,
  gridSpacing: 25,
  geometryMode: 'measured',
};

/** The backend's state, as `get_project_state_binary` decodes it. */
const stateOf = (project: Project) => ({
  project,
  stations: [],
  surveyData: { shots: [], stations: [] },
  stats: null,
  frozen_shots: [],
});

/** A complete `load_project_full` answer for CaveB. */
const caveBLoadResult = () => ({
  ...stateOf(CAVE_B_PROJECT),
  loadingId: 1,
  isRemote: false,
  warnings: [],
  displaySettings: { ...CAVE_B_DISPLAY },
});

/** An anchor the backend handed out for a station (opaque here). */
const ORIGIN_ANCHOR = { shot: 'synthetic-anchor' };

interface Reset {
  /** What the reset is, as the test title names it. */
  name: string;
  /** Put the state CaveA left behind in place. */
  arrange: () => void | Promise<void>;
  /** CaveB starts clean of it. */
  check: () => void | Promise<void>;
}

const RESETS: Reset[] = [
  {
    name: "applies CaveB's stored display settings",
    arrange: () => {
      useUIStore.setState({
        showStations: false,
        showGrid: false,
        gridSpacing: 10,
        showStationLabels: false,
        showSatellite: true,
        showDepthGradient: false,
      });
    },
    check: () => {
      const ui = useUIStore.getState();
      expect({
        showStations: ui.showStations,
        showGrid: ui.showGrid,
        gridSpacing: ui.gridSpacing,
        showStationLabels: ui.showStationLabels,
        showSatellite: ui.showSatellite,
        showDepthGradient: ui.showDepthGradient,
      }).toEqual({
        showStations: true,
        showGrid: true,
        gridSpacing: 25,
        showStationLabels: true,
        showSatellite: false,
        showDepthGradient: true,
      });
    },
  },
  {
    name: "does not restore CaveA's pre-Plan geometry and display onto CaveB",
    arrange: () => {
      // CaveA's survey view, closure-adjusted with stations hidden, then Plan: entering
      // Plan snapshots both, and leaving Plan restores the snapshot.
      useUIStore.setState({ appMode: 'survey', geometryMode: 'closureAdjusted', showStations: false });
      useUIStore.getState().setAppMode('plan');
      expect(useUIStore.getState().appMode).toBe('plan');
    },
    check: () => {
      const ui = useUIStore.getState();
      expect({ appMode: ui.appMode, geometryMode: ui.geometryMode, showStations: ui.showStations }).toEqual({
        appMode: 'survey',
        geometryMode: 'measured',
        showStations: true,
      });
    },
  },
  {
    name: 'turns the session-only debug overlays off',
    arrange: () => {
      useUIStore.setState({ showDevStats: true, showMemoryStats: true, showMapStats: true, showZoomStats: true });
    },
    check: () => {
      const ui = useUIStore.getState();
      expect([ui.showDevStats, ui.showMemoryStats, ui.showMapStats, ui.showZoomStats]).toEqual([
        false,
        false,
        false,
        false,
      ]);
    },
  },
  {
    name: 'resets the Export options',
    arrange: () => {
      useExportStore.setState({
        exportType: 'data',
        dataFormat: 'tml',
        imageOptions: { ...DEFAULT_IMAGE_OPTIONS, format: 'pdf', dpi: 600, show_station_labels: true },
      });
    },
    check: () => {
      const exp = useExportStore.getState();
      expect({ exportType: exp.exportType, dataFormat: exp.dataFormat, imageOptions: exp.imageOptions }).toEqual({
        exportType: 'image',
        dataFormat: 'csv',
        imageOptions: DEFAULT_IMAGE_OPTIONS,
      });
    },
  },
  {
    name: 'fits the map to CaveB',
    arrange: () => {
      useUIStore.setState({ requestFitToBounds: false });
    },
    check: () => {
      expect(useUIStore.getState().requestFitToBounds).toBe('animate');
    },
  },
  {
    name: 'clears the selection',
    arrange: () => {
      useUIStore.getState().setSelection({ type: 'station', stationRef: stationRef(1, 4) });
      useUIStore.getState().addToSelection({ type: 'section', sectionId: 1 as never });
    },
    check: () => {
      expect(useUIStore.getState().selection).toBeNull();
      expect(useUIStore.getState().multiSelection).toEqual([]);
    },
  },
  {
    name: 'disarms the tool state',
    arrange: () => {
      useUIStore.setState({
        activeTool: 'merge',
        mergeStationA: stationRef(1, 2),
        mergeStationB: stationRef(1, 3),
      } as never);
    },
    check: () => {
      const ui = useUIStore.getState();
      expect({ activeTool: ui.activeTool, mergeStationA: ui.mergeStationA, mergeStationB: ui.mergeStationB }).toEqual({
        activeTool: 'select',
        mergeStationA: null,
        mergeStationB: null,
      });
    },
  },
  {
    name: "drops CaveA's dormant distance origin",
    arrange: async () => {
      // A delete displaced CaveA's distance origin: the origin falls back to the
      // entrance and its anchor waits, dormant, for an undo in CaveA.
      const origin = stationRef(1, 5);
      useUIStore.setState({ distanceGradientOrigin: origin, showDistanceGradient: false });
      await remapDistanceOrigin({ anchored: true, ref: origin, anchor: ORIGIN_ANCHOR }, CAVE_A_PROJECT.created_at);
      expect(await captureDistanceOrigin(CAVE_A_PROJECT.created_at)).toEqual({
        anchored: true,
        ref: null,
        anchor: ORIGIN_ANCHOR,
      });
    },
    check: async () => {
      expect(await captureDistanceOrigin(CAVE_A_PROJECT.created_at)).toBeNull();
    },
  },
  {
    name: 'starts with no unsaved changes',
    arrange: () => {
      useProjectStore.setState({ isDirty: true });
    },
    check: () => {
      expect(useProjectStore.getState().isDirty).toBe(false);
    },
  },
  {
    name: "clears CaveA's integrity verdict and its dismissal",
    arrange: () => {
      useProjectStore.setState({
        integrityIssues: ['synthetic issue'],
        integrityDismissed: true,
        integritySaveBlocked: true,
        integrityDialogOpen: true,
      });
    },
    check: () => {
      const p = useProjectStore.getState();
      expect({
        integrityIssues: p.integrityIssues,
        integrityDismissed: p.integrityDismissed,
        integritySaveBlocked: p.integritySaveBlocked,
        integrityDialogOpen: p.integrityDialogOpen,
      }).toEqual({ integrityIssues: [], integrityDismissed: false, integritySaveBlocked: false, integrityDialogOpen: false });
    },
  },
  {
    name: "clears CaveA's history banner",
    arrange: () => {
      useProjectStore.setState({ historyDiscarded: true, historyDiscardedDismissed: true });
    },
    check: () => {
      const p = useProjectStore.getState();
      expect([p.historyDiscarded, p.historyDiscardedDismissed]).toEqual([false, false]);
    },
  },
  {
    name: "clears CaveA's load-repair notice",
    arrange: () => {
      useProjectStore.setState({
        loadRepairs: { shots: 2, sectionOffsets: 0, stationOffsets: 1, gpsPoints: 0, stationComments: 0 },
        loadRepairsDismissed: true,
      });
    },
    check: () => {
      const p = useProjectStore.getState();
      expect([p.loadRepairs, p.loadRepairsDismissed]).toEqual([null, false]);
    },
  },
  {
    name: 'marks the database open',
    arrange: () => {
      // CaveA was a new project whose first save is still deferred: no file yet.
      useProjectStore.setState({ databaseOpen: false });
    },
    check: () => {
      expect(useProjectStore.getState().databaseOpen).toBe(true);
    },
  },
];

beforeEach(() => {
  vi.clearAllMocks();
  dropDormantDistanceOrigin();
  loadBinaryMock.mockRejectedValue(ALREADY_OPEN);
  fetchStateMock.mockResolvedValue(stateOf(CAVE_B_PROJECT));
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'get_project_warnings' || command === 'list_recent_projects') return [];
    if (command === 'load_display_settings') return { ...CAVE_B_ROWS };
    // The displaced origin's station is gone: its anchor names nothing.
    if (command === 'plan_refs_for_anchors') return [null];
    // [#576 W81-1] The re-read takes the unsaved mark from the backend: CaveB's session
    // holds nothing its file does not (and CaveB's file has no flags).
    if (command === 'speleodb_open_file_flags') {
      return { newerSchemaVersion: null, speleoLinkUnreadable: false, unreadableProjectSettings: [], hasUnsavedChanges: false };
    }
    return undefined;
  });
  useProjectStore.setState({
    project: CAVE_A_PROJECT,
    databaseOpen: true,
    isDirty: false,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
  useUIStore.setState({ view: 'single', appMode: 'survey', showToast: vi.fn() } as never);
  useExportStore.getState().reset();
});

describe('a normal load of CaveB, over CaveA (the control)', () => {
  it.each(RESETS)('$name', async ({ arrange, check }) => {
    loadBinaryMock.mockResolvedValue(caveBLoadResult());
    await arrange();

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    expect(outcome).toEqual({ success: true });
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    await check();
  });
});

describe('ALREADY_OPEN for CaveB while the store shows CaveA (#576 W71-NF3)', () => {
  it.each(RESETS)('$name', async ({ arrange, check }) => {
    await arrange();

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    // The re-read ran: CaveB came from the backend's state, and it is on screen.
    expect(outcome).toEqual({ success: true });
    expect(fetchStateMock).toHaveBeenCalledTimes(1);
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    expect(useUIStore.getState().view).toBe('single');
    await check();
  });
});

describe('ALREADY_OPEN for the project the store already shows (the guard)', () => {
  it('re-reads nothing and resets nothing: the live project keeps its state', async () => {
    useUIStore.setState({ showGrid: false, gridSpacing: 10, requestFitToBounds: false });
    useUIStore.getState().setSelection({ type: 'station', stationRef: stationRef(1, 4) });
    useExportStore.setState({ dataFormat: 'tml' });
    useProjectStore.setState({ isDirty: true, integrityDismissed: true });

    const outcome = await useProjectStore.getState().loadProject(CAVE_A);

    expect(outcome).toEqual({ success: true });
    expect(fetchStateMock).not.toHaveBeenCalled();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('load_display_settings', expect.anything(), expect.anything());
    const ui = useUIStore.getState();
    expect({ showGrid: ui.showGrid, gridSpacing: ui.gridSpacing, fit: ui.requestFitToBounds }).toEqual({
      showGrid: false,
      gridSpacing: 10,
      fit: false,
    });
    expect(ui.selection).toEqual({ type: 'station', stationRef: stationRef(1, 4) });
    expect(useExportStore.getState().dataFormat).toBe('tml');
    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(useProjectStore.getState().integrityDismissed).toBe(true);
  });
});
