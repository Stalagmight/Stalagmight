/**
 * #576 D39(g): the distance-gradient origin follows its physical station across
 * survey edits.
 *
 * The origin is a positional `StationRef`, and until now only a section reverse
 * re-pointed it (a mirror in `applyReverseAftermath`). An insert or a delete
 * before it renumbers the section just as silently: the ramp, the legend, the
 * origin ring and the DetailPanel badge quietly re-rooted on the neighbouring
 * station. Decided ("Follow the station"): the mutation seams that already carry
 * the gas-plan pins across an edit with backend anchors (#310) carry the origin
 * too - an anchor taken before the edit, re-resolved after it. A station that no
 * longer exists falls back to the entrance with a notice; the reverse-only mirror
 * is gone, since an anchor already survives a reverse (applying both would move
 * the origin twice).
 *
 * Store tier: the real projectStore seams with the IPC mocked; the anchor IPC is
 * scripted here, so what is pinned is the seam's use of it.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useGasPlanStore } from '../gasPlanStore';
import { useUIStore } from '../uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { applySpeleoTreeReplacement } from '../../utils/speleoTreeReplacement';
import { projectStateEnvelope } from '../../test/projectStateEnvelope';
import type { StationRef } from '../../types/project';
import { invokeIpcMock } from '../../test/factories/ipc';
import { stationRef } from '../../test/factories/survey';

vi.mock('../../utils/ipc');
const loadBinaryMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));

const SECTION = 1;
const ref = (station_index: number): StationRef => stationRef(SECTION, station_index);

function project() {
  return {
    name: 'Origin',
    location: null,
    created_at: '2026-01-01T00:00:00Z',
    modified_at: '2026-01-01T00:00:01Z',
    sections: [
      {
        id: SECTION,
        name: 'CaveA',
        surveyor: null,
        date: null,
        color: '#fff',
        direction: 'in',
        quality: 1,
        shots: [0, 1, 2, 3, 4, 5, 6].map((id) => ({ id, azimuth_in: 0, azimuth_out: 0 })),
        next_shot_id: 7,
      },
    ],
    station_merges: [],
    zero_datums: [],
    loop_closures: [],
    file_path: '/tmp/origin.stalx',
    next_section_id: 2,
    section_offsets: {},
    imported_file_hashes: [],
    imported_section_hashes: [],
  };
}

/**
 * Serve the refresh IPCs and a successful mutation; `resolveTo` is what the anchor of the
 * origin re-resolves to after the edit (null: its station is gone). Every anchor call is
 * recorded, with the ref it was asked about.
 */
function serve(resolveTo: StationRef | null, reversed?: { section_id: number; shot_count: number }[]) {
  const anchorCalls: string[] = [];
  invokeIpcMock.mockImplementation(async (command: string, args?: { refs?: StationRef[] }) => {
    switch (command) {
      case 'plan_anchors_for_refs':
        anchorCalls.push(command);
        return (args?.refs ?? []).map((r) => ({ anchorOf: r }));
      case 'plan_refs_for_anchors':
        anchorCalls.push(command);
        return [resolveTo];
      case 'get_project_state_binary':
        return projectStateEnvelope({ project: project(), stations: [], stats: null, frozen_shots: [] });
      case 'can_undo':
      case 'can_redo':
        return false;
      case 'get_project_warnings':
        return [];
      case 'check_project_integrity':
        return { messages: [], save_blocked: false };
      default:
        // Every mutation's outcome at once (#576 A12): a reverse's log rides
        // `reversed_sections`, an undo/redo's `reversed_steps`.
        return {
          warnings: [],
          remote_link: null,
          section_ids: [],
          ...(reversed ? { reversed_sections: reversed, reversed_steps: reversed } : {}),
        };
    }
  });
  return anchorCalls;
}

