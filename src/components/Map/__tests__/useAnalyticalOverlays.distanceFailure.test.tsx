/**
 * #576 C7-MSG: a distance-gradient field that cannot be computed says so.
 *
 * `get_station_distances_from` REJECTS on failure since C7 (it used to answer
 * an empty field). The overlay already dropped its ramp on a rejection; it now
 * also raises one error toast per failure BURST - not one per refetch, since
 * every edit refetches - and raises it again only after a success (or the
 * overlay being switched off) ended the burst.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';
import type { MutableRefObject } from 'react';

vi.mock('../../../utils/ipc');
// The markers this hook adds import maplibre-gl, which jsdom cannot load; with
// the map not loaded no marker is ever built.
vi.mock('maplibre-gl', () => ({ default: {} }));

import { useAnalyticalOverlays } from '../useAnalyticalOverlays';
import { useUIStore } from '../../../store/uiStore';
import type { DistanceField } from '../../../types/project';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { actMicrotasks } from '../../../test/factories/act';

const FIELD = { stations: [], shots: [], max_distance: 0 } as unknown as DistanceField;

type Options = Parameters<typeof useAnalyticalOverlays>[0];

function options(modifiedAt: string, setDistanceField: Options['setDistanceField'], showDistanceGradient = true): Options {
  return {
    map: { current: null } as MutableRefObject<null>,
    surveyLayerRef: { current: null } as MutableRefObject<null>,
    mapLoaded: false,
    project: { modified_at: modifiedAt } as Options['project'],
    config: null,
    surveyData: null,
    activeStations: [],
    gpsOrigin: null,
    effectiveIssueSeverity: false,
    showDistanceGradient,
    distanceGradientOrigin: null,
    distanceField: null,
    setDistanceField,
    showGasPlan: false,
    gasOriginChosen: false,
    gasOrigin: null,
    gasComputeGate: { skip: false } as Options['gasComputeGate'],
    gasWaypoints: [],
    gasSegments: [],
    gasDiver: {} as Options['gasDiver'],
    gasCylinders: [],
    gasPlanConfig: {} as Options['gasPlanConfig'],
    gasReachField: null,
    setGasReachField: vi.fn(),
  } as unknown as Options;
}

let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  showToast = vi.fn();
  useUIStore.setState({ showToast });
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

const failureToasts = () =>
  showToast.mock.calls.filter((call) => call[1] === 'distance-gradient-failed');

describe('distance gradient: a field that cannot be computed (#576 C7-MSG)', () => {
  it('clears the ramp and raises one error toast per failure burst', async () => {
    const setDistanceField = vi.fn();
    invokeIpcMock.mockRejectedValue(new Error('distance field failed (test)'));
    const { rerender } = renderHook((props: Options) => useAnalyticalOverlays(props), {
      initialProps: options('t1', setDistanceField),
    });
    await actMicrotasks(5);

    // The existing clear-the-ramp behaviour stays.
    expect(setDistanceField).toHaveBeenLastCalledWith(null);
    expect(failureToasts()).toHaveLength(1);
    expect(failureToasts()[0]).toEqual([
      "Couldn't compute distances; the map shows its usual colors.",
      'distance-gradient-failed',
      'error',
    ]);

    // An edit refetches, and it fails again: same burst, no second toast.
    rerender(options('t2', setDistanceField));
    await actMicrotasks(5);
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
    expect(failureToasts()).toHaveLength(1);

    // A success ends the burst...
    invokeIpcMock.mockResolvedValueOnce(FIELD);
    rerender(options('t3', setDistanceField));
    await actMicrotasks(5);
    expect(setDistanceField).toHaveBeenLastCalledWith(FIELD);
    expect(failureToasts()).toHaveLength(1);

    // ...so the next failure is a new one, and says so.
    rerender(options('t4', setDistanceField));
    await actMicrotasks(5);
    expect(failureToasts()).toHaveLength(2);
  });

  it('says so again when the overlay is switched back on and still fails', async () => {
    const setDistanceField = vi.fn();
    invokeIpcMock.mockRejectedValue(new Error('distance field failed (test)'));
    const { rerender } = renderHook((props: Options) => useAnalyticalOverlays(props), {
      initialProps: options('t1', setDistanceField),
    });
    await actMicrotasks(5);
    expect(failureToasts()).toHaveLength(1);

    rerender(options('t1', setDistanceField, false));
    await actMicrotasks(5);
    rerender(options('t1', setDistanceField, true));
    await actMicrotasks(5);
    expect(failureToasts()).toHaveLength(2);
  });

  // [#576 RV7b-14] The failure flag hides the "computing" cue (C7-CUE). It used to
  // be cleared only by a successful field, so after one failure the next enable
  // computed with no cue at all.
  it('switching the overlay off clears the failure, so the next enable shows its computing cue', async () => {
    const setDistanceField = vi.fn();
    invokeIpcMock.mockRejectedValueOnce(new Error('distance field failed (test)'));
    const { result, rerender } = renderHook((props: Options) => useAnalyticalOverlays(props), {
      initialProps: options('t1', setDistanceField),
    });
    await actMicrotasks(5);
    expect(result.current.distanceFieldFailed).toBe(true);

    rerender(options('t1', setDistanceField, false));
    await actMicrotasks(5);
    expect(result.current.distanceFieldFailed).toBe(false);

    // Re-enabled: the recompute is still in flight, and nothing has failed yet.
    invokeIpcMock.mockReturnValueOnce(new Promise(() => {}));
    rerender(options('t1', setDistanceField, true));
    await actMicrotasks(5);
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
    expect(result.current.distanceFieldFailed).toBe(false);
  });

  // [#576 RV9b-2] Only the switch-off cleared it: an edit, a re-root or an
  // include_reference_legs flip after a failure recomputed with the flag still set,
  // so the computing cue stayed hidden for the whole recompute.
  it('a recompute after a failure (an edit) clears the failure while it runs, and a second failure is still one burst', async () => {
    const setDistanceField = vi.fn();
    invokeIpcMock.mockRejectedValueOnce(new Error('distance field failed (test)'));
    const { result, rerender } = renderHook((props: Options) => useAnalyticalOverlays(props), {
      initialProps: options('t1', setDistanceField),
    });
    await actMicrotasks(5);
    expect(result.current.distanceFieldFailed).toBe(true);

    // An edit: the recompute is in flight, and nothing has failed yet.
    let failRecompute: (e: Error) => void = () => {};
    invokeIpcMock.mockReturnValueOnce(new Promise((_resolve, reject) => { failRecompute = reject; }));
    rerender(options('t2', setDistanceField));
    await actMicrotasks(5);
    expect(invokeIpcMock).toHaveBeenCalledTimes(2);
    expect(result.current.distanceFieldFailed).toBe(false);

    // It fails too: failed again, and still the one toast for the burst.
    failRecompute(new Error('distance field failed again (test)'));
    await actMicrotasks(5);
    expect(result.current.distanceFieldFailed).toBe(true);
    expect(failureToasts()).toHaveLength(1);
  });
});
