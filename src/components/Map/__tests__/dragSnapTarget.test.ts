/**
 * Snap-to-station for the map's move drags: the scan never offers a station of
 * a hidden Section's line (#327), nor keeps a held target whose line was hidden.
 */
import { describe, expect, it } from 'vitest';
import { heldSnapStation, nearestSnapStation } from '../dragSnapTarget';

const st = (line_id: number, station_index: number, x: number, y: number) => ({
  station_ref: { line_id, station_index },
  coordinates: { x, y, z: 0 },
});

// Screen = world here: the projection is the identity.
const toScreen = (s: ReturnType<typeof st>) => ({ x: s.coordinates.x, y: s.coordinates.y });
const none = new Set<number>();
const noSkip = () => false;

describe('nearestSnapStation', () => {
  const stations = [st(1, 0, 0, 0), st(1, 1, 10, 0), st(2, 0, 4, 0), st(3, 0, 6, 0)];

  it('picks the nearest station within the threshold', () => {
    expect(nearestSnapStation(stations, toScreen, { x: 5, y: 0 }, 3, none, noSkip)).toEqual({
      line_id: 2,
      station_index: 0,
    });
  });

  it('returns null when nothing is within the threshold', () => {
    expect(nearestSnapStation(stations, toScreen, { x: 50, y: 50 }, 3, none, noSkip)).toBeNull();
  });

  it('never snaps onto a station of a hidden line, even the nearest one', () => {
    // Line 2's station is the nearest; with line 2 hidden the next one wins.
    expect(nearestSnapStation(stations, toScreen, { x: 4.5, y: 0 }, 3, new Set([2]), noSkip)).toEqual({
      line_id: 3,
      station_index: 0,
    });
    // Every candidate in reach hidden: no snap at all.
    expect(nearestSnapStation(stations, toScreen, { x: 4.5, y: 0 }, 3, new Set([2, 3]), noSkip)).toBeNull();
  });

  it('skips the stations the drag names as its own', () => {
    const ownLine = (s: ReturnType<typeof st>) => s.station_ref.line_id === 2;
    expect(nearestSnapStation(stations, toScreen, { x: 4, y: 0 }, 3, none, ownLine)).toEqual({
      line_id: 3,
      station_index: 0,
    });
  });
});

describe('heldSnapStation', () => {
  const stations = [st(1, 0, 0, 0), st(2, 3, 4, 0)];

  it('finds the held target while its line is drawn', () => {
    expect(heldSnapStation(stations, { line_id: 2, station_index: 3 }, none)).toBe(stations[1]);
  });

  it('drops a held target whose line is hidden now', () => {
    expect(heldSnapStation(stations, { line_id: 2, station_index: 3 }, new Set([2]))).toBeUndefined();
  });

  it('drops a held target that no longer exists', () => {
    expect(heldSnapStation(stations, { line_id: 2, station_index: 9 }, none)).toBeUndefined();
  });
});
