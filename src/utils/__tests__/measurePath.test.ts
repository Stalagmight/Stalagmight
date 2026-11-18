import { describe, it, expect } from 'vitest';
import { MEASURE_TINT, buildMeasuredPathIds, buildMeasuredStationIds } from '../measurePath';
import { encodeFeatureId } from '../ids';

describe('measurePath (#26)', () => {
  it('MEASURE_TINT is a 6-digit hex color', () => {
    expect(MEASURE_TINT).toMatch(/^#[0-9a-fA-F]{6}$/);
  });

  it('buildMeasuredPathIds maps path shots to canonical feature ids', () => {
    const ids = buildMeasuredPathIds([
      { line_id: 1, shot_id: 2 },
      { line_id: 3, shot_id: 0 },
    ]);
    expect(ids).toEqual(new Set([encodeFeatureId(1, 2), encodeFeatureId(3, 0)]));
  });

  it('empty path yields an empty set', () => {
    expect(buildMeasuredPathIds([]).size).toBe(0);
  });

  it('dedupes identical shots', () => {
    const ids = buildMeasuredPathIds([
      { line_id: 1, shot_id: 1 },
      { line_id: 1, shot_id: 1 },
    ]);
    expect(ids.size).toBe(1);
  });

  it('buildMeasuredStationIds maps path stations to canonical feature ids', () => {
    const ids = buildMeasuredStationIds([
      { line_id: 1, station_index: 0 },
      { line_id: 2, station_index: 3 },
    ]);
    expect(ids).toEqual(new Set([encodeFeatureId(1, 0), encodeFeatureId(2, 3)]));
  });

  it('buildMeasuredStationIds on an empty path yields an empty set', () => {
    expect(buildMeasuredStationIds([]).size).toBe(0);
  });
});
