import { describe, expect, it } from 'vitest';
import { surveyFrameBounds } from '../surveyFrame';
import type { SurveyData } from '../../types/survey-data';

/** Three stations of line 1 near the origin, two of line 2 far east. */
function data(): SurveyData {
  return {
    stationCount: 5,
    stationCoords: new Float64Array([0, 0, 0, 10, 5, -1, 4, -3, 2, 1000, 0, 0, 1010, 20, 0]),
    stationLineIds: new Uint32Array([1, 1, 1, 2, 2]),
  } as unknown as SurveyData;
}

describe('surveyFrameBounds (#327)', () => {
  it('frames every station when nothing is hidden', () => {
    expect(surveyFrameBounds(data(), new Set())).toMatchObject({ minX: 0, maxX: 1010, minY: -3, maxY: 20 });
  });

  it("leaves a hidden Section's lines out of Fit to View", () => {
    expect(surveyFrameBounds(data(), new Set([2]))).toEqual({
      minX: 0, maxX: 10, minY: -3, maxY: 5, minZ: -1, maxZ: 2,
    });
  });

  it('frames everything when every line is hidden, and ignores ids of no line', () => {
    expect(surveyFrameBounds(data(), new Set([1, 2]))).toMatchObject({ minX: 0, maxX: 1010 });
    expect(surveyFrameBounds(data(), new Set([7]))).toMatchObject({ minX: 0, maxX: 1010 });
  });
});
