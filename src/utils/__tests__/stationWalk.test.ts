/**
 * `walkStations` is the frontend's one station walk, and it is pinned to the
 * backend's by the shared table (#576 D15): the same rows are asserted against
 * `Project::compute_station_coordinates` in
 * `src-tauri/crates/models/src/project/station_walk_parity_tests.rs`.
 */
import { describe, it, expect } from 'vitest';
import { azimuthReferenceFor, walkStations } from '../stationWalk';
import * as shotConversion from '../shotConversion';
import {
  STATION_WALK_VECTORS,
  expectedStations,
  vectorCases,
  vectorShots,
} from './stationWalkTable';

const xyz = (stations: Array<{ x: number; y: number; z: number }>) =>
  stations.map((s) => [s.x, s.y, s.z]);

describe('walkStations matches the backend walk, row for row', () => {
  it('reads the whole shared table', () => {
    expect(STATION_WALK_VECTORS.length).toBeGreaterThanOrEqual(10);
  });

  it.each(vectorCases())('%s', (_name, vector) => {
    const reference = azimuthReferenceFor(vector.reference, vector.project_default);
    expect(xyz(walkStations(vectorShots(vector), reference))).toEqual(expectedStations(vector));
  });
});

describe('azimuthReferenceFor', () => {
  const magnetic = { north: 'magnetic', correction: 5, source: 'manual' } as const;
  const grid = { north: 'grid', correction: -1, source: 'manual' } as const;

  it('takes the section reference, then the project default, then nothing', () => {
    expect(azimuthReferenceFor(grid, magnetic)).toBe(grid);
    expect(azimuthReferenceFor(null, magnetic)).toBe(magnetic);
    expect(azimuthReferenceFor(undefined, undefined)).toBeNull();
  });
});

/**
 * What the backend never walks. `ShotData::all_finite` rejects a non-finite
 * reading at ingest, so these rows have no Rust counterpart - but a preview
 * draws half-typed rows, and one NaN must cost one leg, not every station
 * after it.
 */
describe('walkStations outside the backend domain', () => {
  const flat = (azimuth: number, length = 10) => ({
    azimuth_in: azimuth,
    azimuth_out: azimuth,
    length,
    depth_in: 0,
    depth_out: 0,
  });

  it('keeps one station per shot and carries the position through a gap', () => {
    const gap = { azimuth_in: NaN, azimuth_out: NaN, length: NaN, depth_in: NaN, depth_out: NaN };
    const stations = walkStations([flat(90), gap, flat(90)]);

    expect(stations).toHaveLength(4);
    expect(stations[2]!.x).toBe(stations[1]!.x);
    expect(stations[2]!.y).toBe(stations[1]!.y);
    expect(stations[3]!.x).toBeCloseTo(20, 9);
    // The unread depths are left out of the station depth, not averaged in.
    expect(stations.every((s) => Number.isFinite(s.z))).toBe(true);
  });

  it('walks nowhere for a shot with neither leg', () => {
    const noBearing = { ...flat(0), azimuth_in: NaN, azimuth_out: NaN, depth_out: 3 };
    expect(walkStations([noBearing])[1]).toEqual({ x: 0, y: 0, z: 3 });
  });

  it('walks the lone leg whichever leg is missing', () => {
    const inOnly = walkStations([{ ...flat(60), azimuth_out: NaN }])[1]!;
    const outOnly = walkStations([{ ...flat(60), azimuth_in: NaN }])[1]!;
    expect(outOnly.x).toBeCloseTo(inOnly.x, 12);
    expect(outOnly.y).toBeCloseTo(inOnly.y, 12);
  });

  it('starts an empty section at the origin', () => {
    expect(walkStations([])).toEqual([{ x: 0, y: 0, z: 0 }]);
  });
});

describe('the dead walker stays deleted (D15)', () => {
  it('shotConversion no longer carries calculateStationPositions', () => {
    // It had no production caller and walked the arithmetic mean of the legs,
    // 180 degrees wrong for 350/10.
    expect(shotConversion).not.toHaveProperty('calculateStationPositions');
  });
});
