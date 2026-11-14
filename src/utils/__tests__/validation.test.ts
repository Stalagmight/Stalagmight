import { describe, it, expect } from 'vitest';
import {
  hasAzimuthDiscrepancy,
  hasPitchDiscrepancy,
  validateShot,
  validateSection,
  getHighestSeverity,
  countIssues,
  countUnresolvedErrors,
  validateEditableShot,
  validatePlacement,
  validateMergeTarget,
  DEFAULT_VALIDATION_CONFIG,
  type ValidationIssue,
} from '../validation';
import type { DmpShot, ProjectWarning } from '../../types/commands';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeShot(overrides: Partial<DmpShot> = {}): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: 45,
    heading_out: 45,
    length: 10,
    depth_in: 0,
    depth_out: 1,
    pitch_in: 0,
    pitch_out: 0,
    left: 1,
    right: 1,
    up: 2,
    down: 0.5,
    temperature: 18,
    hour: 12,
    minute: 0,
    second: 0,
    marker_index: 0,
    ...overrides,
  };
}

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
// validateShot
// ---------------------------------------------------------------------------

describe('validateShot', () => {
  it('returns empty array for valid shot', () => {
    const shot = makeShot();
    expect(validateShot(shot, [shot], 0)).toEqual([]);
  });

  it('skips validation for non-Standard shot types', () => {
    const shot = makeShot({ shot_type: 'CsA', length: 0 });
    expect(validateShot(shot, [shot], 0)).toEqual([]);
  });

  it('flags zero length as error', () => {
    const shot = makeShot({ length: 0 });
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'length' && i.severity === 'error')).toBe(true);
  });

  it('flags NaN length as error', () => {
    const shot = makeShot({ length: NaN });
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'length' && i.severity === 'error')).toBe(true);
  });

  it('flags very short length as warning', () => {
    const shot = makeShot({ length: 0.05 }); // below default 0.4 min
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'length' && i.severity === 'warning')).toBe(true);
  });

  it('flags very long length as warning', () => {
    const shot = makeShot({ length: 100 }); // above default 50 max
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'length' && i.severity === 'warning')).toBe(true);
  });

  it('does not flag length warnings when length is zero (already an error)', () => {
    const shot = makeShot({ length: 0 });
    const issues = validateShot(shot, [shot], 0);
    const lengthIssues = issues.filter((i) => i.field === 'length');
    // Should only have the error, not additional warnings
    expect(lengthIssues).toHaveLength(1);
    expect(lengthIssues[0]!.severity).toBe('error');
  });

  it('flags azimuth out of range [0, 360) as error', () => {
    const shot = makeShot({ heading_in: -5, heading_out: 360 });
    const issues = validateShot(shot, [shot], 0);
    const azimuthErrors = issues.filter((i) => i.field === 'azimuth' && i.severity === 'error');
    expect(azimuthErrors).toHaveLength(2);
  });

  it('flags azimuth in/out discrepancy as warning', () => {
    // Default threshold is 20°
    const shot = makeShot({ heading_in: 10, heading_out: 50 });
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'azimuth' && i.severity === 'warning')).toBe(true);
  });

  it('does not flag azimuth discrepancy when values are within threshold', () => {
    const shot = makeShot({ heading_in: 45, heading_out: 50 });
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'azimuth')).toBe(false);
  });

  it('does not flag azimuth discrepancy when azimuths are out of range', () => {
    // If azimuth is invalid (already an error), don't also warn about discrepancy
    const shot = makeShot({ heading_in: -10, heading_out: 400 });
    const issues = validateShot(shot, [shot], 0);
    const azimuthWarnings = issues.filter((i) => i.field === 'azimuth' && i.severity === 'warning');
    expect(azimuthWarnings).toHaveLength(0);
  });

  it('flags excessive depth change as warning', () => {
    // Default max is 10m per shot
    const shot = makeShot({ depth_in: 0, depth_out: 15 });
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'depth' && i.severity === 'warning')).toBe(true);
  });

  it('flags negative LRUD as error', () => {
    const shot = makeShot({ left: -1 });
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'lrud' && i.severity === 'error')).toBe(true);
  });

  it('flags oversized LRUD as warning', () => {
    const shot = makeShot({ up: 60 }); // above LRUD_MAX=50
    const issues = validateShot(shot, [shot], 0);
    expect(issues.some((i) => i.field === 'lrud' && i.severity === 'warning')).toBe(true);
  });

  it('does not flag zero LRUD values', () => {
    const shot = makeShot({ left: 0, right: 0, up: 0, down: 0 });
    const issues = validateShot(shot, [shot], 0);
    expect(issues.filter((i) => i.field === 'lrud')).toHaveLength(0);
  });

  it('uses custom config thresholds', () => {
    const config = {
      ...DEFAULT_VALIDATION_CONFIG,
      warn_shot_length_min: 5,
      warn_shot_length_max: 20,
    };
    // 3m is below custom min of 5
    const short = makeShot({ length: 3 });
    expect(validateShot(short, [short], 0, config).some((i) => i.field === 'length')).toBe(true);

    // 25m is above custom max of 20
    const long = makeShot({ length: 25 });
    expect(validateShot(long, [long], 0, config).some((i) => i.field === 'length')).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// validateSection
// ---------------------------------------------------------------------------

describe('validateSection', () => {
  it('returns empty map for valid shots', () => {
    const shots = [makeShot(), makeShot({ heading_in: 90, heading_out: 90, marker_index: 1 })];
    const issues = validateSection(shots);
    expect(issues.size).toBe(0);
  });

  it('detects duplicate marker indices', () => {
    const shots = [
      makeShot({ marker_index: 5 }),
      makeShot({ marker_index: 5 }),
    ];
    const issues = validateSection(shots);
    // Both shots should have a duplicate marker warning
    expect(issues.get(0)?.some((i) => i.field === 'general')).toBe(true);
    expect(issues.get(1)?.some((i) => i.field === 'general')).toBe(true);
  });

  it('does not flag different marker indices as duplicates', () => {
    const shots = [
      makeShot({ marker_index: 0 }),
      makeShot({ marker_index: 1 }),
    ];
    const issues = validateSection(shots);
    // No general issues
    for (const shotIssues of issues.values()) {
      expect(shotIssues.some((i) => i.field === 'general')).toBe(false);
    }
  });

  it('combines duplicate marker warnings with shot validation issues', () => {
    const shots = [
      makeShot({ marker_index: 1, length: 0 }),
      makeShot({ marker_index: 1 }),
    ];
    const issues = validateSection(shots);
    const shot0Issues = issues.get(0) ?? [];
    // Should have both the duplicate marker warning and the length error
    expect(shot0Issues.some((i) => i.field === 'general')).toBe(true);
    expect(shot0Issues.some((i) => i.field === 'length' && i.severity === 'error')).toBe(true);
  });

  it('skips non-Standard shots for marker duplicate check', () => {
    const shots = [
      makeShot({ shot_type: 'CsA', marker_index: 1 }),
      makeShot({ marker_index: 1 }),
    ];
    const issues = validateSection(shots);
    // Only one Standard shot with marker_index 1, so no duplicate
    expect(issues.size).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// getHighestSeverity
// ---------------------------------------------------------------------------

describe('getHighestSeverity', () => {
  it('returns undefined for empty array', () => {
    expect(getHighestSeverity([])).toBeUndefined();
  });

  it('returns warning for warnings only', () => {
    const issues: ValidationIssue[] = [
      { field: 'length', severity: 'warning', message: 'w' },
    ];
    expect(getHighestSeverity(issues)).toBe('warning');
  });

  it('returns error when errors present', () => {
    const issues: ValidationIssue[] = [
      { field: 'length', severity: 'warning', message: 'w' },
      { field: 'azimuth', severity: 'error', message: 'e' },
    ];
    expect(getHighestSeverity(issues)).toBe('error');
  });
});

// ---------------------------------------------------------------------------
// countIssues
// ---------------------------------------------------------------------------

describe('countIssues', () => {
  it('returns zeros for empty map', () => {
    expect(countIssues(new Map())).toEqual({ errors: 0, warnings: 0 });
  });

  it('counts errors and warnings across sections and shots', () => {
    const innerMap = new Map<number, ValidationIssue[]>();
    innerMap.set(0, [
      { field: 'length', severity: 'error', message: 'e1' },
      { field: 'depth', severity: 'warning', message: 'w1' },
    ]);
    innerMap.set(1, [
      { field: 'azimuth', severity: 'warning', message: 'w2' },
    ]);

    const outerMap = new Map<number, Map<number, ValidationIssue[]>>();
    outerMap.set(0, innerMap);

    expect(countIssues(outerMap)).toEqual({ errors: 1, warnings: 2 });
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

// ---------------------------------------------------------------------------
// validatePlacement
// ---------------------------------------------------------------------------

describe('validatePlacement', () => {
  it('returns null when placed', () => {
    expect(validatePlacement(true, 0)).toBeNull();
  });

  it('returns missingPlacement error when not placed', () => {
    const error = validatePlacement(false, 2);
    expect(error).not.toBeNull();
    expect(error!.type).toBe('missingPlacement');
    expect(error!.sectionIndex).toBe(2);
  });
});

// ---------------------------------------------------------------------------
// validateMergeTarget
// ---------------------------------------------------------------------------

describe('validateMergeTarget', () => {
  it('returns null for non-fromExisting placement', () => {
    expect(validateMergeTarget('standalone', undefined, new Set(), new Set(), 0)).toBeNull();
    expect(validateMergeTarget('manual', undefined, new Set(), new Set(), 0)).toBeNull();
  });

  it('returns mergeConflict when targetSectionId is undefined', () => {
    const error = validateMergeTarget('fromExisting', undefined, new Set([1, 2]), new Set(), 0);
    expect(error).not.toBeNull();
    expect(error!.type).toBe('mergeConflict');
  });

  it('returns null when target exists in existing sections', () => {
    expect(validateMergeTarget('fromExisting', 5, new Set([5, 6]), new Set(), 0)).toBeNull();
  });

  it('returns null when target exists in placed sections', () => {
    expect(validateMergeTarget('fromExisting', 5, new Set(), new Set([5]), 0)).toBeNull();
  });

  it('returns mergeConflict when target not in either set', () => {
    const error = validateMergeTarget('fromExisting', 99, new Set([1, 2]), new Set([3, 4]), 0);
    expect(error).not.toBeNull();
    expect(error!.type).toBe('mergeConflict');
  });
});

describe('countUnresolvedErrors (#737 closure warnings)', () => {
  const warning = (code: string, severity: 'error' | 'warning', isResolved = false): ProjectWarning => ({
    sectionId: 0,
    shotId: 1,
    shotIndex: 0,
    code,
    message: '',
    severity,
    isResolved,
  });

  it('counts a closure across one shot (an error) and not a failed solve (advisory)', () => {
    // The backend makes CLOSURE_ADJUSTMENT_FAILED a warning: it exists only
    // while the cached solve is current, so it must not block an upload
    // depending on which view was open last.
    expect(
      countUnresolvedErrors([
        warning('CLOSURE_ACROSS_ONE_SHOT', 'error'),
        warning('CLOSURE_ADJUSTMENT_FAILED', 'warning'),
        warning('CLOSURE_ADJUSTMENT_FAILED', 'warning', true),
      ])
    ).toBe(1);
  });
});