const SHOT = {
  azimuth_in: 0, azimuth_out: 0, length: 5, depth_in: 0, depth_out: 0, pitch_in: null, pitch_out: null,
  left: null, right: null, up: null, down: null, comment: null, kind: 'Normal' as const,
  marker_index: null, temperature: null,
};

beforeEach(() => {
  vi.clearAllMocks();
  useGasPlanStore.getState().reset();
  useUIStore.setState({ distanceGradientOrigin: null, toasts: [] });
  useProjectStore.setState({ project: project() as never });
});

describe('the distance-gradient origin follows its station (D39(g))', () => {
  it('moves with its station when a shot is inserted before it', async () => {
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    serve(ref(6));
    await useProjectStore.getState().createShot(SECTION, SHOT, undefined, 0);
    expect(useUIStore.getState().distanceGradientOrigin, 'origin after an insert before it').toEqual(ref(6));
  });

  it('follows it back on undo', async () => {
    useUIStore.getState().setDistanceGradientOrigin(ref(6));
    serve(ref(5));
    await useProjectStore.getState().undo();
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(5));
  });

  it('falls back to the entrance with a notice when its station is deleted', async () => {
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    serve(null);
    await useProjectStore.getState().deleteShot(SECTION, 4);
    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
    const toasts = useUIStore.getState().toasts;
    expect(toasts.map((t) => [t.key, t.kind])).toEqual([[TOAST_KEYS.DISTANCE_ORIGIN_RESET, 'info']]);
  });

  it('is not mirrored a second time on a reverse: the anchor already followed it', async () => {
    // The backend reports the reverse; the anchor re-resolves to the mirrored station. A
    // leftover mirror would move it on to 7 - 2 = 5.
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    serve(ref(2), [{ section_id: SECTION, shot_count: 7 }]);
    await useProjectStore.getState().reverseSection(SECTION, false);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(2));
  });

  it('leaves an origin re-picked during the round trip alone', async () => {
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    serve(ref(6));
    invokeIpcMock.mockImplementationOnce(async (command: string, args?: { refs?: StationRef[] }) => {
      // The capture: the user picks another origin while it is on the wire.
      useUIStore.getState().setDistanceGradientOrigin(ref(1));
      return (args?.refs ?? []).map((r) => ({ anchorOf: r, command }));
    });
    await useProjectStore.getState().createShot(SECTION, SHOT, undefined, 0);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(1));
  });

  it('costs no anchor round trip while the origin is the entrance', async () => {
    const anchorCalls = serve(ref(6));
    await useProjectStore.getState().createShot(SECTION, SHOT, undefined, 0);
    expect(anchorCalls).toEqual([]);
    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
  });
});

/**
 * #576 D39(g)-UNDO, decided: "Undo restoring the deleted origin station brings the origin
 * back". The delete drops the origin to the entrance; the anchor it was following is kept,
 * and an undo (or redo) that makes it resolve again puts the origin back on its station -
 * unless the user picked an origin of their own since, or another project is open.
 */
describe('the origin a delete displaced comes back on the undo that restores its station', () => {
  async function deleteTheOriginStation() {
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    serve(null);
    await useProjectStore.getState().deleteShot(SECTION, 4);
    expect(useUIStore.getState().distanceGradientOrigin, 'origin after deleting its station').toBeNull();
  }

  it('brings the origin back when the undo restores the station', async () => {
    await deleteTheOriginStation();
    // The undo restores the station; its anchor resolves again.
    serve(ref(5));
    await useProjectStore.getState().undo();
    expect(useUIStore.getState().distanceGradientOrigin, 'origin after undoing the delete').toEqual(ref(5));
  });

  it('keeps waiting while an undo does not restore it, then restores on the one that does', async () => {
    await deleteTheOriginStation();
    serve(null);
    await useProjectStore.getState().undo();
    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
    serve(ref(5));
    await useProjectStore.getState().undo();
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(5));
  });

  it('leaves an origin the user picked since alone', async () => {
    await deleteTheOriginStation();
    useUIStore.getState().setDistanceGradientOrigin(ref(1));
    useUIStore.getState().setDistanceGradientOrigin(null);
    const anchorCalls = serve(ref(5));
    await useProjectStore.getState().undo();
    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
    expect(anchorCalls, 'nothing left to re-resolve after a pick').toEqual([]);
  });

  it('does not re-resolve it in another project', async () => {
    await deleteTheOriginStation();
    useProjectStore.setState({ project: { ...project(), created_at: '2026-02-02T00:00:00Z' } as never });
    const anchorCalls = serve(ref(5));
    await useProjectStore.getState().undo();
    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
    expect(anchorCalls).toEqual([]);
  });
});

