import { describe, expect, it } from 'vitest';
import { mergedStationLabels } from '../labelMergeGroups';
import { emptySurveyData, type SurveyData } from '../../../../../types/survey-data';

/**
 * H-frontend-wire-10: the label build runs on every viewport rebuild, and the
 * merge-group pass used to scan every station and allocate one array per
 * station each time, junction or not. The junction groups are now built once
 * per SoA (`stationIds` array): a rebuild reads no Station ID again.
 */
function chain(count: number, shared: Array<[number, number]> = []): { data: SurveyData; reads: () => number } {
  const d = emptySurveyData();
  d.stationCount = count;
  d.stationLineIds = new Uint32Array(count).fill(1);
  d.stationIndices = new Uint32Array(Array.from({ length: count }, (_, i) => i));
  const ids = new Uint32Array(Array.from({ length: count }, (_, i) => 100 + i));
  for (const [a, b] of shared) ids[b] = ids[a]!;
  let reads = 0;
  d.stationIds = new Proxy(ids, {
    get(target, prop) {
      if (typeof prop === 'string' && /^\d+$/.test(prop)) reads += 1;
      const value = Reflect.get(target, prop, target) as unknown;
      return typeof value === 'function' ? (value as (...a: unknown[]) => unknown).bind(target) : value;
    },
  });
  d.lines = [{ id: 1, name: 'Main', color: '#fff', direction: 'in' as never, quality: 0, next_shot_id: 0, shotOffset: 0, shotCount: count - 1 }];
  return { data: d, reads: () => reads };
}

const first = new Map([[1, 0]]);

describe('merge groups are built once per SoA', () => {
  it('a survey with no junction reads its Station IDs once, not on every label rebuild', () => {
    const { data, reads } = chain(50);
    const last = new Map([[1, 49]]);
    expect(mergedStationLabels(data, first, last, () => true)).toBeNull();
    const afterFirst = reads();
    expect(afterFirst).toBeGreaterThan(0);
    expect(mergedStationLabels(data, first, last, () => true)).toBeNull();
    expect(reads()).toBe(afterFirst);
  });

  it('a survey with a junction groups it the same on every rebuild', () => {
    const { data, reads } = chain(50, [[10, 40]]);
    const last = new Map([[1, 49]]);
    const one = mergedStationLabels(data, first, last, () => true)!;
    const afterFirst = reads();
    const two = mergedStationLabels(data, first, last, () => true)!;
    expect(reads()).toBe(afterFirst);
    expect([...one.members.entries()]).toEqual([[10, [10, 40]]]);
    expect([...two.members.entries()]).toEqual([[10, [10, 40]]]);
    expect(two.representative[40]).toBe(10);
  });
});
