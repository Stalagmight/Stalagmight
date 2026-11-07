import { describe, it, expect } from 'vitest';
import { hasAzimuthDiscrepancy, hasPitchDiscrepancy, validateEditableShot } from '../validation';

// ---------------------------------------------------------------------------
// hasAzimuthDiscrepancy
// ---------------------------------------------------------------------------

describe('hasAzimuthDiscrepancy', () => {
  it('returns false when difference is within threshold', () => {
    expect(hasAzimuthDiscrepancy(45, 48, 5)).toBe(false);
    expect(hasAzimuthDiscrepancy(100, 100, 0)).toBe(false);
  });

  it('returns true when difference exceeds threshold', () => {
    expect(hasAzimuthDiscrepancy(10, 40, 20)).toBe(true);
    expect(hasAzimuthDiscrepancy(0, 25, 20)).toBe(true);
  });

  it('handles 0/360 wrap-around correctly', () => {
    // 350 and 10 differ by 20 degrees (not 340)
    expect(hasAzimuthDiscrepancy(350, 10, 25)).toBe(false);
    expect(hasAzimuthDiscrepancy(350, 10, 15)).toBe(true);
  });

  it('handles exact 180° difference', () => {
    // 180 is max possible normalized diff
    expect(hasAzimuthDiscrepancy(0, 180, 180)).toBe(false);
    expect(hasAzimuthDiscrepancy(0, 180, 179)).toBe(true);
  });

  it('handles symmetric inputs', () => {
    expect(hasAzimuthDiscrepancy(10, 350, 25)).toBe(false);
    expect(hasAzimuthDiscrepancy(350, 10, 25)).toBe(false);
  });

  it('handles near-360 wrap', () => {
    // 359 and 1 differ by 2 degrees
    expect(hasAzimuthDiscrepancy(359, 1, 5)).toBe(false);
    expect(hasAzimuthDiscrepancy(359, 1, 1)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// hasPitchDiscrepancy
// ---------------------------------------------------------------------------

describe('hasPitchDiscrepancy', () => {
  it('returns false when either value is null', () => {
    expect(hasPitchDiscrepancy(null, 10, 5)).toBe(false);
    expect(hasPitchDiscrepancy(10, null, 5)).toBe(false);
    expect(hasPitchDiscrepancy(null, null, 5)).toBe(false);
  });

  it('returns false when within threshold', () => {
    expect(hasPitchDiscrepancy(10, 12, 5)).toBe(false);
    expect(hasPitchDiscrepancy(-5, -5, 0)).toBe(false);
  });

  it('returns true when exceeding threshold', () => {
    expect(hasPitchDiscrepancy(10, 20, 5)).toBe(true);
    expect(hasPitchDiscrepancy(-10, 10, 15)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// validateEditableShot
// ---------------------------------------------------------------------------

describe('validateEditableShot', () => {
  it('returns empty for valid shot', () => {
    expect(validateEditableShot({ length: 5, azimuth_in: 45, azimuth_out: 45 }, 0, 0)).toEqual([]);
  });

  it('skips deleted shots', () => {
    expect(
      validateEditableShot({ length: 0, azimuth_in: -5, azimuth_out: 400, isDeleted: true }, 0, 0)
    ).toEqual([]);
  });

  it('flags zero length', () => {
    const errors = validateEditableShot({ length: 0, azimuth_in: 45, azimuth_out: 45 }, 0, 0);
    expect(errors.some((e) => e.field === 'length')).toBe(true);
  });

  it('flags negative length', () => {
    const errors = validateEditableShot({ length: -3, azimuth_in: 45, azimuth_out: 45 }, 0, 0);
    expect(errors.some((e) => e.field === 'length')).toBe(true);
  });

  it('flags azimuth_in out of range', () => {
    const errors = validateEditableShot({ length: 5, azimuth_in: -1, azimuth_out: 45 }, 0, 0);
    expect(errors.some((e) => e.field === 'azimuth_in')).toBe(true);
  });

  it('flags azimuth_out out of range', () => {
    const errors = validateEditableShot({ length: 5, azimuth_in: 45, azimuth_out: 360 }, 0, 0);
    expect(errors.some((e) => e.field === 'azimuth_out')).toBe(true);
  });

  it('includes sectionIndex and shotIndex in errors', () => {
    const errors = validateEditableShot({ length: 0, azimuth_in: 45, azimuth_out: 45 }, 3, 7);
    expect(errors[0]!.sectionIndex).toBe(7);
    expect(errors[0]!.shotIndex).toBe(3);
  });
});
