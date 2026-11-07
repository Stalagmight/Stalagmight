/**
 * [#576 NF-W5I-2] The DMP wizard's temporal rerank reads `get_station_distances_from`,
 * whose answer is `DistanceField | null` (null when no project is open). The wizard
 * asserted it non-null (`field!`) and handed it on, so a null answer threw inside the
 * fetch - caught and logged, but by accident. The conversion now takes the command's
 * answer as typed: null is an empty set of distances.
 */
import { describe, it, expect } from 'vitest';
import { distanceMapFromField } from '../dmpTemporalRerank';
import type { DistanceField } from '../../types/ipc/DistanceField';

const origin = { section_id: 3, station_index: 1 };

describe('distanceMapFromField takes the nullable command answer', () => {
  it('a null answer (no project open) is no distances, not a crash', () => {
    let result: Map<string, number> | string;
    try {
      result = distanceMapFromField(origin, null);
    } catch (e) {
      result = `threw: ${String(e)}`;
    }
    console.log('distanceMapFromField(origin, null) ->', result instanceof Map ? `Map(${result.size})` : result);
    expect(result).toEqual(new Map());
  });

  it('a field still maps each finite distance to its key', () => {
    const field: DistanceField = {
      station_refs: [
        { section_id: 3, station_index: 2 },
        { section_id: 4, station_index: 0 },
      ],
      distances: [12.5, null],
      max_reachable: 12.5,
    };
    expect([...distanceMapFromField(origin, field)]).toEqual([['3:1->3:2', 12.5]]);
  });
});
