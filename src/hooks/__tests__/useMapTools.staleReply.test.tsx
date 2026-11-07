/**
 * Forced-interleaving coverage for the stale along-line reply guard in
 * `requestAlongLine` (useMapTools.ts:238-289) - the L0 repro-injection
 * prototype for issue #365.
 *
 * NOTE: the `.tsx` extension is load-bearing - `vite.config.ts`'s
 * `environmentMatchGlobs` routes only `*.test.tsx` to the jsdom environment;
 * renaming this file to `.ts` drops it to the node environment and
 * `renderHook` breaks.
 *
 * `requestAlongLine` issues `get_along_line_distance` then, once the reply
 * lands, discards it unless all three prongs still hold:
 *   1. monotonic request id (:273)
 *   2. endpoint reference identity - activeTool + measureStart/measureEnd (:274)
 *   3. project identity - created_at|modified_at|file_path (:275)
 * The `catch` path (:281-286) re-checks ONLY the id prong (:282) before
 * painting the FAILED shape (#576 C7-MSG: `failed: true`, so the readout says
 * the path could not be computed rather than that there is none) and logging a
 * `console.warn`.
 *
 * Each test below holds the IPC promise pending with a manually-controlled
 * deferred (never a timer), mutates exactly the state one prong reads, then
 * settles it and asserts what lands (or doesn't).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import type { MapMouseEvent } from 'maplibre-gl';

vi.mock('../../utils/ipc');
stubGetErrorMessage(messageOr('unknown error'));

import { useMapTools } from '../useMapTools';
import { useUIStore } from '../../store/uiStore';
import type { MeasureEndpoint } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import type { Project, Station } from '../../types/project';
import type { AlongLineResult } from '../../types/ipc/AlongLineResult';
import { invokeIpcMock, stubGetErrorMessage, messageOr } from '../../test/factories/ipc';
import { makeProject as projectOf, makeStation as stationOf } from '../../test/factories/survey';
import { deferred } from '../../test/factories/async';

function makeProject(overrides: Partial<Project> = {}): Project {
  return projectOf({ name: 'Test Cave', file_path: '/mock/a.stalx', next_section_id: 1, ...overrides });
}

function makeStation(section_id: number, station_index: number, x: number, y: number): Station {
  return stationOf({ station_ref: { section_id, station_index }, coordinates: { x, y, z: 0 }, closures_with: [] });
}

const STATIONS: Station[] = [
  makeStation(1, 0, 0, 0),
  makeStation(1, 1, 10, 0),
  makeStation(1, 2, 20, 0),
];

/** Deep-clone a measure endpoint (including its nested `coords`, where
 *  present) so the clone is structurally equal but reference-distinct -
 *  the shape the :274 endpoint-reference guard must reject on its own,
 *  independent of any deep-equality check. */
function cloneEndpoint(ep: MeasureEndpoint | null): MeasureEndpoint | null {
  if (!ep) return ep;
  if (ep.kind === 'station' || ep.kind === 'shotPoint') {
    return { ...ep, coords: { ...ep.coords } };
  }
  return { ...ep };
}

const clickEvent = { originalEvent: new MouseEvent('click') } as unknown as MapMouseEvent;

const RESULT_A: AlongLineResult = {
  along_line: 111,
  path_shots: [{ section_id: 1, shot_id: 1 }],
  path_stations: [{ section_id: 1, station_index: 1 }],
};
const RESULT_B: AlongLineResult = {
  along_line: 222,
  path_shots: [{ section_id: 1, shot_id: 2 }],
  path_stations: [{ section_id: 1, station_index: 2 }],
};

const RESULT_A_SHAPE = {
  alongLine: RESULT_A.along_line,
  pathShots: RESULT_A.path_shots,
  pathStations: RESULT_A.path_stations,
};
const RESULT_B_SHAPE = {
  alongLine: RESULT_B.along_line,
  pathShots: RESULT_B.path_shots,
  pathStations: RESULT_B.path_stations,
};
const DISCONNECTED_SHAPE = { alongLine: null, pathShots: [], pathStations: [] };
/** A query that REJECTED: no path either, but flagged, so the readout can say
 *  "Couldn't compute the path" instead of "No connected path" (#576 C7-MSG). */
