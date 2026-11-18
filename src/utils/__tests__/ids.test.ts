import { describe, expect, it } from 'vitest';
import { encodeFeatureId, lineIdFromKey, FEATURE_ID_STRIDE } from '../ids';

describe('lineIdFromKey', () => {
  it('reads the line id out of a feature key', () => {
    expect(lineIdFromKey('0:0')).toBe(0);
    expect(lineIdFromKey('1:0')).toBe(1);
    expect(lineIdFromKey('7:42')).toBe(7);
  });

  // The regression #301 turned on. A shot id is ALREADY line-encoded by the
  // backend (next_shot_id = line.id * FEATURE_ID_STRIDE), so a line-1
  // shot's key is "1:100000" and its feature id is 2 * FEATURE_ID_STRIDE.
  // Inverting encodeFeatureId then reports line 2 - the key does not.
  it('is correct for shot keys, where the floor-divide decode is not', () => {
    const lineId = 1;
    const shotId = lineId * FEATURE_ID_STRIDE; // first shot of line 1
    const key = `${lineId}:${shotId}`;

    expect(lineIdFromKey(key)).toBe(1);
    expect(Math.floor(encodeFeatureId(lineId, shotId) / FEATURE_ID_STRIDE)).toBe(2);
  });

  it('agrees with the floor-divide decode for station keys, which are line-local', () => {
    const lineId = 3;
    const stationIndex = 12;
    expect(lineIdFromKey(`${lineId}:${stationIndex}`)).toBe(lineId);
    expect(Math.floor(encodeFeatureId(lineId, stationIndex) / FEATURE_ID_STRIDE)).toBe(lineId);
  });

  it('yields NaN for a malformed key so membership tests fail closed', () => {
    expect(lineIdFromKey('abc:1')).toBeNaN();
    expect(lineIdFromKey('')).toBeNaN();
    expect(new Set([0, 1]).has(lineIdFromKey('abc:1'))).toBe(false);
  });
});
