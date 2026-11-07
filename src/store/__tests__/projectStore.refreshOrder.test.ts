/**
 * #576 RV5-8 - two project refreshes that answer out of order.
 *
 * The commands run on the backend's blocking pool, so two `refreshProjectState`
 * calls (one from an edit inside the mutation queue, one from a caller outside it:
 * a save with integrity heals, Repair, the tree swap, orphan placement) can
 * encode concurrently and answer in either order. The store used to take whatever
 * landed last, so an OLDER envelope could overwrite a newer one until the next
 * refresh. Each refresh now takes a sequence number, and an answer older than one
 * already applied is dropped.
 *
 * Store tier: the real store and decoder; the IPC is mocked and each answer is
 * released by hand, so the test decides the order. All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { projectStateEnvelope, type ProjectStateLike } from '../../test/projectStateEnvelope';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

function state(name: string): ProjectStateLike {
  return {
    project: {
      name,
      file_path: '/caves/CaveA.stalx',
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

/** A binary answer the test releases when it chooses. */
interface Pending {
  release: (binary: ArrayBuffer) => void;
}

function serveHeld(): Pending[] {
  const pending: Pending[] = [];
  invokeIpcMock.mockImplementation((command: string) => {
    switch (command) {
      case 'get_project_state_binary':
        return new Promise<ArrayBuffer>((resolve) => {
          pending.push({ release: resolve });
        });
      case 'can_undo':
      case 'can_redo':
        return Promise.resolve(false);
      default:
        return Promise.resolve([]);
    }
  });
  return pending;
}

/** Let the dynamic import and the awaited IPC reach the held promise. */
async function until(predicate: () => boolean): Promise<void> {
  for (let i = 0; i < 200 && !predicate(); i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  expect(predicate()).toBe(true);
}

beforeEach(() => {
  vi.clearAllMocks();
  useProjectStore.setState({
    project: null,
    stations: [],
    surveyData: null,
    warnings: [],
    canUndo: false,
    canRedo: false,
  });
});

describe('refreshProjectState answer order (#576 RV5-8)', () => {
  it('drops an older answer that lands after a newer one was applied', async () => {
    const pending = serveHeld();

    // Two refreshes in flight: OLDER is issued first, NEWER second.
    const older = useProjectStore.getState().refreshProjectState();
    await until(() => pending.length === 1);
    const newer = useProjectStore.getState().refreshProjectState();
    await until(() => pending.length === 2);

    // The pool answers the NEWER one first, then the OLDER one.
    pending[1]!.release(projectStateEnvelope(state('Newer state')));
    await newer;
    const afterNewer = useProjectStore.getState().project?.name;
    pending[0]!.release(projectStateEnvelope(state('Older state')));
    await older;
    const afterBoth = useProjectStore.getState().project?.name;

    console.log(
      `[RV5-8] project after the newer answer: ${afterNewer}; after the older answer ` +
        `landed last: ${afterBoth} (expected: Newer state)`,
    );
    expect(afterNewer).toBe('Newer state');
    expect(afterBoth).toBe('Newer state');
  });

  it('still applies answers that land in issue order', async () => {
    const pending = serveHeld();

    const first = useProjectStore.getState().refreshProjectState();
    await until(() => pending.length === 1);
    const second = useProjectStore.getState().refreshProjectState();
    await until(() => pending.length === 2);

    pending[0]!.release(projectStateEnvelope(state('First state')));
    await first;
    expect(useProjectStore.getState().project?.name).toBe('First state');
    pending[1]!.release(projectStateEnvelope(state('Second state')));
    await second;
    expect(useProjectStore.getState().project?.name).toBe('Second state');
  });
});
