/**
 * [#576 RVF-a-2 follow-up, W70-NF1] An ALREADY_OPEN answer for a file the store does
 * not show starts a new session, so it owes the dive plan the reset a load gives it.
 *
 * A load or a create resets the gas store in one funnel: `uiStore.applyDisplaySettings`
 * calls `useGasPlanStore.getState().reset()`. The ALREADY_OPEN branch of `loadProject`
 * never reaches that funnel. When the store shows another project it re-reads the
 * backend's (`refreshProjectState`) and then calls `setView('single')`, whose
 * `exitGasPlan()` -> `setAppMode('survey')` leaves the plan alone on purpose (a plan
 * survives a mode switch, #244). So the previous project's origin, endpoint, way
 * points, marked points, pinned drops, loaded plan and staged-reserve acknowledgement
 * stayed in the store, now pointing at stations of the project the backend has open.
 *
 * Store tier: the real stores; the load and the state read are mocked (the setup of
 * `projectStore.alreadyOpenResync.test.ts`). All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject, stationRef } from '../../test/factories/survey';
import type { DisplaySettings } from '../../types/loading';
import type { Project } from '../../types/project';
import { divePlanSnapshotOf } from '../../utils/divePlan';
import { useGasPlanStore } from '../gasPlanStore';
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

/** The backend's state, as `get_project_state_binary` decodes it. */
const stateOf = (project: Project) => ({
  project,
  stations: [],
  surveyData: { shots: [], stations: [] },
  stats: null,
  frozen_shots: [],
});

/** A complete `load_project_full` answer for `project`. */
const loadResultOf = (project: Project) => ({
  ...stateOf(project),
  loadingId: 1,
  isRemote: false,
  warnings: [],
  displaySettings: {
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
  } satisfies DisplaySettings,
});

/** The dive-plan state a project switch must not carry over. */
const planOf = () => {
  const s = useGasPlanStore.getState();
  return {
    originChosen: s.originChosen,
    origin: s.origin,
    endpoint: s.endpoint,
    waypoints: s.waypoints,
    markedPoints: s.markedPoints.map((m) => m.station),
    stageDropOverrides: s.stageDropOverrides,
    loadedPlanId: s.loadedPlanId,
    pushyAck: s.pushyAck,
  };
};

const NO_PLAN = {
  originChosen: false,
  origin: null,
  endpoint: null,
  waypoints: [],
  markedPoints: [],
  stageDropOverrides: [],
  loadedPlanId: null,
  pushyAck: false,
};

/** CaveA's plan: an origin, an endpoint, a way point, a marked point, a pinned drop, saved. */
function planCaveA(): void {
  const gas = useGasPlanStore.getState();
  gas.reset();
  gas.setOrigin(stationRef(1, 3));
  gas.setEndpoint(stationRef(1, 9));
  gas.addWaypoint(stationRef(1, 5));
  gas.addMarkedPoint(stationRef(1, 6));
  gas.setStageDropOverride(0, stationRef(1, 4));
  gas.setPushyAck(true);
  useGasPlanStore.getState().markPlanSaved(7, divePlanSnapshotOf(useGasPlanStore.getState()));
}

const CAVE_A_PLAN = {
  originChosen: true,
  origin: stationRef(1, 3),
  endpoint: stationRef(1, 9),
  waypoints: [stationRef(1, 5)],
  markedPoints: [stationRef(1, 6)],
  stageDropOverrides: [stationRef(1, 4)],
  loadedPlanId: 7,
  pushyAck: true,
};

beforeEach(() => {
  vi.clearAllMocks();
  loadBinaryMock.mockRejectedValue(ALREADY_OPEN);
  fetchStateMock.mockResolvedValue(stateOf(CAVE_B_PROJECT));
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'get_project_warnings' || command === 'list_recent_projects') return [];
    return undefined;
  });
  useProjectStore.setState({
    project: CAVE_A_PROJECT,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
  // The diver is planning CaveA in Plan mode: `setView('single')` then leaves Plan,
  // which is the step that keeps the plan by design.
  useUIStore.setState({ view: 'single', appMode: 'plan' });
  planCaveA();
});

describe('ALREADY_OPEN for a file the store does not show resets the dive plan', () => {
  it('a normal load of another project resets the plan (the control)', async () => {
    loadBinaryMock.mockResolvedValue(loadResultOf(CAVE_B_PROJECT));
    expect(planOf()).toEqual(CAVE_A_PLAN);

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    expect(outcome).toEqual({ success: true });
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    expect(planOf()).toEqual(NO_PLAN);
  });

  it.each([
    ['the store shows CaveA', CAVE_A_PROJECT],
    ['the store shows no project', null],
  ])('resets the plan when the backend has CaveB open and %s', async (_shown, shown) => {
    useProjectStore.setState({ project: shown });
    expect(planOf()).toEqual(CAVE_A_PLAN);

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    // The mismatch path ran: CaveB was read from the backend, and the view left Plan.
    expect(outcome).toEqual({ success: true });
    expect(fetchStateMock).toHaveBeenCalledTimes(1);
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(planOf()).toEqual(NO_PLAN);
  });

  it('keeps the plan when the store already shows the file the backend has open', async () => {
    const outcome = await useProjectStore.getState().loadProject(CAVE_A);

    expect(outcome).toEqual({ success: true });
    expect(fetchStateMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_A);
    expect(planOf()).toEqual(CAVE_A_PLAN);
  });
});
