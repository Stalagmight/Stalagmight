import { describe, it, expect } from 'vitest';
import {
  mirrorStationIndex,
  mirrorPlacementForReverse,
  targetShotCountForMirror,
} from '../reversePlacement';
import type { ImportPlacement } from '../../types/commands';
import type { LineEditState } from '../batchImportHelpers';

describe('mirrorStationIndex', () => {
  it('maps i to N - i', () => {
    // 3 shots => stations 0..3
    expect(mirrorStationIndex(0, 3)).toBe(3);
    expect(mirrorStationIndex(1, 3)).toBe(2);
    expect(mirrorStationIndex(3, 3)).toBe(0);
  });

  it('is an involution', () => {
    for (let i = 0; i <= 5; i++) {
      expect(mirrorStationIndex(mirrorStationIndex(i, 5), 5)).toBe(i);
    }
  });

  it('clamps an out-of-range index into the line instead of going negative', () => {
    // `station_index` is a u32 on the Rust side, so a negative index is not a
    // rejected placement - serde fails to deserialize the whole batch.
    expect(mirrorStationIndex(4, 3)).toBe(0);
    expect(mirrorStationIndex(300, 3)).toBe(0);
    expect(mirrorStationIndex(-1, 3)).toBe(3);
  });
});

describe('mirrorPlacementForReverse', () => {
  const gps = { latitude: 20.2, longitude: -87.4, depth_reference: 0 };

  it('leaves placements untouched when nothing is reversed', () => {
    const p: ImportPlacement = { type: 'from_existing', import_station: 1, existing_station: { line_id: 7, station_index: 2 } };
    expect(mirrorPlacementForReverse(p, false, 3, null)).toEqual(p);
  });

  it('mirrors zero_datum station_index', () => {
    const p: ImportPlacement = { type: 'zero_datum', coordinates: gps, station_index: 0 };
    expect(mirrorPlacementForReverse(p, true, 4, null)).toEqual({ ...p, station_index: 4 });
  });

  it('mirrors import_station but never the existing line it connects to', () => {
    const p: ImportPlacement = {
      type: 'from_existing',
      import_station: 1,
      existing_station: { line_id: 7, station_index: 2 },
    };
    const out = mirrorPlacementForReverse(p, true, 3, null);
    expect(out).toEqual({ ...p, import_station: 2 });
    // The existing project line is not being reversed - its index must stand.
    expect((out as Extract<ImportPlacement, { type: 'from_existing' }>).existing_station.station_index).toBe(2);
  });

  it('mirrors align_station_index for manual placements', () => {
    const p: ImportPlacement = { type: 'manual', align_station_index: 1, offset: null };
    expect(mirrorPlacementForReverse(p, true, 5, null)).toEqual({ ...p, align_station_index: 4 });
  });

  it('mirrors each side of from_importing independently', () => {
    const p: ImportPlacement = {
      type: 'from_importing',
      import_station: 1,
      target_line_index: 2,
      target_station_index: 1,
    };
    // Only this line reversed.
    expect(mirrorPlacementForReverse(p, true, 3, null)).toEqual({ ...p, import_station: 2 });
    // Only the target reversed.
    expect(mirrorPlacementForReverse(p, false, 3, 6)).toEqual({ ...p, target_station_index: 5 });
    // Both, with different shot counts.
    expect(mirrorPlacementForReverse(p, true, 3, 6)).toEqual({
      ...p,
      import_station: 2,
      target_station_index: 5,
    });
  });

  it('mirrors the existing_station of an early-imported target that was reversed', () => {
    // The wizard rewrites a connection into from_existing once its target has
    // been created earlier in the same run. If that target was created reversed,
    // its stations were renumbered after the user picked this index, so the
    // index has to follow - otherwise the line merges onto the wrong end.
    const p: ImportPlacement = {
      type: 'from_existing',
      import_station: 1,
      existing_station: { line_id: 42, station_index: 2 },
    };

    expect(mirrorPlacementForReverse(p, false, 3, 6)).toEqual({
      ...p,
      existing_station: { line_id: 42, station_index: 4 },
    });

    // Both sides reversed, with different shot counts.
    expect(mirrorPlacementForReverse(p, true, 3, 6)).toEqual({
      import_station: 2,
      type: 'from_existing',
      existing_station: { line_id: 42, station_index: 4 },
    });
  });

  it('never mirrors an out-of-range index below zero', () => {
    const p: ImportPlacement = { type: 'zero_datum', coordinates: gps, station_index: 7 };
    expect(mirrorPlacementForReverse(p, true, 4, null)).toEqual({ ...p, station_index: 0 });

    const m: ImportPlacement = {
      type: 'manual_snap',
      align_station_index: 9,
      snapped_to_station: { line_id: 1, station_index: 0 },
    };
    expect(mirrorPlacementForReverse(m, true, 4, null)).toEqual({ ...m, align_station_index: 0 });
  });

  it('leaves origin alone', () => {
    const p: ImportPlacement = { type: 'origin' };
    expect(mirrorPlacementForReverse(p, true, 3, null)).toEqual(p);
  });
});

describe('targetShotCountForMirror', () => {
  const edits = (reverse: boolean, shots: number, deleted = 0): Map<number, LineEditState> =>
    new Map([
      [
        3,
        {
          reverse,
          shots: Array.from({ length: shots + deleted }, (_, i) => ({
            isDeleted: i >= shots,
          })),
        } as unknown as LineEditState,
      ],
    ]);

  const fromImporting: ImportPlacement = {
    type: 'from_importing',
    import_station: 0,
    target_line_index: 3,
    target_station_index: 1,
  };
  const fromExisting: ImportPlacement = {
    type: 'from_existing',
    import_station: 0,
    existing_station: { line_id: 9, station_index: 1 },
  };

  it('counts a reversed from_importing target, excluding deleted shots', () => {
    expect(targetShotCountForMirror(fromImporting, 'importing', 3, edits(true, 5, 2))).toBe(5);
  });

  it('counts a reversed early-imported target reached through from_existing', () => {
    expect(targetShotCountForMirror(fromExisting, 'importing', 3, edits(true, 5))).toBe(5);
  });

  it('returns null when the target is not being reversed', () => {
    expect(targetShotCountForMirror(fromImporting, 'importing', 3, edits(false, 5))).toBeNull();
    expect(targetShotCountForMirror(fromExisting, 'importing', 3, edits(false, 5))).toBeNull();
  });

  it('never mirrors a connection to a line already in the project', () => {
    // targetType 'existing' means the user picked the index off the live
    // project, where the line already has its final numbering - even if a
    // line at that import index happens to be reversed in this batch.
    expect(targetShotCountForMirror(fromExisting, 'existing', 3, edits(true, 5))).toBeNull();
    expect(targetShotCountForMirror(fromExisting, null, 3, edits(true, 5))).toBeNull();
    expect(targetShotCountForMirror(fromExisting, undefined, 3, edits(true, 5))).toBeNull();
  });

  it('returns null without a target import index', () => {
    expect(targetShotCountForMirror(fromExisting, 'importing', null, edits(true, 5))).toBeNull();
    expect(targetShotCountForMirror(fromExisting, 'importing', undefined, edits(true, 5))).toBeNull();
  });

  it('returns null for placements with no target side', () => {
    expect(targetShotCountForMirror({ type: 'origin' }, 'importing', 3, edits(true, 5))).toBeNull();
    expect(
      targetShotCountForMirror({ type: 'manual', align_station_index: 0, offset: null }, 'importing', 3, edits(true, 5))
    ).toBeNull();
  });
});
