/**
 * Pointer leaving the map takes back the queued hover frame (#576 D39(h)).
 *
 * Hover hit-testing runs one animation frame after a mousemove. A move dispatched
 * just before the pointer leaves the canvas (a fast flick off the edge across a
 * station: move + leave inside one frame, as WebKit delivers them) used to be
 * hit-tested AFTER the leave had cleared the hover, re-applying the highlight -
 * and the panels' cross-highlight - with the pointer already gone.
 *
 * Drives a real SurveyLayer over the fake map; only the hit-test itself is
 * stubbed to say "a station is under any point".
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('maplibre-gl', () => ({ default: {} }));

import SurveyLayer from '../SurveyLayer';
import { makeFakeSurveyMap, type FakeSurveyMap } from './surveyLayerFakeMap';
import { microtasks } from '../../../test/factories/async';

/** The hit-test seam: candidate sections + the nearest-feature resolvers. */
interface HitSeam {
  hitCandidateSections: () => { stationSections: Set<number>; shotSections: Set<number> };
  resolveStationAt: () => { ref: { section_id: number; station_index: number } } | null;
  resolveShotAt: () => null;
  getStationHitReach: () => number;
  getShotHitReach: () => number;
}

let fake: FakeSurveyMap;
let layer: SurveyLayer;
const onStationHover = vi.fn();

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(() => cb(0), 16));
  vi.stubGlobal('cancelAnimationFrame', (id: ReturnType<typeof setTimeout>) => clearTimeout(id));
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  onStationHover.mockReset();
  fake = makeFakeSurveyMap();
  layer = new SurveyLayer(fake.map, { onStationHover });
  Object.assign(layer as unknown as HitSeam, {
    hitCandidateSections: () => ({ stationSections: new Set([1]), shotSections: new Set<number>() }),
    resolveStationAt: () => ({ ref: { section_id: 1, station_index: 2 } }),
    resolveShotAt: () => null,
    getStationHitReach: () => 10,
    getShotHitReach: () => 5,
  } satisfies HitSeam);
});

afterEach(() => {
  layer.release();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('SurveyLayer hover vs pointer leave', () => {
  it('hovers the station under the pointer one frame after a move (control)', async () => {
    fake.fire('mousemove', { point: { x: 5, y: 5 } });
    vi.advanceTimersByTime(50);
    await microtasks(3);
    expect(onStationHover).toHaveBeenLastCalledWith({ section_id: 1, station_index: 2 });
  });

  it('a move queued before the pointer leaves does not hover after it', async () => {
    // Move over a station, then leave the canvas before the next frame.
    fake.fire('mousemove', { point: { x: 5, y: 5 } });
    fake.canvas.dispatchEvent(new Event('mouseout'));
    vi.advanceTimersByTime(50);
    await microtasks(3);

    expect(onStationHover).not.toHaveBeenCalledWith({ section_id: 1, station_index: 2 });
  });

  it('a later move after the leave hovers again', async () => {
    fake.fire('mousemove', { point: { x: 5, y: 5 } });
    fake.canvas.dispatchEvent(new Event('mouseout'));
    fake.fire('mousemove', { point: { x: 6, y: 5 } });
    vi.advanceTimersByTime(50);
    await microtasks(3);
    expect(onStationHover).toHaveBeenLastCalledWith({ section_id: 1, station_index: 2 });
  });

  // #576 W39-NF4: one move from the map onto a panel row. The browser fires the
  // canvas's mouseout, during which React enters the row and MapHoverSync hands
  // the row's hover to the layer, and only then the canvas's mouseleave. The
  // leave must clear the map's hover before the row's, not the row's after it.
  it("a panel row's hover pushed in as the pointer leaves outlasts the leave", async () => {
    fake.fire('mousemove', { point: { x: 5, y: 5 } });
    vi.advanceTimersByTime(50);
    await microtasks(3);
    expect(onStationHover).toHaveBeenLastCalledWith({ section_id: 1, station_index: 2 });

    fake.canvas.dispatchEvent(new Event('mouseout'));
    await microtasks(3); // the checkpoint after the canvas's own listener
    const callsBefore = onStationHover.mock.calls.length;
    layer.setHoveredStation({ section_id: 1, station_index: 5 }); // the row, via MapHoverSync
    fake.canvas.dispatchEvent(new Event('mouseleave'));
    await microtasks(3);

    expect(onStationHover.mock.calls.slice(callsBefore)).toEqual([]);
    expect((layer as unknown as { hoveredStationId: string | null }).hoveredStationId).toBe('1:5');
  });
});