const FAILED_SHAPE = { ...DISCONNECTED_SHAPE, failed: true };

/** Settle a deferred (resolve or reject) and flush past its continuation:
 *  first await the deferred's OWN promise (proving the hook's `await
 *  invokeIpc(...)` has had a chance to resume, not just that N microtask
 *  ticks elapsed), then flush a further `act` for any resulting React /
 *  store updates to commit. Replaces a fixed triple-`Promise.resolve` flush,
 *  which would still "pass" if the continuation silently never ran. */
async function settle<T>(d: { promise: Promise<T> }, action: () => void): Promise<void> {
  await act(async () => {
    action();
    await d.promise.catch(() => {});
  });
  await act(async () => {});
}

/** Assert invokeIpc was called the expected number of times, and that every
 *  call was for the along-line distance query. */
function expectAlongLineCallCount(times: number): void {
  expect(invokeIpcMock).toHaveBeenCalledTimes(times);
  for (const call of invokeIpcMock.mock.calls) {
    expect(call[0]).toBe('get_along_line_distance');
  }
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({
    project: makeProject(),
    stations: STATIONS,
    stationsAdjusted: null,
    surveyDataAdjusted: null,
  });
  useUIStore.setState({
    activeTool: 'measure',
    showStations: true,
    measureStart: null,
    measureEnd: null,
    measureDirect: null,
    measureAlongLine: null,
    measureAlongLinePending: false,
    measureSnapShot: null,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('requestAlongLine stale-reply guard (#365)', () => {
  it('drops a reply that lands after the project identity changed underneath it', async () => {
    const d = deferred<AlongLineResult | null>();
    invokeIpcMock.mockReturnValueOnce(d.promise);
    const { result } = renderHook(() => useMapTools());

    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 1 }, clickEvent);
    });
    expectAlongLineCallCount(1);

    // An edit/undo/project switch lands while the along-line query is in flight.
    act(() => {
      useProjectStore.setState({ project: makeProject({ modified_at: '2026-01-02T00:00:00Z' }) });
    });

    await settle(d, () => d.resolve(RESULT_A));

    expect(useUIStore.getState().measureAlongLine).toBeNull();
    // Not asserting measureAlongLinePending here: in this isolated hook test
    // nothing ever resets it back to false once the guard drops the reply
    // (setMeasureAlongLine, the only place that clears it, is never reached).
    // In the real app MapCanvas's clearMeasure effect (MapCanvas.tsx:1911-1912)
    // reacts to this exact same project.modified_at change and clears it -
    // pinning `true` here would only be pinning the absence of that effect.
  });

  it('drops request 1\'s reply once superseded - requires the id guard AND the reference guard together', async () => {
    // Empirical fact from the #365 disable sweep: deleting the :273 id check
    // ALONE leaves every test in this file green, because every UI-reachable
    // click path (applyMeasureClick) always mints fresh measureStart/measureEnd
    // objects, so :274's reference check independently rejects the same stale
    // reply. This test only goes red when :273 AND :274 are BOTH neutralized -
    // it pins that defense-in-depth pair jointly, not either prong in isolation.
    const d1 = deferred<AlongLineResult | null>();
    const d2 = deferred<AlongLineResult | null>();
    invokeIpcMock.mockReturnValueOnce(d1.promise).mockReturnValueOnce(d2.promise);
    const { result } = renderHook(() => useMapTools());

    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 1 }, clickEvent);
    });
    expectAlongLineCallCount(1);

    // Clicking again on a completed measurement resets and restarts it (#26) -
    // a fast second measurement with fresh endpoints, request 1 still pending.
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 2 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    expectAlongLineCallCount(2);

    // Resolve request 1 LAST, with a value distinct from request 2's.
    await settle(d1, () => d1.resolve(RESULT_A));
    expect(useUIStore.getState().measureAlongLine).toBeNull();

    await settle(d2, () => d2.resolve(RESULT_B));
    expect(useUIStore.getState().measureAlongLine).toEqual(RESULT_B_SHAPE);
  });

  it('drops a reply once the live endpoints are structurally-equal-but-fresh objects (reference identity, not deep equality)', async () => {
    const d = deferred<AlongLineResult | null>();
    invokeIpcMock.mockReturnValueOnce(d.promise);
    const { result } = renderHook(() => useMapTools());

    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 1 }, clickEvent);
    });
    expectAlongLineCallCount(1);

    // Replace the live endpoints with deep clones - equal by value, distinct
    // by reference - WITHOUT going through clearMeasure(). A deep-equality
    // weakening of the :274 guard would wrongly accept these as "the same"
    // endpoints and must turn this test red.
    const { measureStart, measureEnd } = useUIStore.getState();
    act(() => {
      useUIStore.setState({
        measureStart: cloneEndpoint(measureStart),
        measureEnd: cloneEndpoint(measureEnd),
      });
    });

    await settle(d, () => d.resolve(RESULT_A));

    expect(useUIStore.getState().measureAlongLine).toBeNull();
    // Not asserting measureAlongLinePending: see the project-identity test's
    // comment above - MapCanvas.tsx:1911-1912 is the real-app clearer.
  });

  it('stores a live reply when nothing went stale (control)', async () => {
    const d = deferred<AlongLineResult | null>();
    invokeIpcMock.mockReturnValueOnce(d.promise);
    const { result } = renderHook(() => useMapTools());

    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 1 }, clickEvent);
    });
    expect(useUIStore.getState().measureAlongLinePending).toBe(true);

    await settle(d, () => d.resolve(RESULT_A));

    expect(useUIStore.getState().measureAlongLine).toEqual(RESULT_A_SHAPE);
    expect(useUIStore.getState().measureAlongLinePending).toBe(false);
  });

  it('drops a rejected request-1 reply once request 2 has superseded it (catch-path id guard)', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const d1 = deferred<AlongLineResult | null>();
    const d2 = deferred<AlongLineResult | null>();
    invokeIpcMock.mockReturnValueOnce(d1.promise).mockReturnValueOnce(d2.promise);
    const { result } = renderHook(() => useMapTools());

    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 1 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 2 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    expectAlongLineCallCount(2);

    await settle(d2, () => d2.resolve(RESULT_B));
    expect(useUIStore.getState().measureAlongLine).toEqual(RESULT_B_SHAPE);

    // Request 1 rejects LAST, after being superseded - the catch-path id
    // guard (:282) must discard it without repainting request 2's state.
    await settle(d1, () => d1.reject(new Error('request 1 failed')));

    expect(useUIStore.getState().measureAlongLine).toEqual(RESULT_B_SHAPE);
    expect(warnSpy).not.toHaveBeenCalled();
  });

  it('paints the failed shape, not the disconnected one, on a live rejection (#576 C7-MSG)', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const d = deferred<AlongLineResult | null>();
    invokeIpcMock.mockReturnValueOnce(d.promise);
    const { result } = renderHook(() => useMapTools());

    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 1 }, clickEvent);
    });
    expectAlongLineCallCount(1);

    await settle(d, () => d.reject(new Error('along-line query failed')));

    // toStrictEqual: `failed` must be present, not merely absent-and-equal.
    expect(useUIStore.getState().measureAlongLine).toStrictEqual(FAILED_SHAPE);
    expect(useUIStore.getState().measureAlongLinePending).toBe(false);
    expect(warnSpy).toHaveBeenCalledTimes(1);
  });

  it('paints the disconnected shape, unflagged, for a disconnected ANSWER (null)', async () => {
    const d = deferred<AlongLineResult | null>();
    invokeIpcMock.mockReturnValueOnce(d.promise);
    const { result } = renderHook(() => useMapTools());

    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 0 }, clickEvent);
    });
    act(() => {
      result.current.onStationClick?.({ section_id: 1, station_index: 1 }, clickEvent);
    });

    await settle(d, () => d.resolve(null));

    expect(useUIStore.getState().measureAlongLine).toStrictEqual(DISCONNECTED_SHAPE);
  });
});