/**
 * #576 RV3-FE-NITS (D39(g)): a reverse still re-points an origin whose anchor could not be
 * taken. Before the anchors, `applyReverseAftermath` mirrored the origin; the anchors replaced
 * that mirror, so a failed capture (IPC error, or no anchor for the ref) left the origin
 * positional across the reverse - naming the opposite end of the section. The capture now
 * says it failed, and only then does the reverse aftermath mirror the origin as it used to:
 * the anchor stays the normal path, and the two never both move it.
 */
describe('a reverse mirrors the origin when its anchor could not be taken', () => {
  /** Serve as `serve` does, except that taking the origin's anchor fails (`how`). */
  function serveWithFailedCapture(how: 'throws' | 'returns no anchor', reversed?: { section_id: number; shot_count: number }[]) {
    const anchorCalls = serve(ref(0), reversed);
    const served = invokeIpcMock.getMockImplementation()!;
    invokeIpcMock.mockImplementation(async (command: string, args?: { refs?: StationRef[] }) => {
      if (command !== 'plan_anchors_for_refs') return served(command, args);
      if (how === 'throws') throw new Error('anchor IPC unavailable');
      return [null];
    });
    return anchorCalls;
  }

  it.each(['throws', 'returns no anchor'] as const)('mirrors it across a reverse when the capture %s', async (how) => {
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    const anchorCalls = serveWithFailedCapture(how, [{ section_id: SECTION, shot_count: 7 }]);
    await useProjectStore.getState().reverseSection(SECTION, false);
    expect(useUIStore.getState().distanceGradientOrigin, 'origin after a reverse of 7 shots').toEqual(ref(2));
    expect(anchorCalls, 'no re-resolve without an anchor: never both').toEqual([]);
  });

  it('mirrors it back on the undo of a reverse when the capture fails', async () => {
    useUIStore.getState().setDistanceGradientOrigin(ref(2));
    serveWithFailedCapture('throws', [{ section_id: SECTION, shot_count: 7 }]);
    await useProjectStore.getState().undo();
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(5));
  });

  it('leaves it positional across an edit that is not a reverse, as before', async () => {
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    serveWithFailedCapture('throws');
    await useProjectStore.getState().createShot(SECTION, SHOT, undefined, 0);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(5));
  });

  it('leaves an origin re-picked during the failed capture alone', async () => {
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    serveWithFailedCapture('throws', [{ section_id: SECTION, shot_count: 7 }]);
    const served = invokeIpcMock.getMockImplementation()!;
    invokeIpcMock.mockImplementationOnce(async (command: string, args?: { refs?: StationRef[] }) => {
      // The capture: the user picks another origin while it is on the wire, then it fails.
      useUIStore.getState().setDistanceGradientOrigin(ref(1));
      return served(command, args);
    });
    await useProjectStore.getState().reverseSection(SECTION, false);
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(1));
  });
});

/**
 * #576 RV4-FE-FEA2: the anchor a delete displaced is kept for an undo only while the
 * choice it stood for is still the user's. Turning the overlay off (which clears the
 * origin so re-enabling starts at the entrance), opening a project - another one, or
 * the same one again - and an in-place tree swap all replace that choice; the anchor
 * used to survive every one of them, because the origin was already null and only a
 * CHANGE of origin dropped it.
 */
