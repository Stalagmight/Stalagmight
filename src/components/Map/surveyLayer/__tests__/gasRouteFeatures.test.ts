/** The gas route highlight leaves out the legs on hidden Sections' lines (#327). */
import { describe, expect, it } from 'vitest';
import { gasRouteFeatures } from '../gasRouteFeatures';

const st = (line_id: number, station_index: number, x: number) => ({
  station_ref: { line_id, station_index },
  coordinates: { x, y: 0, z: 0 },
});
const leg = (from: [number, number], to: [number, number], is_jump = false) => ({
  from: { line_id: from[0], station_index: from[1] },
  to: { line_id: to[0], station_index: to[1] },
  is_jump,
  color: '#22d3ee',
});

describe('gasRouteFeatures', () => {
  const stations = [st(1, 0, 0), st(1, 1, 10), st(2, 0, 10), st(2, 1, 20)];
  const legs = [leg([1, 0], [1, 1]), leg([1, 1], [2, 0], true), leg([2, 0], [2, 1])];

  it('draws one line per leg', () => {
    const features = gasRouteFeatures(legs, stations, new Set(), null);
    expect(features).toHaveLength(3);
    expect(features.map((f) => f.properties.isJump)).toEqual([false, true, false]);
  });

  it('leaves out every leg that touches a hidden line', () => {
    const features = gasRouteFeatures(legs, stations, new Set([2]), null);
    expect(features).toHaveLength(1);
    expect(features[0]!.properties.isJump).toBe(false);
  });

  it('skips a leg whose station is unknown', () => {
    expect(gasRouteFeatures([leg([9, 0], [1, 0])], stations, new Set(), null)).toEqual([]);
  });
});
