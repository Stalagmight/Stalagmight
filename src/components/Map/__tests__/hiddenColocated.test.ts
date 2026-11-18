import { describe, expect, it } from 'vitest';
import { emptySurveyData, type SurveyData } from '../../../types/survey-data';
import { expandColocated } from '../hiddenColocated';

/** Stations 1:0, 1:1, 2:0, 3:0; 1:1 is merged with 2:0, and 2:0 is tied to
 *  3:0 by closure 5n. */
function data(): SurveyData {
  const d = emptySurveyData();
  d.stationCount = 4;
  d.stationLineIds = Uint32Array.from([1, 1, 2, 3]);
  d.stationIndices = Uint32Array.from([0, 1, 0, 0]);
  d.stationMergedWith = new Map([
    [1, [{ line_id: 2, station_index: 0 }]],
    [2, [{ line_id: 1, station_index: 1 }]],
  ]);
  d.stationClosuresWith = new Map([
    [2, [{ closure_id: 5n, station_ref: { line_id: 3, station_index: 0 }, via_station: null, is_transitive: false }]],
  ]);
  return d;
}

describe('expandColocated', () => {
  it('adds stations merged with the dragged one through the whole survey', () => {
    const out = expandColocated([{ line_id: 1, station_index: 1 }], data(), new Set());
    expect(out).toEqual([
      { line_id: 1, station_index: 1 },
      { line_id: 2, station_index: 0 },
    ]);
  });

  it('follows an aligned closure, and only an aligned one', () => {
    const aligned = expandColocated([{ line_id: 1, station_index: 1 }], data(), new Set([5n]));
    expect(aligned.map((r) => `${r.line_id}:${r.station_index}`)).toEqual(['1:1', '2:0', '3:0']);
  });

  it('keeps the seeds, once each, and adds nothing to an unlinked station', () => {
    const seeds = [{ line_id: 1, station_index: 0 }];
    expect(expandColocated(seeds, data(), new Set([5n]))).toEqual(seeds);
  });
});
