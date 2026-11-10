/**
 * #681: the repeated shots the orphan-placement dialog resolves. The "Merge at"
 * station sets how long the repeat is: shots on the dangling side of it are the
 * repeat, shots past it are new survey and must never be resolved (made
 * Reference), or the rest of the section is cut off in Plan mode.
 *
 * INJECTION (tried):
 * - pairing the whole overlap whatever the merge station (the previous rule,
 *   `computeOverlapIds` instead of `computeActionableIds`) reds "never reaches
 *   past the merge station" for all four cases and "a mid-repeat merge lists
 *   only the dangling side";
 * - returning `.slice(0, -1)` of the pairs reds every "whole repeat" case.
 */
import { describe, it, expect } from 'vitest';
import {
  computeMergeOptions,
  computeOverlapIds,
  computeOverlapPairs,
  type OverlapParams,
} from '../overlapHelpers';

const pairs = (params: OverlapParams, mergeImportStation: number) =>
  computeOverlapPairs(params, mergeImportStation).map((p) => [p.orphanShotIndex, p.targetShotIndex]);

describe('computeOverlapPairs - merged at the end of the repeat, the whole repeat (#681)', () => {
  it('Front + forward: new shots 0..n-1 repeat existing shots from the station on', () => {
    expect(pairs({ importStationIndex: 0, matchedShots: 2, existingStationIndex: 5, direction: 'forward' }, 2))
      .toEqual([[0, 5], [1, 6]]);
  });

  it('Front + reverse: new shots run back down the existing section', () => {
    expect(pairs({ importStationIndex: 0, matchedShots: 2, existingStationIndex: 7, direction: 'reverse' }, 2))
      .toEqual([[0, 6], [1, 5]]);
  });

  it('Back + forward: the last n new shots, in new-section order', () => {
    expect(pairs({ importStationIndex: 6, matchedShots: 2, existingStationIndex: 3, direction: 'forward' }, 4))
      .toEqual([[4, 4], [5, 3]]);
  });

  it('Back + reverse: the last n new shots, in new-section order', () => {
    expect(pairs({ importStationIndex: 6, matchedShots: 2, existingStationIndex: 5, direction: 'reverse' }, 4))
      .toEqual([[4, 3], [5, 4]]);
  });

  it('an overlap of one shot yields exactly that shot', () => {
    expect(pairs({ importStationIndex: 0, matchedShots: 1, existingStationIndex: 9, direction: 'forward' }, 1))
      .toEqual([[0, 9]]);
    expect(pairs({ importStationIndex: 4, matchedShots: 1, existingStationIndex: 9, direction: 'reverse' }, 3))
      .toEqual([[3, 8]]);
  });

  it('no overlap yields no pairs', () => {
    expect(pairs({ importStationIndex: 0, matchedShots: 0, existingStationIndex: 3, direction: 'forward' }, 0))
      .toEqual([]);
  });
});

describe('computeOverlapPairs - the merge station sets how long the repeat is (#681)', () => {
  it('a mid-repeat merge lists only the dangling side', () => {
    // Front: merged at new stn 1, so only shot 0 hangs off the merge.
    expect(pairs({ importStationIndex: 0, matchedShots: 3, existingStationIndex: 4, direction: 'forward' }, 1))
      .toEqual([[0, 4]]);
    // Back: merged at new stn 7 of 8, so only shot 7 hangs off the merge.
    expect(pairs({ importStationIndex: 8, matchedShots: 3, existingStationIndex: 2, direction: 'forward' }, 7))
      .toEqual([[7, 2]]);
  });

  // [label, params, new-section shot count]
  const CASES: Array<[string, OverlapParams, number]> = [
    ['Front + forward', { importStationIndex: 0, matchedShots: 3, existingStationIndex: 4, direction: 'forward' }, 8],
    ['Front + reverse', { importStationIndex: 0, matchedShots: 3, existingStationIndex: 7, direction: 'reverse' }, 8],
    ['Back + forward', { importStationIndex: 8, matchedShots: 3, existingStationIndex: 2, direction: 'forward' }, 8],
    ['Back + reverse', { importStationIndex: 8, matchedShots: 3, existingStationIndex: 6, direction: 'reverse' }, 8],
  ];

  for (const [label, params, shotCount] of CASES) {
    const front = params.importStationIndex === 0;
    const options = computeMergeOptions({ ...params, isPartial: false }, shotCount);

    it(`${label}: never reaches past the merge station, at any merge station`, () => {
      expect(options.length).toBe(params.matchedShots + 1);
      for (const option of options) {
        const resolved = computeOverlapPairs(params, option.importStation).map((p) => p.orphanShotIndex);
        // The shots from the merge station on to the new section's far end
        // (Front: station N; Back: station 0) carry the section on.
        const pastMerge = (shot: number) => (front ? shot >= option.importStation : shot < option.importStation);
        expect(resolved.filter(pastMerge)).toEqual([]);
        // And everything on the dangling side IS resolved: one shot per offset.
        expect(resolved.length).toBe(option.offset);
      }
    });

    it(`${label}: the end of the repeat, the default, resolves the whole repeat`, () => {
      const end = options[options.length - 1]!;
      const resolved = computeOverlapPairs(params, end.importStation).map((p) => p.orphanShotIndex);
      expect(resolved).toEqual([...computeOverlapIds(params.importStationIndex, params.matchedShots)]);
    });
  }
});
