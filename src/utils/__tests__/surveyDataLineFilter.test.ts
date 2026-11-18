import { describe, expect, it } from 'vitest';
import { emptySurveyData, type SurveyData } from '../../types/survey-data';
import {
  filterSurveyDataLines,
  filterSurveyDataLinesCached,
  OTHER_FIELDS,
  SHOT_ARRAY_FIELDS,
  STATION_ARRAY_FIELDS,
  STATION_MAP_FIELDS,
} from '../surveyDataLineFilter';

/**
 * Three lines, 2 / 1 / 2 shots, stations 3 / 2 / 3 per line. Every per-shot
 * and per-station value encodes its line and position so a mis-copy shows.
 * Stations of line 20 sit at the END of the arrays to prove the filter keys
 * stations by their line, not by position.
 */
function sample(): SurveyData {
  const data = emptySurveyData();
  const shotLines = [10, 10, 20, 30, 30];
  const n = shotLines.length;
  data.lines = [
    { id: 10, name: 'A', color: '#a', direction: 'in', quality: 1, next_shot_id: 2, shotOffset: 0, shotCount: 2 },
    { id: 20, name: 'B', color: '#b', direction: 'in', quality: 1, next_shot_id: 1, shotOffset: 2, shotCount: 1 },
    { id: 30, name: 'C', color: '#c', direction: 'in', quality: 1, next_shot_id: 2, shotOffset: 3, shotCount: 2 },
  ];
  data.shotCount = n;
  data.shotIds = Uint32Array.from(shotLines.map((l, i) => l * 100 + i));
  data.shotLineIds = Uint32Array.from(shotLines);
  for (const f of SHOT_ARRAY_FIELDS) {
    if (f === 'shotIds' || f === 'shotLineIds') continue;
    const Ctor = (data[f] as unknown as { constructor: new (n: number) => { [i: number]: number } }).constructor;
    const arr = new Ctor(n);
    for (let i = 0; i < n; i++) arr[i] = shotLines[i]! + i;
    (data as unknown as Record<string, unknown>)[f] = arr;
  }
  data.shotComments = new Map([[2, 'on B'], [4, 'on C']]);

  const stationLines = [10, 10, 10, 30, 30, 30, 20, 20];
  const stationIdx = [0, 1, 2, 0, 1, 2, 0, 1];
  data.stationCount = stationLines.length;
  data.stationLineIds = Uint32Array.from(stationLines);
  data.stationIndices = Uint32Array.from(stationIdx);
  data.stationFlags = Uint8Array.from(stationLines.map((_, g) => g % 2));
  data.stationIds = Uint32Array.from(stationLines.map((_, g) => 1000 + g));
  data.stationCoords = Float64Array.from(stationLines.flatMap((l, g) => [l, g, -g]));
  data.stationNames = new Map(stationLines.map((l, g) => [g, `${l}/${stationIdx[g]}`]));
  data.stationComments = new Map([[6, 'B start']]);
  data.stationMergedWith = new Map([[2, [{ line_id: 20, station_index: 0 }]], [6, [{ line_id: 10, station_index: 2 }]]]);
  data.stationClosuresWith = new Map();
  data.stationGps = new Map([[3, { latitude: 1, longitude: 2, depth_reference: 0 }]]);
  return data;
}

describe('filterSurveyDataLines', () => {
  it('handles every SurveyData field explicitly', () => {
    // A new field must be added to one of the lists, or the filter would
    // spread it through at the unfiltered length.
    const handled = new Set<string>([...SHOT_ARRAY_FIELDS, ...STATION_ARRAY_FIELDS, ...STATION_MAP_FIELDS, ...OTHER_FIELDS]);
    expect(Object.keys(emptySurveyData()).sort()).toEqual([...handled].sort());
  });

  it('returns the same object when no line of it is hidden', () => {
    const data = sample();
    expect(filterSurveyDataLines(data, new Set())).toBe(data);
    expect(filterSurveyDataLines(data, new Set([99]))).toBe(data);
  });

  it('drops a hidden line from every shot and station array, keeping ids', () => {
    const data = sample();
    const out = filterSurveyDataLines(data, new Set([20]));
    expect(out.lines.map((l) => [l.id, l.shotOffset, l.shotCount])).toEqual([[10, 0, 2], [30, 2, 2]]);
    expect(out.shotCount).toBe(4);
    expect([...out.shotLineIds]).toEqual([10, 10, 30, 30]);
    expect([...out.shotIds]).toEqual([1000, 1001, 3003, 3004]);
    expect([...out.shotLength]).toEqual([10, 11, 33, 34]);
    expect(out.shotComments).toEqual(new Map([[3, 'on C']]));

    expect(out.stationCount).toBe(6);
    expect([...out.stationLineIds]).toEqual([10, 10, 10, 30, 30, 30]);
    expect([...out.stationIndices]).toEqual([0, 1, 2, 0, 1, 2]);
    expect([...out.stationIds]).toEqual([1000, 1001, 1002, 1003, 1004, 1005]);
    expect([...out.stationCoords]).toEqual([10, 0, -0, 10, 1, -1, 10, 2, -2, 30, 3, -3, 30, 4, -4, 30, 5, -5]);
    expect(out.stationNames.get(5)).toBe('30/2');
    expect(out.stationComments.size).toBe(0);
    expect(out.stationGps.get(3)).toEqual({ latitude: 1, longitude: 2, depth_reference: 0 });
    // A merge into the hidden line is kept as a ref (it resolves to nothing).
    expect(out.stationMergedWith.get(2)).toEqual([{ line_id: 20, station_index: 0 }]);
    // The source is untouched.
    expect(data.shotCount).toBe(5);
  });

  it('drops stations of a hidden line wherever they sit', () => {
    const out = filterSurveyDataLines(sample(), new Set([10, 30]));
    expect(out.lines.map((l) => l.id)).toEqual([20]);
    expect([...out.stationLineIds]).toEqual([20, 20]);
    expect(out.stationComments).toEqual(new Map([[0, 'B start']]));
    expect(out.stationMergedWith.get(0)).toEqual([{ line_id: 10, station_index: 2 }]);
    expect([...out.shotIds]).toEqual([2002]);
  });

  it('caches per source and key', () => {
    const data = sample();
    const a = filterSurveyDataLinesCached(data, new Set([20]), '20');
    expect(filterSurveyDataLinesCached(data, new Set([20]), '20')).toBe(a);
    expect(filterSurveyDataLinesCached(data, new Set([30]), '30')).not.toBe(a);
    expect(filterSurveyDataLinesCached(data, new Set(), '')).toBe(data);
  });
});