describe('the displaced origin is forgotten once the overlay or the project moves on', () => {
  async function deleteTheOriginStation() {
    useUIStore.getState().setDistanceGradientOrigin(ref(5));
    serve(null);
    await useProjectStore.getState().deleteShot(SECTION, 4);
    expect(useUIStore.getState().distanceGradientOrigin, 'origin after deleting its station').toBeNull();
  }

  /** The load result for `p`, as the binary loader hands it to loadProject. */
  function loaded(p: ReturnType<typeof project>) {
    return {
      loadingId: 1,
      isRemote: false,
      project: p,
      stations: [],
      surveyData: { shots: [], stations: [] },
      stats: null,
      warnings: [],
      frozen_shots: [],
      displaySettings: {},
    };
  }

  /** `serve`, plus the recent-projects list a load refreshes. */
  function serveForLoad(resolveTo: StationRef | null) {
    const anchorCalls = serve(resolveTo);
    const served = invokeIpcMock.getMockImplementation()!;
    invokeIpcMock.mockImplementation(async (command: string, args?: { refs?: StationRef[] }) =>
      command === 'list_recent_projects' ? [] : served(command, args),
    );
    return anchorCalls;
  }

  it('drops it when the distance gradient is turned off', async () => {
    useUIStore.setState({ showDistanceGradient: true });
    await deleteTheOriginStation();
    useUIStore.getState().toggleDistanceGradient();

    const anchorCalls = serve(ref(5));
    await useProjectStore.getState().undo();
    console.log('[FEA2] gradient off, then undo: origin =', JSON.stringify(useUIStore.getState().distanceGradientOrigin));
    expect(useUIStore.getState().distanceGradientOrigin, 'origin set while the gradient is off').toBeNull();
    expect(anchorCalls).toEqual([]);
    // Re-enabling starts fresh at the entrance.
    useUIStore.getState().toggleDistanceGradient();
    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
  });

  it('keeps it across Plan, which suppresses the overlay and restores it on return', async () => {
    useUIStore.setState({ showDistanceGradient: true, appMode: 'survey' });
    await deleteTheOriginStation();
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().showDistanceGradient).toBe(true);

    serve(ref(5));
    await useProjectStore.getState().undo();
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(ref(5));
  });

  it('drops it when another project is opened, even once the first is open again', async () => {
    await deleteTheOriginStation();
    serveForLoad(null);
    loadBinaryMock.mockResolvedValueOnce(loaded({ ...project(), created_at: '2026-02-02T00:00:00Z', file_path: '/tmp/other.stalx' }));
    await useProjectStore.getState().loadProject('/tmp/other.stalx');
    loadBinaryMock.mockResolvedValueOnce(loaded(project()));
    await useProjectStore.getState().loadProject('/tmp/origin.stalx');
    expect(useProjectStore.getState().project?.created_at).toBe(project().created_at);

    const anchorCalls = serveForLoad(ref(5));
    await useProjectStore.getState().undo();
    console.log('[FEA2] switch away and back, then undo: origin =', JSON.stringify(useUIStore.getState().distanceGradientOrigin));
    expect(useUIStore.getState().distanceGradientOrigin, 'dormant survived the switch').toBeNull();
    expect(anchorCalls).toEqual([]);
  });

  it('drops it on an in-place tree swap', async () => {
    await deleteTheOriginStation();
    serve(null);
    await applySpeleoTreeReplacement(undefined, true);

    const anchorCalls = serve(ref(5));
    await useProjectStore.getState().undo();
    console.log('[FEA2] tree swap, then undo: origin =', JSON.stringify(useUIStore.getState().distanceGradientOrigin));
    expect(useUIStore.getState().distanceGradientOrigin).toBeNull();
    expect(anchorCalls).toEqual([]);
  });
});
