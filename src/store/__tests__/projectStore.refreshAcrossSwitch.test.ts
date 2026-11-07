/**
 * #576 NF-FIX5-1 - a project refresh that answers after the project was replaced.
 *
 * `refreshProjectState` reads `get_project_state_binary` and writes the answer
 * over the store. RV5-8 ordered refreshes against EACH OTHER (an older answer is
 * dropped once a newer one landed), but a load, a create and a close replace the
 * project without a refresh: a refresh issued before one of them (an import's
 * post-step, Repair, orphan placement - callers outside the mutation queue the
 * switch waits for) that answered after it wrote the OLD project back over the one
 * just opened, or over the closed state. The refresh now captures the project
 * session (`store/projectSession.ts`) when it issues and drops an answer whose
 * session moved.
 *
 * Store tier: the real store and the real refresh decoder; the IPC is mocked and
 * the refresh answer is released by hand, so the test decides the order. All data
 * is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { projectStateEnvelope, type ProjectStateLike } from '../../test/projectStateEnvelope';

import { invokeIpcMock } from '../../test/factories/ipc';

const loadBinaryMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/loadProjectBinary')>()),
  loadProjectFullBinary: loadBinaryMock,
}));

function state(name: string, createdAt: string): ProjectStateLike {
  return {
    project: {
      name,
      created_at: createdAt,
      file_path: `/caves/${name}.stalx`,
      sections: [
        {
          id: 1,
          name: 'SectionA',
          color: '#336699',
          shots: [
            { id: 100_000, azimuth_in: 10, azimuth_out: 190, length: 4, depth_in: 0, depth_out: 1, kind: 'Normal' },
          ],
        },
      ],
    },
    stations: [0, 1].map((i) => ({
      station_ref: { section_id: 1, station_index: i },
      coordinates: { x: i * 4, y: 0, z: i },
    })),
    stats: {
      section_count: 1, shot_count: 1, total_length: 4, min_depth: 0, max_depth: 1,
      station_count: 2, loop_count: 0, penetration_distance: 4,
    },
    frozen_shots: [],
  };
}

/** The load of CaveB: a committed project with nothing in it. */
function loadResult(name: string) {
  return {
    loadingId: 1,
    isRemote: false,
    newerSchemaVersion: null,
    speleoLinkUnreadable: false,
    project: {
      name,
      location: null,
      created_at: '2026-02-02T10:00:00Z',
      modified_at: '2026-02-02T10:00:00Z',
      sections: [],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
      file_path: `/caves/${name}.stalx`,
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
    displaySettings: {
      showStations: true, showShots: true, showSectionNames: true, showStationLabels: false,
      showShotData: false, showReferenceShots: true, showJumps: true, showZeroDatum: true,
      showGrid: false, showSatellite: false, showDepthGradient: false, showStationComments: false,
      showShotComments: false, gridSpacing: 10, geometryMode: 'measured',
    },
  };
}

/** The held refresh answers, released when the test chooses. */
const pending: Array<(binary: ArrayBuffer) => void> = [];

async function until(predicate: () => boolean): Promise<void> {
  for (let i = 0; i < 200 && !predicate(); i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  expect(predicate()).toBe(true);
}

beforeEach(() => {
  vi.clearAllMocks();
  pending.length = 0;
  invokeIpcMock.mockImplementation((command: string) => {
    switch (command) {
      case 'get_project_state_binary':
        return new Promise<ArrayBuffer>((resolve) => pending.push(resolve));
      case 'close_project':
        return Promise.resolve({ remote_link: null });
      case 'can_undo':
      case 'can_redo':
        return Promise.resolve(false);
      case 'speleodb_link_status':
        return new Promise(() => {});
      default:
        return Promise.resolve([]);
    }
  });
  loadBinaryMock.mockResolvedValue(loadResult('CaveB'));
  useProjectStore.setState({
    project: null,
    isClosing: false,
    stations: [],
    surveyData: null,
    warnings: [],
    canUndo: false,
    canRedo: false,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
  });
});

describe('a refresh issued before a project switch (#576 NF-FIX5-1)', () => {
  it('does not write the previous project over the one a load just opened', async () => {
    const refresh = useProjectStore.getState().refreshProjectState();
    await until(() => pending.length === 1);

    const loaded = await useProjectStore.getState().loadProject('/caves/CaveB.stalx');
    const afterLoad = useProjectStore.getState().project?.name;
    pending[0]!(projectStateEnvelope(state('CaveA', '2026-01-01T10:00:00Z')));
    await refresh;
    const afterRefresh = useProjectStore.getState().project?.name;

    console.log(
      `[NF-FIX5-1] load: success=${loaded.success}, project after the load: ${afterLoad}; ` +
        `after the older refresh answered: ${afterRefresh} (expected: CaveB)`,
    );
    expect(loaded.success).toBe(true);
    expect(afterLoad).toBe('CaveB');
    expect(afterRefresh).toBe('CaveB');
  });

  it('does not reopen a project the user closed', async () => {
    useProjectStore.setState({ project: state('CaveA', '2026-01-01T10:00:00Z').project as never });
    const refresh = useProjectStore.getState().refreshProjectState();
    await until(() => pending.length === 1);

    const closed = await useProjectStore.getState().closeProject();
    pending[0]!(projectStateEnvelope(state('CaveA', '2026-01-01T10:00:00Z')));
    await refresh;
    const afterRefresh = useProjectStore.getState().project?.name ?? null;

    console.log(
      `[NF-FIX5-1] close: success=${closed.success}; project after the older refresh answered: ` +
        `${afterRefresh} (expected: null)`,
    );
    expect(closed.success).toBe(true);
    expect(afterRefresh).toBeNull();
  });

  it('still applies a refresh that answers within its own session', async () => {
    const refresh = useProjectStore.getState().refreshProjectState();
    await until(() => pending.length === 1);
    pending[0]!(projectStateEnvelope(state('CaveA', '2026-01-01T10:00:00Z')));
    await refresh;
    expect(useProjectStore.getState().project?.name).toBe('CaveA');
  });
});
