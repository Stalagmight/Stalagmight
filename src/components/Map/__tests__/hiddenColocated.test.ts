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

  it("does not follow a partner's aligned closure through a closure-chain entry (D15)", () => {
    // X = 1:0 holds closure 7 to Y = 2:0 (unaligned) and closure 8 to Z = 3:0
    // (aligned). The wire gives Y the chain entry "Z via X" under 8. Dragging Y
    // must not move Z: Z is tied to X, and moving it alone tears that tie.
    const x = { line_id: 1, station_index: 0 };
    const y = { line_id: 2, station_index: 0 };
    const z = { line_id: 3, station_index: 0 };
    const d = emptySurveyData();
    d.stationCount = 3;
    d.stationLineIds = Uint32Array.from([1, 2, 3]);
    d.stationIndices = Uint32Array.from([0, 0, 0]);
    d.stationClosuresWith = new Map([
      [0, [
        { closure_id: 7n, station_ref: y, via_station: null, is_transitive: false },
        { closure_id: 8n, station_ref: z, via_station: null, is_transitive: false },
      ]],
      [1, [
        { closure_id: 7n, station_ref: x, via_station: null, is_transitive: false },
        { closure_id: 8n, station_ref: z, via_station: x, is_transitive: true },
      ]],
      [2, [
        { closure_id: 8n, station_ref: x, via_station: null, is_transitive: false },
        { closure_id: 7n, station_ref: y, via_station: x, is_transitive: true },
      ]],
    ]);
    const aligned = new Set([8n]);
    expect(expandColocated([y], d, aligned)).toEqual([y]);
    // Z itself still drags X with it: that is the aligned tie.
    expect(expandColocated([z], d, aligned).map((r) => `${r.line_id}:${r.station_index}`)).toEqual(['3:0', '1:0']);
  });
});
