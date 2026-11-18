import { describe, expect, it } from 'vitest';
import { resolveSemanticMapTarget, type SemanticMapGeometry } from '../semanticTarget';

const geometry: SemanticMapGeometry = {
  stations: [
    { lineId: 1, stationIndex: 2, lngLat: [10, 20] },
    { lineId: 7, stationIndex: 8, lngLat: [10, 20] },
  ],
  shots: [
    { lineId: 1, shotId: 3, from: [0, 20], to: [20, 20] },
    { lineId: 7, shotId: 9, from: [0, 20], to: [20, 20] },
  ],
};

const project = ([x, y]: [number, number]) => ({ x, y });

describe('resolveSemanticMapTarget', () => {
  it('selects the requested station when stations are exactly co-located', () => {
    expect(
      resolveSemanticMapTarget(
        { featureType: 'station', lineId: 7, stationIndex: 8 },
        geometry,
        project,
        { x: 10, y: 20 },
        10,
        4,
      ),
    ).toEqual({ type: 'station', lineId: 7, stationIndex: 8 });
  });

  it('selects the requested shot when shot segments are exactly co-located', () => {
    expect(
      resolveSemanticMapTarget(
        { featureType: 'shot', lineId: 7, shotId: 9 },
        geometry,
        project,
        { x: 5, y: 20 },
        10,
        4,
      ),
    ).toEqual({ type: 'shot', lineId: 7, shotId: 9, alongT: 0.25 });
  });

  it('rejects a requested feature outside its normal hit reach', () => {
    expect(
      resolveSemanticMapTarget(
        { featureType: 'station', lineId: 7, stationIndex: 8 },
        geometry,
        project,
        { x: 30, y: 20 },
        10,
        4,
      ),
    ).toBeNull();
  });
});
