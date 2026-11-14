/**
 * #741: the accuracy field's parser mirrors the backend's accepted range
 * (`validate_gps_accuracy`, 0 ..= 1000 m), so a value the save would refuse is
 * flagged in the field instead.
 */
import { describe, it, expect } from 'vitest';
import { MAX_DATUM_ACCURACY_M, datumAccuracyToSend, parseDatumAccuracy } from '../zeroDatumPreview';

describe('datumAccuracyToSend', () => {
  it('sends nothing when the user left the default on a datum that stores none', () => {
    expect(datumAccuracyToSend(0, undefined)).toBeUndefined();
  });

  it('keeps what is stored when the user did not change it', () => {
    expect(datumAccuracyToSend(12, 12)).toBe(12);
    expect(datumAccuracyToSend(0, 0)).toBe(0);
  });

  it('sends the entered value when it differs from what the datum reads as', () => {
    expect(datumAccuracyToSend(3, undefined)).toBe(3);
    expect(datumAccuracyToSend(3, 12)).toBe(3);
    expect(datumAccuracyToSend(0, 12)).toBe(0);
  });
});

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
