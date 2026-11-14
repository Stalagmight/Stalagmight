/**
 * #741: the accuracy field's parser mirrors the backend's accepted range
 * (`validate_gps_accuracy`, 0 ..= 1000 m), so a value the save would refuse is
 * flagged in the field instead.
 */
import { describe, it, expect } from 'vitest';
import { MAX_DATUM_ACCURACY_M, parseDatumAccuracy } from '../zeroDatumPreview';

describe('parseDatumAccuracy', () => {
  it('accepts the whole backend range, ends included', () => {
    expect(parseDatumAccuracy('0')).toBe(0);
    expect(parseDatumAccuracy(' 2.5 ')).toBe(2.5);
    expect(parseDatumAccuracy('1000')).toBe(MAX_DATUM_ACCURACY_M);
  });

  it('refuses anything the backend would refuse, and an empty field', () => {
    for (const bad of ['', '  ', '-0.1', '1000.001', '5000', 'abc', 'Infinity', 'NaN', '1e400']) {
      expect(parseDatumAccuracy(bad), bad).toBeNull();
    }
  });
});
