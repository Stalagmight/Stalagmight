import { describe, it, expect } from 'vitest';
import { findStation } from '../findStation';
import type { Station } from '../../types/project';

const station = (section_id: number, station_index: number, name?: string): Station => ({
  station_ref: { section_id, station_index },
  coordinates: { x: 0, y: 0, z: 0 },
  is_zero_datum: false,
  gps_coordinates: null,
  merged_with: [],
  ...(name !== undefined ? { name } : {}),
});

describe('findStation', () => {
  const stations = [station(1, 0, '0'), station(1, 1, '1'), station(2, 1)];

  it('finds by section and index together', () => {
    expect(findStation(stations, 1, 1)?.name).toBe('1');
    expect(findStation(stations, 2, 1)?.station_ref).toEqual({ section_id: 2, station_index: 1 });
  });

  it('is undefined when nothing is there', () => {
    expect(findStation(stations, 2, 0)).toBeUndefined();
    expect(findStation(stations, undefined, 0)).toBeUndefined();
    expect(findStation([], 1, 0)).toBeUndefined();
  });
});
