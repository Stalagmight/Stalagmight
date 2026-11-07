/**
 * Unit tests for useMapTools helpers. Covers nearestReachableStationRef - the Plan-mode
 * "set a turnaround anywhere" resolver (#236): right-clicking the map (even with no turn-frontier
 * dots, e.g. low RMV / lots of gas, or with stations hidden) snaps to the nearest reachable station.
 */

import { describe, it, expect } from 'vitest';

import { nearestReachableStationRef } from '../useMapTools';

const st = (section_id: number, station_index: number, x: number, y: number) => ({
  station_ref: { section_id, station_index },
  coordinates: { x, y, z: 0 },
});

describe('nearestReachableStationRef', () => {
  // A straight line of stations 10 m apart on the x-axis.
  const stations = [st(1, 0, 0, 0), st(1, 1, 10, 0), st(1, 2, 20, 0), st(1, 3, 30, 0)];

  it('returns the nearest station when no reachability filter is applied (field not loaded yet)', () => {
    expect(nearestReachableStationRef({ x: 12, y: 1 }, stations, null)).toEqual({
      section_id: 1,
      station_index: 1,
    });
    expect(nearestReachableStationRef({ x: 28, y: 0 }, stations, null)).toEqual({
      section_id: 1,
      station_index: 3,
    });
  });

  it('with the whole cave reachable, picks the plain nearest station (the low-RMV use case)', () => {
    const allReachable = new Set(['1:0', '1:1', '1:2', '1:3']);
    expect(nearestReachableStationRef({ x: 21, y: 0 }, stations, allReachable)).toEqual({
      section_id: 1,
      station_index: 2,
    });
  });

  it('restricts to reachable stations - skips a closer unreachable one', () => {
    // The click is nearest station 3 (x=30), but only 0..1 are reachable → picks station 1, the
    // nearest one a diver can actually turn at (an unreachable pick would fall back to deepest).
    const reachable = new Set(['1:0', '1:1']);
    expect(nearestReachableStationRef({ x: 29, y: 0 }, stations, reachable)).toEqual({
      section_id: 1,
      station_index: 1,
    });
  });

  it('returns null when there are no eligible stations', () => {
    expect(nearestReachableStationRef({ x: 0, y: 0 }, [], null)).toBeNull();
    expect(nearestReachableStationRef({ x: 0, y: 0 }, stations, new Set())).toBeNull();
  });

  it('an empty reachable set still yields a station once the caller retries unfiltered (#298)', () => {
    // The caller (handleContextMenu) pairs these two calls: prefer a reachable station, but fall
    // back to any station when NOTHING is reachable. Without the fallback no menu opens at all,
    // and with way points able to make the whole map unreachable that would lock the user out of
    // the very entries ("Set as start point", removing a way point) that recover the plan.
    const nothingReachable = new Set<string>();
    expect(nearestReachableStationRef({ x: 29, y: 0 }, stations, nothingReachable)).toBeNull();
    expect(nearestReachableStationRef({ x: 29, y: 0 }, stations, null)).toEqual({
      section_id: 1,
      station_index: 3,
    });
  });
});
